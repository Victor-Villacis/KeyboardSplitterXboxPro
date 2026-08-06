//! Shared engine-test fixtures: the real I-PAC4 cabinet presets, hand-ported
//! row-for-row (including the inert `None` rows) from
//! `crates/ksx-legacy-import/tests/fixtures/splitter_presets.xml` — one
//! physical keyboard-encoder feeding 4 slots with disjoint key sets.
#![allow(dead_code)]

use ksx_core::{
    Axis, Binding, DeviceId, Engine, Key, KeyEvent, Preset, ResolvedSlot, SlotSpec, Trigger,
    XButton, AXIS_MAX, AXIS_MIN,
};

pub const IPAC: &str = r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000";

pub fn ipac_device() -> DeviceId {
    DeviceId::new(IPAC)
}

pub fn ev(dev: &DeviceId, key: Key, down: bool) -> KeyEvent {
    KeyEvent {
        device: dev.clone(),
        key,
        down,
        t: 0,
    }
}

pub fn preset(name: &str, entries: Vec<(Key, Binding)>) -> Preset {
    Preset {
        name: name.to_owned(),
        entries,
        chords: Vec::new(),
        macros: Default::default(),
        protected: false,
    }
}

/// Same, plus guarded entries (chords).
pub fn preset_with_chords(
    name: &str,
    entries: Vec<(Key, Binding)>,
    chords: Vec<ksx_core::Chord>,
) -> Preset {
    Preset {
        name: name.to_owned(),
        entries,
        chords,
        macros: Default::default(),
        protected: false,
    }
}

/// Same, plus macros (definitions and the keys that start them).
pub fn preset_with_macros(
    name: &str,
    entries: Vec<(Key, Binding)>,
    macros: ksx_core::Macros,
) -> Preset {
    Preset {
        name: name.to_owned(),
        entries,
        chords: Vec::new(),
        macros,
        protected: false,
    }
}

/// One slot, one keyboard, one preset — the shape every chord test uses.
pub fn engine_for(preset: Preset) -> Engine {
    let dev = ipac_device();
    Engine::new(vec![ResolvedSlot {
        spec: SlotSpec::new(1, Some(dev), None, preset.name.clone()).expect("valid slot"),
        preset,
    }])
}

fn axis(axis: Axis, value: i16) -> Binding {
    Binding::Axis { axis, value }
}

pub fn ipac_p1() -> Preset {
    preset(
        "IPAC P1",
        vec![
            (Key::G, Binding::Button(XButton::A)),
            (Key::F, Binding::Button(XButton::B)),
            (Key::J, Binding::Button(XButton::X)),
            (Key::I, Binding::Button(XButton::Y)),
            (Key::D, Binding::Button(XButton::LeftBumper)),
            (Key::C, Binding::Button(XButton::RightBumper)),
            (Key::A, Binding::Button(XButton::Back)),
            (Key::B, Binding::Button(XButton::Start)),
            (Key::None, Binding::Button(XButton::LeftThumb)),
            (Key::None, Binding::Button(XButton::RightThumb)),
            (Key::Q, Binding::Button(XButton::Guide)),
            (Key::H, Binding::Trigger(Trigger::Left)),
            (Key::E, Binding::Trigger(Trigger::Right)),
            (Key::M, axis(Axis::X, AXIS_MIN)),
            (Key::N, axis(Axis::X, AXIS_MAX)),
            (Key::K, axis(Axis::Y, AXIS_MIN)),
            (Key::L, axis(Axis::Y, AXIS_MAX)),
            (Key::None, axis(Axis::Rx, AXIS_MIN)),
            (Key::None, axis(Axis::Rx, AXIS_MAX)),
            (Key::None, axis(Axis::Ry, AXIS_MIN)),
            (Key::None, axis(Axis::Ry, AXIS_MAX)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Up)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Down)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Left)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Right)),
        ],
    )
}

pub fn ipac_p2() -> Preset {
    preset(
        "IPAC P2",
        vec![
            (Key::Eight, Binding::Button(XButton::A)),
            (Key::Seven, Binding::Button(XButton::B)),
            (Key::Enter, Binding::Button(XButton::X)),
            (Key::Zero, Binding::Button(XButton::Y)),
            (Key::Five, Binding::Button(XButton::LeftBumper)),
            (Key::Four, Binding::Button(XButton::RightBumper)),
            (Key::Two, Binding::Button(XButton::Back)),
            (Key::Three, Binding::Button(XButton::Start)),
            (Key::None, Binding::Button(XButton::LeftThumb)),
            (Key::None, Binding::Button(XButton::RightThumb)),
            (Key::Nine, Binding::Trigger(Trigger::Left)),
            (Key::Six, Binding::Trigger(Trigger::Right)),
            (Key::Tab, axis(Axis::X, AXIS_MIN)),
            (Key::Space, axis(Axis::X, AXIS_MAX)),
            (Key::Escape, axis(Axis::Y, AXIS_MIN)),
            (Key::Backspace, axis(Axis::Y, AXIS_MAX)),
            (Key::None, axis(Axis::Rx, AXIS_MIN)),
            (Key::None, axis(Axis::Rx, AXIS_MAX)),
            (Key::None, axis(Axis::Ry, AXIS_MIN)),
            (Key::None, axis(Axis::Ry, AXIS_MAX)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Up)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Down)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Left)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Right)),
        ],
    )
}

pub fn ipac_p3() -> Preset {
    preset(
        "IPAC P3",
        vec![
            (Key::T, Binding::Button(XButton::A)),
            (Key::S, Binding::Button(XButton::B)),
            (Key::W, Binding::Button(XButton::X)),
            (Key::V, Binding::Button(XButton::Y)),
            (Key::None, Binding::Button(XButton::LeftBumper)),
            (Key::None, Binding::Button(XButton::RightBumper)),
            (Key::O, Binding::Button(XButton::Back)),
            (Key::P, Binding::Button(XButton::Start)),
            (Key::None, Binding::Button(XButton::LeftThumb)),
            (Key::None, Binding::Button(XButton::RightThumb)),
            (Key::U, Binding::Trigger(Trigger::Left)),
            (Key::R, Binding::Trigger(Trigger::Right)),
            (Key::Z, axis(Axis::X, AXIS_MIN)),
            (Key::One, axis(Axis::X, AXIS_MAX)),
            (Key::X, axis(Axis::Y, AXIS_MIN)),
            (Key::Y, axis(Axis::Y, AXIS_MAX)),
            (Key::None, axis(Axis::Rx, AXIS_MIN)),
            (Key::None, axis(Axis::Rx, AXIS_MAX)),
            (Key::None, axis(Axis::Ry, AXIS_MIN)),
            (Key::None, axis(Axis::Ry, AXIS_MAX)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Up)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Down)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Left)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Right)),
        ],
    )
}

pub fn ipac_p4() -> Preset {
    preset(
        "IPAC P4",
        vec![
            (Key::BackslashPipe, Binding::Button(XButton::A)),
            (Key::CloseBracketBrace, Binding::Button(XButton::B)),
            (Key::Tilde, Binding::Button(XButton::X)),
            (Key::SingleDoubleQuote, Binding::Button(XButton::Y)),
            (Key::None, Binding::Button(XButton::LeftBumper)),
            (Key::None, Binding::Button(XButton::RightBumper)),
            (Key::DashUnderscore, Binding::Button(XButton::Back)),
            (Key::PlusEquals, Binding::Button(XButton::Start)),
            (Key::None, Binding::Button(XButton::LeftThumb)),
            (Key::None, Binding::Button(XButton::RightThumb)),
            (Key::SemicolonColon, Binding::Trigger(Trigger::Left)),
            (Key::OpenBracketBrace, Binding::Trigger(Trigger::Right)),
            (Key::ForwardSlashQuestionMark, axis(Axis::X, AXIS_MIN)),
            (Key::CapsLock, axis(Axis::X, AXIS_MAX)),
            (Key::CommaLeftArrow, axis(Axis::Y, AXIS_MIN)),
            (Key::PeriodRightArrow, axis(Axis::Y, AXIS_MAX)),
            (Key::None, axis(Axis::Rx, AXIS_MIN)),
            (Key::None, axis(Axis::Rx, AXIS_MAX)),
            (Key::None, axis(Axis::Ry, AXIS_MIN)),
            (Key::None, axis(Axis::Ry, AXIS_MAX)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Up)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Down)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Left)),
            (Key::None, Binding::Dpad(ksx_core::DpadDirection::Right)),
        ],
    )
}

/// The cabinet configuration: one I-PAC4 keyboard device feeding all 4 slots.
pub fn ipac_engine() -> Engine {
    let dev = ipac_device();
    let presets = [ipac_p1(), ipac_p2(), ipac_p3(), ipac_p4()];
    let slots = presets
        .into_iter()
        .enumerate()
        .map(|(i, preset)| ResolvedSlot {
            spec: SlotSpec::new((i + 1) as u8, Some(dev.clone()), None, preset.name.clone())
                .expect("valid slot number"),
            preset,
        })
        .collect();
    Engine::new(slots)
}

/// Every key bound (non-`None`) across the four I-PAC presets.
pub fn ipac_bound_keys() -> Vec<Key> {
    [ipac_p1(), ipac_p2(), ipac_p3(), ipac_p4()]
        .iter()
        .flat_map(|p| p.entries.iter().map(|(k, _)| *k))
        .filter(|k| *k != Key::None)
        .collect()
}
