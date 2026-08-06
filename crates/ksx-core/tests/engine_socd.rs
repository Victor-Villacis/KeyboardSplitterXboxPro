//! SOCD cleaning end to end (docs/INPUT-TRANSFORMS.md §2.6).
//!
//! Nothing here reaches into the engine: every case is a preset plus a
//! generated chord set, run through the ordinary event path. That is the claim
//! being tested — SOCD is chord-with-consumption, not a new engine rule.

mod common;

use common::{engine_for, ev, ipac_device, preset};
use ksx_core::{
    Axis, Binding, Chord, DpadDirection, Engine, Key, PadState, Preset, Socd, XButtons, AXIS_MAX,
    AXIS_MIN,
};

fn axis(axis: Axis, value: i16) -> Binding {
    Binding::Axis { axis, value }
}

/// Left stick + dpad on eight distinct keys — a stick panel, both controls.
fn panel() -> Preset {
    preset(
        "panel",
        vec![
            (Key::Left, axis(Axis::X, AXIS_MIN)),
            (Key::Right, axis(Axis::X, AXIS_MAX)),
            (Key::Up, axis(Axis::Y, AXIS_MAX)),
            (Key::Down, axis(Axis::Y, AXIS_MIN)),
            (Key::J, Binding::Dpad(DpadDirection::Left)),
            (Key::L, Binding::Dpad(DpadDirection::Right)),
            (Key::I, Binding::Dpad(DpadDirection::Up)),
            (Key::K, Binding::Dpad(DpadDirection::Down)),
        ],
    )
}

fn engine_with(policy: Socd) -> Engine {
    let mut preset = panel();
    preset.apply_socd(policy);
    engine_for(preset)
}

/// Press/release a sequence and read the slot's final pad state.
fn run(engine: &mut Engine, steps: &[(Key, bool)]) -> PadState {
    let dev = ipac_device();
    for &(key, down) in steps {
        engine.handle(&ev(&dev, key, down));
    }
    engine.pad_state(1).expect("slot 1")
}

// ---- off: the regression guarantee ----------------------------------------

#[test]
fn off_reports_both_directions_exactly_as_before() {
    let mut engine = engine_with(Socd::Off);
    let state = run(&mut engine, &[(Key::Left, true), (Key::Right, true)]);
    // Last press wins the axis field, which is precisely today's behavior.
    assert_eq!(state.lx, AXIS_MAX);

    let mut engine = engine_with(Socd::Off);
    let state = run(&mut engine, &[(Key::I, true), (Key::K, true)]);
    assert!(state.buttons.contains(XButtons::DPAD_UP));
    assert!(state.buttons.contains(XButtons::DPAD_DOWN));

    // ...and the preset is untouched: no chords, so the chord-free engine
    // path is what ran.
    let mut untouched = panel();
    untouched.apply_socd(Socd::Off);
    assert!(untouched.chords.is_empty());
}

// ---- neutral ---------------------------------------------------------------

#[test]
fn neutral_centres_the_stick_on_left_plus_right() {
    let mut engine = engine_with(Socd::Neutral);
    assert_eq!(run(&mut engine, &[(Key::Left, true)]).lx, AXIS_MIN);
    assert_eq!(run(&mut engine, &[(Key::Right, true)]).lx, 0);
    // Releasing the newcomer hands the axis back to the survivor, in one
    // batch and with no intermediate state.
    assert_eq!(run(&mut engine, &[(Key::Right, false)]).lx, AXIS_MIN);
    assert_eq!(run(&mut engine, &[(Key::Left, false)]).lx, 0);
}

#[test]
fn neutral_centres_the_stick_on_up_plus_down() {
    let mut engine = engine_with(Socd::Neutral);
    assert_eq!(run(&mut engine, &[(Key::Up, true)]).ly, AXIS_MAX);
    assert_eq!(run(&mut engine, &[(Key::Down, true)]).ly, 0);
    assert_eq!(run(&mut engine, &[(Key::Up, false)]).ly, AXIS_MIN);
    assert_eq!(run(&mut engine, &[(Key::Down, false)]).ly, 0);
}

#[test]
fn neutral_clears_both_dpad_bits() {
    let mut engine = engine_with(Socd::Neutral);
    let state = run(&mut engine, &[(Key::J, true), (Key::L, true)]);
    assert!(!state.buttons.contains(XButtons::DPAD_LEFT));
    assert!(!state.buttons.contains(XButtons::DPAD_RIGHT));

    let state = run(&mut engine, &[(Key::I, true), (Key::K, true)]);
    assert!(!state.buttons.contains(XButtons::DPAD_UP));
    assert!(!state.buttons.contains(XButtons::DPAD_DOWN));

    // Lift one half of each pair: the other comes straight back.
    let state = run(&mut engine, &[(Key::L, false), (Key::K, false)]);
    assert!(state.buttons.contains(XButtons::DPAD_LEFT));
    assert!(state.buttons.contains(XButtons::DPAD_UP));
}

#[test]
fn neutral_leaves_the_perpendicular_axis_alone() {
    // Left+Right cancels; Up, held throughout, must not even flicker.
    let mut engine = engine_with(Socd::Neutral);
    let state = run(
        &mut engine,
        &[(Key::Up, true), (Key::Left, true), (Key::Right, true)],
    );
    assert_eq!((state.lx, state.ly), (0, AXIS_MAX));
}

// ---- up-priority -----------------------------------------------------------

#[test]
fn up_priority_keeps_up_when_down_arrives() {
    let mut engine = engine_with(Socd::UpPriority);
    assert_eq!(run(&mut engine, &[(Key::Up, true)]).ly, AXIS_MAX);
    // Down is swallowed entirely — this is the down-back → up-back jump.
    assert_eq!(run(&mut engine, &[(Key::Down, true)]).ly, AXIS_MAX);
    // Releasing Up while Down is still held hands the axis to Down.
    assert_eq!(run(&mut engine, &[(Key::Up, false)]).ly, AXIS_MIN);
    assert_eq!(run(&mut engine, &[(Key::Down, false)]).ly, 0);
}

#[test]
fn up_priority_also_wins_when_up_arrives_second() {
    // Activation is state, not sequence: press order must not matter.
    let mut engine = engine_with(Socd::UpPriority);
    assert_eq!(run(&mut engine, &[(Key::Down, true)]).ly, AXIS_MIN);
    assert_eq!(run(&mut engine, &[(Key::Up, true)]).ly, AXIS_MAX);
    assert_eq!(run(&mut engine, &[(Key::Down, false)]).ly, AXIS_MAX);
    assert_eq!(run(&mut engine, &[(Key::Up, false)]).ly, 0);
}

#[test]
fn up_priority_keeps_the_dpad_up_bit_and_drops_down() {
    let mut engine = engine_with(Socd::UpPriority);
    let state = run(&mut engine, &[(Key::I, true), (Key::K, true)]);
    assert!(state.buttons.contains(XButtons::DPAD_UP));
    assert!(!state.buttons.contains(XButtons::DPAD_DOWN));
    // Release Up: Down takes over rather than being stuck out.
    let state = run(&mut engine, &[(Key::I, false)]);
    assert!(!state.buttons.contains(XButtons::DPAD_UP));
    assert!(state.buttons.contains(XButtons::DPAD_DOWN));
}

#[test]
fn up_priority_still_neutralizes_left_plus_right() {
    // The tournament rule is asymmetric on purpose: horizontal cancels.
    let mut engine = engine_with(Socd::UpPriority);
    let state = run(&mut engine, &[(Key::Left, true), (Key::Right, true)]);
    assert_eq!(state.lx, 0);
    let state = run(&mut engine, &[(Key::J, true), (Key::L, true)]);
    assert!(!state.buttons.contains(XButtons::DPAD_LEFT));
    assert!(!state.buttons.contains(XButtons::DPAD_RIGHT));
}

// ---- the invariants the chord engine already promised ----------------------

#[test]
fn nothing_is_ever_stuck_once_every_key_is_up() {
    for policy in [Socd::Off, Socd::Neutral, Socd::UpPriority] {
        let mut engine = engine_with(policy);
        let keys = [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::J,
            Key::L,
            Key::I,
            Key::K,
        ];
        let downs: Vec<(Key, bool)> = keys.iter().map(|&k| (k, true)).collect();
        run(&mut engine, &downs);
        // Reverse order on the way out — the worst case for a handover.
        let ups: Vec<(Key, bool)> = keys.iter().rev().map(|&k| (k, false)).collect();
        assert_eq!(
            run(&mut engine, &ups),
            PadState::default(),
            "{policy} left something held"
        );
    }
}

#[test]
fn a_yanked_device_releases_everything_a_consume_chord_was_holding() {
    let mut engine = engine_with(Socd::UpPriority);
    let dev = ipac_device();
    run(
        &mut engine,
        &[
            (Key::Left, true),
            (Key::Right, true),
            (Key::Up, true),
            (Key::Down, true),
        ],
    );
    engine.release_device(&dev);
    assert_eq!(engine.pad_state(1), Some(PadState::default()));
}

#[test]
fn a_consume_only_chord_holds_no_endpoint() {
    // Directly, without generation: the primitive on its own suppresses its
    // constituents and presses nothing.
    let mut preset = panel();
    preset
        .chords
        .push(Chord::consuming(Key::Left, vec![Key::Right]));
    let mut engine = engine_for(preset);
    assert_eq!(run(&mut engine, &[(Key::Left, true)]).lx, AXIS_MIN);
    assert_eq!(run(&mut engine, &[(Key::Right, true)]), PadState::default());
    assert_eq!(run(&mut engine, &[(Key::Left, false)]).lx, AXIS_MAX);
    assert_eq!(
        run(&mut engine, &[(Key::Right, false)]),
        PadState::default()
    );
}

#[test]
fn a_user_chord_over_the_pair_is_what_runs() {
    // The user says Left+Right → RT. Generation must not fight it.
    let mut preset = panel();
    preset.chords.push(Chord::new(
        Key::Left,
        Binding::Trigger(ksx_core::Trigger::Right),
        vec![Key::Right],
    ));
    preset.apply_socd(Socd::Neutral);
    let mut engine = engine_for(preset);
    let state = run(&mut engine, &[(Key::Left, true), (Key::Right, true)]);
    assert_eq!(state.rt, u8::MAX);
    assert_eq!(state.lx, 0, "the constituents are still consumed");
    // The vertical pair, which the user said nothing about, is still cleaned.
    let state = run(&mut engine, &[(Key::Up, true), (Key::Down, true)]);
    assert_eq!(state.ly, 0);
}

#[test]
fn multi_bind_directions_are_cleaned_on_every_pair() {
    // Two keys for Left, two for Right — every combination must cancel.
    let mut preset = preset(
        "multi",
        vec![
            (Key::A, axis(Axis::X, AXIS_MIN)),
            (Key::S, axis(Axis::X, AXIS_MIN)),
            (Key::D, axis(Axis::X, AXIS_MAX)),
            (Key::F, axis(Axis::X, AXIS_MAX)),
        ],
    );
    preset.apply_socd(Socd::Neutral);
    assert_eq!(preset.chords.len(), 4);
    for (left, right) in [
        (Key::A, Key::D),
        (Key::A, Key::F),
        (Key::S, Key::D),
        (Key::S, Key::F),
    ] {
        let mut engine = engine_for(preset.clone());
        assert_eq!(run(&mut engine, &[(left, true)]).lx, AXIS_MIN);
        assert_eq!(
            run(&mut engine, &[(right, true)]).lx,
            0,
            "{left:?}+{right:?} did not cancel"
        );
        assert_eq!(run(&mut engine, &[(right, false)]).lx, AXIS_MIN);
        assert_eq!(run(&mut engine, &[(left, false)]).lx, 0);
    }
}
