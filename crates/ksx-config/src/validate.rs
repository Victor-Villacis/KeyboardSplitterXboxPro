//! Cross-file validation: structured issues, never panics, never fails.
//!
//! Loading is lenient (see [`crate::store`]); validation is where problems
//! become actionable. Every issue carries enough context for the CLI to print
//! a root-cause message (and serializes cleanly for `--json` output).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;

use crate::config::ConfigFile;
use crate::error::ConfigError;
use crate::function::parse_function;
use crate::games::GamesFile;
use crate::preset::{BindingEntry, PresetFile};
use ksx_core::{Key, Preset, MAX_SLOTS};

/// One validation finding. All findings are non-fatal: the caller decides
/// whether to refuse to start emulation (unknown preset ref) or just warn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Issue {
    /// `settings.mouse_move_deadzone` outside 0..=12.
    MouseMoveDeadzoneOutOfRange { value: u8 },
    /// `settings.starting_user_index` outside 1..=4.
    StartingUserIndexOutOfRange { value: u8 },
    /// Two `[[device]]` entries share an alias (references become ambiguous).
    DuplicateDeviceAlias { alias: String },
    /// Two `[[slot]]` entries share a number.
    DuplicateSlotNumber { number: u8 },
    /// `[[slot]]` number outside 1..=4.
    SlotNumberOutOfRange { number: u8 },
    /// Slot references a preset that is neither a preset file nor a built-in.
    UnknownPresetRef { slot: u8, preset: String },
    /// Slot keyboard/mouse is neither a `[[device]]` alias nor an instance
    /// path (paths are recognized by containing `\`).
    UnknownDeviceRef { slot: u8, reference: String },
    /// Preset binding key that is not a function name.
    UnknownFunction { preset: String, function: String },
    /// Axis function with a value that is not `min`/`max`/i16.
    InvalidAxisValue { preset: String, function: String },
    /// Preset binding value that fails `Key::from_name`.
    UnknownKeyName {
        preset: String,
        function: String,
        key: String,
    },
    /// Game slot number outside 1..=4.
    GameSlotNumberOutOfRange { game: String, number: u8 },
    /// Two slots of one game share a number.
    GameDuplicateSlotNumber { game: String, number: u8 },
    /// Game slot references an unknown preset.
    GameUnknownPresetRef {
        game: String,
        slot: u8,
        preset: String,
    },
    /// Advisory `user_index` outside 1..=4.
    GameUserIndexOutOfRange { game: String, slot: u8, value: u8 },
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::MouseMoveDeadzoneOutOfRange { value } => {
                write!(
                    f,
                    "settings.mouse_move_deadzone is {value}, allowed range is 0..=12"
                )
            }
            Issue::StartingUserIndexOutOfRange { value } => {
                write!(
                    f,
                    "settings.starting_user_index is {value}, allowed range is 1..=4"
                )
            }
            Issue::DuplicateDeviceAlias { alias } => {
                write!(f, "more than one [[device]] uses alias '{alias}'")
            }
            Issue::DuplicateSlotNumber { number } => {
                write!(f, "more than one [[slot]] uses number {number}")
            }
            Issue::SlotNumberOutOfRange { number } => {
                write!(f, "[[slot]] number {number} is outside 1..={MAX_SLOTS}")
            }
            Issue::UnknownPresetRef { slot, preset } => {
                write!(f, "slot {slot} references preset '{preset}', which is neither a preset file nor a built-in")
            }
            Issue::UnknownDeviceRef { slot, reference } => {
                write!(f, "slot {slot} references device '{reference}', which is neither a [[device]] alias nor an instance path")
            }
            Issue::UnknownFunction { preset, function } => {
                write!(f, "preset '{preset}': '{function}' is not a function name")
            }
            Issue::InvalidAxisValue { preset, function } => {
                write!(
                    f,
                    "preset '{preset}': '{function}' needs min, max or a signed 16-bit value"
                )
            }
            Issue::UnknownKeyName {
                preset,
                function,
                key,
            } => {
                write!(
                    f,
                    "preset '{preset}': '{function}' is bound to unknown key '{key}'"
                )
            }
            Issue::GameSlotNumberOutOfRange { game, number } => {
                write!(
                    f,
                    "game '{game}': slot number {number} is outside 1..={MAX_SLOTS}"
                )
            }
            Issue::GameDuplicateSlotNumber { game, number } => {
                write!(f, "game '{game}': more than one slot uses number {number}")
            }
            Issue::GameUnknownPresetRef { game, slot, preset } => {
                write!(
                    f,
                    "game '{game}': slot {slot} references unknown preset '{preset}'"
                )
            }
            Issue::GameUserIndexOutOfRange { game, slot, value } => {
                write!(
                    f,
                    "game '{game}': slot {slot} user_index is {value}, allowed range is 1..=4"
                )
            }
        }
    }
}

/// Validate the main config plus the loaded preset files. Built-in presets
/// (`default`, `empty`) always count as known.
pub fn validate(config: &ConfigFile, presets: &[PresetFile]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let known_presets = known_preset_names(presets);

    let settings = &config.settings;
    if settings.mouse_move_deadzone > 12 {
        issues.push(Issue::MouseMoveDeadzoneOutOfRange {
            value: settings.mouse_move_deadzone,
        });
    }
    if !(1..=4).contains(&settings.starting_user_index) {
        issues.push(Issue::StartingUserIndexOutOfRange {
            value: settings.starting_user_index,
        });
    }

    let mut alias_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for device in &config.devices {
        *alias_counts.entry(device.alias.as_str()).or_default() += 1;
    }
    for (alias, count) in &alias_counts {
        if *count > 1 {
            issues.push(Issue::DuplicateDeviceAlias {
                alias: (*alias).to_owned(),
            });
        }
    }

    let mut number_counts: BTreeMap<u8, usize> = BTreeMap::new();
    for slot in &config.slots {
        *number_counts.entry(slot.number).or_default() += 1;
    }
    for (number, count) in &number_counts {
        if *count > 1 {
            issues.push(Issue::DuplicateSlotNumber { number: *number });
        }
    }

    for slot in &config.slots {
        if slot.number == 0 || slot.number > MAX_SLOTS {
            issues.push(Issue::SlotNumberOutOfRange {
                number: slot.number,
            });
        }
        if !known_presets.contains(slot.preset.as_str()) {
            issues.push(Issue::UnknownPresetRef {
                slot: slot.number,
                preset: slot.preset.clone(),
            });
        }
        for reference in [&slot.keyboard, &slot.mouse].into_iter().flatten() {
            let is_path = reference.contains('\\');
            let is_alias = alias_counts.contains_key(reference.as_str());
            if !is_path && !is_alias {
                issues.push(Issue::UnknownDeviceRef {
                    slot: slot.number,
                    reference: reference.clone(),
                });
            }
        }
    }

    for preset in presets {
        validate_preset(preset, &mut issues);
    }
    issues
}

/// Validate `games.toml` against the loaded preset files.
pub fn validate_games(games: &GamesFile, presets: &[PresetFile]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let known_presets = known_preset_names(presets);
    for game in &games.games {
        let mut number_counts: BTreeMap<u8, usize> = BTreeMap::new();
        for slot in &game.slots {
            *number_counts.entry(slot.number).or_default() += 1;
        }
        for (number, count) in &number_counts {
            if *count > 1 {
                issues.push(Issue::GameDuplicateSlotNumber {
                    game: game.title.clone(),
                    number: *number,
                });
            }
        }
        for slot in &game.slots {
            if slot.number == 0 || slot.number > MAX_SLOTS {
                issues.push(Issue::GameSlotNumberOutOfRange {
                    game: game.title.clone(),
                    number: slot.number,
                });
            }
            if !known_presets.contains(slot.preset.as_str()) {
                issues.push(Issue::GameUnknownPresetRef {
                    game: game.title.clone(),
                    slot: slot.number,
                    preset: slot.preset.clone(),
                });
            }
            if let Some(value) = slot.user_index {
                if !(1..=4).contains(&value) {
                    issues.push(Issue::GameUserIndexOutOfRange {
                        game: game.title.clone(),
                        slot: slot.number,
                        value,
                    });
                }
            }
        }
    }
    issues
}

fn known_preset_names(presets: &[PresetFile]) -> BTreeSet<&str> {
    let mut names: BTreeSet<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    names.insert(Preset::DEFAULT_NAME);
    names.insert(Preset::EMPTY_NAME);
    names
}

fn validate_preset(preset: &PresetFile, issues: &mut Vec<Issue>) {
    let mut pairs = Vec::new();
    for (function, entry) in &preset.bindings {
        flatten_bindings(function, entry, &mut pairs);
    }
    let mut checked_functions = BTreeSet::new();
    for (function, key) in pairs {
        if checked_functions.insert(function.clone()) {
            match parse_function(&function) {
                Ok(_) => {}
                Err(ConfigError::InvalidAxisValue(_)) => issues.push(Issue::InvalidAxisValue {
                    preset: preset.name.clone(),
                    function: function.clone(),
                }),
                Err(_) => issues.push(Issue::UnknownFunction {
                    preset: preset.name.clone(),
                    function: function.clone(),
                }),
            }
        }
        if Key::from_name(key).is_none() {
            issues.push(Issue::UnknownKeyName {
                preset: preset.name.clone(),
                function,
                key: key.to_owned(),
            });
        }
    }
}

/// Flatten nested dotted-key groups into `(function, key_name)` pairs, the
/// same shape [`PresetFile::to_core`] consumes.
fn flatten_bindings<'a>(function: &str, entry: &'a BindingEntry, out: &mut Vec<(String, &'a str)>) {
    match entry {
        BindingEntry::Key(key) => out.push((function.to_owned(), key.as_str())),
        BindingEntry::Keys(keys) => {
            out.extend(keys.iter().map(|k| (function.to_owned(), k.as_str())));
        }
        BindingEntry::Group(group) => {
            for (sub, entry) in group {
                flatten_bindings(&format!("{function}.{sub}"), entry, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DeviceEntry, SlotEntry};

    fn preset(name: &str, toml_bindings: &str) -> PresetFile {
        toml::from_str(&format!("name = \"{name}\"\n[bindings]\n{toml_bindings}")).unwrap()
    }

    fn config(toml_str: &str) -> ConfigFile {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn clean_config_has_no_issues() {
        let cfg = config(
            r#"
schema_version = 1

[[device]]
id = "HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"
alias = "P1 I-PAC"

[[slot]]
number = 1
keyboard = "P1 I-PAC"
preset = "sf2"

[[slot]]
number = 2
keyboard = 'HID\VID_D209&PID_0430&MI_00\9&1&0'
preset = "default"
"#,
        );
        let presets = vec![preset(
            "sf2",
            "A = \"S\"\ndpad.up = \"I\"\n\"lx.min\" = \"Left\"",
        )];
        assert_eq!(validate(&cfg, &presets), Vec::new());
    }

    #[test]
    fn settings_ranges_are_checked() {
        let mut cfg = ConfigFile::default();
        cfg.settings.mouse_move_deadzone = 13;
        cfg.settings.starting_user_index = 0;
        let issues = validate(&cfg, &[]);
        assert!(issues.contains(&Issue::MouseMoveDeadzoneOutOfRange { value: 13 }));
        assert!(issues.contains(&Issue::StartingUserIndexOutOfRange { value: 0 }));
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn slot_and_device_references_are_checked() {
        let cfg = ConfigFile {
            slots: vec![
                SlotEntry {
                    number: 1,
                    keyboard: Some("Ghost Alias".into()),
                    mouse: None,
                    preset: "missing".into(),
                },
                SlotEntry {
                    number: 1,
                    keyboard: None,
                    mouse: None,
                    preset: "default".into(),
                },
                SlotEntry {
                    number: 5,
                    keyboard: None,
                    mouse: None,
                    preset: "empty".into(),
                },
            ],
            ..ConfigFile::default()
        };
        let issues = validate(&cfg, &[]);
        assert!(issues.contains(&Issue::DuplicateSlotNumber { number: 1 }));
        assert!(issues.contains(&Issue::SlotNumberOutOfRange { number: 5 }));
        assert!(issues.contains(&Issue::UnknownPresetRef {
            slot: 1,
            preset: "missing".into()
        }));
        assert!(issues.contains(&Issue::UnknownDeviceRef {
            slot: 1,
            reference: "Ghost Alias".into()
        }));
        assert_eq!(issues.len(), 4);
    }

    #[test]
    fn duplicate_device_aliases_are_reported_once() {
        let device = |alias: &str| DeviceEntry {
            id: r"HID\X".into(),
            alias: alias.into(),
            backend: crate::config::Backend::Interception,
        };
        let cfg = ConfigFile {
            devices: vec![device("P1"), device("P1"), device("P1"), device("P2")],
            ..ConfigFile::default()
        };
        let issues = validate(&cfg, &[]);
        assert_eq!(
            issues,
            vec![Issue::DuplicateDeviceAlias { alias: "P1".into() }]
        );
    }

    #[test]
    fn preset_bindings_are_checked() {
        let presets = vec![preset(
            "bad",
            "warp = \"S\"\n\"lx.zillion\" = \"A\"\nA = [\"S\", \"NotAKey\"]\ndpad.up = \"AlsoFake\"",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(issues.contains(&Issue::UnknownFunction {
            preset: "bad".into(),
            function: "warp".into()
        }));
        assert!(issues.contains(&Issue::InvalidAxisValue {
            preset: "bad".into(),
            function: "lx.zillion".into()
        }));
        assert!(issues.contains(&Issue::UnknownKeyName {
            preset: "bad".into(),
            function: "A".into(),
            key: "NotAKey".into()
        }));
        assert!(issues.contains(&Issue::UnknownKeyName {
            preset: "bad".into(),
            function: "dpad.up".into(),
            key: "AlsoFake".into()
        }));
        assert_eq!(issues.len(), 4);
    }

    #[test]
    fn builtin_presets_round_as_valid() {
        let files: Vec<PresetFile> = ksx_core::Preset::builtins()
            .iter()
            .map(PresetFile::from_core)
            .collect();
        assert_eq!(validate(&ConfigFile::default(), &files), Vec::new());
    }

    #[test]
    fn games_are_checked() {
        let games: GamesFile = toml::from_str(
            r#"
[[game]]
title = "Steam"
path = 'C:\steam.exe'

[[game.slot]]
number = 1
user_index = 5
preset = "missing"

[[game.slot]]
number = 1
preset = "default"

[[game.slot]]
number = 0
preset = "empty"
"#,
        )
        .unwrap();
        let issues = validate_games(&games, &[]);
        assert!(issues.contains(&Issue::GameDuplicateSlotNumber {
            game: "Steam".into(),
            number: 1
        }));
        assert!(issues.contains(&Issue::GameSlotNumberOutOfRange {
            game: "Steam".into(),
            number: 0
        }));
        assert!(issues.contains(&Issue::GameUnknownPresetRef {
            game: "Steam".into(),
            slot: 1,
            preset: "missing".into()
        }));
        assert!(issues.contains(&Issue::GameUserIndexOutOfRange {
            game: "Steam".into(),
            slot: 1,
            value: 5
        }));
        assert_eq!(issues.len(), 4);
    }

    #[test]
    fn issues_display_and_serialize() {
        let issue = Issue::UnknownPresetRef {
            slot: 2,
            preset: "sf2".into(),
        };
        assert_eq!(
            issue.to_string(),
            "slot 2 references preset 'sf2', which is neither a preset file nor a built-in"
        );
        let json = serde_json_shape(&issue);
        assert!(json.contains("unknown_preset_ref"));
    }

    // toml is the only serializer in-tree; shape-check via toml, which uses
    // the same serde field names a future --json path will see.
    fn serde_json_shape(issue: &Issue) -> String {
        #[derive(Serialize)]
        struct Wrap<'a> {
            issue: &'a Issue,
        }
        toml::to_string(&Wrap { issue }).unwrap()
    }
}
