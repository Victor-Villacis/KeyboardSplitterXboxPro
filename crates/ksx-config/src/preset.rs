//! Preset file schema: `%APPDATA%\ksx\presets\<name>.toml` — one file per
//! preset (diffable, shareable).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::function::{function_name, parse_function};

/// One preset file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFile {
    pub name: String,
    /// Keys are function names (see [`crate::function`]); values are legacy
    /// key-name strings or arrays of them.
    #[serde(default)]
    pub bindings: BTreeMap<String, BindingEntry>,
}

/// Value side of a `[bindings]` entry: one key, several keys, a GUARDED key
/// (a chord), a mixed list of those, or a nested group produced by TOML dotted
/// keys (`dpad.up = "I"` parses as a `dpad` table containing `up`). A quoted
/// literal key with a dot (`"lx.min"`) and a nested group are equivalent;
/// conversion flattens groups with `.`.
///
/// ```toml
/// [bindings]
/// A  = "G"                                  # plain, exactly as before
/// rt = { key = "A", when = ["B"] }           # chord: A while B is held
/// lb = ["Q", { key = "A", when = ["C"] }]    # both, on one function
/// ```
///
/// Variant order is load-bearing: `serde(untagged)` tries them top to bottom,
/// so a plain string still parses as [`BindingEntry::Key`] and a table with a
/// `key` field is a guard rather than a dotted group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindingEntry {
    Key(String),
    Keys(Vec<String>),
    Guarded(GuardedEntry),
    /// Mixed list: plain keys and guarded keys on the same function.
    Many(Vec<BindingEntry>),
    Group(BTreeMap<String, BindingEntry>),
}

/// A guarded key — the file spelling of [`ksx_core::Chord`].
///
/// `key` is the trigger; `when` keys must all be held, `unless` keys must not
/// be. `deny_unknown_fields` is what keeps a dotted group (`lx = { min = … }`)
/// from ever being mistaken for a guard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardedEntry {
    /// Trigger key name (legacy spelling), or `"None"` for an inert row.
    pub key: String,
    /// All of these must be held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<String>,
    /// None of these may be held (MAME's `NOT`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unless: Vec<String>,
}

impl PresetFile {
    /// Convert to the core model. Key names use exact legacy spelling
    /// (`Key::from_name`); `"None"` entries are preserved as inert
    /// placeholders. `protected` is always `false`: built-ins live in code,
    /// files are user presets.
    pub fn to_core(&self) -> Result<ksx_core::Preset, ConfigError> {
        let mut entries = Vec::new();
        let mut chords = Vec::new();
        for (function, entry) in &self.bindings {
            collect_entries(function, entry, &mut entries, &mut chords)?;
        }
        Ok(ksx_core::Preset {
            name: self.name.clone(),
            entries,
            chords,
            protected: false,
        })
    }

    /// Convert from the core model. Entries are grouped by function name
    /// (multiple keys become an array); emission uses flat literal keys
    /// (`"dpad.up"`), which parse back identically to the nested form.
    /// `protected` does not survive the trip.
    ///
    /// Chords are emitted after the function's plain keys, so a preset with no
    /// chords serializes to exactly the bytes it always did.
    pub fn from_core(preset: &ksx_core::Preset) -> Self {
        let mut grouped: BTreeMap<String, (Vec<String>, Vec<GuardedEntry>)> = BTreeMap::new();
        for (key, binding) in &preset.entries {
            grouped
                .entry(function_name(binding))
                .or_default()
                .0
                .push(key.name().to_owned());
        }
        for chord in &preset.chords {
            grouped
                .entry(function_name(&chord.binding))
                .or_default()
                .1
                .push(GuardedEntry {
                    key: chord.key.name().to_owned(),
                    when: chord.when.iter().map(|k| k.name().to_owned()).collect(),
                    unless: chord.unless.iter().map(|k| k.name().to_owned()).collect(),
                });
        }
        let bindings = grouped
            .into_iter()
            .map(|(function, (mut keys, mut guards))| {
                let entry = match (keys.len(), guards.len()) {
                    (1, 0) => BindingEntry::Key(keys.remove(0)),
                    (0, 1) => BindingEntry::Guarded(guards.remove(0)),
                    (_, 0) => BindingEntry::Keys(keys),
                    _ => BindingEntry::Many(
                        keys.into_iter()
                            .map(BindingEntry::Key)
                            .chain(guards.into_iter().map(BindingEntry::Guarded))
                            .collect(),
                    ),
                };
                (function, entry)
            })
            .collect();
        Self {
            name: preset.name.clone(),
            bindings,
        }
    }
}

fn collect_entries(
    function: &str,
    entry: &BindingEntry,
    out: &mut Vec<(ksx_core::Key, ksx_core::Binding)>,
    chords: &mut Vec<ksx_core::Chord>,
) -> Result<(), ConfigError> {
    match entry {
        BindingEntry::Key(key) => push_entry(function, key, out),
        BindingEntry::Keys(keys) => {
            for key in keys {
                push_entry(function, key, out)?;
            }
            Ok(())
        }
        BindingEntry::Guarded(guarded) if guarded.when.is_empty() && guarded.unless.is_empty() => {
            push_entry(function, &guarded.key, out)
        }
        BindingEntry::Guarded(guarded) => push_chord(function, guarded, chords),
        BindingEntry::Many(entries) => {
            for entry in entries {
                collect_entries(function, entry, out, chords)?;
            }
            Ok(())
        }
        BindingEntry::Group(group) => {
            for (sub, entry) in group {
                collect_entries(&format!("{function}.{sub}"), entry, out, chords)?;
            }
            Ok(())
        }
    }
}

fn push_entry(
    function: &str,
    key_name: &str,
    out: &mut Vec<(ksx_core::Key, ksx_core::Binding)>,
) -> Result<(), ConfigError> {
    let binding = parse_function(function)?;
    let key = key_named(key_name)?;
    out.push((key, binding));
    Ok(())
}

/// A guarded entry with a non-empty guard. An EMPTY guard is normalized to a
/// plain entry by the caller: `{ key = "G" }` means exactly `"G"`, and must
/// not consume anything (a zero-key "chord" that suppressed its own trigger
/// would silently disable that key's other bindings).
fn push_chord(
    function: &str,
    guarded: &GuardedEntry,
    chords: &mut Vec<ksx_core::Chord>,
) -> Result<(), ConfigError> {
    let binding = parse_function(function)?;
    let key = key_named(&guarded.key)?;
    let when = guarded
        .when
        .iter()
        .map(|k| key_named(k))
        .collect::<Result<Vec<_>, _>>()?;
    let unless = guarded
        .unless
        .iter()
        .map(|k| key_named(k))
        .collect::<Result<Vec<_>, _>>()?;
    chords.push(ksx_core::Chord {
        key,
        binding,
        when,
        unless,
    });
    Ok(())
}

fn key_named(name: &str) -> Result<ksx_core::Key, ConfigError> {
    ksx_core::Key::from_name(name).ok_or_else(|| ConfigError::UnknownKey(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_core::{Axis, Binding, DpadDirection, Key, Preset, Trigger, XButton, AXIS_MIN};

    // docs/research/design-architecture.md §4.1, verbatim.
    const DOC_EXAMPLE: &str = r#"
name = "street-fighter-p1"
[bindings]
A = "S"
B = "D"
lt = "Q"
"lx.min" = "Left"
"lx.-16384" = "None"
dpad.up = "I"
"#;

    fn sorted_entries(preset: &Preset) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = preset
            .entries
            .iter()
            .map(|(k, b)| (function_name(b), k.name().to_owned()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn doc_example_parses_and_converts() {
        let file: PresetFile = toml::from_str(DOC_EXAMPLE).unwrap();
        assert_eq!(file.name, "street-fighter-p1");

        let core = file.to_core().unwrap();
        assert_eq!(core.name, "street-fighter-p1");
        assert!(!core.protected);
        assert_eq!(core.entries.len(), 6);
        assert!(core
            .entries
            .contains(&(Key::S, Binding::Button(XButton::A))));
        assert!(core
            .entries
            .contains(&(Key::D, Binding::Button(XButton::B))));
        assert!(core
            .entries
            .contains(&(Key::Q, Binding::Trigger(Trigger::Left))));
        assert!(core.entries.contains(&(
            Key::Left,
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN
            }
        )));
        // Custom axis value with a placeholder key survives.
        assert!(core.entries.contains(&(
            Key::None,
            Binding::Axis {
                axis: Axis::X,
                value: -16384
            }
        )));
        assert!(core
            .entries
            .contains(&(Key::I, Binding::Dpad(DpadDirection::Up))));
    }

    #[test]
    fn file_round_trips_through_toml() {
        let file: PresetFile = toml::from_str(DOC_EXAMPLE).unwrap();
        let serialized = toml::to_string(&file).unwrap();
        let reparsed: PresetFile = toml::from_str(&serialized).unwrap();
        assert_eq!(file, reparsed);
    }

    #[test]
    fn arrays_mean_many_to_one() {
        let file: PresetFile = toml::from_str(
            r#"
name = "p"
[bindings]
A = ["S", "Enter"]
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        assert_eq!(
            core.entries,
            vec![
                (Key::S, Binding::Button(XButton::A)),
                (Key::Enter, Binding::Button(XButton::A)),
            ]
        );
    }

    #[test]
    fn nested_and_literal_dotted_keys_are_equivalent() {
        let nested: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\ndpad.up = \"I\"\n").unwrap();
        let literal: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\n\"dpad.up\" = \"I\"\n").unwrap();
        // Different in-memory shapes...
        assert_ne!(nested, literal);
        // ...identical core meaning.
        assert_eq!(
            nested.to_core().unwrap().entries,
            literal.to_core().unwrap().entries
        );
    }

    #[test]
    fn builtin_default_survives_core_round_trip() {
        let original = Preset::builtin_default();
        let file = PresetFile::from_core(&original);
        // S is listed before Enter for button A (legacy entry order).
        assert_eq!(
            file.bindings.get("A"),
            Some(&BindingEntry::Keys(vec!["S".into(), "Enter".into()]))
        );
        let back = file.to_core().unwrap();
        assert_eq!(back.name, original.name);
        assert_eq!(sorted_entries(&back), sorted_entries(&original));
        // protected is a code-level attribute and does not survive.
        assert!(!back.protected);
    }

    #[test]
    fn builtin_empty_survives_core_round_trip() {
        let original = Preset::builtin_empty();
        let file = PresetFile::from_core(&original);
        assert_eq!(file.bindings.len(), 25);
        assert!(file
            .bindings
            .values()
            .all(|e| *e == BindingEntry::Key("None".into())));
        let back = file.to_core().unwrap();
        assert_eq!(sorted_entries(&back), sorted_entries(&original));
    }

    #[test]
    fn from_core_emission_reparses_via_toml() {
        let file = PresetFile::from_core(&Preset::builtin_default());
        let serialized = toml::to_string(&file).unwrap();
        let reparsed: PresetFile = toml::from_str(&serialized).unwrap();
        assert_eq!(file, reparsed);
    }

    // ---- chords (docs/INPUT-TRANSFORMS.md §1b) ----------------------------

    /// The documented file shape, and the promise that adding one does not
    /// disturb the plain rows around it.
    #[test]
    fn a_guarded_entry_parses_as_a_chord() {
        let file: PresetFile = toml::from_str(
            r#"
name = "chords"
[bindings]
A = "G"
rt = { key = "A", when = ["B"] }
lb = { key = "A", when = ["B", "C"], unless = ["LeftShift"] }
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        // Plain rows stay plain rows.
        assert_eq!(core.entries, vec![(Key::G, Binding::Button(XButton::A))]);
        assert_eq!(core.chords.len(), 2);
        assert!(core.chords.contains(&ksx_core::Chord {
            key: Key::A,
            binding: Binding::Trigger(Trigger::Right),
            when: vec![Key::B],
            unless: vec![],
        }));
        assert!(core.chords.contains(&ksx_core::Chord {
            key: Key::A,
            binding: Binding::Button(XButton::LeftBumper),
            when: vec![Key::B, Key::C],
            unless: vec![Key::LeftShift],
        }));
    }

    /// One function can carry plain keys AND chords; the list form round-trips.
    #[test]
    fn a_function_can_hold_plain_keys_and_chords_together() {
        let original = Preset {
            name: "mixed".into(),
            entries: vec![
                (Key::Q, Binding::Trigger(Trigger::Right)),
                (Key::E, Binding::Trigger(Trigger::Right)),
            ],
            chords: vec![ksx_core::Chord::new(
                Key::A,
                Binding::Trigger(Trigger::Right),
                vec![Key::B],
            )],
            protected: false,
        };
        let file = PresetFile::from_core(&original);
        let text = toml::to_string(&file).unwrap();
        let reparsed: PresetFile = toml::from_str(&text).unwrap();
        assert_eq!(file, reparsed, "{text}");
        let back = reparsed.to_core().unwrap();
        assert_eq!(back.entries, original.entries);
        assert_eq!(back.chords, original.chords);
    }

    /// Chords survive core → file → TOML → file → core unchanged, including
    /// dotted function names.
    #[test]
    fn chords_round_trip_through_toml() {
        let original = Preset {
            name: "rt".into(),
            entries: vec![(Key::G, Binding::Button(XButton::A))],
            chords: vec![
                ksx_core::Chord::new(Key::D, Binding::Dpad(DpadDirection::Up), vec![Key::F]),
                ksx_core::Chord {
                    key: Key::D,
                    binding: Binding::Axis {
                        axis: Axis::X,
                        value: AXIS_MIN,
                    },
                    when: vec![Key::F, Key::G],
                    unless: vec![Key::LeftShift],
                },
            ],
            protected: false,
        };
        let text = toml::to_string(&PresetFile::from_core(&original)).unwrap();
        let back: PresetFile = toml::from_str(&text).unwrap();
        let core = back.to_core().unwrap();
        assert_eq!(core.entries, original.entries, "{text}");
        assert_eq!(core.chords, original.chords, "{text}");
    }

    /// A guard with nothing in it is not a chord: it must not consume its own
    /// trigger key (which would silently disable that key's other bindings).
    #[test]
    fn an_empty_guard_is_a_plain_binding() {
        let file: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nA = { key = \"G\" }\n").unwrap();
        let core = file.to_core().unwrap();
        assert!(core.chords.is_empty());
        assert_eq!(core.entries, vec![(Key::G, Binding::Button(XButton::A))]);
    }

    /// The regression guarantee at the file layer: a preset without chords
    /// emits exactly the bytes it always did — no guard syntax anywhere.
    #[test]
    fn a_chordless_preset_emits_no_guard_syntax() {
        for preset in Preset::builtins() {
            let text = toml::to_string(&PresetFile::from_core(&preset)).unwrap();
            assert!(!text.contains("when"), "{text}");
            assert!(!text.contains("unless"), "{text}");
            assert!(!text.contains("key ="), "{text}");
        }
    }

    #[test]
    fn unknown_guard_keys_are_errors() {
        let file: PresetFile = toml::from_str(
            "name = \"p\"\n[bindings]\nrt = { key = \"A\", when = [\"NotAKey\"] }\n",
        )
        .unwrap();
        assert!(matches!(file.to_core(), Err(ConfigError::UnknownKey(_))));
    }

    #[test]
    fn unknown_names_are_errors() {
        let bad_function: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nwarp = \"S\"\n").unwrap();
        assert!(matches!(
            bad_function.to_core(),
            Err(ConfigError::UnknownFunction(_))
        ));

        let bad_key: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nA = \"eight\"\n").unwrap();
        assert!(matches!(bad_key.to_core(), Err(ConfigError::UnknownKey(_))));
    }
}
