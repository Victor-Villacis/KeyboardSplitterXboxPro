//! Golden-file tests over the real cabinet corpus (`tests/fixtures/`).
//!
//! The three `splitter_*.xml` files are the user's live config: the importer
//! must convert them with ZERO warnings — a warning means the mapping is
//! wrong, not the data. `splitter_presets.original.bak` is a second corpus
//! (pre-cab presets incl. `<custom>` rows); whatever it produces is
//! snapshotted as-is.

use std::path::{Path, PathBuf};

use ksx_legacy_import::{decode_xml, import_dir, parse_presets, LegacyImport};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn bundle(import: &LegacyImport) -> String {
    let mut out = String::new();
    for file in import.rendered_files().expect("render") {
        out.push_str(&format!("═══ {} ═══\n{}\n", file.path, file.content));
    }
    out
}

#[test]
fn primary_corpus_imports_with_zero_warnings() {
    let import = import_dir(&fixtures_dir()).expect("import");

    assert!(
        import.report.warnings.is_empty(),
        "the live cab XML must import warning-free, got: {:#?}",
        import.report.warnings
    );
    assert!(import.report.skipped.is_empty());
    assert_eq!(import.report.presets, 4);
    assert_eq!(import.report.games, 1);

    // The importer carries the real legacy settings, not the ksx defaults.
    let settings = import.settings.as_ref().expect("settings imported");
    assert_eq!(settings.mouse_move_deadzone, 1);
    assert_eq!(settings.starting_user_index, 1);
    assert_eq!(settings.block_keyboards, ksx_core::Blocking::Whole);
    assert!(!settings.block_mice);

    // HWIDs land verbatim; the empty legacy Mouse attribute becomes absent.
    let game = &import.games.as_ref().unwrap().games[0];
    assert_eq!(game.slots.len(), 4);
    for (i, slot) in game.slots.iter().enumerate() {
        assert_eq!(slot.number, (i + 1) as u8);
        assert_eq!(slot.user_index, Some((i + 1) as u8));
        assert_eq!(
            slot.keyboard.as_deref(),
            Some(r"HID\VID_D209&PID_0430&REV_0056&MI_00")
        );
        assert_eq!(slot.mouse, None);
        assert_eq!(slot.preset, format!("IPAC P{}", i + 1));
    }

    insta::assert_snapshot!("primary_corpus", bundle(&import));
}

#[test]
fn primary_corpus_report_json_is_stable() {
    let import = import_dir(&fixtures_dir()).expect("import");
    insta::assert_snapshot!("primary_corpus_report_json", import.report.to_json());
}

#[test]
fn original_bak_corpus_imports_with_zero_warnings() {
    // Second corpus: the pre-cab presets file, incl. real <custom> rows
    // (function="2" = Dpad_Down) on 'Phase Shift (Autostrum)'.
    let bytes = std::fs::read(fixtures_dir().join("splitter_presets.original.bak")).unwrap();
    let (text, had_errors) = decode_xml(&bytes);
    assert!(!had_errors);

    let parsed = parse_presets(&text, "splitter_presets.original.bak");
    assert!(
        parsed.warnings.is_empty(),
        "expected zero warnings, got: {:#?}",
        parsed.warnings
    );
    assert_eq!(parsed.presets.len(), 4);

    // The custom functions expand into plain dpad.down bindings, aggregated
    // after the native dpad row (entry order preserved).
    let autostrum = parsed
        .presets
        .iter()
        .find(|p| p.name == "Phase Shift (Autostrum)")
        .unwrap();
    assert_eq!(
        autostrum.bindings.get("dpad.down"),
        Some(&ksx_config::BindingEntry::Keys(vec![
            "Down".into(),
            "LeftShift".into(),
            "Z".into(),
            "X".into(),
            "C".into(),
            "V".into(),
        ]))
    );

    let mut out = String::new();
    for preset in &parsed.presets {
        out.push_str(&format!(
            "═══ {} ═══\n{}\n",
            preset.name,
            toml::to_string(preset).unwrap()
        ));
    }
    insta::assert_snapshot!("original_bak_corpus", out);
}

#[test]
fn v1_schema_upgrades_transparently() {
    // v1 markers: <preset Name>, named ID attributes, Position names, <pov>,
    // pre-rename trigger names, named custom functions
    // (legacy/KeyboardSplitter/Presets/PresetUpgrader.cs).
    let v1 = r#"<?xml version="1.0" encoding="utf-16"?>
<preset_data>
  <preset Name="v1 preset">
    <button ID="A">S</button>
    <button ID="LeftBumper">Z</button>
    <trigger ID="LeftTrigger">Q</trigger>
    <trigger ID="Right">E</trigger>
    <axis ID="X" Position="Min">Left</axis>
    <axis ID="Ry" Position="Max">Numpad8</axis>
    <pov ID="Up">I</pov>
    <dpad ID="Down">K</dpad>
    <custom ID="Button_A">Enter</custom>
    <custom ID="Axis_X_Min">None</custom>
  </preset>
</preset_data>"#;

    // Feed it as BOM-carrying UTF-16LE, exactly as a real legacy file.
    let mut bytes = vec![0xFF, 0xFE];
    for unit in v1.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let (text, had_errors) = decode_xml(&bytes);
    assert!(!had_errors);

    let parsed = parse_presets(&text, "v1.xml");
    assert!(
        parsed.warnings.is_empty(),
        "v1 upgrade must be transparent, got: {:#?}",
        parsed.warnings
    );
    assert_eq!(parsed.presets.len(), 1);
    let preset = &parsed.presets[0];
    assert_eq!(
        preset.bindings.get("A"),
        // Native button row + expanded Button_A custom function.
        Some(&ksx_config::BindingEntry::Keys(vec![
            "S".into(),
            "Enter".into()
        ]))
    );
    assert_eq!(
        preset.bindings.get("lt"),
        Some(&ksx_config::BindingEntry::Key("Q".into()))
    );
    assert_eq!(
        preset.bindings.get("dpad.up"),
        Some(&ksx_config::BindingEntry::Key("I".into()))
    );

    insta::assert_snapshot!("v1_upgrade", toml::to_string(preset).unwrap());
}

#[test]
fn unmappable_entries_warn_precisely_without_aborting() {
    let xml = r#"<preset_data>
  <preset name="mixed">
    <button id="4096">G</button>
    <button id="99">Q</button>
    <button id="8192">NoSuchKey</button>
    <trigger id="3">H</trigger>
    <axis id="1" value="notanumber">M</axis>
    <dpad direction="0">K</dpad>
    <custom function="524288">L</custom>
    <telemetry>x</telemetry>
  </preset>
</preset_data>"#;
    let parsed = parse_presets(xml, "bad.xml");

    // The good entry survives.
    assert_eq!(parsed.presets.len(), 1);
    assert_eq!(
        parsed.presets[0].bindings.get("A"),
        Some(&ksx_config::BindingEntry::Key("G".into()))
    );

    let warned: Vec<String> = parsed.warnings.iter().map(ToString::to_string).collect();
    insta::assert_snapshot!("unmappable_warnings", warned.join("\n"));

    // Every warning is precise: file + preset + entry + reason.
    for warning in &parsed.warnings {
        assert_eq!(warning.file, "bad.xml");
        assert_eq!(warning.preset.as_deref(), Some("mixed"));
        assert!(warning.entry.is_some());
    }
}

#[test]
fn utf8_without_bom_is_accepted_defensively() {
    let xml =
        "<preset_data><preset name=\"p\"><button id=\"4096\">S</button></preset></preset_data>";
    let (text, had_errors) = decode_xml(xml.as_bytes());
    assert!(!had_errors);
    let parsed = parse_presets(&text, "f.xml");
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.presets.len(), 1);
}

#[test]
fn missing_directory_and_empty_directory_are_hard_errors() {
    let missing = fixtures_dir().join("no-such-dir");
    assert!(matches!(
        import_dir(&missing),
        Err(ksx_legacy_import::ImportError::MissingDir(_))
    ));

    let empty = std::env::temp_dir().join("ksx-import-empty-dir-test");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(matches!(
        import_dir(&empty),
        Err(ksx_legacy_import::ImportError::NoLegacyFiles(_))
    ));
}

#[test]
fn write_outputs_merges_settings_into_existing_config() {
    let root = std::env::temp_dir().join(format!(
        "ksx-import-write-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // Pre-existing config with a device + slot the import must not clobber.
    let existing = r#"schema_version = 1

[settings]
block_keyboards = false
block_mice = true
mouse_move_deadzone = 9
starting_user_index = 3

[[device]]
id = "HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"
alias = "P1 I-PAC"

[[slot]]
number = 1
keyboard = "P1 I-PAC"
preset = "IPAC P1"
"#;
    std::fs::write(root.join("config.toml"), existing).unwrap();

    let import = import_dir(&fixtures_dir()).expect("import");
    let written = import.write_outputs(&root).expect("write");

    let names: Vec<String> = written
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "config.toml",
            "games.toml",
            "presets/IPAC P1.toml",
            "presets/IPAC P2.toml",
            "presets/IPAC P3.toml",
            "presets/IPAC P4.toml",
        ]
    );

    let merged: ksx_config::ConfigFile =
        toml::from_str(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    // Settings replaced by the import...
    assert_eq!(merged.settings.mouse_move_deadzone, 1);
    assert_eq!(merged.settings.block_keyboards, ksx_core::Blocking::Whole);
    // ...devices/slots preserved.
    assert_eq!(merged.devices.len(), 1);
    assert_eq!(merged.devices[0].alias, "P1 I-PAC");
    assert_eq!(merged.slots.len(), 1);

    // Re-import over the written outputs is idempotent for presets/games.
    let games: ksx_config::GamesFile =
        toml::from_str(&std::fs::read_to_string(root.join("games.toml")).unwrap()).unwrap();
    assert_eq!(games, *import.games.as_ref().unwrap());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn corrupt_existing_config_is_never_clobbered() {
    let root = std::env::temp_dir().join(format!(
        "ksx-import-corrupt-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "this is [not toml").unwrap();

    let import = import_dir(&fixtures_dir()).expect("import");
    assert!(matches!(
        import.write_outputs(&root),
        Err(ksx_legacy_import::ImportError::ExistingConfig { .. })
    ));
    // The corrupt file is untouched.
    assert_eq!(
        std::fs::read_to_string(root.join("config.toml")).unwrap(),
        "this is [not toml"
    );

    let _ = std::fs::remove_dir_all(&root);
}
