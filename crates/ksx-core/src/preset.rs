//! Bindings and presets, including the two built-ins ported from
//! `legacy/KeyboardSplitter/Presets/Preset.cs`.

use crate::key::Key;
use crate::pad::{Axis, DpadDirection, Trigger, XButton, AXIS_MAX, AXIS_MIN};

/// One preset entry: a key drives one xbox function.
///
/// Custom axis values are first-class: `Axis { value }` holds any `i16`, not
/// just Min/Max (legacy `<axis id value>` preserved raw shorts the same way).
/// The legacy flat `XboxCustomFunction` space collapses into these four
/// variants at import time; because a legacy custom axis function always drove
/// full-scale Min/Max, equality on `Binding` reproduces the legacy
/// cross-category `GetKeys` aggregation exactly (all-keys-up rule).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Binding {
    Button(XButton),
    Trigger(Trigger),
    Axis { axis: Axis, value: i16 },
    Dpad(DpadDirection),
}

/// A named set of key→function bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    /// No uniqueness constraint in either direction: many keys → one function
    /// and one key → many functions are both native (legacy needed the
    /// `<custom>` category for this; here it is just more entries).
    /// Entries keyed [`Key::None`] are inert placeholders ("function present,
    /// unbound") preserved for legacy fidelity; engines must ignore them.
    pub entries: Vec<(Key, Binding)>,
    /// Built-ins ship in code, are never saved, and cannot be edited/deleted
    /// (legacy `ImuttablePresets` semantics).
    pub protected: bool,
}

impl Preset {
    pub const DEFAULT_NAME: &'static str = "default";
    pub const EMPTY_NAME: &'static str = "empty";

    /// The protected `default` preset, ported entry-for-entry from legacy
    /// `Preset.CreateDefaultPreset()` including its custom-function quirk: the
    /// legacy `<custom function=0x1000>Enter</custom>` row (Button_A) becomes a
    /// second `Button(A)` entry, so both `S` and `Enter` drive A.
    pub fn builtin_default() -> Self {
        let entries = vec![
            (Key::Escape, Binding::Button(XButton::Start)),
            (Key::Backspace, Binding::Button(XButton::Back)),
            (Key::LeftShift, Binding::Button(XButton::LeftThumb)),
            (Key::RightShift, Binding::Button(XButton::RightThumb)),
            (Key::Z, Binding::Button(XButton::LeftBumper)),
            (Key::C, Binding::Button(XButton::RightBumper)),
            (Key::LeftWindows, Binding::Button(XButton::Guide)),
            (Key::S, Binding::Button(XButton::A)),
            (Key::D, Binding::Button(XButton::B)),
            (Key::A, Binding::Button(XButton::X)),
            (Key::W, Binding::Button(XButton::Y)),
            (Key::Q, Binding::Trigger(Trigger::Left)),
            (Key::E, Binding::Trigger(Trigger::Right)),
            (
                Key::Left,
                Binding::Axis {
                    axis: Axis::X,
                    value: AXIS_MIN,
                },
            ),
            (
                Key::Right,
                Binding::Axis {
                    axis: Axis::X,
                    value: AXIS_MAX,
                },
            ),
            (
                Key::Down,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MIN,
                },
            ),
            (
                Key::Up,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MAX,
                },
            ),
            (
                Key::Numpad4,
                Binding::Axis {
                    axis: Axis::Rx,
                    value: AXIS_MIN,
                },
            ),
            (
                Key::Numpad6,
                Binding::Axis {
                    axis: Axis::Rx,
                    value: AXIS_MAX,
                },
            ),
            (
                Key::Numpad2,
                Binding::Axis {
                    axis: Axis::Ry,
                    value: AXIS_MIN,
                },
            ),
            (
                Key::Numpad8,
                Binding::Axis {
                    axis: Axis::Ry,
                    value: AXIS_MAX,
                },
            ),
            (Key::I, Binding::Dpad(DpadDirection::Up)),
            (Key::K, Binding::Dpad(DpadDirection::Down)),
            (Key::J, Binding::Dpad(DpadDirection::Left)),
            (Key::L, Binding::Dpad(DpadDirection::Right)),
            (Key::Enter, Binding::Button(XButton::A)),
        ];

        Self {
            name: Self::DEFAULT_NAME.to_owned(),
            entries,
            protected: true,
        }
    }

    /// The protected `empty` preset: every function present with `Key::None`,
    /// no custom functions — legacy `Preset.Reset()` / `CreateEmptyPreset()`.
    /// Dead code in the legacy fork (docs referenced it but `ImuttablePresets`
    /// never contained it); restored as a real built-in per the design.
    pub fn builtin_empty() -> Self {
        let mut entries = Vec::with_capacity(25);
        for button in XButton::ALL {
            entries.push((Key::None, Binding::Button(*button)));
        }
        for trigger in Trigger::ALL {
            entries.push((Key::None, Binding::Trigger(*trigger)));
        }
        for axis in Axis::ALL {
            entries.push((
                Key::None,
                Binding::Axis {
                    axis: *axis,
                    value: AXIS_MIN,
                },
            ));
            entries.push((
                Key::None,
                Binding::Axis {
                    axis: *axis,
                    value: AXIS_MAX,
                },
            ));
        }
        for direction in DpadDirection::ALL {
            entries.push((Key::None, Binding::Dpad(*direction)));
        }

        Self {
            name: Self::EMPTY_NAME.to_owned(),
            entries,
            protected: true,
        }
    }

    /// Both built-ins, `default` first (legacy list order).
    pub fn builtins() -> Vec<Self> {
        vec![Self::builtin_default(), Self::builtin_empty()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_for(preset: &Preset, binding: Binding) -> Vec<Key> {
        preset
            .entries
            .iter()
            .filter(|(_, b)| *b == binding)
            .map(|(k, _)| *k)
            .collect()
    }

    #[test]
    fn default_preset_matches_legacy_exactly() {
        let p = Preset::builtin_default();
        assert_eq!(p.name, "default");
        assert!(p.protected);
        // 11 buttons + 2 triggers + 8 axes + 4 dpads + 1 custom = 26 rows.
        assert_eq!(p.entries.len(), 26);

        assert_eq!(
            keys_for(&p, Binding::Button(XButton::Start)),
            vec![Key::Escape]
        );
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::Back)),
            vec![Key::Backspace]
        );
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::LeftThumb)),
            vec![Key::LeftShift]
        );
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::RightThumb)),
            vec![Key::RightShift]
        );
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::LeftBumper)),
            vec![Key::Z]
        );
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::RightBumper)),
            vec![Key::C]
        );
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::Guide)),
            vec![Key::LeftWindows]
        );
        // The custom-function quirk: A is driven by S AND Enter (many-to-one).
        assert_eq!(
            keys_for(&p, Binding::Button(XButton::A)),
            vec![Key::S, Key::Enter]
        );
        assert_eq!(keys_for(&p, Binding::Button(XButton::B)), vec![Key::D]);
        assert_eq!(keys_for(&p, Binding::Button(XButton::X)), vec![Key::A]);
        assert_eq!(keys_for(&p, Binding::Button(XButton::Y)), vec![Key::W]);

        assert_eq!(keys_for(&p, Binding::Trigger(Trigger::Left)), vec![Key::Q]);
        assert_eq!(keys_for(&p, Binding::Trigger(Trigger::Right)), vec![Key::E]);

        let axis = |axis, value| Binding::Axis { axis, value };
        assert_eq!(keys_for(&p, axis(Axis::X, AXIS_MIN)), vec![Key::Left]);
        assert_eq!(keys_for(&p, axis(Axis::X, AXIS_MAX)), vec![Key::Right]);
        assert_eq!(keys_for(&p, axis(Axis::Y, AXIS_MIN)), vec![Key::Down]);
        assert_eq!(keys_for(&p, axis(Axis::Y, AXIS_MAX)), vec![Key::Up]);
        assert_eq!(keys_for(&p, axis(Axis::Rx, AXIS_MIN)), vec![Key::Numpad4]);
        assert_eq!(keys_for(&p, axis(Axis::Rx, AXIS_MAX)), vec![Key::Numpad6]);
        assert_eq!(keys_for(&p, axis(Axis::Ry, AXIS_MIN)), vec![Key::Numpad2]);
        assert_eq!(keys_for(&p, axis(Axis::Ry, AXIS_MAX)), vec![Key::Numpad8]);

        assert_eq!(keys_for(&p, Binding::Dpad(DpadDirection::Up)), vec![Key::I]);
        assert_eq!(
            keys_for(&p, Binding::Dpad(DpadDirection::Down)),
            vec![Key::K]
        );
        assert_eq!(
            keys_for(&p, Binding::Dpad(DpadDirection::Left)),
            vec![Key::J]
        );
        assert_eq!(
            keys_for(&p, Binding::Dpad(DpadDirection::Right)),
            vec![Key::L]
        );
    }

    #[test]
    fn empty_preset_matches_legacy_reset() {
        let p = Preset::builtin_empty();
        assert_eq!(p.name, "empty");
        assert!(p.protected);
        // 11 buttons + 2 triggers + 4 axes × (Min, Max) + 4 dpads = 25 rows.
        assert_eq!(p.entries.len(), 25);
        assert!(p.entries.iter().all(|(k, _)| *k == Key::None));
        // Every function appears exactly once.
        let mut bindings: Vec<Binding> = p.entries.iter().map(|(_, b)| *b).collect();
        bindings.sort();
        bindings.dedup();
        assert_eq!(bindings.len(), 25);
    }

    #[test]
    fn builtins_order_and_protection() {
        let builtins = Preset::builtins();
        assert_eq!(builtins.len(), 2);
        assert_eq!(builtins[0].name, Preset::DEFAULT_NAME);
        assert_eq!(builtins[1].name, Preset::EMPTY_NAME);
        assert!(builtins.iter().all(|p| p.protected));
    }
}
