//! Games file schema: `%APPDATA%\ksx\games.toml` — same content as legacy
//! `splitter_games.xml` (title, path, args, block flags, per-slot
//! device-by-id + preset-by-name).

use ksx_core::{DeviceId, Persona, SlotSpec};
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GamesFile {
    #[serde(default, rename = "game", skip_serializing_if = "Vec::is_empty")]
    pub games: Vec<GameEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameEntry {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
    /// Image name (`mame.exe`) to track once the launched process is gone.
    ///
    /// No legacy equivalent — the legacy app simply kept emulation running
    /// forever when a launcher handed off. Required in practice for `steam://`
    /// and other protocol profiles, where the shell returns immediately and
    /// there is no process handle to wait on; optional for a plain `.exe`,
    /// where it only matters if that exe is a shim that exits early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
    /// How long a launched program may live and still count as a *launcher*
    /// rather than the game (milliseconds).
    ///
    /// Legacy hard-coded 3 s, and on this cabinet that was wrong: `steam.exe`
    /// took **5 seconds** to hand off to the already-running client, so ksx read
    /// its exit as "the player quit" and stopped emulation mid-launch. The
    /// default is now 10 s (`ksx_games::DEFAULT_LAUNCHER_GRACE_MS`); this key
    /// exists because the right number is a property of the launcher, not of
    /// ksx.
    ///
    /// The trade runs both ways and neither end is free:
    /// - **too low** — a slow launcher's exit is mistaken for the game's, and
    ///   emulation stops while the game is still starting (the observed bug);
    /// - **too high** — a genuinely short session (start a game, quit after
    ///   8 seconds) is mistaken for a hand-off, so ksx spends the hand-off grace
    ///   hunting for a process that has already gone before it notices.
    ///
    /// The second failure is the milder one — it delays a stop, it never stops
    /// a live session — which is why the default errs high.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_grace_ms: Option<u64>,
    /// Legacy `BlockKeyboards` (default true).
    #[serde(default = "default_true")]
    pub block_keyboards: bool,
    /// Legacy `BlockMice` (default false).
    #[serde(default)]
    pub block_mice: bool,
    #[serde(default, rename = "slot", skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<GameSlotEntry>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSlotEntry {
    pub number: u8,
    /// Legacy `GamepadUserIndex` (1..=4). Advisory only: the actual XInput
    /// user index comes from ViGEm's notification callback at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_index: Option<u8>,
    /// Device instance path — games persist ids directly, not aliases
    /// (legacy already persisted hardware-id strings here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<String>,
    pub preset: String,
    /// See [`crate::config::SlotEntry::persona`]. A per-game override: the same
    /// panel can be four Xbox pads for a Steam title and four PlayStation pads
    /// for a PS2 emulator without touching the presets.
    #[serde(
        default,
        with = "crate::persona_serde",
        skip_serializing_if = "crate::persona_serde::is_default"
    )]
    pub persona: Persona,
}

impl GameSlotEntry {
    pub fn to_spec(&self) -> Result<SlotSpec, ConfigError> {
        SlotSpec::new(
            self.number,
            self.keyboard.as_deref().map(DeviceId::new),
            self.mouse.as_deref().map(DeviceId::new),
            self.preset.clone(),
        )
        .map(|spec| spec.with_persona(self.persona))
        .map_err(Into::into)
    }

    /// `user_index` is runtime-discovered and comes back as `None`.
    pub fn from_spec(spec: &SlotSpec) -> Self {
        Self {
            number: spec.number,
            user_index: None,
            persona: spec.persona,
            keyboard: spec.keyboard.as_ref().map(|d| d.as_str().to_owned()),
            mouse: spec.mouse.as_ref().map(|d| d.as_str().to_owned()),
            preset: spec.preset.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the cab's real splitter_games.xml fixture (first two slots).
    const EXAMPLE: &str = r#"
[[game]]
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

[[game.slot]]
number = 2
user_index = 2
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P2"
"#;

    #[test]
    fn example_parses() {
        let games: GamesFile = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(games.games.len(), 1);
        let game = &games.games[0];
        assert_eq!(game.title, "Steam");
        assert_eq!(game.path, r"C:\Program Files (x86)\Steam\steam.exe");
        assert_eq!(game.arguments, "");
        assert!(game.block_keyboards);
        assert!(!game.block_mice);
        assert_eq!(game.slots.len(), 2);
        assert_eq!(game.slots[1].number, 2);
        assert_eq!(game.slots[1].user_index, Some(2));
        assert_eq!(game.slots[1].preset, "IPAC P2");
    }

    #[test]
    fn defaults_match_legacy() {
        let game: GameEntry = toml::from_str("title = \"t\"\npath = \"C:\\\\g.exe\"\n").unwrap();
        assert!(game.block_keyboards);
        assert!(!game.block_mice);
        assert!(game.slots.is_empty());
        assert_eq!(game.notes, "");
        assert_eq!(game.arguments, "");
        assert_eq!(game.process_name, None);
        assert_eq!(game.launcher_grace_ms, None);
    }

    /// The per-profile launcher grace: optional, absent from every imported
    /// legacy file, and never emitted when unset.
    #[test]
    fn launcher_grace_is_optional_and_round_trips() {
        let game: GameEntry = toml::from_str(
            "title = \"t\"\npath = \"C:\\\\steam.exe\"\nlauncher_grace_ms = 20000\n",
        )
        .unwrap();
        assert_eq!(game.launcher_grace_ms, Some(20_000));
        let back: GameEntry = toml::from_str(&toml::to_string(&game).unwrap()).unwrap();
        assert_eq!(back, game);

        let plain: GameEntry = toml::from_str("title = \"t\"\npath = \"C:\\\\g.exe\"\n").unwrap();
        assert!(!toml::to_string(&plain)
            .unwrap()
            .contains("launcher_grace_ms"));
    }

    /// M5 addition: the launcher-handoff hint. Optional, and absent from every
    /// imported legacy file — so it must never be required to parse.
    #[test]
    fn process_name_is_optional_and_round_trips() {
        let game: GameEntry = toml::from_str(
            "title = \"t\"\npath = \"steam://rungameid/620\"\nprocess_name = \"portal2.exe\"\n",
        )
        .unwrap();
        assert_eq!(game.process_name.as_deref(), Some("portal2.exe"));
        let back: GameEntry = toml::from_str(&toml::to_string(&game).unwrap()).unwrap();
        assert_eq!(back, game);
        // ...and it is not emitted when unset, so imported files stay clean.
        let plain: GameEntry = toml::from_str("title = \"t\"\npath = \"C:\\\\g.exe\"\n").unwrap();
        assert!(!toml::to_string(&plain).unwrap().contains("process_name"));
    }

    #[test]
    fn round_trips_through_toml() {
        let games: GamesFile = toml::from_str(EXAMPLE).unwrap();
        let serialized = toml::to_string(&games).unwrap();
        let reparsed: GamesFile = toml::from_str(&serialized).unwrap();
        assert_eq!(games, reparsed);
    }

    #[test]
    fn slot_converts_to_and_from_core() {
        let games: GamesFile = toml::from_str(EXAMPLE).unwrap();
        let entry = &games.games[0].slots[0];
        let spec = entry.to_spec().unwrap();
        assert_eq!(spec.number, 1);
        assert_eq!(
            spec.keyboard,
            Some(ksx_core::DeviceId::new(
                r"HID\VID_D209&PID_0430&REV_0056&MI_00"
            ))
        );
        assert_eq!(spec.mouse, None);
        assert_eq!(spec.preset, "IPAC P1");

        let back = GameSlotEntry::from_spec(&spec);
        assert_eq!(back.number, entry.number);
        assert_eq!(back.keyboard, entry.keyboard);
        assert_eq!(back.mouse, entry.mouse);
        assert_eq!(back.preset, entry.preset);
        // user_index is runtime-discovered; it does not survive.
        assert_eq!(back.user_index, None);
    }

    #[test]
    fn bad_slot_number_is_an_error() {
        let entry = GameSlotEntry {
            number: 0,
            user_index: None,
            keyboard: None,
            mouse: None,
            preset: "p".into(),
            persona: Persona::default(),
        };
        assert!(matches!(
            entry.to_spec(),
            Err(ConfigError::InvalidSlotNumber(_))
        ));
    }
}
