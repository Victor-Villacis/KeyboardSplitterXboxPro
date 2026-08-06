//! **`ksx slot assign` — which preset a slot uses.**
//!
//! docs/CONTROL-SURFACE.md's honest gaps 1 and 5, closed. The mapper edits
//! bindings *inside* a preset; nothing until now could say which preset slot 3
//! points at except a hand edit of `config.toml` — or, since the interop verbs
//! landed, a whole-file `ksx config export | edit | import`, which works and
//! **loses every comment in the file**. That is exactly the loss the TOML-is-
//! canonical decision exists to prevent, so the narrow verb is worth having:
//! this one rewrites one field of one entry.
//!
//! # One writer, like every other write
//!
//! Same shape as [`crate::mapping`]: a typed spec in, a typed "here is what
//! landed" out, a timestamped backup taken before the write, and the store's
//! atomic save doing the actual I/O. The CLI verb, the pipe verb and any GUI
//! all call [`assign`] — there is no second editor, which is the standing rule
//! ("every front-door action maps to an existing backend verb").
//!
//! # Why a slot assignment BOUNCES the pads
//!
//! Every other write on the control surface is a key→function table and the
//! live engine takes it in place. This one is not, and the surface has to say
//! so *before* it is used. See [`ksx_api::SlotOutcome::restarted`] for the full
//! reasoning; the short version is that this verb writes the slot ENTRY, and a
//! verb whose pad behaviour depends on which field you happened to touch is a
//! verb nobody can predict.

use std::path::PathBuf;

use ksx_config::{ConfigError, GameSlotEntry, SlotEntry, Store};
use ksx_core::MAX_SLOTS;

/// One slot assignment, as any surface spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSpec {
    /// 1..=[`MAX_SLOTS`].
    pub slot: u8,
    /// The preset's `name`, matched case-insensitively against what is on disk
    /// (a preset that exists keeps its own spelling — the same rule the macro
    /// writer uses for table names).
    pub preset: String,
    /// A games.toml profile title, or `None` for `config.toml`'s `[[slot]]`
    /// list. The same either/or `ksx setup` asks about.
    pub profile: Option<String>,
}

/// What a successful [`assign`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedSlot {
    pub path: PathBuf,
    pub slot: u8,
    /// The preset as the FILE now spells it.
    pub preset: String,
    /// What the slot pointed at before; `None` when the slot was created.
    pub previous: Option<String>,
    pub profile: Option<String>,
    /// The slot was added rather than repointed.
    pub created: bool,
    /// The slot already used that preset and the file was left alone.
    pub unchanged: bool,
    /// The timestamped copy taken before the write. `None` when nothing was
    /// written (`unchanged`) or the file did not exist yet.
    pub backup: Option<PathBuf>,
}

impl AppliedSlot {
    /// The one sentence a surface prints. It always names the pad bounce,
    /// because the pad bounce is the part a user must not have to infer.
    pub fn message(&self) -> String {
        let where_ = match &self.profile {
            Some(profile) => format!(" in profile \"{profile}\""),
            None => String::new(),
        };
        if self.unchanged {
            return format!(
                "slot {} already uses \"{}\"{where_} — nothing was written",
                self.slot, self.preset
            );
        }
        let change = match &self.previous {
            Some(previous) => format!(
                "slot {}{where_}: \"{previous}\" → \"{}\"",
                self.slot, self.preset
            ),
            None => format!(
                "slot {} added{where_}, using \"{}\"",
                self.slot, self.preset
            ),
        };
        format!("{change} — a slot change needs the pads replugged")
    }
}

/// Why an assignment was refused. Every variant carries what the caller should
/// have said instead — a refusal that lists the presets on disk is the
/// difference between a dead end and a next step.
#[derive(Debug, thiserror::Error)]
pub enum SlotError {
    #[error("slot number must be 1..={MAX_SLOTS}, got {given}")]
    BadSlot { given: u8 },
    #[error(
        "no preset called \"{preset}\"{}",
        list_or_none(available, "presets on disk")
    )]
    UnknownPreset {
        preset: String,
        available: Vec<String>,
    },
    #[error(
        "no games.toml profile called \"{profile}\"{}",
        list_or_none(available, "profiles in games.toml")
    )]
    UnknownProfile {
        profile: String,
        available: Vec<String>,
    },
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl SlotError {
    /// The stable refusal word — the same one `--json` prints and the pipe
    /// answers with.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadSlot { .. } => ksx_api::codes::BAD_SLOT,
            Self::UnknownPreset { .. } => ksx_api::codes::UNKNOWN_PRESET,
            Self::UnknownProfile { .. } => ksx_api::codes::UNKNOWN_PROFILE,
            Self::Config(_) => "config-error",
        }
    }
}

/// `" — presets on disk: a, b, c"`, or a plain "(none)" note when the cabinet
/// genuinely has none. Never an empty tail that reads as if the list was
/// forgotten.
fn list_or_none(available: &[String], what: &str) -> String {
    if available.is_empty() {
        format!(" — there are no {what}")
    } else {
        format!(" — {what}: {}", available.join(", "))
    }
}

/// Point `spec.slot` at `spec.preset`, in `config.toml` or in one games.toml
/// profile.
///
/// Order of checks is deliberate and matches the mapper's: everything that can
/// refuse does so **before** anything is copied or written, so a refusal never
/// leaves a stray backup behind.
pub fn assign(store: &Store, spec: &SlotSpec) -> Result<AppliedSlot, SlotError> {
    if spec.slot == 0 || spec.slot > MAX_SLOTS {
        return Err(SlotError::BadSlot { given: spec.slot });
    }
    // The preset has to EXIST. A slot pointing at a preset that is not there is
    // a cabinet that refuses to start, and it would refuse at the next boot —
    // long after the person who typed this walked away.
    let preset = resolve_preset(store, &spec.preset)?;

    match &spec.profile {
        None => assign_in_config(store, spec.slot, &preset),
        Some(profile) => assign_in_profile(store, spec.slot, &preset, profile),
    }
}

/// The preset's name as the FILE spells it, or the refusal listing what is
/// there. Case-insensitive on the way in, canonical on the way out — so
/// `--preset "ipac p1"` writes `IPAC P1` and the config keeps one spelling.
fn resolve_preset(store: &Store, wanted: &str) -> Result<String, SlotError> {
    let loaded = store.load_presets()?;
    let mut available: Vec<String> = loaded.value.iter().map(|p| p.name.clone()).collect();
    match loaded
        .value
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(wanted))
    {
        Some(found) => Ok(found.name.clone()),
        None => {
            available.sort_by_key(|name| name.to_ascii_lowercase());
            Err(SlotError::UnknownPreset {
                preset: wanted.to_owned(),
                available,
            })
        }
    }
}

fn assign_in_config(store: &Store, slot: u8, preset: &str) -> Result<AppliedSlot, SlotError> {
    let mut config = store.load_config()?.value;
    let existing = config.slots.iter_mut().find(|s| s.number == slot);
    let (previous, created) = match existing {
        Some(entry) => {
            if entry.preset.eq_ignore_ascii_case(preset) {
                return Ok(AppliedSlot {
                    path: store.root().config_path(),
                    slot,
                    preset: entry.preset.clone(),
                    previous: Some(entry.preset.clone()),
                    profile: None,
                    created: false,
                    unchanged: true,
                    backup: None,
                });
            }
            let previous = std::mem::replace(&mut entry.preset, preset.to_owned());
            (Some(previous), false)
        }
        None => {
            // A brand-new slot inherits nothing: no keyboard (so it accepts
            // any), the default persona, the default SOCD and macro switch.
            // Wiring a DEVICE to it is `ksx setup`'s job — it identifies the
            // board by press, which is the only honest way to do it and is
            // deliberately not something a preset picker can guess.
            config.slots.push(SlotEntry {
                number: slot,
                keyboard: None,
                mouse: None,
                preset: preset.to_owned(),
                persona: Default::default(),
                socd: Default::default(),
                macros: Default::default(),
            });
            config.slots.sort_by_key(|s| s.number);
            (None, true)
        }
    };

    let path = store.root().config_path();
    let backup = store.backup(&path)?;
    let written = store.save_config(&config)?;
    Ok(AppliedSlot {
        path: written,
        slot,
        preset: preset.to_owned(),
        previous,
        profile: None,
        created,
        unchanged: false,
        backup,
    })
}

fn assign_in_profile(
    store: &Store,
    slot: u8,
    preset: &str,
    profile: &str,
) -> Result<AppliedSlot, SlotError> {
    let mut games = store.load_games()?.value;
    let titles: Vec<String> = games.games.iter().map(|g| g.title.clone()).collect();
    let Some(game) = games
        .games
        .iter_mut()
        .find(|g| g.title.eq_ignore_ascii_case(profile))
    else {
        return Err(SlotError::UnknownProfile {
            profile: profile.to_owned(),
            available: titles,
        });
    };
    let title = game.title.clone();

    let existing = game.slots.iter_mut().find(|s| s.number == slot);
    let (previous, created) = match existing {
        Some(entry) => {
            if entry.preset.eq_ignore_ascii_case(preset) {
                return Ok(AppliedSlot {
                    path: store.root().games_path(),
                    slot,
                    preset: entry.preset.clone(),
                    previous: Some(entry.preset.clone()),
                    profile: Some(title),
                    created: false,
                    unchanged: true,
                    backup: None,
                });
            }
            let previous = std::mem::replace(&mut entry.preset, preset.to_owned());
            (Some(previous), false)
        }
        None => {
            game.slots.push(GameSlotEntry {
                number: slot,
                // Advisory only (the real XInput index comes from ViGEm's
                // callback), and `ksx setup` writes it the same way.
                user_index: Some(slot),
                keyboard: None,
                mouse: None,
                preset: preset.to_owned(),
                persona: Default::default(),
                socd: Default::default(),
                macros: Default::default(),
            });
            game.slots.sort_by_key(|s| s.number);
            (None, true)
        }
    };

    let path = store.root().games_path();
    let backup = store.backup(&path)?;
    let written = store.save_games(&games)?;
    Ok(AppliedSlot {
        path: written,
        slot,
        preset: preset.to_owned(),
        previous,
        profile: Some(title),
        created,
        unchanged: false,
        backup,
    })
}

/// One slot as a picker shows it: the number, the preset it uses, and where
/// that wiring is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRow {
    pub slot: u8,
    pub preset: String,
    /// `None` = config.toml.
    pub profile: Option<String>,
    /// The keyboard alias or id, `"(any)"` when unassigned — the same wording
    /// the mapper's slot strip uses.
    pub keyboard: String,
}

/// Every slot of one destination, in slot order. The read side of this verb,
/// and daemon-free like every other read.
pub fn list(store: &Store, profile: Option<&str>) -> Result<Vec<SlotRow>, SlotError> {
    match profile {
        None => Ok(store
            .load_config()?
            .value
            .slots
            .iter()
            .map(|s| SlotRow {
                slot: s.number,
                preset: s.preset.clone(),
                profile: None,
                keyboard: s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
            })
            .collect()),
        Some(profile) => {
            let games = store.load_games()?.value;
            let titles: Vec<String> = games.games.iter().map(|g| g.title.clone()).collect();
            let Some(game) = games
                .games
                .iter()
                .find(|g| g.title.eq_ignore_ascii_case(profile))
            else {
                return Err(SlotError::UnknownProfile {
                    profile: profile.to_owned(),
                    available: titles,
                });
            };
            Ok(game
                .slots
                .iter()
                .map(|s| SlotRow {
                    slot: s.number,
                    preset: s.preset.clone(),
                    profile: Some(game.title.clone()),
                    keyboard: s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::{ConfigFile, ConfigRoot, GamesFile, PresetFile};

    /// A throwaway config root. Same shape as `mapping.rs`'s, so the two
    /// writers' tests read the same way.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-slots-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// An empty config, an empty games file and two presets — the smallest
        /// cabinet an assignment can be made on.
        fn store(&self) -> Store {
            let store = Store::new(ConfigRoot::at(&self.0));
            store.save_config(&ConfigFile::default()).unwrap();
            store.save_games(&GamesFile::default()).unwrap();
            for name in ["IPAC P1", "IPAC P2"] {
                let file: PresetFile =
                    toml::from_str(&format!("name = \"{name}\"\n[bindings]\nA = \"S\"\n")).unwrap();
                store.save_preset(&file).unwrap();
            }
            store
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn one_profile(store: &Store) {
        let file: GamesFile =
            toml::from_str("[[game]]\ntitle = \"Steam\"\npath = 'C:\\steam.exe'\n").unwrap();
        store.save_games(&file).unwrap();
    }

    fn spec(slot: u8, preset: &str) -> SlotSpec {
        SlotSpec {
            slot,
            preset: preset.to_owned(),
            profile: None,
        }
    }

    /// The happy path in config.toml — and the fact the message leads with:
    /// the pads replug.
    #[test]
    fn assigning_a_slot_writes_config_toml_and_says_the_pads_bounce() {
        let root = TempRoot::new("assign");
        let store = root.store();

        let applied = assign(&store, &spec(1, "IPAC P1")).expect("a preset that exists");
        assert!(applied.created, "slot 1 did not exist yet");
        assert_eq!(applied.previous, None);
        assert!(
            applied.message().contains("pads replugged"),
            "the bounce must never be something a user has to infer: {}",
            applied.message()
        );

        let config = store.load_config().unwrap().value;
        assert_eq!(config.slots.len(), 1);
        assert_eq!(config.slots[0].preset, "IPAC P1");

        // Repointing reports BOTH halves — what it was and what it is.
        let moved = assign(&store, &spec(1, "IPAC P2")).unwrap();
        assert_eq!(moved.previous.as_deref(), Some("IPAC P1"));
        assert!(!moved.created);
        assert!(moved.backup.is_some(), "a whole-file write backs up first");
        assert!(moved.message().contains("\"IPAC P1\" → \"IPAC P2\""));
    }

    /// Re-asserting the state a slot is already in is success and a NO-OP —
    /// nothing written, no backup, and the message says so rather than letting
    /// someone wait for a pad bounce that is never coming.
    #[test]
    fn assigning_the_preset_a_slot_already_uses_writes_nothing() {
        let root = TempRoot::new("noop");
        let store = root.store();
        assign(&store, &spec(1, "IPAC P1")).unwrap();
        let again = assign(&store, &spec(1, "IPAC P1")).unwrap();
        assert!(again.unchanged);
        assert!(again.backup.is_none(), "nothing was overwritten");
        assert!(again.message().contains("nothing was written"));
    }

    /// A preset name is matched case-insensitively and stored in the FILE's
    /// spelling, so a config never grows two spellings of one preset.
    #[test]
    fn the_preset_is_written_in_the_spelling_the_file_uses() {
        let root = TempRoot::new("spelling");
        let store = root.store();
        let applied = assign(&store, &spec(2, "ipac p2")).unwrap();
        assert_eq!(applied.preset, "IPAC P2");
    }

    /// A slot pointing at a preset that is not there is a cabinet that refuses
    /// to start — at the next boot, long after whoever typed this walked away.
    /// So it is refused now, with the list.
    #[test]
    fn a_preset_that_does_not_exist_is_refused_with_the_ones_that_do() {
        let root = TempRoot::new("unknown-preset");
        let store = root.store();
        let err = assign(&store, &spec(1, "Nope")).unwrap_err();
        assert_eq!(err.code(), ksx_api::codes::UNKNOWN_PRESET);
        let message = err.to_string();
        assert!(
            message.contains("IPAC P1") && message.contains("IPAC P2"),
            "{message}"
        );
        assert!(
            store.load_config().unwrap().value.slots.is_empty(),
            "a refusal writes nothing"
        );
    }

    #[test]
    fn a_slot_number_off_the_end_is_refused_before_anything_is_read() {
        let root = TempRoot::new("bad-slot");
        let store = root.store();
        for slot in [0, MAX_SLOTS + 1] {
            let err = assign(&store, &spec(slot, "IPAC P1")).unwrap_err();
            assert_eq!(err.code(), ksx_api::codes::BAD_SLOT);
        }
    }

    /// The games.toml half: the same write, in one profile, and it never
    /// touches config.toml.
    #[test]
    fn assigning_inside_a_profile_writes_only_that_profile() {
        let root = TempRoot::new("profile");
        let store = root.store();
        one_profile(&store);

        let applied = assign(
            &store,
            &SlotSpec {
                slot: 3,
                preset: "IPAC P1".into(),
                // Title matching is case-insensitive; the FILE's spelling wins.
                profile: Some("steam".into()),
            },
        )
        .unwrap();
        assert_eq!(applied.profile.as_deref(), Some("Steam"));
        assert!(applied.created);
        assert!(applied.message().contains("profile \"Steam\""));

        let games = store.load_games().unwrap().value;
        assert_eq!(games.games[0].slots.len(), 1);
        assert_eq!(games.games[0].slots[0].preset, "IPAC P1");
        assert!(
            store.load_config().unwrap().value.slots.is_empty(),
            "a profile write must not touch config.toml"
        );
    }

    #[test]
    fn an_unknown_profile_is_refused_with_the_titles_that_exist() {
        let root = TempRoot::new("unknown-profile");
        let store = root.store();
        one_profile(&store);
        let err = assign(
            &store,
            &SlotSpec {
                slot: 1,
                preset: "IPAC P1".into(),
                profile: Some("MAME".into()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), ksx_api::codes::UNKNOWN_PROFILE);
        assert!(err.to_string().contains("Steam"), "{err}");
    }

    /// The read side a picker uses: slot order, whatever order they were
    /// written in.
    #[test]
    fn list_reports_the_slots_of_one_destination_in_order() {
        let root = TempRoot::new("list");
        let store = root.store();
        assign(&store, &spec(2, "IPAC P2")).unwrap();
        assign(&store, &spec(1, "IPAC P1")).unwrap();
        let rows = list(&store, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].slot, 1);
        assert_eq!(rows[0].preset, "IPAC P1");
        assert_eq!(rows[0].keyboard, "(any)");
        assert_eq!(rows[1].slot, 2);
    }
}
