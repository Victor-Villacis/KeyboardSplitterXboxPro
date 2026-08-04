//! Fuzz surface 2 (PLAYBOOK "M3/M6 fuzzing"): TOML config + preset parsing.
//!
//! Entry points: the typed `toml::from_str` deserializations behind
//! `Store::load_*`, the `Store` loads themselves (arbitrary file bytes on
//! disk), `parse_function`, and `preset_file_name`. Invariant: never panic;
//! failures are `toml::de::Error` / `ConfigError`.

use std::path::PathBuf;

use ksx_config::{
    function_name, parse_function, preset_file_name, ConfigFile, ConfigRoot, GamesFile, PresetFile,
    Store,
};
use ksx_fuzz::{mutated_bytes, mutated_text};
use proptest::prelude::*;

/// Realistic file shapes (mirrors `ksx-config/tests/snapshots/`), plus the
/// serialized default config, so mutations start near the real grammar.
const POPULATED_CONFIG: &str = r#"schema_version = 1

[settings]
block_keyboards = true
block_mice = false
mouse_move_deadzone = 5
starting_user_index = 1

[[device]]
id = 'HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000'
alias = "P1 I-PAC"
backend = "interception"

[[slot]]
number = 1
keyboard = "P1 I-PAC"
preset = "street-fighter-p1"
"#;

const PRESET_TOML: &str = r#"name = "default"

[bindings]
A = ["S", "Enter"]
B = "D"
back = "Backspace"
"dpad.down" = "K"
"dpad.up" = "I"
guide = "LeftWindows"
lt = "Q"
"lx.max" = "Right"
"lx.min" = "Left"
"lx.-16384" = "Numpad4"
start = "Escape"
"#;

const GAMES_TOML: &str = r#"[[game]]
title = "Steam"
notes = "Steam"
path = 'C:\Program Files (x86)\Steam\steam.exe'
block_keyboards = true
block_mice = false

[[game.slot]]
number = 1
user_index = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P1"
"#;

fn toml_seeds() -> Vec<String> {
    vec![
        toml::to_string(&ConfigFile::default()).expect("default config renders"),
        POPULATED_CONFIG.to_owned(),
        PRESET_TOML.to_owned(),
        GAMES_TOML.to_owned(),
    ]
}

proptest! {
    #![proptest_config(ksx_fuzz::persisting("regressions-config.txt"))]

    /// The three typed deserializations never panic, and whatever parses must
    /// serialize back (saves round-trip through the same schema).
    #[test]
    fn typed_toml_parsing_never_panics(text in mutated_text(toml_seeds(), 4096)) {
        if let Ok(config) = toml::from_str::<ConfigFile>(&text) {
            prop_assert!(toml::to_string(&config).is_ok());
        }
        if let Ok(preset) = toml::from_str::<PresetFile>(&text) {
            // Conversion to the core model is total: Ok or typed ConfigError.
            match preset.to_core() {
                Ok(_) => {}
                Err(err) => prop_assert!(!err.to_string().is_empty()),
            }
            prop_assert!(toml::to_string(&preset).is_ok());
        }
        if let Ok(games) = toml::from_str::<GamesFile>(&text) {
            prop_assert!(toml::to_string(&games).is_ok());
        }
    }

    /// Function names: parsing is total, errors are typed, and every parsed
    /// binding's canonical name reparses to the same binding.
    #[test]
    fn parse_function_is_total_and_canonical_names_round_trip(
        name in prop_oneof![
            2 => proptest::sample::select(vec![
                "A", "b", "X", "y", "start", "back", "guide", "lb", "rb", "lthumb",
                "rthumb", "lt", "rt", "dpad.up", "dpad.down", "dpad.left", "dpad.right",
                "lx.min", "ly.max", "rx.-16384", "ry.32767", "lx.", "dpad.diagonal",
                "zz.min", "lx.99999", "LX.MIN", "Dpad.Up",
            ]).prop_map(str::to_owned),
            2 => ("(lx|ly|rx|ry|dpad|zz)\\.", any::<i32>()).prop_map(|(p, v)| format!("{p}{v}")),
            1 => ".{0,32}",
        ],
    ) {
        match parse_function(&name) {
            Ok(binding) => {
                let canonical = function_name(&binding);
                prop_assert_eq!(
                    parse_function(&canonical).ok(),
                    Some(binding),
                    "canonical name '{}' did not reparse",
                    canonical
                );
            }
            Err(err) => prop_assert!(!err.to_string().is_empty()),
        }
    }

    /// Preset-name sanitization is total and its output is Windows-safe.
    #[test]
    fn preset_file_name_is_total_and_windows_safe(name in ".{0,64}") {
        match preset_file_name(&name) {
            Ok(stem) => {
                prop_assert!(!stem.is_empty());
                prop_assert!(!stem.ends_with(['.', ' ']));
                let illegal = stem.chars().any(|c| {
                    c.is_control() || ['\\', '/', ':', '*', '?', '"', '<', '>', '|'].contains(&c)
                });
                prop_assert!(!illegal, "illegal character in stem {:?}", stem);
            }
            Err(err) => prop_assert!(!err.to_string().is_empty()),
        }
    }
}

proptest! {
    // Filesystem per case: fewer cases than the pure parsers, still scaled by
    // PROPTEST_CASES (burst runs raise it proportionally).
    #![proptest_config(ProptestConfig {
        cases: (ProptestConfig::default().cases / 8).max(16),
        ..ksx_fuzz::persisting("regressions-config.txt")
    })]

    /// `Store::load_*` over arbitrary file bytes (this is the path an
    /// untrusted/corrupted config file actually takes, including the lenient
    /// serde_ignored parse, BOM handling, UTF-8 validation, and the
    /// schema-version gate). Ok or typed `ConfigError`, never a panic.
    #[test]
    fn store_loads_never_panic_on_arbitrary_file_bytes(
        bytes in mutated_bytes(
            toml_seeds().into_iter().map(String::into_bytes).collect(),
            2048,
        ),
    ) {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ksx-fuzz-store");
        std::fs::create_dir_all(root.join("presets")).expect("create fuzz store root");
        std::fs::write(root.join("config.toml"), &bytes).expect("write config.toml");
        std::fs::write(root.join("games.toml"), &bytes).expect("write games.toml");
        std::fs::write(root.join("presets").join("fuzz.toml"), &bytes)
            .expect("write preset");

        let store = Store::new(ConfigRoot::at(&root));
        match store.load_config() {
            Ok(loaded) => {
                for warning in &loaded.warnings {
                    prop_assert!(!warning.to_string().is_empty());
                }
            }
            Err(err) => prop_assert!(!err.to_string().is_empty()),
        }
        match store.load_games() {
            Ok(loaded) => prop_assert!(loaded.warnings.len() < 10_000),
            Err(err) => prop_assert!(!err.to_string().is_empty()),
        }
        match store.load_presets() {
            // A broken preset file is a SkippedPreset warning, never an error.
            Ok(loaded) => prop_assert!(loaded.value.len() <= 1),
            Err(err) => prop_assert!(!err.to_string().is_empty()),
        }
    }
}
