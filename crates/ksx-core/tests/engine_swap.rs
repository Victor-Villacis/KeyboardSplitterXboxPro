//! The binding hot-swap ([`Engine::swap_tables`]): a preset edit must reach a
//! RUNNING session without the pads being unplugged.
//!
//! Victor's question, verbatim: "why does it need to disconnect to reconnect?"
//! The old answer was a full daemon Reload — teardown, unplug, replug — which
//! Windows announces with its device chime, Steam re-enumerates, and a game in
//! progress sees as four controllers vanishing. These tests pin the engine half
//! of the fix: new tables in, same slots out, and no control left held.

mod common;

use common::{ev, ipac_device, ipac_p1, ipac_p2, ipac_p3, ipac_p4, preset};
use ksx_core::{
    Binding, DeviceId, Engine, EngineTables, Key, PadState, Preset, ResolvedSlot, SlotSpec,
    XButton, XButtons,
};

fn slot(number: u8, device: &DeviceId, preset: Preset) -> ResolvedSlot {
    ResolvedSlot {
        spec: SlotSpec::new(number, Some(device.clone()), None, preset.name.clone()).unwrap(),
        preset,
    }
}

fn cabinet(device: &DeviceId) -> Vec<ResolvedSlot> {
    [ipac_p1(), ipac_p2(), ipac_p3(), ipac_p4()]
        .into_iter()
        .enumerate()
        .map(|(i, p)| slot(i as u8 + 1, device, p))
        .collect()
}

/// The whole point: after a swap the engine answers to the NEW key and no
/// longer to the old one, with the slot set untouched.
#[test]
fn swapping_tables_rebinds_without_touching_the_slot_set() {
    let dev = ipac_device();
    let mut engine = Engine::new(cabinet(&dev));

    // Before: G is slot 1's A (the cabinet's real layout).
    let deltas = engine.handle(&ev(&dev, Key::G, true));
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].state.buttons, XButtons::A);
    engine.handle(&ev(&dev, Key::G, false));

    // The edit: P1's A moves from G to F1. Everything else is identical.
    let mut edited = cabinet(&dev);
    for entry in &mut edited[0].preset.entries {
        if entry.1 == Binding::Button(XButton::A) {
            entry.0 = Key::F1;
        }
    }
    let deltas = engine.swap_tables(EngineTables::build(edited));
    assert!(
        deltas.is_empty(),
        "nothing was held, so a swap must be silent: {deltas:?}"
    );

    // After: the new key drives A…
    let deltas = engine.handle(&ev(&dev, Key::F1, true));
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].slot, 1);
    assert_eq!(deltas[0].state.buttons, XButtons::A);
    engine.handle(&ev(&dev, Key::F1, false));

    // …and the old one drives nothing on slot 1 any more (it is still slot 4's
    // own binding in the real presets, so assert on slot 1 specifically).
    let deltas = engine.handle(&ev(&dev, Key::G, true));
    assert!(
        deltas.iter().all(|d| d.slot != 1),
        "the retired key still reaches slot 1: {deltas:?}"
    );

    // Every slot survived the swap.
    for number in 1..=4u8 {
        assert!(engine.pad_state(number).is_some(), "slot {number} vanished");
    }
}

/// The stuck-key invariant. Dense key ids belong to the old tables, so key
/// state cannot carry across a swap — which means a control held at the moment
/// of the edit MUST come back as an explicit neutral delta, never be forgotten
/// while the pad still shows it pressed.
#[test]
fn a_control_held_across_a_swap_is_released_explicitly() {
    let dev = ipac_device();
    let mut engine = Engine::new(cabinet(&dev));

    // Slot 1 holds A, slot 2 holds A: two pads mid-press.
    engine.handle(&ev(&dev, Key::G, true));
    engine.handle(&ev(&dev, Key::Eight, true));
    assert_eq!(engine.pad_state(1).unwrap().buttons, XButtons::A);
    assert_eq!(engine.pad_state(2).unwrap().buttons, XButtons::A);

    let deltas = engine.swap_tables(EngineTables::build(cabinet(&dev)));
    let mut slots: Vec<u8> = deltas.iter().map(|d| d.slot).collect();
    slots.sort_unstable();
    assert_eq!(
        slots,
        vec![1, 2],
        "exactly the two pads that were holding something must be released"
    );
    for delta in &deltas {
        assert_eq!(
            delta.state,
            PadState::default(),
            "slot {} was not returned to neutral",
            delta.slot
        );
    }
    for number in 1..=4u8 {
        assert_eq!(engine.pad_state(number).unwrap(), PadState::default());
    }
}

/// A swap re-baselines the down bitset, so the physical release that follows an
/// edit must not underflow into a spurious delta.
#[test]
fn a_release_that_straddles_a_swap_is_harmless() {
    let dev = ipac_device();
    let mut engine = Engine::new(cabinet(&dev));

    engine.handle(&ev(&dev, Key::G, true));
    engine.swap_tables(EngineTables::build(cabinet(&dev)));
    // The player's finger comes off AFTER the swap: the engine already
    // released the control, so this must change nothing.
    let deltas = engine.handle(&ev(&dev, Key::G, false));
    assert!(deltas.is_empty(), "{deltas:?}");
    assert_eq!(engine.pad_state(1).unwrap(), PadState::default());
}

/// Slot state is matched by NUMBER, not by build position — a swap that keeps
/// the same slots in a different order still reports the right pads.
#[test]
fn slot_state_follows_the_slot_number_not_its_position() {
    let dev = ipac_device();
    let mut engine = Engine::new(cabinet(&dev));
    engine.handle(&ev(&dev, Key::G, true)); // slot 1 holds A

    let mut reordered = cabinet(&dev);
    reordered.reverse(); // slot 4, 3, 2, 1
    let deltas = engine.swap_tables(EngineTables::build(reordered));
    assert_eq!(deltas.len(), 1);
    assert_eq!(
        deltas[0].slot, 1,
        "the release must name slot 1, not slot 4"
    );
}

/// A swap onto tables built from a preset with nothing bound leaves the engine
/// alive and silent — the degenerate case a "clear every binding" edit hits.
#[test]
fn swapping_onto_an_empty_preset_keeps_the_slot_and_stops_translating() {
    let dev = ipac_device();
    let mut engine = Engine::new(vec![slot(1, &dev, ipac_p1())]);
    engine.handle(&ev(&dev, Key::G, true));

    let empty = preset("IPAC P1", Vec::new());
    let deltas = engine.swap_tables(EngineTables::build(vec![slot(1, &dev, empty)]));
    assert_eq!(deltas.len(), 1, "the held A must be released: {deltas:?}");
    assert_eq!(deltas[0].state, PadState::default());
    assert!(engine.handle(&ev(&dev, Key::G, true)).is_empty());
    assert_eq!(engine.pad_state(1).unwrap(), PadState::default());
}

/// `EngineTables::build` is the off-thread half of the contract: it must be
/// buildable anywhere and carry the slot shape the caller checks eligibility
/// against.
#[test]
fn tables_report_their_slot_shape_and_build_off_thread() {
    let dev = ipac_device();
    let handle = std::thread::spawn(move || EngineTables::build(cabinet(&dev)));
    let tables = handle.join().expect("tables build on any thread");
    assert_eq!(tables.slot_numbers(), vec![1, 2, 3, 4]);
    let engine = Engine::from_tables(tables);
    assert_eq!(engine.pad_state(4).unwrap(), PadState::default());
}
