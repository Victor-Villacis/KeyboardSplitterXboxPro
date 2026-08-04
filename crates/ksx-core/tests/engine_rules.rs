//! Unit tests for every engine rule, including the exact I-PAC4 cabinet
//! scenario from `crates/ksx-legacy-import/tests/fixtures/splitter_presets.xml`.

mod common;

use common::{ev, ipac_device, ipac_engine, preset};
use ksx_core::{
    Axis, Binding, DeviceId, DpadDirection, Engine, Key, PadState, Preset, ResolvedSlot, SlotSpec,
    Trigger, XButton, XButtons, AXIS_CENTER, AXIS_MAX, AXIS_MIN,
};

fn slot(number: u8, device: &DeviceId, preset: Preset) -> ResolvedSlot {
    ResolvedSlot {
        spec: SlotSpec::new(number, Some(device.clone()), None, preset.name.clone()).unwrap(),
        preset,
    }
}

fn axis(a: Axis, value: i16) -> Binding {
    Binding::Axis { axis: a, value }
}

// ---------------------------------------------------------------- I-PAC4 case

#[test]
fn ipac4_simultaneous_press_on_two_slots_no_crosstalk() {
    let mut engine = ipac_engine();
    let dev = ipac_device();

    // G = slot 1's A button, Eight = slot 2's A button (disjoint key sets).
    let d = engine.handle(&ev(&dev, Key::G, true));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 1);
    assert_eq!(d[0].state.buttons, XButtons::A);

    let d = engine.handle(&ev(&dev, Key::Eight, true));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 2);
    assert_eq!(d[0].state.buttons, XButtons::A);

    // Both pads hold A; slots 3 and 4 untouched.
    assert_eq!(engine.pad_state(1).unwrap().buttons, XButtons::A);
    assert_eq!(engine.pad_state(2).unwrap().buttons, XButtons::A);
    assert_eq!(engine.pad_state(3).unwrap(), PadState::default());
    assert_eq!(engine.pad_state(4).unwrap(), PadState::default());

    // Releasing G releases only slot 1.
    let d = engine.handle(&ev(&dev, Key::G, false));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 1);
    assert_eq!(d[0].state, PadState::default());
    assert_eq!(engine.pad_state(2).unwrap().buttons, XButtons::A);

    let d = engine.handle(&ev(&dev, Key::Eight, false));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 2);
    assert_eq!(engine.pad_state(2).unwrap(), PadState::default());
}

#[test]
fn ipac4_all_four_players_mash_at_once() {
    let mut engine = ipac_engine();
    let dev = ipac_device();

    // One "player button" per panel section of the same physical board.
    for (key, slot_no) in [
        (Key::G, 1),
        (Key::Eight, 2),
        (Key::T, 3),
        (Key::BackslashPipe, 4),
    ] {
        let d = engine.handle(&ev(&dev, key, true));
        assert_eq!(d.len(), 1, "key {key:?} must feed exactly slot {slot_no}");
        assert_eq!(d[0].slot, slot_no);
        assert_eq!(d[0].state.buttons, XButtons::A);
    }
    for n in 1..=4 {
        assert_eq!(engine.pad_state(n).unwrap().buttons, XButtons::A);
    }
}

// ----------------------------------------------------------------- rule 1 & 3

#[test]
fn same_key_on_different_devices_is_distinct_state() {
    let dev_a = DeviceId::new("DEV_A");
    let dev_b = DeviceId::new("DEV_B");
    let p = |name: &str| preset(name, vec![(Key::S, Binding::Button(XButton::A))]);
    let mut engine = Engine::new(vec![slot(1, &dev_a, p("pa")), slot(2, &dev_b, p("pb"))]);

    let d = engine.handle(&ev(&dev_a, Key::S, true));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 1);

    let d = engine.handle(&ev(&dev_b, Key::S, true));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 2);

    // Releasing S on A must not release slot 2's press.
    let d = engine.handle(&ev(&dev_a, Key::S, false));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 1);
    assert_eq!(engine.pad_state(2).unwrap().buttons, XButtons::A);
}

#[test]
fn one_key_fans_out_to_all_matching_slots() {
    // Legacy dispatch loop has no break (Splitter.cs:408-419): the same key on
    // the same device may drive several slots at once.
    let dev = DeviceId::new("DEV_A");
    let p1 = preset("p1", vec![(Key::S, Binding::Button(XButton::A))]);
    let p2 = preset("p2", vec![(Key::S, Binding::Trigger(Trigger::Left))]);
    let p3 = preset("p3", vec![(Key::W, Binding::Button(XButton::B))]);
    let mut engine = Engine::new(vec![
        slot(1, &dev, p1),
        slot(2, &dev, p2),
        slot(3, &dev, p3),
    ]);

    let d = engine.handle(&ev(&dev, Key::S, true));
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].slot, 1);
    assert_eq!(d[0].state.buttons, XButtons::A);
    assert_eq!(d[1].slot, 2);
    assert_eq!(d[1].state.lt, 255);
    assert_eq!(engine.pad_state(3).unwrap(), PadState::default());

    let d = engine.handle(&ev(&dev, Key::S, false));
    assert_eq!(d.len(), 2);
    assert!(d.iter().all(|pd| pd.state == PadState::default()));
}

// --------------------------------------------------------------------- rule 4

#[test]
fn all_keys_up_release_default_preset_quirk() {
    // builtin_default: S and Enter both drive Button A (the legacy
    // <custom 0x1000>Enter quirk). A releases only when BOTH are up.
    let dev = DeviceId::new("DEV_A");
    let mut engine = Engine::new(vec![slot(1, &dev, Preset::builtin_default())]);

    let d = engine.handle(&ev(&dev, Key::S, true));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].state.buttons, XButtons::A);

    // Second key to the same endpoint: state unchanged, so no delta (diffing).
    let d = engine.handle(&ev(&dev, Key::Enter, true));
    assert!(d.is_empty());

    // Releasing S leaves A held through Enter.
    let d = engine.handle(&ev(&dev, Key::S, false));
    assert!(d.is_empty());
    assert_eq!(engine.pad_state(1).unwrap().buttons, XButtons::A);

    let d = engine.handle(&ev(&dev, Key::Enter, false));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].state, PadState::default());
}

#[test]
fn all_keys_up_release_for_triggers() {
    let dev = DeviceId::new("DEV_A");
    let p = preset(
        "p",
        vec![
            (Key::Q, Binding::Trigger(Trigger::Left)),
            (Key::W, Binding::Trigger(Trigger::Left)),
        ],
    );
    let mut engine = Engine::new(vec![slot(1, &dev, p)]);

    engine.handle(&ev(&dev, Key::Q, true));
    engine.handle(&ev(&dev, Key::W, true));
    let d = engine.handle(&ev(&dev, Key::Q, false));
    assert!(d.is_empty());
    assert_eq!(engine.pad_state(1).unwrap().lt, 255);
    let d = engine.handle(&ev(&dev, Key::W, false));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].state.lt, 0);
}

// --------------------------------------------------------------------- rule 5

#[test]
fn opposite_axis_snap_to_held_extreme() {
    // builtin_default: Left = X Min, Right = X Max.
    let dev = DeviceId::new("DEV_A");
    let mut engine = Engine::new(vec![slot(1, &dev, Preset::builtin_default())]);

    let d = engine.handle(&ev(&dev, Key::Left, true));
    assert_eq!(d[0].state.lx, AXIS_MIN);
    // Last press wins (SOCD).
    let d = engine.handle(&ev(&dev, Key::Right, true));
    assert_eq!(d[0].state.lx, AXIS_MAX);
    // Releasing Right while Left held snaps BACK to Left's value, not center.
    let d = engine.handle(&ev(&dev, Key::Right, false));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].state.lx, AXIS_MIN);
    // Releasing Left with nothing opposite held centers.
    let d = engine.handle(&ev(&dev, Key::Left, false));
    assert_eq!(d[0].state.lx, AXIS_CENTER);
}

#[test]
fn opposite_axis_snap_release_of_stomped_direction_emits_nothing() {
    let dev = DeviceId::new("DEV_A");
    let mut engine = Engine::new(vec![slot(1, &dev, Preset::builtin_default())]);

    engine.handle(&ev(&dev, Key::Left, true)); // lx = MIN
    engine.handle(&ev(&dev, Key::Right, true)); // lx = MAX (last wins)

    // Releasing Left while Right held snaps to Right's value — already MAX,
    // so no delta (diffing).
    let d = engine.handle(&ev(&dev, Key::Left, false));
    assert!(d.is_empty());
    assert_eq!(engine.pad_state(1).unwrap().lx, AXIS_MAX);

    let d = engine.handle(&ev(&dev, Key::Right, false));
    assert_eq!(d[0].state.lx, AXIS_CENTER);
}

#[test]
fn opposite_axis_snap_uses_custom_bound_value_not_hardcoded_extreme() {
    // Deliberate divergence from legacy Splitter.cs:161 (which hardcoded
    // Min/Max and missed custom-valued opposites) — see
    // docs/research/design-risk-review.md §3 item 6.
    let dev = DeviceId::new("DEV_A");
    let p = preset(
        "walk-run",
        vec![
            (Key::A, axis(Axis::X, -16384)), // walk left
            (Key::D, axis(Axis::X, AXIS_MAX)),
        ],
    );
    let mut engine = Engine::new(vec![slot(1, &dev, p)]);

    engine.handle(&ev(&dev, Key::A, true)); // lx = -16384
    engine.handle(&ev(&dev, Key::D, true)); // lx = 32767
    let d = engine.handle(&ev(&dev, Key::D, false));
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].state.lx, -16384,
        "snap to the held binding's OWN value"
    );
    let d = engine.handle(&ev(&dev, Key::A, false));
    assert_eq!(d[0].state.lx, AXIS_CENTER);
}

#[test]
fn opposite_axis_snap_prefers_largest_held_deflection() {
    let dev = DeviceId::new("DEV_A");
    let p = preset(
        "p",
        vec![
            (Key::A, axis(Axis::X, -16384)),
            (Key::S, axis(Axis::X, AXIS_MIN)),
            (Key::D, axis(Axis::X, AXIS_MAX)),
        ],
    );
    let mut engine = Engine::new(vec![slot(1, &dev, p)]);

    engine.handle(&ev(&dev, Key::A, true));
    engine.handle(&ev(&dev, Key::S, true));
    engine.handle(&ev(&dev, Key::D, true)); // lx = MAX
    let d = engine.handle(&ev(&dev, Key::D, false));
    assert_eq!(d[0].state.lx, AXIS_MIN, "largest opposite deflection wins");
}

#[test]
fn axis_release_with_same_direction_still_held_stays_deflected() {
    // Same endpoint (same axis AND same value) held by another key: the
    // all-keys-up rule keeps it active.
    let dev = DeviceId::new("DEV_A");
    let p = preset(
        "p",
        vec![
            (Key::A, axis(Axis::Y, AXIS_MIN)),
            (Key::S, axis(Axis::Y, AXIS_MIN)),
        ],
    );
    let mut engine = Engine::new(vec![slot(1, &dev, p)]);

    engine.handle(&ev(&dev, Key::A, true));
    engine.handle(&ev(&dev, Key::S, true));
    let d = engine.handle(&ev(&dev, Key::A, false));
    assert!(d.is_empty());
    assert_eq!(engine.pad_state(1).unwrap().ly, AXIS_MIN);
}

// --------------------------------------------------------------------- rule 6

#[test]
fn trigger_is_full_scale_or_zero() {
    let dev = ipac_device();
    let mut engine = ipac_engine();
    // H = slot 1 left trigger, E = slot 1 right trigger.
    let d = engine.handle(&ev(&dev, Key::H, true));
    assert_eq!(d[0].state.lt, 255);
    assert_eq!(d[0].state.rt, 0);
    let d = engine.handle(&ev(&dev, Key::E, true));
    assert_eq!(d[0].state.rt, 255);
    let d = engine.handle(&ev(&dev, Key::H, false));
    assert_eq!(d[0].state.lt, 0);
    assert_eq!(d[0].state.rt, 255);
}

#[test]
fn dpad_directions_or_and_andnot_flags() {
    let dev = DeviceId::new("DEV_A");
    let p = preset(
        "p",
        vec![
            (Key::I, Binding::Dpad(DpadDirection::Up)),
            (Key::L, Binding::Dpad(DpadDirection::Right)),
            (Key::S, Binding::Button(XButton::A)),
        ],
    );
    let mut engine = Engine::new(vec![slot(1, &dev, p)]);

    let d = engine.handle(&ev(&dev, Key::I, true));
    assert_eq!(d[0].state.buttons, XButtons::DPAD_UP);
    // Diagonal: flags OR together.
    let d = engine.handle(&ev(&dev, Key::L, true));
    assert_eq!(d[0].state.buttons, XButtons::DPAD_UP | XButtons::DPAD_RIGHT);
    // Dpad shares wButtons with face buttons without clobbering them.
    let d = engine.handle(&ev(&dev, Key::S, true));
    assert_eq!(
        d[0].state.buttons,
        XButtons::DPAD_UP | XButtons::DPAD_RIGHT | XButtons::A
    );
    let d = engine.handle(&ev(&dev, Key::I, false));
    assert_eq!(d[0].state.buttons, XButtons::DPAD_RIGHT | XButtons::A);
}

// --------------------------------------------------------------------- rule 7

#[test]
fn repeated_down_emits_no_delta() {
    let mut engine = ipac_engine();
    let dev = ipac_device();
    assert_eq!(engine.handle(&ev(&dev, Key::G, true)).len(), 1);
    // Keyboard auto-repeat.
    assert!(engine.handle(&ev(&dev, Key::G, true)).is_empty());
    assert!(engine.handle(&ev(&dev, Key::G, true)).is_empty());
    assert_eq!(engine.handle(&ev(&dev, Key::G, false)).len(), 1);
}

#[test]
fn spurious_release_emits_no_delta() {
    let mut engine = ipac_engine();
    let dev = ipac_device();
    assert!(engine.handle(&ev(&dev, Key::G, false)).is_empty());
}

#[test]
fn unassigned_device_and_unbound_key_produce_nothing() {
    let mut engine = ipac_engine();
    let stray = DeviceId::new("SOME_OTHER_KEYBOARD");
    assert!(engine.handle(&ev(&stray, Key::G, true)).is_empty());
    assert!(engine.release_device(&stray).is_empty());

    let dev = ipac_device();
    // F12 is bound nowhere in the fixture presets.
    assert!(engine.handle(&ev(&dev, Key::F12, true)).is_empty());
}

#[test]
fn key_none_entries_never_match() {
    let dev = DeviceId::new("DEV_A");
    let mut engine = Engine::new(vec![slot(1, &dev, Preset::builtin_empty())]);
    // builtin_empty binds every function to Key::None; nothing may ever fire.
    for key in [Key::None, Key::S, Key::Enter, Key::Left] {
        assert!(engine.handle(&ev(&dev, key, true)).is_empty());
        assert!(engine.handle(&ev(&dev, key, false)).is_empty());
    }
    assert_eq!(engine.pad_state(1).unwrap(), PadState::default());
}

// --------------------------------------------------------------------- rule 8

#[test]
fn release_device_clears_every_contribution() {
    let mut engine = ipac_engine();
    let dev = ipac_device();

    // Contributions across all 4 slots and all function types.
    for key in [Key::G, Key::H, Key::M, Key::Eight, Key::Tab, Key::T, Key::Z] {
        engine.handle(&ev(&dev, key, true));
    }
    let d = engine.release_device(&dev);
    // Slots 1..3 changed back to neutral; slot 4 never had a contribution.
    assert_eq!(d.len(), 3);
    for n in 1..=4 {
        assert_eq!(engine.pad_state(n).unwrap(), PadState::default());
    }
    // Idempotent: nothing left to release.
    assert!(engine.release_device(&dev).is_empty());
}

#[test]
fn release_device_leaves_other_devices_alone() {
    let dev_a = DeviceId::new("DEV_A");
    let dev_b = DeviceId::new("DEV_B");
    let p = |name: &str| preset(name, vec![(Key::S, Binding::Button(XButton::A))]);
    let mut engine = Engine::new(vec![slot(1, &dev_a, p("pa")), slot(2, &dev_b, p("pb"))]);

    engine.handle(&ev(&dev_a, Key::S, true));
    engine.handle(&ev(&dev_b, Key::S, true));
    let d = engine.release_device(&dev_a);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].slot, 1);
    assert_eq!(engine.pad_state(1).unwrap(), PadState::default());
    assert_eq!(engine.pad_state(2).unwrap().buttons, XButtons::A);
}

#[test]
fn release_device_resolves_held_opposites_to_center() {
    let dev = ipac_device();
    let mut engine = ipac_engine();
    engine.handle(&ev(&dev, Key::M, true)); // slot 1: X Min
    engine.handle(&ev(&dev, Key::N, true)); // slot 1: X Max
    let d = engine.release_device(&dev);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].state, PadState::default());
}

// ---------------------------------------------------------------------- reset

#[test]
fn reset_clears_key_state_and_pads_silently() {
    let mut engine = ipac_engine();
    let dev = ipac_device();
    engine.handle(&ev(&dev, Key::G, true));
    engine.handle(&ev(&dev, Key::M, true));

    engine.reset();
    for n in 1..=4 {
        assert_eq!(engine.pad_state(n).unwrap(), PadState::default());
    }
    // Releases of pre-reset presses find no key state and emit nothing.
    assert!(engine.handle(&ev(&dev, Key::G, false)).is_empty());
    assert!(engine.handle(&ev(&dev, Key::M, false)).is_empty());
}

#[test]
fn pad_state_unknown_slot_is_none() {
    let engine = ipac_engine();
    assert!(engine.pad_state(0).is_none());
    assert!(engine.pad_state(5).is_none());
}
