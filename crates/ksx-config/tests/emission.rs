//! Pins the on-disk TOML shapes. A snapshot change here means the file format
//! changed — that must be deliberate (schema_version bump territory).

use ksx_config::{ConfigFile, GamesFile, PresetFile};

#[test]
fn default_config_shape() {
    insta::assert_snapshot!(toml::to_string(&ConfigFile::default()).unwrap());
}

#[test]
fn populated_config_shape() {
    let cfg: ConfigFile = toml::from_str(
        r#"
schema_version = 1

[settings]
block_keyboards = true
block_mice = false
mouse_move_deadzone = 5
starting_user_index = 1

[[device]]
id = "HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"
alias = "P1 I-PAC"
backend = "interception"

[[slot]]
number = 1
keyboard = "P1 I-PAC"
preset = "street-fighter-p1"
"#,
    )
    .unwrap();
    insta::assert_snapshot!(toml::to_string(&cfg).unwrap());
}

#[test]
fn builtin_default_preset_shape() {
    let file = PresetFile::from_core(&ksx_core::Preset::builtin_default());
    insta::assert_snapshot!(toml::to_string(&file).unwrap());
}

#[test]
fn builtin_empty_preset_shape() {
    let file = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
    insta::assert_snapshot!(toml::to_string(&file).unwrap());
}

#[test]
fn games_shape() {
    let games: GamesFile = toml::from_str(
        r#"
[[game]]
title = "Steam"
notes = "Steam"
path = 'C:\Program Files (x86)\Steam\steam.exe'

[[game.slot]]
number = 1
user_index = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P1"
"#,
    )
    .unwrap();
    insta::assert_snapshot!(toml::to_string(&games).unwrap());
}
