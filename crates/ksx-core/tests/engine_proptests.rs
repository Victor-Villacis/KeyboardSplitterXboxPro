//! Property tests for the engine invariants (risk review §5: stuck-key,
//! all-keys-up, opposite-axis, fan-out, diff idempotence).

mod common;

use std::collections::{HashMap, HashSet};

use common::{ev, ipac_p1, preset};
use ksx_core::{
    Axis, Binding, DeviceId, DpadDirection, Engine, Key, PadState, Preset, ResolvedSlot, SlotSpec,
    Trigger, XButton, AXIS_MAX, AXIS_MIN,
};
use proptest::prelude::*;

const KEY_POOL: &[Key] = &[
    // builtin_default keys
    Key::S,
    Key::Enter,
    Key::Left,
    Key::Right,
    Key::Down,
    Key::Up,
    Key::Q,
    Key::E,
    Key::I,
    Key::K,
    Key::J,
    Key::L,
    Key::Escape,
    Key::Backspace,
    Key::Z,
    Key::C,
    Key::W,
    Key::A,
    Key::D,
    Key::LeftShift,
    Key::RightShift,
    Key::LeftWindows,
    Key::Numpad2,
    Key::Numpad4,
    Key::Numpad6,
    Key::Numpad8,
    // ipac_p1 extras
    Key::G,
    Key::F,
    Key::B,
    Key::H,
    Key::M,
    Key::N,
    // bound nowhere
    Key::F12,
];

/// Device 0 feeds slots 1+2 (fan-out), device 1 feeds slots 3+4, device 2 is
/// assigned nowhere.
fn devices() -> [DeviceId; 3] {
    [
        DeviceId::new("DEV_A"),
        DeviceId::new("DEV_B"),
        DeviceId::new("DEV_UNASSIGNED"),
    ]
}

fn test_engine() -> Engine {
    let [a, b, _] = devices();
    let cross = preset(
        "cross",
        vec![
            (Key::Q, Binding::Trigger(Trigger::Left)),
            (
                Key::W,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MAX,
                },
            ),
            (
                Key::S,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MIN,
                },
            ),
            (Key::E, Binding::Dpad(DpadDirection::Up)),
            (Key::S, Binding::Button(XButton::A)),
        ],
    );
    let slot = |n: u8, dev: &DeviceId, p: Preset| ResolvedSlot {
        spec: SlotSpec::new(n, Some(dev.clone()), None, p.name.clone()).unwrap(),
        preset: p,
    };
    Engine::new(vec![
        slot(1, &a, Preset::builtin_default()),
        slot(2, &a, ipac_p1()),
        slot(3, &b, Preset::builtin_default()),
        slot(4, &b, cross),
    ])
}

fn arb_events() -> impl Strategy<Value = Vec<(usize, Key, bool)>> {
    proptest::collection::vec(
        (0..3usize, proptest::sample::select(KEY_POOL), any::<bool>()),
        0..150,
    )
}

proptest! {
    /// (a) Any event sequence ending with all keys up leaves every pad neutral.
    #[test]
    fn all_keys_up_means_all_pads_neutral(events in arb_events()) {
        let devs = devices();
        let mut engine = test_engine();
        let mut held: HashSet<(usize, Key)> = HashSet::new();

        for (d, key, down) in events {
            engine.handle(&ev(&devs[d], key, down));
            if down {
                held.insert((d, key));
            } else {
                held.remove(&(d, key));
            }
        }
        let mut held: Vec<_> = held.into_iter().collect();
        held.sort_by_key(|(d, k)| (*d, k.value()));
        for (d, key) in held {
            engine.handle(&ev(&devs[d], key, false));
        }

        for n in 1..=4u8 {
            prop_assert_eq!(engine.pad_state(n).unwrap(), PadState::default());
        }
    }

    /// (b) release_device removes every contribution of that device and
    /// touches nothing else.
    #[test]
    fn release_device_removes_exactly_its_contributions(events in arb_events()) {
        let devs = devices();
        let mut engine = test_engine();
        for (d, key, down) in events {
            engine.handle(&ev(&devs[d], key, down));
        }

        let s3 = engine.pad_state(3).unwrap();
        let s4 = engine.pad_state(4).unwrap();

        let deltas = engine.release_device(&devs[0]);
        prop_assert!(deltas.iter().all(|pd| pd.slot == 1 || pd.slot == 2));
        prop_assert_eq!(engine.pad_state(1).unwrap(), PadState::default());
        prop_assert_eq!(engine.pad_state(2).unwrap(), PadState::default());
        prop_assert_eq!(engine.pad_state(3).unwrap(), s3);
        prop_assert_eq!(engine.pad_state(4).unwrap(), s4);

        // Nothing left of that device: releasing again is a no-op.
        prop_assert!(engine.release_device(&devs[0]).is_empty());
    }

    /// (c) Diffing: consecutive emitted states for one slot always differ.
    #[test]
    fn consecutive_identical_states_never_emit(events in arb_events()) {
        let devs = devices();
        let mut engine = test_engine();
        let mut last: HashMap<u8, PadState> = HashMap::new();

        for (d, key, down) in events {
            for delta in engine.handle(&ev(&devs[d], key, down)) {
                if let Some(prev) = last.get(&delta.slot) {
                    prop_assert_ne!(*prev, delta.state, "duplicate emission for slot {}", delta.slot);
                }
                last.insert(delta.slot, delta.state);
            }
        }
    }
}

// -------------------------------------------------------- (d) fan-out mapping

fn arb_binding() -> impl Strategy<Value = Binding> {
    prop_oneof![
        prop_oneof![
            Just(XButton::A),
            Just(XButton::B),
            Just(XButton::X),
            Just(XButton::Y),
            Just(XButton::Start),
            Just(XButton::Guide),
        ]
        .prop_map(Binding::Button),
        prop_oneof![Just(Trigger::Left), Just(Trigger::Right)].prop_map(Binding::Trigger),
        prop_oneof![
            Just(DpadDirection::Up),
            Just(DpadDirection::Down),
            Just(DpadDirection::Left),
            Just(DpadDirection::Right),
        ]
        .prop_map(Binding::Dpad),
        (
            prop_oneof![Just(Axis::X), Just(Axis::Y), Just(Axis::Rx), Just(Axis::Ry)],
            // Nonzero values only: pressing must always change a neutral pad.
            prop_oneof![
                Just(AXIS_MIN),
                Just(AXIS_MAX),
                Just(-16384i16),
                Just(20000i16)
            ],
        )
            .prop_map(|(axis, value)| Binding::Axis { axis, value }),
    ]
}

fn arb_entries() -> impl Strategy<Value = Vec<(Key, Binding)>> {
    proptest::collection::vec((proptest::sample::select(KEY_POOL), arb_binding()), 0..6)
}

proptest! {
    /// (d) Events on a multi-slot device produce deltas for exactly the slots
    /// whose preset binds the key (the precompiled mapping).
    #[test]
    fn fanout_matches_precompiled_mapping(
        entries in proptest::collection::vec(arb_entries(), 4),
        pressed in proptest::sample::select(KEY_POOL),
    ) {
        let dev = DeviceId::new("IPAC");
        let slots: Vec<ResolvedSlot> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| ResolvedSlot {
                spec: SlotSpec::new((i + 1) as u8, Some(dev.clone()), None, "p").unwrap(),
                preset: preset(&format!("p{}", i + 1), e.clone()),
            })
            .collect();
        let expected: HashSet<u8> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.iter().any(|(k, _)| *k == pressed))
            .map(|(i, _)| (i + 1) as u8)
            .collect();

        let mut engine = Engine::new(slots);
        let got: HashSet<u8> = engine
            .handle(&ev(&dev, pressed, true))
            .iter()
            .map(|d| d.slot)
            .collect();
        prop_assert_eq!(got, expected);
    }
}

// ------------------------------------------- (e) SOCD / opposite-axis snap

proptest! {
    /// (e) With both directions bindable, the axis follows the "last press
    /// wins; release snaps to the held opposite's own value; center only when
    /// both released" model — under fully arbitrary sequences including
    /// repeats and spurious releases.
    #[test]
    fn socd_model_holds(
        vmin in prop_oneof![Just(AXIS_MIN), Just(-16384i16), Just(-1i16)],
        vmax in prop_oneof![Just(AXIS_MAX), Just(16384i16), Just(1i16)],
        moves in proptest::collection::vec((any::<bool>(), any::<bool>()), 0..60),
    ) {
        let dev = DeviceId::new("DEV_A");
        let p = preset(
            "socd",
            vec![
                (Key::A, Binding::Axis { axis: Axis::X, value: vmin }),
                (Key::D, Binding::Axis { axis: Axis::X, value: vmax }),
            ],
        );
        let mut engine = Engine::new(vec![ResolvedSlot {
            spec: SlotSpec::new(1, Some(dev.clone()), None, "socd").unwrap(),
            preset: p,
        }]);

        let (mut a_down, mut d_down) = (false, false);

        for (use_a, down) in moves {
            let key = if use_a { Key::A } else { Key::D };
            engine.handle(&ev(&dev, key, down));

            if use_a {
                a_down = down;
            } else {
                d_down = down;
            }
            let expected: i16 = if down {
                if use_a { vmin } else { vmax }
            } else if use_a && d_down {
                vmax
            } else if !use_a && a_down {
                vmin
            } else {
                0
            };
            let lx = engine.pad_state(1).unwrap().lx;
            prop_assert_eq!(lx, expected);
            // The named invariant: one direction still held never yields center.
            if a_down != d_down {
                prop_assert_ne!(lx, 0);
            }
        }
    }
}
