//! Macros: the timed-sequence transform (docs/INPUT-TRANSFORMS.md §1c).
//!
//! Every test here drives a FAKE CLOCK. That is the point of the design: the
//! engine is handed `now` and never reads one, so a hadouken is reproducible to
//! the millisecond in CI with no hardware, no sleeping, and no flake.

mod common;

use common::{ev, ipac_device, preset, preset_with_macros};
use ksx_core::{
    Axis, Binding, DpadDirection, Engine, EngineTables, Interrupt, Key, Macro, MacroStep,
    MacroTrigger, Macros, OnRelease, PadState, Repeat, ResolvedSlot, Retrigger, SlotSpec, Trigger,
    TurboRate, XButton, XButtons, AXIS_MAX, MIN_STEP_MS,
};

const DOWN: Binding = Binding::Dpad(DpadDirection::Down);
const RIGHT: Binding = Binding::Dpad(DpadDirection::Right);
const PUNCH: Binding = Binding::Button(XButton::A);

/// ↓, ↘, →, punch — 50 ms a step, the case Victor described.
fn hadouken() -> Macro {
    Macro::new(
        "hadouken",
        vec![
            MacroStep::new(vec![DOWN], 50),
            MacroStep::new(vec![DOWN, RIGHT], 50),
            MacroStep::new(vec![RIGHT], 50),
            MacroStep::new(vec![PUNCH], 50),
        ],
    )
}

fn macros(defs: Vec<Macro>, triggers: Vec<MacroTrigger>) -> Macros {
    Macros { defs, triggers }
}

/// One slot, one keyboard, one macro on `P`.
fn engine_with(mac: Macro) -> Engine {
    engine_with_entries(mac, Vec::new())
}

fn engine_with_entries(mac: Macro, entries: Vec<(Key, Binding)>) -> Engine {
    let p = preset_with_macros(
        "macros",
        entries,
        macros(vec![mac], vec![MacroTrigger::new(Key::P, 0)]),
    );
    let dev = ipac_device();
    Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(dev), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }])
}

fn state(engine: &Engine) -> PadState {
    engine.pad_state(1).expect("slot 1")
}

fn press(engine: &mut Engine, key: Key, now: u64) -> usize {
    engine.handle_at(&ev(&ipac_device(), key, true), now).len()
}

fn release(engine: &mut Engine, key: Key, now: u64) -> usize {
    engine.handle_at(&ev(&ipac_device(), key, false), now).len()
}

// ---------------------------------------------------------------------------
// The headline case
// ---------------------------------------------------------------------------

/// A quarter-circle-forward punch, step by step, on a clock we control.
#[test]
fn a_hadouken_walks_its_steps_on_the_clock() {
    let mut engine = engine_with(hadouken());

    // Press → step 0 is live IMMEDIATELY, in the same delta batch as the press.
    assert_eq!(press(&mut engine, Key::P, 0), 1);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_DOWN);
    // ...and the engine now knows exactly when it must be woken again.
    assert_eq!(engine.next_deadline(), Some(50));

    // Nothing happens early. This is not a rounding detail: a step that ended
    // before its time would be the sampling bug §0.2 exists to prevent.
    assert!(engine.tick(49).is_empty());
    assert_eq!(state(&engine).buttons, XButtons::DPAD_DOWN);

    // ↘ — the diagonal is ONE state with two bits, not two events (§0.1).
    assert_eq!(engine.tick(50).len(), 1);
    assert_eq!(
        state(&engine).buttons,
        XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT
    );
    assert_eq!(engine.next_deadline(), Some(100));

    assert_eq!(engine.tick(100).len(), 1);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_RIGHT);

    assert_eq!(engine.tick(150).len(), 1);
    assert_eq!(state(&engine).buttons, XButtons::A);

    // Last step ends: back to neutral, and nothing is armed any more.
    assert_eq!(engine.tick(200).len(), 1);
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
    assert!(engine.tick(1_000).is_empty());
}

/// Two steps that share an endpoint must not blink it. Step 0 holds ↓ and step
/// 1 holds ↓+→; the game must never see the dpad-down bit drop.
#[test]
fn an_endpoint_carried_between_steps_never_flickers() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);

    let deltas = engine.tick(50);
    // ONE delta for the whole transition — releases and presses in one batch.
    assert_eq!(deltas.len(), 1);
    assert!(deltas[0].state.buttons.contains(XButtons::DPAD_DOWN));
    assert!(deltas[0].state.buttons.contains(XButtons::DPAD_RIGHT));
}

/// An empty `hold` is a deliberate neutral gap — how a macro says "let go, then
/// press again" so the game sees two presses instead of one long hold.
#[test]
fn an_empty_step_is_a_gap_the_game_can_see() {
    let mut engine = engine_with(Macro::new(
        "double-tap",
        vec![
            MacroStep::new(vec![PUNCH], 50),
            MacroStep::new(vec![], 50),
            MacroStep::new(vec![PUNCH], 50),
        ],
    ));
    press(&mut engine, Key::P, 0);
    assert_eq!(state(&engine).buttons, XButtons::A);
    engine.tick(50);
    assert_eq!(state(&engine).buttons, XButtons::empty());
    engine.tick(100);
    assert_eq!(state(&engine).buttons, XButtons::A);
    engine.tick(150);
    assert_eq!(state(&engine), PadState::default());
}

// ---------------------------------------------------------------------------
// The sampling rule (§0.2)
// ---------------------------------------------------------------------------

/// A step nobody could see is RAISED to the floor, silently to the engine and
/// loudly to validation. The macro still runs; it just takes longer.
#[test]
fn a_step_below_the_sampling_floor_is_raised() {
    let mut engine = engine_with(Macro::new("blink", vec![MacroStep::new(vec![PUNCH], 5)]));
    press(&mut engine, Key::P, 0);
    assert_eq!(engine.next_deadline(), Some(u64::from(MIN_STEP_MS)));
    // At the requested 5 ms it is still held — that is the whole point.
    assert!(engine.tick(5).is_empty());
    assert_eq!(state(&engine).buttons, XButtons::A);
    engine.tick(u64::from(MIN_STEP_MS));
    assert_eq!(state(&engine), PadState::default());
}

/// ...and the opt-out is honored exactly as written.
#[test]
fn allow_short_runs_the_duration_the_author_asked_for() {
    let mut engine = engine_with(Macro::new("blink", vec![MacroStep::short(vec![PUNCH], 5)]));
    press(&mut engine, Key::P, 0);
    assert_eq!(engine.next_deadline(), Some(5));
    engine.tick(5);
    assert_eq!(state(&engine), PadState::default());
}

/// The anti-drift rule: deadlines are ABSOLUTE offsets from the macro's start,
/// so a wake that is a few ms late is corrected at the very next step instead
/// of pushing the whole sequence back — four steps do not accumulate four
/// scheduler jitters.
#[test]
fn deadlines_are_absolute_so_jitter_never_accumulates() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);

    // Every wake is 3 ms late. The schedule does not care.
    engine.tick(53);
    assert_eq!(engine.next_deadline(), Some(100));
    engine.tick(103);
    assert_eq!(engine.next_deadline(), Some(150));
    engine.tick(153);
    assert_eq!(engine.next_deadline(), Some(200));
    // ...and the macro still ends at 200, not at 212.
    assert_eq!(engine.tick(203).len(), 1);
    assert_eq!(state(&engine), PadState::default());
}

/// A wake so late that a step's whole window has already passed. The step is
/// NOT skipped — a skipped step is an input the game never sampled, which §0.2
/// forbids — it is published for its sampling minimum and the timeline slides.
#[test]
fn a_very_late_tick_still_publishes_every_step_for_the_sampling_minimum() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);

    // The loop was starved for 500 ms: steps 1, 2 and 3 were all due.
    assert_eq!(engine.tick(500).len(), 1);
    assert_eq!(
        state(&engine).buttons,
        XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT
    );
    // Its absolute slot (100 ms) is long gone, so it gets the floor instead.
    assert_eq!(engine.next_deadline(), Some(500 + u64::from(MIN_STEP_MS)));

    // And every remaining step is still delivered, each visibly.
    let mut seen = vec![state(&engine).buttons];
    let mut now = 500;
    while let Some(deadline) = engine.next_deadline() {
        now = deadline;
        engine.tick(now);
        seen.push(state(&engine).buttons);
    }
    assert_eq!(
        seen,
        vec![
            XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT,
            XButtons::DPAD_RIGHT,
            XButtons::A,
            XButtons::empty(),
        ],
        "no step may be swallowed by a late scheduler"
    );
    // Three more steps, each given exactly the floor and no more.
    assert_eq!(now, 500 + 3 * u64::from(MIN_STEP_MS));
}

/// `frames = N` is the fighting-game unit, converted once at 60 Hz. It buys
/// readability, not a frame lock (the game's polling phase is unknowable).
#[test]
fn frames_are_a_readable_unit_for_the_same_schedule() {
    let mut engine = engine_with(Macro::new(
        "shoryuken",
        vec![
            MacroStep::frames(vec![RIGHT], 3),
            MacroStep::frames(vec![DOWN], 3),
            MacroStep::frames(vec![PUNCH], 2),
        ],
    ));
    press(&mut engine, Key::P, 0);
    // 3 frames = 50 ms, 2 frames = 33 ms — rounded once, so three frames is 50
    // and not 3 x 17.
    assert_eq!(engine.next_deadline(), Some(50));
    engine.tick(50);
    assert_eq!(engine.next_deadline(), Some(100));
    engine.tick(100);
    assert_eq!(state(&engine).buttons, XButtons::A);
    // Two frames is exactly the sampling floor, so it is never raised.
    assert_eq!(engine.next_deadline(), Some(100 + u64::from(MIN_STEP_MS)));
}

// ---------------------------------------------------------------------------
// Interruption policy
// ---------------------------------------------------------------------------

/// The default, and the fighting-game expectation: you tap the button and the
/// quarter-circle comes out whole.
#[test]
fn on_release_finish_completes_after_the_key_is_already_up() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);
    // Let go 10 ms in. Nothing changes.
    assert_eq!(release(&mut engine, Key::P, 10), 0);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_DOWN);
    assert_eq!(engine.next_deadline(), Some(50));
    for (t, buttons) in [
        (50, XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT),
        (100, XButtons::DPAD_RIGHT),
        (150, XButtons::A),
        (200, XButtons::empty()),
    ] {
        engine.tick(t);
        assert_eq!(state(&engine).buttons, buttons, "at {t} ms");
    }
}

/// `abort` is the hold-to-autofire shape: let go and it stops NOW, with
/// everything it held released in that same batch.
#[test]
fn on_release_abort_neutralizes_immediately() {
    let mut mac = hadouken();
    mac.on_release = OnRelease::Abort;
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert_eq!(
        state(&engine).buttons,
        XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT
    );

    let deltas = release(&mut engine, Key::P, 60);
    assert_eq!(deltas, 1, "one batch, everything down");
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
    // And no ghost step ever fires.
    assert!(engine.tick(1_000).is_empty());
}

/// Mashing a macro button must not stutter the sequence back to step 0 — which
/// is exactly what a panel with any switch bounce would otherwise do.
#[test]
fn retrigger_ignore_leaves_a_run_in_flight_alone() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert_eq!(engine.next_deadline(), Some(100));

    release(&mut engine, Key::P, 60);
    assert_eq!(press(&mut engine, Key::P, 70), 0, "the press is swallowed");
    // Still on step 1, still due at 100 — the second press changed nothing.
    assert_eq!(
        state(&engine).buttons,
        XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT
    );
    assert_eq!(engine.next_deadline(), Some(100));
}

#[test]
fn retrigger_restart_goes_back_to_step_zero_in_one_batch() {
    let mut mac = hadouken();
    mac.retrigger = Retrigger::Restart;
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    engine.tick(50);
    engine.tick(100);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_RIGHT);

    // Restart at 120: step 0 again, and the right-dpad bit the old step held is
    // released in the SAME delta as the new step's press.
    release(&mut engine, Key::P, 110);
    let deltas = engine.handle_at(&ev(&ipac_device(), Key::P, true), 120);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state.buttons, XButtons::DPAD_DOWN);
    assert_eq!(engine.next_deadline(), Some(170));
}

/// `interrupt = "any-input"`: touch the panel and the sequence stops. The abort
/// takes the same release-everything path as every other exit.
#[test]
fn interrupt_any_input_aborts_on_any_other_bound_key() {
    let mut mac = hadouken();
    mac.interrupt = Interrupt::AnyInput;
    let mut engine = engine_with_entries(mac, vec![(Key::G, Binding::Button(XButton::B))]);

    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert!(!state(&engine).buttons.is_empty());

    // One press: the macro's state goes and B arrives, in one batch.
    let deltas = engine.handle_at(&ev(&ipac_device(), Key::G, true), 60);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state.buttons, XButtons::B);
    assert_eq!(engine.next_deadline(), None);

    // A RELEASE is not "input" for this purpose — only a key going down is.
    // (P has to come up first: a second key-down for a key that is already
    // down is an autorepeat, and autorepeat starts nothing — see
    // `an_autorepeated_trigger_runs_the_macro_exactly_once`.)
    release(&mut engine, Key::P, 90);
    press(&mut engine, Key::P, 100);
    assert_eq!(release(&mut engine, Key::G, 110), 1);
    assert_eq!(engine.next_deadline(), Some(150));
}

/// The macro's own trigger never interrupts it — that is a retrigger, and
/// `Retrigger` decides it.
#[test]
fn interrupt_never_fires_on_the_macros_own_trigger() {
    let mut mac = hadouken();
    mac.interrupt = Interrupt::AnyInput;
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    release(&mut engine, Key::P, 10);
    press(&mut engine, Key::P, 20);
    // Still running, still on step 0, still due at 50 (retrigger = ignore).
    assert_eq!(state(&engine).buttons, XButtons::DPAD_DOWN);
    assert_eq!(engine.next_deadline(), Some(50));
}

/// `interrupt = "opposing"`: only input that CONTRADICTS the macro stops it.
/// Rule 1 — a direction against one the current step is holding.
#[test]
fn interrupt_opposing_aborts_on_a_contradicting_direction_only() {
    let mut mac = hadouken();
    mac.interrupt = Interrupt::Opposing;
    let mut engine = engine_with_entries(
        mac,
        vec![
            (Key::J, Binding::Dpad(DpadDirection::Left)),
            (Key::G, Binding::Button(XButton::B)),
        ],
    );

    // Step 0 holds ↓. A punch is unrelated and passes straight through.
    press(&mut engine, Key::P, 0);
    press(&mut engine, Key::G, 5);
    assert_eq!(
        engine.next_deadline(),
        Some(50),
        "a punch is not opposition"
    );
    assert!(state(&engine).buttons.contains(XButtons::DPAD_DOWN));
    release(&mut engine, Key::G, 6);

    // ← against ↓ is not opposition either: different control.
    press(&mut engine, Key::J, 10);
    assert_eq!(engine.next_deadline(), Some(50));
    release(&mut engine, Key::J, 11);

    // Step 2 holds →. NOW ← contradicts it, and the macro stops.
    engine.tick(50);
    engine.tick(100);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_RIGHT);
    let deltas = engine.handle_at(&ev(&ipac_device(), Key::J, true), 110);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state.buttons, XButtons::DPAD_LEFT);
    assert_eq!(engine.next_deadline(), None);
}

/// Rule 2 — asking for a DIFFERENT sequence is asking to stop this one.
#[test]
fn interrupt_opposing_aborts_when_another_macro_is_asked_for() {
    let mut first = hadouken();
    first.interrupt = Interrupt::Opposing;
    let defs = vec![
        first,
        Macro::new("uppercut", vec![MacroStep::new(vec![PUNCH], 50)]),
    ];
    let triggers = vec![MacroTrigger::new(Key::P, 0), MacroTrigger::new(Key::Q, 1)];
    let p = preset_with_macros("two", Vec::new(), macros(defs, triggers));
    let mut engine = Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }]);

    press(&mut engine, Key::P, 0);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_DOWN);
    // One press: the quarter-circle is abandoned and the uppercut starts, in
    // the same batch — no frame where the pad holds both.
    let deltas = engine.handle_at(&ev(&ipac_device(), Key::Q, true), 20);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state.buttons, XButtons::A);
    assert_eq!(engine.next_deadline(), Some(70));
}

/// `interrupt = "none"` (the default) means exactly that: a macro you started
/// is a macro that runs.
#[test]
fn interrupt_none_ignores_everything_but_the_trigger() {
    let mut engine = engine_with_entries(
        hadouken(),
        vec![(Key::J, Binding::Dpad(DpadDirection::Left))],
    );
    press(&mut engine, Key::P, 0);
    engine.tick(50);
    engine.tick(100);
    // ← straight against the macro's →, and the run does not care.
    press(&mut engine, Key::J, 110);
    assert_eq!(engine.next_deadline(), Some(150));
    assert!(state(&engine).buttons.contains(XButtons::DPAD_RIGHT));
}

// ---------------------------------------------------------------------------
// Everything releases on the way out
// ---------------------------------------------------------------------------

/// Device yank mid-macro. Nobody is going to release a macro's "key", so the
/// engine has to cancel it — this is the stuck-key invariant for sequences.
#[test]
fn a_device_yank_cancels_the_macro_and_releases_what_it_held() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert!(!state(&engine).buttons.is_empty());

    let deltas = engine.release_device(&ipac_device());
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state, PadState::default());
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
    assert!(engine.tick(1_000).is_empty());
}

/// The binding hot-swap. A run in flight is dropped with the tables it came
/// from — stepping a sequence whose steps no longer exist is not a thing ksx
/// will do — and the neutral deltas say so on the wire.
#[test]
fn a_hot_swap_releases_what_a_macro_held() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);
    for t in [50, 100, 150] {
        engine.tick(t);
    }
    assert_eq!(state(&engine).buttons, XButtons::A);

    let plain = preset("plain", vec![(Key::G, Binding::Button(XButton::B))]);
    let tables = EngineTables::build(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, plain.name.clone()).expect("valid slot"),
        preset: plain,
    }]);
    let deltas = engine.swap_tables(tables);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state, PadState::default());
    assert_eq!(engine.next_deadline(), None);
    assert!(engine.tick(1_000).is_empty());
}

/// The primitive session stop and the emergency escape gesture both use.
#[test]
fn cancel_macros_releases_everything_in_one_batch() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);
    engine.tick(50);

    let deltas = engine.cancel_macros();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state, PadState::default());
    assert_eq!(engine.next_deadline(), None);
    // Idempotent: cancelling nothing produces nothing.
    assert!(engine.cancel_macros().is_empty());
}

#[test]
fn reset_clears_macro_state_and_every_armed_deadline() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);
    engine.tick(50);

    engine.reset();
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
    assert!(engine.tick(1_000).is_empty());
    // ...and the macro can be started again from scratch.
    press(&mut engine, Key::P, 2_000);
    assert_eq!(state(&engine).buttons, XButtons::DPAD_DOWN);
}

// ---------------------------------------------------------------------------
// Composition with the rest of the engine
// ---------------------------------------------------------------------------

/// A macro is a HOLDER, so the all-keys-up rule covers it exactly like a key:
/// an endpoint driven by both stays down while either drives it.
#[test]
fn a_macro_and_a_key_can_hold_the_same_endpoint() {
    let mut engine = engine_with_entries(
        Macro::new("punch", vec![MacroStep::new(vec![PUNCH], 50)]),
        vec![(Key::G, PUNCH)],
    );
    press(&mut engine, Key::G, 0);
    assert_eq!(state(&engine).buttons, XButtons::A);

    press(&mut engine, Key::P, 10);
    // The macro's step ends while G is still down: A must survive.
    engine.tick(60);
    assert_eq!(state(&engine).buttons, XButtons::A);
    release(&mut engine, Key::G, 70);
    assert_eq!(state(&engine), PadState::default());
}

/// The opposite-axis snap sees a macro step like any other holder.
#[test]
fn a_macro_axis_step_snaps_against_a_held_key() {
    let left = Binding::Axis {
        axis: Axis::X,
        value: -20_000,
    };
    let mut engine = engine_with_entries(
        Macro::new(
            "dash",
            vec![MacroStep::new(
                vec![Binding::Axis {
                    axis: Axis::X,
                    value: AXIS_MAX,
                }],
                50,
            )],
        ),
        vec![(Key::G, left)],
    );
    press(&mut engine, Key::G, 0);
    assert_eq!(state(&engine).lx, -20_000);
    press(&mut engine, Key::P, 0);
    assert_eq!(state(&engine).lx, AXIS_MAX);
    // Step ends with the LEFT key still held: snap back to its own value, not
    // to centre and not to a hardcoded ±32767.
    engine.tick(50);
    assert_eq!(state(&engine).lx, -20_000);
}

/// One key may start several macros, and several keys may start one — multi
/// bind is native here too (§1a).
#[test]
fn triggers_fan_out_in_both_directions() {
    let defs = vec![
        Macro::new("a", vec![MacroStep::new(vec![PUNCH], 50)]),
        Macro::new(
            "b",
            vec![MacroStep::new(vec![Binding::Trigger(Trigger::Right)], 50)],
        ),
    ];
    let triggers = vec![
        MacroTrigger::new(Key::P, 0),
        MacroTrigger::new(Key::P, 1),
        MacroTrigger::new(Key::Q, 0),
    ];
    let p = preset_with_macros("fanout", Vec::new(), macros(defs, triggers));
    let mut engine = Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }]);

    // One key, both macros, one batch.
    assert_eq!(press(&mut engine, Key::P, 0), 1);
    assert_eq!(state(&engine).buttons, XButtons::A);
    assert_eq!(state(&engine).rt, u8::MAX);
    engine.tick(50);
    assert_eq!(state(&engine), PadState::default());

    // The other key starts the first macro on its own.
    press(&mut engine, Key::Q, 100);
    assert_eq!(state(&engine).buttons, XButtons::A);
    assert_eq!(state(&engine).rt, 0);
}

/// Two macros armed for the same millisecond fire in the order they were
/// armed — a tie is never a coin flip, because the replay corpus depends on it.
#[test]
fn simultaneous_deadlines_are_deterministic() {
    let defs = vec![
        Macro::new("a", vec![MacroStep::new(vec![PUNCH], 50)]),
        Macro::new(
            "b",
            vec![MacroStep::new(vec![Binding::Button(XButton::B)], 50)],
        ),
    ];
    let triggers = vec![MacroTrigger::new(Key::P, 0), MacroTrigger::new(Key::P, 1)];
    let p = preset_with_macros("tie", Vec::new(), macros(defs, triggers));
    let mut engine = Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }]);

    press(&mut engine, Key::P, 0);
    assert_eq!(state(&engine).buttons, XButtons::A | XButtons::B);
    // Both due at 50: one tick clears both, in one delta.
    let deltas = engine.tick(50);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state, PadState::default());
}

// ---------------------------------------------------------------------------
// Inert shapes, and the regression guarantee
// ---------------------------------------------------------------------------

/// A macro with no steps and a trigger pointing nowhere are both accepted and
/// do nothing at all — loading is lenient, validation names them.
#[test]
fn an_empty_macro_and_a_dangling_trigger_are_inert() {
    let p = preset_with_macros(
        "inert",
        vec![(Key::G, PUNCH)],
        macros(
            vec![Macro::new("nothing", vec![])],
            vec![MacroTrigger::new(Key::P, 0), MacroTrigger::new(Key::Q, 7)],
        ),
    );
    let mut engine = Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }]);
    assert_eq!(press(&mut engine, Key::P, 0), 0);
    assert_eq!(press(&mut engine, Key::Q, 0), 0);
    assert_eq!(engine.next_deadline(), None);
    assert_eq!(state(&engine), PadState::default());
    // The ordinary binding in the same preset is untouched.
    assert_eq!(press(&mut engine, Key::G, 0), 1);
    assert_eq!(state(&engine).buttons, XButtons::A);
}

/// The regression guarantee, stated as code: a preset with no macros never
/// arms anything, so `next_deadline` is `None` forever and the engine thread
/// sleeps on its input channel exactly as it did before macros existed. This is
/// what keeps the M3 replay corpus digest fixed.
#[test]
fn a_macro_free_preset_never_arms_a_deadline() {
    let mut engine = common::ipac_engine();
    assert_eq!(engine.next_deadline(), None);
    for key in common::ipac_bound_keys().into_iter().take(12) {
        engine.handle_at(&ev(&ipac_device(), key, true), 1);
        assert_eq!(engine.next_deadline(), None, "{key:?} armed something");
    }
    assert!(engine.tick(10_000).is_empty());
    assert!(engine.cancel_macros().is_empty());
}

// ---------------------------------------------------------------------------
// The diagonal, on the mechanism Victor's cabinet actually drives
// ---------------------------------------------------------------------------
//
// His stick is bound to the LEFT ANALOG AXES (`lx.min/max`, `ly.min/max`) with
// the dpad unbound, which is a different code path from the dpad hadouken
// above: dpad directions are independent BITS in one field, while axes are
// whole-field ASSIGNMENTS with an opposite-axis snap on release. A two-binding
// step therefore has to survive a mechanism where "press" can overwrite and
// "release" can snap — so it gets its own tests rather than an argument.

const DOWN_AX: Binding = Binding::Axis {
    axis: Axis::Y,
    value: ksx_core::AXIS_MIN,
};
const RIGHT_AX: Binding = Binding::Axis {
    axis: Axis::X,
    value: AXIS_MAX,
};

/// The same motion as `hadouken()`, written the way his preset drives.
fn axis_hadouken() -> Macro {
    Macro::new(
        "hadouken",
        vec![
            MacroStep::frames(vec![DOWN_AX], 3),
            MacroStep::frames(vec![DOWN_AX, RIGHT_AX], 3),
            MacroStep::frames(vec![RIGHT_AX], 3),
            MacroStep::frames(vec![PUNCH], 3),
        ],
    )
}

/// His preset's own direction rows: axes bound, dpad unbound.
fn axis_entries() -> Vec<(Key, Binding)> {
    vec![
        (Key::K, DOWN_AX),
        (
            Key::L,
            Binding::Axis {
                axis: Axis::Y,
                value: AXIS_MAX,
            },
        ),
        (
            Key::M,
            Binding::Axis {
                axis: Axis::X,
                value: ksx_core::AXIS_MIN,
            },
        ),
        (Key::N, RIGHT_AX),
    ]
}

/// ↘ written as ONE step holding two axis bindings is one published state with
/// both axes deflected — and it stays that state for the step's whole duration,
/// so a poller sampling anywhere inside the step reads the diagonal.
#[test]
fn an_axis_diagonal_is_one_stable_state_for_the_whole_step() {
    let mut engine = engine_with_entries(axis_hadouken(), axis_entries());
    press(&mut engine, Key::P, 0);
    assert_eq!(
        (state(&engine).lx, state(&engine).ly),
        (0, ksx_core::AXIS_MIN)
    );

    // One delta for the transition, carrying BOTH deflections.
    let deltas = engine.tick(50);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state.lx, AXIS_MAX);
    assert_eq!(deltas[0].state.ly, ksx_core::AXIS_MIN);

    // Sampled every millisecond of the step, it never stops being the diagonal:
    // no release-then-press hole, and the shared ly binding never blinks.
    for t in 50..100 {
        engine.tick(t);
        assert_eq!(
            (state(&engine).lx, state(&engine).ly),
            (AXIS_MAX, ksx_core::AXIS_MIN),
            "lost the diagonal at t={t}"
        );
    }
    // ...and only then does the down half let go, snapping to centre.
    engine.tick(100);
    assert_eq!((state(&engine).lx, state(&engine).ly), (AXIS_MAX, 0));
}

/// A 60 Hz consumer — the one the sampling rule is written for — sees all three
/// direction states, diagonal included.
#[test]
fn a_60hz_sampler_sees_every_step_of_an_axis_motion() {
    let mut engine = engine_with_entries(axis_hadouken(), axis_entries());
    press(&mut engine, Key::P, 0);
    let mut sampled: Vec<(i16, i16)> = Vec::new();
    for t in 0..250 {
        engine.tick(t);
        if t % 16 == 0 {
            let s = state(&engine);
            if sampled.last() != Some(&(s.lx, s.ly)) {
                sampled.push((s.lx, s.ly));
            }
        }
    }
    assert!(
        sampled.contains(&(AXIS_MAX, ksx_core::AXIS_MIN)),
        "a 60 Hz sampler never saw the diagonal: {sampled:?}"
    );
}

/// Perpendicular is not opposing: SOCD cleaning cancels ←+→ and ↑+↓, and must
/// leave ↓+→ completely alone (`crate::socd::opposes` is the one definition,
/// shared with `Interrupt::Opposing`).
#[test]
fn socd_cleaning_never_touches_a_perpendicular_diagonal() {
    for policy in [ksx_core::Socd::Neutral, ksx_core::Socd::UpPriority] {
        let mut p = preset_with_macros(
            "socd",
            axis_entries(),
            macros(vec![axis_hadouken()], vec![MacroTrigger::new(Key::P, 0)]),
        );
        p.apply_socd(policy);
        let mut engine = Engine::new(vec![ResolvedSlot {
            spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
            preset: p,
        }]);
        press(&mut engine, Key::P, 0);
        engine.tick(50);
        assert_eq!(
            (state(&engine).lx, state(&engine).ly),
            (AXIS_MAX, ksx_core::AXIS_MIN),
            "socd {policy} ate the diagonal"
        );
    }
}

/// A step may hold two DIFFERENT mechanisms at once (a dpad bit and a stick
/// axis). Both are published; neither overwrites the other.
#[test]
fn a_step_may_hold_a_dpad_bit_and_an_axis_together() {
    let mac = Macro::new(
        "mixed",
        vec![
            MacroStep::new(vec![DOWN], 50),
            MacroStep::new(vec![DOWN, RIGHT_AX], 50),
            MacroStep::new(vec![RIGHT_AX], 50),
        ],
    );
    let mut engine = engine_with_entries(mac, axis_entries());
    press(&mut engine, Key::P, 0);
    engine.tick(50);
    let s = state(&engine);
    assert!(
        s.buttons.contains(XButtons::DPAD_DOWN) && s.lx == AXIS_MAX,
        "{s:?}"
    );
}

// ---------------------------------------------------------------------------
// Repeat: once (the default), while-held, turbo (docs/INPUT-TRANSFORMS.md §1c)
// ---------------------------------------------------------------------------

/// A one-step macro, so a run is one step long and the repeat arithmetic is
/// readable at a glance.
fn tap() -> Macro {
    Macro::new("tap", vec![MacroStep::new(vec![PUNCH], 50)])
}

/// **The turbo bug.** Windows repeats a held key ~30 times a second, and every
/// repeat is another key-DOWN for a key that is already down. Before this, each
/// one re-armed a finished macro — "if I hold it, it never stops".
///
/// The default is one run per press, and a key that never came up never
/// produced a second press.
#[test]
fn an_autorepeated_trigger_runs_the_macro_exactly_once() {
    let mut engine = engine_with(hadouken());
    press(&mut engine, Key::P, 0);

    // Held down the whole time; autorepeat every 33 ms, as a real keyboard does.
    let mut repeat = 33;
    for t in 1..=400 {
        if t == repeat {
            assert_eq!(
                press(&mut engine, Key::P, t),
                0,
                "autorepeat at {t} moved a pad"
            );
            repeat += 33;
        }
        engine.tick(t);
    }
    // One run, 200 ms, finished — and nothing armed, however long he leans on it.
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);

    // A genuine second press (key up, key down) still starts a second run.
    release(&mut engine, Key::P, 500);
    press(&mut engine, Key::P, 501);
    assert_eq!(engine.next_deadline(), Some(551));
}

/// The same, said as an invariant: an autorepeat is not an interrupt either.
/// `interrupt = "any-input"` must not abort a run because some other key is
/// repeating under the player's thumb.
#[test]
fn an_autorepeat_of_another_key_does_not_interrupt() {
    let mut mac = hadouken();
    mac.interrupt = Interrupt::AnyInput;
    let mut engine = engine_with_entries(mac, vec![(Key::G, Binding::Button(XButton::B))]);

    press(&mut engine, Key::G, 0); // already leaning on B
    press(&mut engine, Key::P, 10);
    assert_eq!(engine.next_deadline(), Some(60));
    // G repeats while held: not a new press, so not an interrupt.
    assert_eq!(press(&mut engine, Key::G, 20), 0);
    assert_eq!(engine.next_deadline(), Some(60));
    // A genuine new press of G still aborts, exactly as documented.
    release(&mut engine, Key::G, 30);
    press(&mut engine, Key::G, 40);
    assert_eq!(engine.next_deadline(), None);
}

/// `repeat = "while-held"`: the sequence runs again the instant it ends, for as
/// long as the trigger is down — and the run in flight is never cut short.
#[test]
fn while_held_reruns_until_the_trigger_comes_up() {
    let mut mac = tap();
    mac.repeat = Repeat::WhileHeld;
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    assert_eq!(state(&engine).buttons, XButtons::A);
    // End of run 1 goes straight into run 2, no gap: A never lets go.
    engine.tick(50);
    assert_eq!(state(&engine).buttons, XButtons::A);
    assert_eq!(engine.next_deadline(), Some(100));
    engine.tick(100);
    assert_eq!(state(&engine).buttons, XButtons::A);

    // Let go mid-run: `on_release = finish` (the default) means this run
    // completes — and then nothing follows it.
    release(&mut engine, Key::P, 120);
    assert_eq!(engine.next_deadline(), Some(150));
    assert_eq!(state(&engine).buttons, XButtons::A);
    engine.tick(150);
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
}

/// `repeat = "turbo"`: the same, with a REAL released window between runs, so
/// the game reads two presses instead of one hold.
#[test]
fn turbo_puts_a_visible_gap_between_runs() {
    let mut mac = tap();
    mac.repeat = Repeat::Turbo;
    mac.turbo = Some(TurboRate::GapMs(50));
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    assert_eq!(state(&engine).buttons, XButtons::A);
    // Run ends at 50: everything releases, and the gap is a published state.
    assert_eq!(engine.tick(50).len(), 1);
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), Some(100));
    // Still neutral all the way through the gap — a gap nobody samples is not
    // a gap (§0.2).
    for t in 50..100 {
        engine.tick(t);
        assert_eq!(state(&engine), PadState::default(), "gap broke at t={t}");
    }
    // Gap over: run 2.
    assert_eq!(engine.tick(100).len(), 1);
    assert_eq!(state(&engine).buttons, XButtons::A);
    assert_eq!(engine.next_deadline(), Some(150));

    // Released during the gap: the next run never starts, nothing is armed,
    // and nothing is left held.
    release(&mut engine, Key::P, 160);
    engine.tick(150);
    engine.tick(200);
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
}

/// Everything releases on the way out — from a turbo GAP too, where the macro
/// holds nothing but is still very much armed.
#[test]
fn cancelling_during_a_turbo_gap_disarms_it() {
    let mut mac = tap();
    mac.repeat = Repeat::Turbo;
    mac.turbo = Some(TurboRate::Hz(10));
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert_eq!(state(&engine), PadState::default());
    assert!(engine.next_deadline().is_some(), "a gap is still armed");

    engine.cancel_macros();
    assert_eq!(engine.next_deadline(), None);
    assert!(engine.tick(10_000).is_empty());
    assert_eq!(state(&engine), PadState::default());
}

/// Precedence, as code: `interrupt` beats `repeat`. An abort is an EXIT, so no
/// repeat follows it however hard the trigger is held.
#[test]
fn an_interrupt_ends_the_repeat_loop_not_just_the_run() {
    let mut mac = tap();
    mac.repeat = Repeat::WhileHeld;
    mac.interrupt = Interrupt::AnyInput;
    let mut engine = engine_with_entries(mac, vec![(Key::G, Binding::Button(XButton::B))]);

    press(&mut engine, Key::P, 0);
    engine.tick(50); // run 2 under way, trigger still held
    assert_eq!(state(&engine).buttons, XButtons::A);
    press(&mut engine, Key::G, 60);
    assert_eq!(engine.next_deadline(), None);
    assert_eq!(state(&engine).buttons, XButtons::B);
    assert!(engine.tick(10_000).is_empty());
}

/// ...and so does `on_release = "abort"`: it stops the run, which stops the
/// loop, in the same batch.
#[test]
fn abort_on_release_stops_a_while_held_loop_immediately() {
    let mut mac = tap();
    mac.repeat = Repeat::WhileHeld;
    mac.on_release = OnRelease::Abort;
    let mut engine = engine_with(mac);

    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert_eq!(state(&engine).buttons, XButtons::A);
    assert_eq!(release(&mut engine, Key::P, 60), 1);
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
}

/// A repeat is not a press: `retrigger` only ever decides what a NEW press does
/// to a run in flight, and a while-held loop keeps running under either policy.
#[test]
fn repeat_and_retrigger_are_different_questions() {
    for policy in [Retrigger::Ignore, Retrigger::Restart] {
        let mut mac = tap();
        mac.repeat = Repeat::WhileHeld;
        mac.retrigger = policy;
        let mut engine = engine_with(mac);
        press(&mut engine, Key::P, 0);
        engine.tick(50);
        engine.tick(100);
        assert_eq!(state(&engine).buttons, XButtons::A, "{policy}");
        assert_eq!(engine.next_deadline(), Some(150), "{policy}");
    }
}

/// The regression guarantee for the whole feature: `once` is the default, and a
/// macro that does not ask to repeat behaves exactly as it always did.
#[test]
fn the_default_is_still_one_run_per_press() {
    assert_eq!(Repeat::default(), Repeat::Once);
    let mut engine = engine_with(tap());
    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert_eq!(state(&engine), PadState::default());
    assert_eq!(engine.next_deadline(), None);
}

// ---------------------------------------------------------------------------
// The two cabinet reports, as tests (2026-08-06)
// ---------------------------------------------------------------------------
//
// Both arrived as "the engine is doing something my macro does not say", and
// `ksx macro trace` on his own preset showed what was really happening: his key
// `Q` starts THREE macros (`op`, `ppp`, `test`), so what a poller reads is their
// SUPERPOSITION, and one of the three is a `repeat = "turbo"`. The tests below
// pin both halves — the engine's own guarantees, which hold, and the shape that
// produced the report, which is a configuration nobody was told about.

/// **Report A: "a phantom down between the diagonal and forward".**
///
/// The engine's half of it, stated exactly: a step transition publishes ONE
/// state, and a binding both steps hold is never released and re-pressed. The
/// assertion is the whole published sequence, so ANY intermediate state — a
/// phantom direction, a phantom neutral, a re-press — fails it.
#[test]
fn a_step_transition_publishes_exactly_one_state_with_no_intermediate() {
    fn qcf() -> Macro {
        Macro::new(
            "qcf",
            vec![
                MacroStep::frames(vec![DOWN_AX], 7),
                MacroStep::frames(vec![DOWN_AX, RIGHT_AX], 7),
                MacroStep::frames(vec![RIGHT_AX], 7),
            ],
        )
    }
    let mut engine = engine_with_entries(qcf(), axis_entries());

    let mut published: Vec<(i16, i16)> = Vec::new();
    for delta in engine.handle_at(&ev(&ipac_device(), Key::P, true), 0) {
        published.push((delta.state.lx, delta.state.ly));
    }
    for t in 1..=400 {
        for delta in engine.tick(t) {
            published.push((delta.state.lx, delta.state.ly));
        }
    }

    // Four states for three steps plus the release. Nothing between them: no
    // (0, AXIS_MIN) after the diagonal (the phantom down), and no (0, 0)
    // either (the phantom neutral).
    assert_eq!(
        published,
        vec![
            (0, ksx_core::AXIS_MIN),
            (AXIS_MAX, ksx_core::AXIS_MIN),
            (AXIS_MAX, 0),
            (0, 0),
        ],
        "a step transition published more than one state"
    );

    // ...and said the other way, per millisecond: 7 frames is 117 ms, so lx is
    // handed from step 1 to step 2 at t=234 and must not blink across it.
    let mut engine = engine_with_entries(qcf(), axis_entries());
    press(&mut engine, Key::P, 0);
    for t in 1..=350 {
        engine.tick(t);
        if t >= 117 {
            assert_eq!(state(&engine).lx, AXIS_MAX, "lx blinked at t={t}");
        }
    }
}

/// The same guarantee for a BUTTON carried across steps, which is the case
/// where a re-press is not cosmetic: a game reading the press EDGE would fire
/// the move twice.
#[test]
fn a_button_held_across_steps_is_pressed_once_and_never_re_pressed() {
    let mut engine = engine_with(Macro::new(
        "hold-through",
        vec![
            MacroStep::new(vec![PUNCH, DOWN], 50),
            MacroStep::new(vec![PUNCH], 50),
            MacroStep::new(vec![PUNCH, RIGHT], 50),
        ],
    ));

    let mut published: Vec<XButtons> = Vec::new();
    for delta in engine.handle_at(&ev(&ipac_device(), Key::P, true), 0) {
        published.push(delta.state.buttons);
    }
    for t in 1..=200 {
        for delta in engine.tick(t) {
            published.push(delta.state.buttons);
        }
    }
    assert_eq!(
        published,
        vec![
            XButtons::A | XButtons::DPAD_DOWN,
            XButtons::A,
            XButtons::A | XButtons::DPAD_RIGHT,
            XButtons::empty(),
        ]
    );
    // The A bit is set in every state but the last: it was pressed once, at the
    // start, and released once, at the end.
    assert!(published[..3].iter().all(|b| b.contains(XButtons::A)));
}

/// **What actually produced report A.** Two macros on ONE key superimpose, and
/// the superposition contains states neither macro's step list has. This is
/// documented multi-bind (`triggers_fan_out_in_both_directions`), not a bug —
/// but it is exactly what "the engine invented a direction my macro does not
/// have" looks like from the cabinet, so it gets a test that says so.
#[test]
fn two_macros_on_one_key_superimpose_into_states_neither_one_holds() {
    // His shapes: both are down / forward, one at 50 ms a step and one at 7
    // frames (117 ms), both started by the same key.
    let defs = vec![
        Macro::new(
            "fast",
            vec![
                MacroStep::new(vec![DOWN_AX], 50),
                MacroStep::new(vec![RIGHT_AX], 50),
            ],
        ),
        Macro::new(
            "slow",
            vec![
                MacroStep::frames(vec![DOWN_AX], 7),
                MacroStep::frames(vec![RIGHT_AX], 7),
            ],
        ),
    ];
    let p = preset_with_macros(
        "shared-trigger",
        axis_entries(),
        macros(
            defs,
            vec![MacroTrigger::new(Key::P, 0), MacroTrigger::new(Key::P, 1)],
        ),
    );
    let mut engine = Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }]);

    press(&mut engine, Key::P, 0);
    assert_eq!(
        (state(&engine).lx, state(&engine).ly),
        (0, ksx_core::AXIS_MIN)
    );
    // t=50: `fast` moved to forward, `slow` is still down. Neither macro has a
    // diagonal step; together they publish one anyway.
    engine.tick(50);
    assert_eq!(
        (state(&engine).lx, state(&engine).ly),
        (AXIS_MAX, ksx_core::AXIS_MIN),
        "the two macros did not superimpose"
    );
    // t=100: `fast` is done and lets go of forward; `slow` is still on down, so
    // the "phantom down after the diagonal" is simply the slower macro, still
    // running.
    engine.tick(100);
    assert_eq!(
        (state(&engine).lx, state(&engine).ly),
        (0, ksx_core::AXIS_MIN)
    );
    engine.tick(117);
    assert_eq!((state(&engine).lx, state(&engine).ly), (AXIS_MAX, 0));
}

/// **Report B: "`repeat = once` still repeats while I hold the key".**
///
/// The trigger is pressed and HELD for ten run-lengths of virtual time, and the
/// number of RUNS is counted off the published states. `once` runs exactly one,
/// whatever the key does afterwards; `while-held` runs one per run-length; and
/// `turbo` runs one per cycle, which is the run PLUS its gap.
#[test]
fn holding_the_trigger_repeats_exactly_as_the_policy_says() {
    // A run is 100 ms and ends released, so every re-run is a visible press
    // EDGE of A — which is what a game counts, and therefore what this counts.
    fn cycle_macro() -> Macro {
        Macro::new(
            "cycle",
            vec![MacroStep::new(vec![PUNCH], 50), MacroStep::new(vec![], 50)],
        )
    }

    // Press, hold for `hold_ms` of virtual time, and count press edges of A.
    fn runs_while_held(mac: Macro, hold_ms: u64) -> usize {
        let mut engine = engine_with(mac);
        press(&mut engine, Key::P, 0);
        let mut down = state(&engine).buttons.contains(XButtons::A);
        let mut runs = usize::from(down);
        for t in 1..=hold_ms {
            engine.tick(t);
            let now = state(&engine).buttons.contains(XButtons::A);
            runs += usize::from(now && !down);
            down = now;
        }
        runs
    }

    // `once`: ONE run in ten run-lengths of holding. This is the report.
    let mut once = cycle_macro();
    once.repeat = Repeat::Once;
    assert_eq!(runs_while_held(once, 1_000), 1);
    // ...and nothing is left armed, so no later wake can start another.
    let mut engine = engine_with(cycle_macro());
    press(&mut engine, Key::P, 0);
    for t in 1..=1_000 {
        engine.tick(t);
    }
    assert_eq!(engine.next_deadline(), None);
    assert_eq!(state(&engine), PadState::default());

    // `while-held`: a run per 100 ms run-length — t=0, 100, ... 1000.
    let mut held = cycle_macro();
    held.repeat = Repeat::WhileHeld;
    assert_eq!(runs_while_held(held, 1_000), 11);

    // `turbo`: a run per CYCLE, and the cycle is the run plus the gap it asked
    // for — 100 + 50 = 150 ms, so t=0, 150, ... 900.
    let mut turbo = cycle_macro();
    turbo.repeat = Repeat::Turbo;
    turbo.turbo = Some(TurboRate::GapMs(50));
    assert_eq!(turbo.turbo_cycle_ms(), 150);
    assert_eq!(runs_while_held(turbo, 1_000), 7);
}

/// The other half of report B, said as an invariant: NOTHING re-arms a `once`
/// macro whose run has ended. Not the trigger still being down, not the
/// keyboard repeating it, and not another key on the same slot moving.
#[test]
fn nothing_re_arms_a_once_macro_while_its_trigger_is_still_down() {
    let mut engine = engine_with_entries(tap(), vec![(Key::G, Binding::Button(XButton::B))]);
    press(&mut engine, Key::P, 0);
    engine.tick(50);
    assert_eq!(engine.next_deadline(), None);

    for t in 51..=1_000 {
        // Autorepeat of the trigger, of another bound key, and that key going
        // genuinely up and down — none of it is a new press of P.
        if t % 33 == 0 {
            press(&mut engine, Key::P, t);
        }
        if t % 50 == 0 {
            press(&mut engine, Key::G, t);
        }
        if t % 70 == 0 {
            release(&mut engine, Key::G, t);
        }
        engine.tick(t);
        assert_eq!(engine.next_deadline(), None, "something re-armed at t={t}");
        assert!(
            !state(&engine).buttons.contains(XButtons::A),
            "the macro ran again at t={t}"
        );
    }
}

/// **What actually produced report B.** A `once` macro whose key ALSO starts a
/// `turbo` macro: the `once` one runs once, exactly as it says, and the turbo
/// one keeps going for as long as the key is down — which from the cabinet is
/// indistinguishable from "once still repeats".
#[test]
fn a_turbo_macro_on_the_same_key_is_what_keeps_repeating() {
    let mut turbo = Macro::new(
        "op",
        vec![MacroStep::new(vec![Binding::Trigger(Trigger::Right)], 50)],
    );
    turbo.repeat = Repeat::Turbo;
    turbo.turbo = Some(TurboRate::GapMs(50));
    let p = preset_with_macros(
        "shared-trigger-turbo",
        Vec::new(),
        macros(
            vec![tap(), turbo],
            vec![MacroTrigger::new(Key::P, 0), MacroTrigger::new(Key::P, 1)],
        ),
    );
    let mut engine = Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(ipac_device()), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }]);

    press(&mut engine, Key::P, 0);
    let mut a_runs = 0usize;
    let mut rt_runs = 0usize;
    let (mut a_down, mut rt_down) = (false, false);
    for t in 0..=1_000 {
        engine.tick(t);
        let s = state(&engine);
        let (a, rt) = (s.buttons.contains(XButtons::A), s.rt != 0);
        a_runs += usize::from(a && !a_down);
        rt_runs += usize::from(rt && !rt_down);
        (a_down, rt_down) = (a, rt);
    }
    // The `once` macro obeyed its policy...
    assert_eq!(a_runs, 1);
    // ...and the turbo on the same key is the thing that never stopped.
    assert!(rt_runs > 5, "the turbo macro stopped repeating: {rt_runs}");

    // Both still go out the same door: one cancel releases everything and
    // disarms everything, turbo gap included.
    engine.cancel_macros();
    assert_eq!(engine.next_deadline(), None);
    assert_eq!(state(&engine), PadState::default());
    assert!(engine.tick(10_000).is_empty());
}
