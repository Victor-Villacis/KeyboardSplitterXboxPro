//! Per-binding turbo: auto-fire written on the binding row itself
//! (docs/INPUT-TRANSFORMS.md §3).
//!
//! Same fake clock as the macro tests, and for the same reason: the engine is
//! handed `now` and never reads one, so "A auto-fires at 12 Hz" is reproducible
//! to the millisecond in CI with no hardware and no sleeping.

mod common;

use common::{ev, ipac_device, preset, preset_with_macros, preset_with_turbo};
use ksx_core::{
    Axis, Binding, Chord, DeviceId, Engine, EngineTables, Key, Macro, MacroStep, MacroTrigger,
    Macros, PadState, ResolvedSlot, SlotSpec, TurboBinding, XButton, XButtons, AXIS_MAX,
    MIN_STEP_MS, TURBO_MAX_HZ,
};

const A: Binding = Binding::Button(XButton::A);
const RT: Binding = Binding::Trigger(ksx_core::Trigger::Right);

/// 12 Hz is the doc's own example, and it is deliverable exactly: an 83 ms
/// cycle splits into 42 ms pressed and 41 ms released, both comfortably above
/// the sampling floor.
const HZ: u32 = 12;
const ON_MS: u64 = 42;
const OFF_MS: u64 = 41;

fn engine_for(p: ksx_core::Preset) -> Engine {
    let dev = ipac_device();
    Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(dev), None, p.name.clone()).expect("valid slot"),
        preset: p,
    }])
}

/// `A = { key = "G", turbo_hz = 12 }` — the whole feature in one row.
fn engine_turbo_a() -> Engine {
    engine_for(preset_with_turbo(
        "turbo",
        vec![(Key::G, A)],
        Vec::new(),
        vec![TurboBinding::new(A, HZ)],
    ))
}

fn state(engine: &Engine) -> PadState {
    engine.pad_state(1).expect("slot 1")
}

fn a_down(engine: &Engine) -> bool {
    state(engine).buttons.contains(XButtons::A)
}

fn press(engine: &mut Engine, key: Key, now: u64) {
    engine.handle_at(&ev(&ipac_device(), key, true), now);
}

fn release(engine: &mut Engine, key: Key, now: u64) {
    engine.handle_at(&ev(&ipac_device(), key, false), now);
}

// ---- the rate arithmetic ---------------------------------------------------

/// The clamp is arithmetic, not policy: one cycle is a press AND a release, and
/// each half must survive a 60 Hz poll, so the ceiling a file may ASK for is
/// [`TURBO_MAX_HZ`] and the rate it GETS is capped again by the floor.
#[test]
fn the_rate_is_clamped_to_what_a_sampler_can_see() {
    let asked = TurboBinding::new(A, 12);
    assert_eq!(asked.on_ms(), ON_MS as u32);
    assert_eq!(asked.off_ms(), OFF_MS as u32);
    assert_eq!(asked.effective_hz(), 12);
    assert_eq!(asked.clamped(), None, "12 Hz is deliverable as written");

    // Above the ceiling: clamped to 30, and then each half is floored at
    // MIN_STEP_MS, which is why the honest answer is ~15 Hz and not 30.
    let greedy = TurboBinding::new(A, 240);
    assert_eq!(greedy.clamped_hz(), TURBO_MAX_HZ);
    assert_eq!(greedy.on_ms(), MIN_STEP_MS);
    assert_eq!(greedy.off_ms(), MIN_STEP_MS);
    assert_eq!(greedy.effective_hz(), 15);
    assert_eq!(greedy.clamped(), Some((240, 15)));

    // 15 Hz is the fastest rate that is BOTH askable and deliverable.
    assert_eq!(TurboBinding::new(A, 15).clamped(), None);
    assert_eq!(TurboBinding::new(A, 16).clamped(), Some((16, 15)));

    // Zero means "off" in a file, never a division by zero here.
    assert_eq!(TurboBinding::new(A, 0).effective_hz(), 1);
}

// ---- the schedule ----------------------------------------------------------

/// Hold the key, the button auto-fires. The first press is on the DOWN EDGE,
/// not half a cycle later — an auto-fire that swallows the first press is a
/// button that feels broken.
#[test]
fn holding_the_key_alternates_the_button_on_schedule() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    assert!(a_down(&e), "the first press is immediate");
    assert_eq!(e.next_deadline(), Some(ON_MS));

    // A tick before the deadline changes nothing and emits nothing.
    assert!(e.tick(ON_MS - 1).is_empty());
    assert!(a_down(&e));

    assert_eq!(e.tick(ON_MS).len(), 1, "the released half is a real delta");
    assert!(!a_down(&e));
    assert_eq!(e.next_deadline(), Some(ON_MS + OFF_MS));

    assert_eq!(e.tick(ON_MS + OFF_MS).len(), 1);
    assert!(a_down(&e), "and round again");
    assert_eq!(e.next_deadline(), Some(ON_MS + OFF_MS + ON_MS));
}

/// Letting go releases NOW, mid-press. A player who let go does not owe the
/// game the rest of a cycle.
#[test]
fn releasing_the_key_releases_the_button_immediately() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    assert!(a_down(&e));

    release(&mut e, Key::G, 10);
    assert!(!a_down(&e));
    assert_eq!(e.next_deadline(), None, "and the clock is disarmed");

    // Nothing is left armed to press it again behind the player's back.
    assert!(e.tick(1_000).is_empty());
    assert!(!a_down(&e));
}

/// Letting go during the RELEASED half must not leave the clock armed either —
/// a turbo resting between presses holds nothing but is still very much alive.
#[test]
fn releasing_during_the_off_half_disarms_the_clock() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    e.tick(ON_MS);
    assert!(!a_down(&e));

    release(&mut e, Key::G, ON_MS + 5);
    assert_eq!(e.next_deadline(), None);
    assert!(e.tick(10_000).is_empty());
    assert!(!a_down(&e));
}

/// Keyboard autorepeat is not a new press: Windows resends key-down ~30 times a
/// second for a held key, and a turbo that restarted its cycle on each one
/// would never reach its released half at all.
#[test]
fn autorepeat_does_not_restart_the_cycle() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    for t in 1..ON_MS {
        press(&mut e, Key::G, t); // the same key, still down
    }
    assert_eq!(
        e.next_deadline(),
        Some(ON_MS),
        "still the original deadline"
    );
    e.tick(ON_MS);
    assert!(!a_down(&e));
}

// ---- multi-bind ------------------------------------------------------------

/// Several keys → one turbo output is ONE clock. Two keys cannot phase-fight
/// over one button because there is only ever one phase, and the button stays
/// firing while either is down (the all-keys-up rule, one level up).
#[test]
fn several_keys_driving_one_turbo_output_share_one_clock() {
    let mut e = engine_for(preset_with_turbo(
        "multi",
        vec![(Key::G, A), (Key::H, A)],
        Vec::new(),
        vec![TurboBinding::new(A, HZ)],
    ));

    press(&mut e, Key::G, 0);
    assert!(a_down(&e));
    let armed = e.next_deadline();
    assert_eq!(armed, Some(ON_MS));

    // The second key joins a cycle already in flight rather than starting one.
    press(&mut e, Key::H, 10);
    assert_eq!(e.next_deadline(), armed, "no second clock, no re-phasing");
    assert!(a_down(&e));

    e.tick(ON_MS);
    assert!(!a_down(&e), "one phase, both keys");

    // Releasing one of two keys keeps the auto-fire running.
    release(&mut e, Key::G, 50);
    assert!(e.next_deadline().is_some());
    e.tick(ON_MS + OFF_MS);
    assert!(a_down(&e));

    // Releasing the last one stops it.
    release(&mut e, Key::H, 100);
    assert!(!a_down(&e));
    assert_eq!(e.next_deadline(), None);
}

// ---- guards ----------------------------------------------------------------

/// A turbo binding with a `when` guard is legal, and it composes the only way
/// that makes sense: the GUARD decides whether the chord is driving the
/// endpoint, and the TURBO decides what the endpoint does while it is.
#[test]
fn a_guarded_turbo_fires_only_while_the_guard_is_satisfied() {
    let mut e = engine_for(preset_with_turbo(
        "guarded",
        Vec::new(),
        vec![Chord::new(Key::G, RT, vec![Key::F])],
        vec![TurboBinding::new(RT, HZ)],
    ));

    // Trigger alone: the chord is not satisfied, so nothing drives RT.
    press(&mut e, Key::G, 0);
    assert_eq!(state(&e).rt, 0);
    assert_eq!(e.next_deadline(), None);

    // Guard arrives: the chord activates and the auto-fire starts pressed.
    press(&mut e, Key::F, 10);
    assert_eq!(state(&e).rt, u8::MAX);
    assert_eq!(e.next_deadline(), Some(10 + ON_MS));

    e.tick(10 + ON_MS);
    assert_eq!(state(&e).rt, 0, "released half");

    // Guard falls mid-cycle: the chord stops driving, so the turbo stops and
    // the endpoint is released — in the same delta batch, with nothing armed.
    e.tick(10 + ON_MS + OFF_MS);
    assert_eq!(state(&e).rt, u8::MAX);
    release(&mut e, Key::F, 200);
    assert_eq!(state(&e).rt, 0);
    assert_eq!(e.next_deadline(), None);
}

// ---- composition with macros ----------------------------------------------

/// A macro step that holds a turbo'd endpoint drives it FLAT for the step's
/// duration. A sequence already owns a timeline; running its steps through a
/// second clock would make the sequence unreproducible.
#[test]
fn a_macro_step_drives_a_turbo_endpoint_flat() {
    let mut p = preset_with_macros(
        "both",
        vec![(Key::G, A)],
        Macros {
            defs: vec![Macro::new("jab", vec![MacroStep::new(vec![A], 100)])],
            triggers: vec![MacroTrigger::new(Key::P, 0)],
        },
    );
    p.turbo = vec![TurboBinding::new(A, HZ)];
    let mut e = engine_for(p);

    press(&mut e, Key::P, 0);
    assert!(a_down(&e));
    // The macro's own step end is the only deadline the step contributes; the
    // turbo never armed because no BINDING row is driving A.
    assert_eq!(e.next_deadline(), Some(100));
    e.tick(ON_MS);
    assert!(a_down(&e), "the step is not chopped up by the turbo clock");
    e.tick(100);
    assert!(!a_down(&e));
}

// ---- every exit path -------------------------------------------------------

/// Session stop / emergency escape.
#[test]
fn cancel_releases_a_turbo_caught_mid_press() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    assert!(a_down(&e));

    let deltas = e.cancel_macros();
    assert_eq!(
        deltas.len(),
        1,
        "the release is published, not just recorded"
    );
    assert!(!a_down(&e));
    assert_eq!(e.next_deadline(), None);
}

/// Cancelling during the released half must still disarm: nothing is held, but
/// the clock would otherwise press a button on a pad the player has left.
#[test]
fn cancel_disarms_a_turbo_caught_between_presses() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    e.tick(ON_MS);
    assert!(!a_down(&e));

    assert!(e.cancel_macros().is_empty(), "nothing was held to release");
    assert_eq!(e.next_deadline(), None);
    assert!(e.tick(10_000).is_empty());
}

/// Device yank mid-press: nobody is going to release that key.
#[test]
fn a_device_yank_releases_a_turbo_caught_mid_press() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    assert!(a_down(&e));

    let deltas = e.release_device(&ipac_device());
    assert_eq!(deltas.len(), 1);
    assert_eq!(state(&e), PadState::default());
    assert_eq!(e.next_deadline(), None);
}

/// Binding hot-swap mid-press: dense ids are an artifact of the old tables, so
/// the auto-fire cannot carry over — and must not be left holding anything.
#[test]
fn a_hot_swap_releases_a_turbo_caught_mid_press() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    assert!(a_down(&e));

    let replacement = preset("plain", vec![(Key::G, Binding::Button(XButton::B))]);
    let dev = ipac_device();
    let deltas = e.swap_tables(EngineTables::build(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(dev), None, replacement.name.clone()).expect("valid slot"),
        preset: replacement,
    }]));
    assert_eq!(deltas.len(), 1);
    assert_eq!(state(&e), PadState::default());
    assert_eq!(e.next_deadline(), None);
}

/// Emulation stop: reset drops the clock with everything else.
#[test]
fn reset_disarms_every_turbo() {
    let mut e = engine_turbo_a();
    press(&mut e, Key::G, 0);
    assert!(a_down(&e));

    e.reset();
    assert_eq!(state(&e), PadState::default());
    assert_eq!(e.next_deadline(), None);
    assert!(e.tick(10_000).is_empty());
}

// ---- regression guarantee --------------------------------------------------

/// A preset with no turbo arms nothing and looks at no clock — the same "you
/// pay for it only if you use it" guarantee chords and macros give.
#[test]
fn a_turbo_free_preset_arms_nothing() {
    let mut e = engine_for(preset("plain", vec![(Key::G, A)]));
    press(&mut e, Key::G, 0);
    assert!(a_down(&e));
    assert_eq!(e.next_deadline(), None);
    assert!(e.tick(10_000).is_empty());
    assert!(a_down(&e), "a plain binding still holds, forever");
}

/// Turbo is not limited to buttons: an axis endpoint auto-fires by snapping
/// back to centre, and the opposite-axis rule still sees the turbo holder.
#[test]
fn an_axis_endpoint_can_auto_fire() {
    let left = Binding::Axis {
        axis: Axis::X,
        value: AXIS_MAX,
    };
    let mut e = engine_for(preset_with_turbo(
        "axis",
        vec![(Key::N, left)],
        Vec::new(),
        vec![TurboBinding::new(left, HZ)],
    ));
    press(&mut e, Key::N, 0);
    assert_eq!(state(&e).lx, AXIS_MAX);
    e.tick(ON_MS);
    assert_eq!(state(&e).lx, 0);
    e.tick(ON_MS + OFF_MS);
    assert_eq!(state(&e).lx, AXIS_MAX);
}

/// Fan-out is untouched: one keyboard feeding two slots, only one of which
/// turbos the button, still drives both.
#[test]
fn turbo_in_one_slot_does_not_disturb_another() {
    let dev: DeviceId = ipac_device();
    let plain = preset("plain", vec![(Key::G, A)]);
    let fast = preset_with_turbo(
        "fast",
        vec![(Key::G, A)],
        Vec::new(),
        vec![TurboBinding::new(A, HZ)],
    );
    let mut e = Engine::new(vec![
        ResolvedSlot {
            spec: SlotSpec::new(1, Some(dev.clone()), None, plain.name.clone()).expect("slot 1"),
            preset: plain,
        },
        ResolvedSlot {
            spec: SlotSpec::new(2, Some(dev), None, fast.name.clone()).expect("slot 2"),
            preset: fast,
        },
    ]);

    press(&mut e, Key::G, 0);
    assert!(e
        .pad_state(1)
        .expect("slot 1")
        .buttons
        .contains(XButtons::A));
    assert!(e
        .pad_state(2)
        .expect("slot 2")
        .buttons
        .contains(XButtons::A));

    e.tick(ON_MS);
    assert!(
        e.pad_state(1)
            .expect("slot 1")
            .buttons
            .contains(XButtons::A),
        "slot 1 has no turbo and must be unaffected by slot 2's clock"
    );
    assert!(!e
        .pad_state(2)
        .expect("slot 2")
        .buttons
        .contains(XButtons::A));
}
