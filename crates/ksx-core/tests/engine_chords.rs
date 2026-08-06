//! CHORDS: many physical keys → one virtual output, with consumption
//! (docs/INPUT-TRANSFORMS.md §1b).
//!
//! Every test here asserts the two properties Victor asked for by name:
//!
//! - **Consumption** — while a chord is active, the individual bindings of its
//!   constituent keys are suppressed, and anything they were holding is
//!   released in the SAME delta batch (no stranded buttons).
//! - **Resumption** — lifting one constituent releases the chord and the keys
//!   still held resume their own bindings in that same batch (no flicker, no
//!   stuck bit).
//!
//! The absence of chords is covered elsewhere and is the regression guarantee:
//! `engine_rules.rs`, `engine_proptests.rs` and the M3 replay corpus all run
//! chord-free presets and must stay byte-identical.

mod common;

use common::{engine_for, ev, ipac_device, preset_with_chords};
use ksx_core::{
    Axis, Binding, Chord, DeviceId, Engine, Key, PadState, Trigger, XButton, AXIS_MAX, AXIS_MIN,
};

fn button(b: XButton) -> Binding {
    Binding::Button(b)
}

/// Feed one event and return the slot's state afterwards, plus the deltas.
fn step(engine: &mut Engine, dev: &DeviceId, key: Key, down: bool) -> (PadState, usize) {
    let deltas = engine.handle(&ev(dev, key, down));
    (engine.pad_state(1).expect("slot 1"), deltas.len())
}

fn state(engine: &Engine) -> PadState {
    engine.pad_state(1).expect("slot 1")
}

// ---------------------------------------------------------------------------
// Activation, consumption, release
// ---------------------------------------------------------------------------

/// The shape the docs recommend: chord keys with NO individual binding. Then
/// consumption costs nothing, there is no flash, and no press is deferred.
#[test]
fn dedicated_chord_keys_produce_the_chord_and_nothing_else() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "chord",
        vec![(Key::G, button(XButton::A))],
        vec![Chord::new(
            Key::D,
            Binding::Trigger(Trigger::Right),
            vec![Key::F],
        )],
    ));

    // Half the chord: nothing at all reaches the pad.
    let (after, deltas) = step(&mut engine, &dev, Key::F, true);
    assert_eq!(deltas, 0, "an unbound chord key alone must emit nothing");
    assert_eq!(after, PadState::default());

    // Completed: RT, in one delta.
    let (after, deltas) = step(&mut engine, &dev, Key::D, true);
    assert_eq!(deltas, 1);
    assert_eq!(after.rt, u8::MAX);
    assert_eq!(after.buttons.bits(), 0);

    // Lift either half and it releases.
    let (after, deltas) = step(&mut engine, &dev, Key::F, false);
    assert_eq!(deltas, 1);
    assert_eq!(after, PadState::default());
    step(&mut engine, &dev, Key::D, false);
    assert_eq!(state(&engine), PadState::default());
}

/// The consumption rule proper: A→X, B→Y, and A+B→RT. Completing the chord
/// must take X and Y away in the same batch that puts RT down, and lifting B
/// must give A's X back in the same batch that takes RT away.
#[test]
fn a_chord_consumes_its_constituents_and_hands_them_back_on_release() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "consume",
        vec![(Key::A, button(XButton::X)), (Key::B, button(XButton::Y))],
        vec![Chord::new(
            Key::A,
            Binding::Trigger(Trigger::Right),
            vec![Key::B],
        )],
    ));

    // The documented, deliberate flash: A is individually bound, so the game
    // sees X between the two presses. ksx does not defer input.
    let (after, _) = step(&mut engine, &dev, Key::A, true);
    assert_eq!(
        after.buttons,
        XButton::X.flag(),
        "the honest caveat, in one line"
    );

    // Completing the chord: X released, Y never appears, RT down — ONE delta,
    // so no intermediate state ever reaches the pad.
    let (after, deltas) = step(&mut engine, &dev, Key::B, true);
    assert_eq!(deltas, 1, "activation must be a single delta batch");
    assert_eq!(
        after.buttons.bits(),
        0,
        "consumed constituents drive nothing"
    );
    assert_eq!(after.rt, u8::MAX);

    // Lift B: RT goes, A resumes X — again one batch, no neutral flicker.
    let (after, deltas) = step(&mut engine, &dev, Key::B, false);
    assert_eq!(deltas, 1, "release + resume must be a single delta batch");
    assert_eq!(after.rt, 0);
    assert_eq!(after.buttons, XButton::X.flag(), "A is held: X comes back");

    // And back to neutral.
    step(&mut engine, &dev, Key::A, false);
    assert_eq!(state(&engine), PadState::default());
}

/// Order does not matter: a chord is a SET of held keys, not a sequence
/// (docs/INPUT-TRANSFORMS.md §0 — ksx publishes state).
#[test]
fn either_press_order_completes_the_chord() {
    let dev = ipac_device();
    let make = || {
        engine_for(preset_with_chords(
            "order",
            vec![(Key::A, button(XButton::X)), (Key::B, button(XButton::Y))],
            vec![Chord::new(
                Key::A,
                Binding::Trigger(Trigger::Left),
                vec![Key::B],
            )],
        ))
    };

    let mut ab = make();
    step(&mut ab, &dev, Key::A, true);
    step(&mut ab, &dev, Key::B, true);

    let mut ba = make();
    step(&mut ba, &dev, Key::B, true);
    step(&mut ba, &dev, Key::A, true);

    assert_eq!(state(&ab), state(&ba));
    assert_eq!(state(&ab).lt, u8::MAX);
    assert_eq!(state(&ab).buttons.bits(), 0);
}

/// `unless` is MAME's NOT: the same binding, refused while a key is held.
#[test]
fn an_unless_key_blocks_the_chord_and_releasing_it_lets_the_chord_through() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "unless",
        vec![(Key::A, button(XButton::X))],
        vec![Chord {
            key: Key::A,
            binding: Binding::Trigger(Trigger::Right),
            when: vec![Key::B],
            unless: vec![Key::LeftShift],
        }],
    ));

    step(&mut engine, &dev, Key::LeftShift, true);
    step(&mut engine, &dev, Key::B, true);
    let (after, _) = step(&mut engine, &dev, Key::A, true);
    assert_eq!(
        after.buttons,
        XButton::X.flag(),
        "blocked chord consumes nothing, so A keeps its own binding"
    );
    assert_eq!(after.rt, 0);

    // Drop the exclusion: the chord takes over, X goes, in one batch.
    let (after, deltas) = step(&mut engine, &dev, Key::LeftShift, false);
    assert_eq!(deltas, 1);
    assert_eq!(after.rt, u8::MAX);
    assert_eq!(after.buttons.bits(), 0);
}

// ---------------------------------------------------------------------------
// Specificity
// ---------------------------------------------------------------------------

/// A bigger guard wins where the two overlap, and the smaller one comes back
/// the moment the bigger one stops being satisfied.
#[test]
fn the_more_specific_chord_wins_and_the_less_specific_one_returns() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "specificity",
        vec![],
        vec![
            // Deliberately listed least-specific first: build order must not
            // be what decides this.
            Chord::new(Key::A, Binding::Trigger(Trigger::Right), vec![Key::B]),
            Chord::new(Key::A, button(XButton::LeftBumper), vec![Key::B, Key::C]),
        ],
    ));

    step(&mut engine, &dev, Key::A, true);
    let (after, _) = step(&mut engine, &dev, Key::B, true);
    assert_eq!(after.rt, u8::MAX, "A+B alone is the two-key chord");

    let (after, deltas) = step(&mut engine, &dev, Key::C, true);
    assert_eq!(deltas, 1, "hand-over in one batch");
    assert_eq!(after.rt, 0, "A+B is suppressed by A+B+C");
    assert_eq!(after.buttons, XButton::LeftBumper.flag());

    let (after, deltas) = step(&mut engine, &dev, Key::C, false);
    assert_eq!(deltas, 1);
    assert_eq!(after.buttons.bits(), 0);
    assert_eq!(after.rt, u8::MAX, "A+B is satisfied again");

    step(&mut engine, &dev, Key::B, false);
    step(&mut engine, &dev, Key::A, false);
    assert_eq!(state(&engine), PadState::default());
}

/// Two chords with the SAME guard are a multi-bind (one chord, several
/// outputs) — native everywhere else in ksx, and native here. Equal
/// specificity with DIFFERENT guards is what validation refuses.
#[test]
fn identical_guards_are_a_multi_bind_not_a_race() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "multibind",
        vec![],
        vec![
            Chord::new(Key::A, Binding::Trigger(Trigger::Right), vec![Key::B]),
            Chord::new(Key::A, button(XButton::LeftBumper), vec![Key::B]),
        ],
    ));
    step(&mut engine, &dev, Key::A, true);
    let (after, _) = step(&mut engine, &dev, Key::B, true);
    assert_eq!(after.rt, u8::MAX);
    assert_eq!(after.buttons, XButton::LeftBumper.flag());
    step(&mut engine, &dev, Key::B, false);
    assert_eq!(state(&engine), PadState::default());
}

/// Chords of the same size on DIFFERENT triggers never interfere.
#[test]
fn disjoint_chords_coexist() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "disjoint",
        vec![],
        vec![
            Chord::new(Key::A, Binding::Trigger(Trigger::Left), vec![Key::B]),
            Chord::new(Key::C, Binding::Trigger(Trigger::Right), vec![Key::D]),
        ],
    ));
    for key in [Key::A, Key::B, Key::C, Key::D] {
        step(&mut engine, &dev, key, true);
    }
    let after = state(&engine);
    assert_eq!((after.lt, after.rt), (u8::MAX, u8::MAX));
}

// ---------------------------------------------------------------------------
// The rules chords must not break
// ---------------------------------------------------------------------------

/// All-keys-up, across holder kinds: an endpoint driven by BOTH a plain key
/// and a chord stays down while either drives it.
#[test]
fn all_keys_up_counts_a_chord_as_a_holder() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "shared-endpoint",
        vec![(Key::G, button(XButton::A))],
        vec![Chord::new(Key::D, button(XButton::A), vec![Key::F])],
    ));

    step(&mut engine, &dev, Key::G, true);
    assert_eq!(state(&engine).buttons, XButton::A.flag());
    step(&mut engine, &dev, Key::F, true);
    let (after, deltas) = step(&mut engine, &dev, Key::D, true);
    assert_eq!(deltas, 0, "A was already down; no state change to emit");
    assert_eq!(after.buttons, XButton::A.flag());

    // The plain key lets go, the chord still holds it.
    let (after, deltas) = step(&mut engine, &dev, Key::G, false);
    assert_eq!(deltas, 0, "the chord still drives A");
    assert_eq!(after.buttons, XButton::A.flag());

    // Now the chord lets go too.
    let (after, _) = step(&mut engine, &dev, Key::F, false);
    assert_eq!(after, PadState::default());
}

/// Opposite-axis snap still applies when the axis is driven by a chord.
#[test]
fn a_chord_participates_in_the_opposite_axis_snap() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "axis",
        vec![(
            Key::N,
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MAX,
            },
        )],
        vec![Chord::new(
            Key::D,
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN,
            },
            vec![Key::F],
        )],
    ));

    step(&mut engine, &dev, Key::N, true);
    assert_eq!(state(&engine).lx, AXIS_MAX);
    step(&mut engine, &dev, Key::F, true);
    step(&mut engine, &dev, Key::D, true);
    assert_eq!(state(&engine).lx, AXIS_MIN, "the chord pressed last wins");
    // Break the chord: the held opposite key snaps the axis back to ITS value.
    step(&mut engine, &dev, Key::F, false);
    assert_eq!(state(&engine).lx, AXIS_MAX);
    step(&mut engine, &dev, Key::N, false);
    assert_eq!(state(&engine).lx, 0);
}

/// Unplug mid-chord: nothing may be left holding anything.
#[test]
fn release_device_mid_chord_leaves_no_stuck_output() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "unplug",
        vec![(Key::A, button(XButton::X))],
        vec![Chord::new(
            Key::A,
            Binding::Trigger(Trigger::Right),
            vec![Key::B],
        )],
    ));
    step(&mut engine, &dev, Key::A, true);
    step(&mut engine, &dev, Key::B, true);
    assert_eq!(state(&engine).rt, u8::MAX);

    let deltas = engine.release_device(&dev);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state, PadState::default());
    assert_eq!(state(&engine), PadState::default());
    // Idempotent, and the engine is usable afterwards.
    assert!(engine.release_device(&dev).is_empty());
    step(&mut engine, &dev, Key::A, true);
    assert_eq!(state(&engine).buttons, XButton::X.flag());
}

/// `reset` clears chord state too — otherwise a stopped session would come
/// back believing a chord was still held.
#[test]
fn reset_clears_chord_state() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "reset",
        vec![],
        vec![Chord::new(
            Key::A,
            Binding::Trigger(Trigger::Right),
            vec![Key::B],
        )],
    ));
    step(&mut engine, &dev, Key::A, true);
    step(&mut engine, &dev, Key::B, true);
    assert_eq!(state(&engine).rt, u8::MAX);

    engine.reset();
    assert_eq!(state(&engine), PadState::default());
    // A fresh press sequence behaves as if nothing had ever been held.
    step(&mut engine, &dev, Key::A, true);
    assert_eq!(state(&engine), PadState::default());
    step(&mut engine, &dev, Key::B, true);
    assert_eq!(state(&engine).rt, u8::MAX);
}

/// The binding hot-swap must neutralize a chord that was held across the edit,
/// exactly as it does for a plain binding (the `swap_tables` discipline).
#[test]
fn swapping_tables_mid_chord_neutralizes_the_pad() {
    use ksx_core::{EngineTables, ResolvedSlot, SlotSpec};
    let dev = ipac_device();
    let chorded = preset_with_chords(
        "swap",
        vec![],
        vec![Chord::new(
            Key::A,
            Binding::Trigger(Trigger::Right),
            vec![Key::B],
        )],
    );
    let mut engine = engine_for(chorded.clone());
    step(&mut engine, &dev, Key::A, true);
    step(&mut engine, &dev, Key::B, true);
    assert_eq!(state(&engine).rt, u8::MAX);

    let mut edited = chorded;
    edited.chords[0].binding = Binding::Trigger(Trigger::Left);
    let deltas = engine.swap_tables(EngineTables::build(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(dev.clone()), None, edited.name.clone()).expect("slot"),
        preset: edited,
    }]));
    assert_eq!(
        deltas.len(),
        1,
        "the held RT must be released across the swap"
    );
    assert_eq!(deltas[0].state, PadState::default());
    assert_eq!(state(&engine), PadState::default());

    // Re-press against the new tables: the edited chord is what fires.
    step(&mut engine, &dev, Key::A, true);
    step(&mut engine, &dev, Key::B, true);
    assert_eq!(state(&engine).lt, u8::MAX);
    assert_eq!(state(&engine).rt, 0);
}

/// A chord in slot 1 must not disturb slot 2's plain bindings off the same
/// keyboard — the I-PAC fan-out is untouched by consumption.
#[test]
fn consumption_is_per_slot() {
    use ksx_core::{Engine, ResolvedSlot, SlotSpec};
    let dev = ipac_device();
    let chorded = preset_with_chords(
        "P1",
        vec![(Key::A, button(XButton::X))],
        vec![Chord::new(
            Key::A,
            Binding::Trigger(Trigger::Right),
            vec![Key::B],
        )],
    );
    let plain = common::preset(
        "P2",
        vec![(Key::A, button(XButton::Y)), (Key::B, button(XButton::B))],
    );
    let mut engine = Engine::new(vec![
        ResolvedSlot {
            spec: SlotSpec::new(1, Some(dev.clone()), None, "P1".to_owned()).expect("slot"),
            preset: chorded,
        },
        ResolvedSlot {
            spec: SlotSpec::new(2, Some(dev.clone()), None, "P2".to_owned()).expect("slot"),
            preset: plain,
        },
    ]);

    engine.handle(&ev(&dev, Key::A, true));
    engine.handle(&ev(&dev, Key::B, true));
    let p1 = engine.pad_state(1).expect("slot 1");
    let p2 = engine.pad_state(2).expect("slot 2");
    assert_eq!(p1.rt, u8::MAX);
    assert_eq!(p1.buttons.bits(), 0, "slot 1 consumed A");
    assert_eq!(
        p2.buttons,
        XButton::Y.flag() | XButton::B.flag(),
        "slot 2 never heard of the chord"
    );
}

/// A chord keyed `Key::None` is an inert placeholder, like an unbound entry
/// row — it must never fire and must never consume.
#[test]
fn a_chord_with_no_trigger_key_is_inert() {
    let dev = ipac_device();
    let mut engine = engine_for(preset_with_chords(
        "inert",
        vec![(Key::B, button(XButton::Y))],
        vec![Chord::new(
            Key::None,
            Binding::Trigger(Trigger::Right),
            vec![Key::B],
        )],
    ));
    let (after, _) = step(&mut engine, &dev, Key::B, true);
    assert_eq!(after.buttons, XButton::Y.flag());
    assert_eq!(after.rt, 0);
}

// ---------------------------------------------------------------------------
// Model-based property test
// ---------------------------------------------------------------------------

/// An independent, DECLARATIVE statement of the same semantics: given the set
/// of keys currently down, which buttons should be set.
///
/// The engine computes this incrementally, edge by edge; this computes it from
/// scratch, whole-state at a time. They must agree after every single event —
/// which is what makes "no stuck key, no missed hand-over, no order
/// dependence" a proof rather than a hope. (Buttons only: axes are
/// deliberately order-dependent by design — see the opposite-axis snap — and
/// so are not a pure function of the down set.)
fn model_buttons(
    entries: &[(Key, XButton)],
    chords: &[(Key, XButton, Vec<Key>, Vec<Key>)],
    down: &std::collections::BTreeSet<Key>,
) -> u16 {
    let specificity = |c: &(Key, XButton, Vec<Key>, Vec<Key>)| c.2.len() + c.3.len();
    let mut ordered: Vec<&(Key, XButton, Vec<Key>, Vec<Key>)> = chords.iter().collect();
    ordered.sort_by_key(|c| std::cmp::Reverse(specificity(c)));

    let mut bits = 0u16;
    let mut consumed: std::collections::BTreeSet<Key> = Default::default();
    let mut i = 0;
    while i < ordered.len() {
        let level = specificity(ordered[i]);
        let mut j = i;
        while j < ordered.len() && specificity(ordered[j]) == level {
            j += 1;
        }
        let blocked = consumed.clone();
        for chord in &ordered[i..j] {
            let (trigger, button, when, unless) = chord;
            let satisfied = down.contains(trigger)
                && when.iter().all(|k| down.contains(k))
                && !unless.iter().any(|k| down.contains(k));
            let is_blocked = blocked.contains(trigger) || when.iter().any(|k| blocked.contains(k));
            if satisfied && !is_blocked {
                bits |= button.flag().bits();
                consumed.insert(*trigger);
                consumed.extend(when.iter().copied());
            }
        }
        i = j;
    }
    for (key, button) in entries {
        if down.contains(key) && !consumed.contains(key) {
            bits |= button.flag().bits();
        }
    }
    bits
}

proptest::proptest! {
    /// Random press/release traffic over a preset with overlapping chords:
    /// the engine's state must equal the declarative model after EVERY event,
    /// and everything must be neutral once the keys come up.
    #[test]
    fn engine_matches_the_declarative_model(
        script in proptest::collection::vec((0usize..5, proptest::bool::ANY), 1..80)
    ) {
        let keys = [Key::A, Key::B, Key::C, Key::D, Key::LeftShift];
        let entries: Vec<(Key, XButton)> = vec![
            (Key::A, XButton::X),
            (Key::B, XButton::Y),
            (Key::D, XButton::Back),
        ];
        let chords: Vec<(Key, XButton, Vec<Key>, Vec<Key>)> = vec![
            (Key::A, XButton::RightBumper, vec![Key::B], vec![Key::LeftShift]),
            (Key::A, XButton::LeftBumper, vec![Key::B, Key::C], vec![]),
            (Key::D, XButton::Start, vec![Key::C], vec![]),
        ];

        let dev = ipac_device();
        let mut engine = engine_for(preset_with_chords(
            "model",
            entries.iter().map(|(k, b)| (*k, Binding::Button(*b))).collect(),
            chords
                .iter()
                .map(|(k, b, when, unless)| Chord {
                    key: *k,
                    binding: Binding::Button(*b),
                    when: when.clone(),
                    unless: unless.clone(),
                })
                .collect(),
        ));

        let mut down: std::collections::BTreeSet<Key> = Default::default();
        for (i, (index, is_down)) in script.iter().enumerate() {
            let key = keys[*index];
            engine.handle(&ev(&dev, key, *is_down));
            if *is_down {
                down.insert(key);
            } else {
                down.remove(&key);
            }
            let expected = model_buttons(&entries, &chords, &down);
            let actual = engine.pad_state(1).expect("slot 1").buttons.bits();
            proptest::prop_assert_eq!(
                actual,
                expected,
                "event {} ({:?} {}) diverged from the model; down = {:?}",
                i,
                key,
                if *is_down { "down" } else { "up" },
                down
            );
        }

        // Everything up: nothing may be left held.
        for key in keys {
            engine.handle(&ev(&dev, key, false));
        }
        proptest::prop_assert_eq!(engine.pad_state(1).expect("slot 1"), PadState::default());
    }
}
