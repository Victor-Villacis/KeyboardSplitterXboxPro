//! Bindings and presets, including the two built-ins ported from
//! `legacy/KeyboardSplitter/Presets/Preset.cs`.

use crate::key::Key;
use crate::macros::{Macro, MacroTrigger};
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
    Axis {
        axis: Axis,
        value: i16,
    },
    Dpad(DpadDirection),
    /// **Output nothing.** Only meaningful on a [`Chord`], where the chord's
    /// value is entirely in its CONSUMPTION: the constituents are suppressed
    /// and no endpoint is driven in their place. `[Left+Right] → Consume` is
    /// SOCD-neutral (see [`crate::socd`]).
    ///
    /// Modeled as a `Binding` variant rather than `Option<Binding>` on
    /// [`Chord`] for two reasons:
    ///
    /// 1. **The file format is keyed by function name.** `[bindings]` maps a
    ///    function to its key(s), so a consume-only row needs a *name* to live
    ///    under; as a variant it gets `consume` from the existing
    ///    `function_name`/`parse_function` pair and round-trips with no new
    ///    machinery. An `Option` would have needed a parallel spelling.
    /// 2. **`Chord.binding` is public API constructed by struct literal in
    ///    three crates.** A new variant is additive; changing the field type is
    ///    not.
    ///
    /// The cost is that `Consume` is *representable* in [`Preset::entries`],
    /// where it means nothing — an unguarded row consumes nothing by
    /// definition. The engine ignores such rows exactly like a [`Key::None`]
    /// placeholder, and validation reports them.
    Consume,
}

/// A binding that only applies while other keys are (or are not) held — the
/// CHORD (docs/INPUT-TRANSFORMS.md §1b).
///
/// A chord is deliberately **not** a new [`Binding`] kind: it is an ordinary
/// binding plus a guard, so it composes with buttons, triggers, axes and dpad
/// alike ("A+B → RT" and "A+B → lx.min" are the same construct).
///
/// - `key` is the TRIGGER: the key whose press completes the chord.
/// - `when` keys must ALL be held for the chord to apply.
/// - `unless` keys must NONE be held (MAME's `NOT`).
///
/// **Consumption**: while the chord is active, its constituents (`key` plus
/// every `when` key) are suppressed — their own unguarded entries produce
/// nothing, so the game sees the chord's output instead of the parts. `unless`
/// keys are a negative condition, never consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    /// Trigger key. [`Key::None`] makes the whole chord an inert placeholder.
    pub key: Key,
    /// What the chord drives while it is active.
    pub binding: Binding,
    /// All of these must be held.
    pub when: Vec<Key>,
    /// None of these may be held.
    pub unless: Vec<Key>,
}

impl Chord {
    /// A `when`-only chord (the common shape).
    pub fn new(key: Key, binding: Binding, when: Vec<Key>) -> Self {
        Self {
            key,
            binding,
            when,
            unless: Vec::new(),
        }
    }

    /// A chord that outputs NOTHING and exists purely to consume its
    /// constituents ([`Binding::Consume`]) — the SOCD-neutral primitive.
    pub fn consuming(key: Key, when: Vec<Key>) -> Self {
        Self::new(key, Binding::Consume, when)
    }

    /// Does this chord drive an endpoint at all?
    pub fn emits(&self) -> bool {
        self.binding != Binding::Consume
    }

    /// Guard size: how many extra conditions this chord carries. A chord with
    /// a LARGER guard is more specific and wins over one with a smaller guard
    /// that shares a constituent (A+B+C beats A+B). Equal specificity is a
    /// config error, caught by `ksx-config`'s validation rather than resolved
    /// by a coin flip.
    pub fn specificity(&self) -> usize {
        self.when.len() + self.unless.len()
    }

    /// The keys this chord CONSUMES while active: the trigger plus every
    /// `when` key. Guard keys listed in `unless` are not consumed — they are
    /// not held in the first place.
    pub fn constituents(&self) -> impl Iterator<Item = Key> + '_ {
        std::iter::once(self.key).chain(self.when.iter().copied())
    }

    /// Every key the guard mentions (`when` then `unless`) — the keys whose
    /// state can change this chord's activation, minus the trigger.
    pub fn guard_keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.when.iter().copied().chain(self.unless.iter().copied())
    }
}

/// A preset's macro table: the definitions and the rows that start them
/// (docs/INPUT-TRANSFORMS.md §1c).
///
/// One struct rather than two [`Preset`] fields so `preset.macros.is_empty()`
/// is the single check for "this preset predates macros, take the old path" —
/// the same regression guarantee `chords.is_empty()` gives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Macros {
    /// Definitions, in file order. [`MacroTrigger::index`] indexes this.
    pub defs: Vec<Macro>,
    /// `key → macro` rows. Many keys → one macro and one key → several macros
    /// are both native, exactly as for plain bindings.
    pub triggers: Vec<MacroTrigger>,
}

impl Macros {
    /// Nothing defined and nothing triggered — the byte-identical path.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty() && self.triggers.is_empty()
    }

    /// Position of the macro called `name`, compared case-insensitively (the
    /// file's function names are).
    pub fn index_of(&self, name: &str) -> Option<u16> {
        self.defs
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
            .and_then(|i| u16::try_from(i).ok())
    }

    /// The macro a trigger points at, or `None` for a dangling index (which
    /// validation reports and the engine ignores).
    pub fn get(&self, index: u16) -> Option<&Macro> {
        self.defs.get(usize::from(index))
    }

    pub fn named(&self, name: &str) -> Option<&Macro> {
        self.index_of(name).and_then(|i| self.get(i))
    }

    /// Every key that starts `index`, in trigger order.
    pub fn keys_for(&self, index: u16) -> impl Iterator<Item = Key> + '_ {
        self.triggers
            .iter()
            .filter(move |t| t.index == index)
            .map(|t| t.key)
    }
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
    ///
    /// Entries here are UNGUARDED: they apply whenever their key is down (and
    /// no active chord is consuming that key). Guarded entries live in
    /// [`Preset::chords`], which keeps this field — and every file, importer
    /// and test that predates chords — byte-for-byte unchanged.
    pub entries: Vec<(Key, Binding)>,
    /// Guarded entries: the same bindings, conditional on other keys. Empty in
    /// every preset that predates chords, which is exactly why the chord-free
    /// engine path is unchanged.
    pub chords: Vec<Chord>,
    /// Timed sequences and the keys that start them
    /// (docs/INPUT-TRANSFORMS.md §1c). Empty in every preset that predates
    /// macros, which is exactly why the macro-free engine path is unchanged.
    pub macros: Macros,
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
            chords: Vec::new(),
            macros: Default::default(),
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
            chords: Vec::new(),
            macros: Default::default(),
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
    fn a_chord_names_its_specificity_and_its_constituents() {
        let chord = Chord {
            key: Key::A,
            binding: Binding::Trigger(Trigger::Right),
            when: vec![Key::B],
            unless: vec![Key::LeftShift],
        };
        assert_eq!(chord.specificity(), 2);
        // The trigger and every `when` key are consumed; `unless` is not.
        assert_eq!(
            chord.constituents().collect::<Vec<_>>(),
            vec![Key::A, Key::B]
        );
        assert_eq!(
            chord.guard_keys().collect::<Vec<_>>(),
            vec![Key::B, Key::LeftShift]
        );
        // A+B+C is more specific than A+B, and wins where they overlap.
        let bigger = Chord::new(
            Key::A,
            Binding::Button(XButton::LeftBumper),
            vec![Key::B, Key::C],
        );
        assert!(
            bigger.specificity() > Chord::new(Key::A, chord.binding, vec![Key::B]).specificity()
        );
        assert!(bigger.unless.is_empty());
    }

    #[test]
    fn builtins_carry_no_chords() {
        // The regression guarantee: nothing that predates chords grows one.
        assert!(Preset::builtins().iter().all(|p| p.chords.is_empty()));
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
