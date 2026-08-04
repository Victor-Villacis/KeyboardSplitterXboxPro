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

/// Value side of a `[bindings]` entry: one key, several keys, or a nested
/// group produced by TOML dotted keys (`dpad.up = "I"` parses as a `dpad`
/// table containing `up`). A quoted literal key with a dot (`"lx.min"`) and a
/// nested group are equivalent; conversion flattens groups with `.`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindingEntry {
    Key(String),
    Keys(Vec<String>),
    Group(BTreeMap<String, BindingEntry>),
}

impl PresetFile {
    /// Convert to the core model. Key names use exact legacy spelling
    /// (`Key::from_name`); `"None"` entries are preserved as inert
    /// placeholders. `protected` is always `false`: built-ins live in code,
    /// files are user presets.
    pub fn to_core(&self) -> Result<ksx_core::Preset, ConfigError> {
        let mut entries = Vec::new();
        for (function, entry) in &self.bindings {
            collect_entries(function, entry, &mut entries)?;
        }
        Ok(ksx_core::Preset {
            name: self.name.clone(),
            entries,
            protected: false,
        })
    }

    /// Convert from the core model. Entries are grouped by function name
    /// (multiple keys become an array); emission uses flat literal keys
    /// (`"dpad.up"`), which parse back identically to the nested form.
    /// `protected` does not survive the trip.
    pub fn from_core(preset: &ksx_core::Preset) -> Self {
        let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, binding) in &preset.entries {
            grouped
                .entry(function_name(binding))
                .or_default()
                .push(key.name().to_owned());
        }
        let bindings = grouped
            .into_iter()
            .map(|(function, mut keys)| {
                let entry = if keys.len() == 1 {
                    BindingEntry::Key(keys.remove(0))
                } else {
                    BindingEntry::Keys(keys)
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
) -> Result<(), ConfigError> {
    match entry {
        BindingEntry::Key(key) => push_entry(function, key, out),
        BindingEntry::Keys(keys) => {
            for key in keys {
                push_entry(function, key, out)?;
            }
            Ok(())
        }
        BindingEntry::Group(group) => {
            for (sub, entry) in group {
                collect_entries(&format!("{function}.{sub}"), entry, out)?;
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
    let key = ksx_core::Key::from_name(key_name)
        .ok_or_else(|| ConfigError::UnknownKey(key_name.to_owned()))?;
    out.push((key, binding));
    Ok(())
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
