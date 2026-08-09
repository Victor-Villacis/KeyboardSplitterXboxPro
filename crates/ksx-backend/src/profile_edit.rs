//! Creating a `[[game]]` profile — the write half of games.toml.
//!
//! # Why this module exists at all
//!
//! Every other thing a person can do to their configuration had a verb.
//! Creating a PROFILE did not, and the gap was not obvious from any one
//! surface: `ksx setup` writes into a profile and bails with `no games.toml
//! profile called "…"` when it is absent, `ksx config import` replaces the
//! whole file, `ksx slot assign` edits slots inside a profile that already
//! exists. So the only supported way to get a first profile was to write TOML
//! by hand, and the reported symptom was exactly that — *"I can't create a new
//! profile"*.
//!
//! # One writer, like every other write
//!
//! Same shape as [`crate::device_edit`] and [`crate::slots`]: a typed spec in,
//! a pure plan out, a timestamped backup taken before the write, and the
//! store's atomic save doing the I/O. [`plan_new`] takes the LOADED
//! `GamesFile` and the preset names rather than reaching for a store, so every
//! refusal below is exercised in CI on any platform, with no config root and
//! no disk.
//!
//! # Slots are seeded, and that is not a convenience
//!
//! A `[[game]]` with no `[[game.slot]]` entries hands out no pads. `ksx run
//! --game` refuses it, and the profile list will show it as usable. "Create"
//! that produced an empty shell would answer "I can't make a profile" with a
//! profile that cannot be played — so [`NewProfile::slots`] is required and
//! every seeded slot names a preset that is on disk (checked here, not at
//! launch).
//!
//! # What is deliberately NOT set: the device
//!
//! `GameSlotEntry::keyboard` stays `None` — "(any keyboard)". Wiring a
//! specific board to a slot needs the device tree in front of you, which is
//! `ksx setup`'s job and the device picker's; a create form guessing it would
//! write a device id nobody chose. `None` is a working default (every
//! keyboard drives the slot), not a placeholder.

use std::path::PathBuf;

use ksx_config::{ConfigError, GameEntry, GameSlotEntry, GamesFile, Store};
use ksx_core::MAX_SLOTS;

/// One "create a profile", as any surface spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProfileSpec {
    pub title: String,
    /// A full path to the program, or a launcher URL (`steam://…`).
    pub path: String,
    pub arguments: String,
    /// `[[game.slot]]` entries to seed, `1..=MAX_SLOTS`.
    pub slots: u8,
    /// The preset each seeded slot starts on.
    pub preset: String,
}

/// Everything `new` decided, before a byte is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProfilePlan {
    /// The entry that will be appended, whole.
    pub entry: GameEntry,
}

/// Why a profile could not be created. Every one but `Config` is decided
/// before any I/O.
///
/// No `code()` here, unlike [`crate::device_edit::PickError`] and
/// [`crate::preset_edit::PresetError`]: those codes are the `--json` refusal
/// words of a CLI verb, and this verb has no CLI yet. Inventing the
/// vocabulary before the surface that speaks it is how a code ends up meaning
/// two things. The day `ksx games new` lands, it brings its own.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("a profile needs a title — it is the name `ksx run --game` takes")]
    EmptyTitle,
    #[error(
        "profile \"{title}\" needs a `path`: the game's .exe, or a launcher URL like \
         steam://rungameid/620"
    )]
    EmptyPath { title: String },
    #[error("a profile called \"{title}\" already exists in games.toml")]
    Duplicate { title: String },
    #[error("no preset called \"{name}\" is on disk, so a slot cannot start on it")]
    NoSuchPreset { name: String },
    #[error("a profile hands out 1..={MAX_SLOTS} slots; asked for {asked}")]
    BadSlots { asked: u8 },
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl ProfileError {
    /// What to do about it. A refusal with no way forward is just an error
    /// message.
    pub fn advice(&self) -> String {
        match self {
            Self::EmptyTitle => {
                "the title is how the profile is named everywhere else — `ksx run --game \
                 \"<TITLE>\"`, the tray menu, the Start buttons on Studio's status page."
                    .to_owned()
            }
            Self::EmptyPath { .. } => {
                "ksx launches this and stops emulation when it exits. A path that does not \
                 exist yet is allowed — the profile list marks it broken and prints it back \
                 — but an empty one names nothing at all."
                    .to_owned()
            }
            Self::Duplicate { title } => format!(
                "titles are how profiles are named, so two cannot share one. Choose another, \
                 or edit the existing profile's slots:\n  ksx slot assign 1 --preset \
                 \"<PRESET>\" --profile \"{title}\""
            ),
            Self::NoSuchPreset { .. } => {
                "`ksx preset list` names the ones on disk; `ksx preset new \"<NAME>\" \
                 --from-template <ID>` makes one from an in-box layout."
                    .to_owned()
            }
            Self::BadSlots { .. } => {
                "one slot per player. They can be added or removed afterwards with `ksx slot \
                 assign` without recreating the profile."
                    .to_owned()
            }
            Self::Config(_) => String::new(),
        }
    }
}

/// Decide the whole entry. Pure: no store, no disk, no platform.
///
/// `presets` is the list of preset names on disk. It is checked HERE rather
/// than at launch for the same reason the profile list preflights its paths:
/// a slot naming a preset that is not there starts a session that refuses,
/// long after the moment anyone could connect the two facts.
pub fn plan_new(
    games: &GamesFile,
    presets: &[String],
    spec: &NewProfileSpec,
) -> Result<NewProfilePlan, ProfileError> {
    let title = spec.title.trim();
    if title.is_empty() {
        return Err(ProfileError::EmptyTitle);
    }
    let path = spec.path.trim();
    if path.is_empty() {
        return Err(ProfileError::EmptyPath {
            title: title.to_owned(),
        });
    }
    // Case-insensitively, because two profiles differing only in case are two
    // rows a human reads as one, and `--game` matching is not the place to
    // discover that.
    if games
        .games
        .iter()
        .any(|g| g.title.trim().eq_ignore_ascii_case(title))
    {
        return Err(ProfileError::Duplicate {
            title: title.to_owned(),
        });
    }
    if spec.slots == 0 || spec.slots > MAX_SLOTS {
        return Err(ProfileError::BadSlots { asked: spec.slots });
    }
    let preset = spec.preset.trim();
    if !presets.iter().any(|p| p.eq_ignore_ascii_case(preset)) {
        return Err(ProfileError::NoSuchPreset {
            name: preset.to_owned(),
        });
    }
    // Take the preset's spelling from DISK, not from the form: the file name
    // is what `ksx run` resolves, and writing "arcade" where the file is
    // "Arcade" would work on Windows and break the day the config is read on
    // anything case-sensitive.
    let preset = presets
        .iter()
        .find(|p| p.eq_ignore_ascii_case(preset))
        .cloned()
        .unwrap_or_else(|| preset.to_owned());

    let slots = (1..=spec.slots)
        .map(|number| GameSlotEntry {
            number,
            user_index: None,
            keyboard: None,
            mouse: None,
            preset: preset.clone(),
            persona: Default::default(),
            socd: Default::default(),
            macros: Default::default(),
        })
        .collect();

    Ok(NewProfilePlan {
        entry: GameEntry {
            title: title.to_owned(),
            notes: String::new(),
            path: path.to_owned(),
            arguments: spec.arguments.trim().to_owned(),
            process_name: None,
            launcher_grace_ms: None,
            block_keyboards: Default::default(),
            block_mice: false,
            slots,
        },
    })
}

/// What landed on disk.
#[derive(Clone, Debug)]
pub struct NewProfileOutcome {
    /// The timestamped copy taken before the write; `None` when there was no
    /// games.toml yet. Carried rather than merely taken, because "a backup
    /// exists" is the sentence that makes a write to a shared file survivable
    /// and the caller is the one that reports it.
    pub backup: Option<PathBuf>,
    pub plan: NewProfilePlan,
}

/// Write the entry `plan` describes.
///
/// Re-reads games.toml rather than trusting the copy the plan was made
/// against: between the read that drew the page and this write, `ksx setup` or
/// a hand edit may have added a profile, and appending to a stale in-memory
/// file would silently delete it.
pub fn apply_new(store: &Store, plan: &NewProfilePlan) -> Result<NewProfileOutcome, ProfileError> {
    let mut games = store.load_games()?.value;
    if games
        .games
        .iter()
        .any(|g| g.title.trim().eq_ignore_ascii_case(plan.entry.title.trim()))
    {
        return Err(ProfileError::Duplicate {
            title: plan.entry.title.clone(),
        });
    }
    games.games.push(plan.entry.clone());
    let backup = store.backup(&store.root().games_path())?;
    store.save_games(&games)?;
    Ok(NewProfileOutcome {
        backup,
        plan: plan.clone(),
    })
}

impl NewProfileOutcome {
    /// The one-line report a surface flashes.
    ///
    /// Deliberately one line and under the 300-character flash cap
    /// (`ksx-studio` truncates): the STRUCTURE — every slot, its preset, the
    /// preflight verdict on the path — is what the profile list renders the
    /// moment this redirects back to it, so repeating it here would only be a
    /// worse copy of the page underneath.
    pub fn message(&self) -> String {
        let plan = &self.plan;
        format!(
            "created profile \"{}\" — {} slot(s) on preset \"{}\" → {}{}",
            plan.entry.title,
            plan.entry.slots.len(),
            plan.entry
                .slots
                .first()
                .map_or("(none)", |s| s.preset.as_str()),
            plan.entry.path,
            // The clause, not the path: a backup path is 60+ characters and
            // the flash has 300 to spend on the whole sentence. What matters
            // at this moment is that games.toml was copied first.
            if self.backup.is_some() {
                " (games.toml backed up first)"
            } else {
                ""
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(title: &str, path: &str) -> NewProfileSpec {
        NewProfileSpec {
            title: title.to_owned(),
            path: path.to_owned(),
            arguments: String::new(),
            slots: 2,
            preset: "Arcade".to_owned(),
        }
    }

    fn presets() -> Vec<String> {
        vec!["Arcade".to_owned(), "default".to_owned()]
    }

    fn existing(title: &str) -> GamesFile {
        GamesFile {
            games: vec![GameEntry {
                title: title.to_owned(),
                notes: String::new(),
                path: "C:\\games\\x.exe".to_owned(),
                arguments: String::new(),
                process_name: None,
                launcher_grace_ms: None,
                block_keyboards: Default::default(),
                block_mice: false,
                slots: Vec::new(),
            }],
        }
    }

    #[test]
    fn a_plan_seeds_one_slot_per_player_all_on_the_named_preset() {
        let plan = plan_new(
            &GamesFile::default(),
            &presets(),
            &spec("Street Fighter", "C:\\games\\sf.exe"),
        )
        .unwrap();
        assert_eq!(plan.entry.slots.len(), 2);
        assert_eq!(plan.entry.slots[0].number, 1);
        assert_eq!(plan.entry.slots[1].number, 2);
        for slot in &plan.entry.slots {
            assert_eq!(slot.preset, "Arcade");
            // The device is deliberately unset — see the module docs.
            assert_eq!(slot.keyboard, None);
        }
    }

    #[test]
    fn the_preset_spelling_comes_from_disk_not_from_the_form() {
        let mut asked = spec("SF", "C:\\x.exe");
        asked.preset = "ARCADE".to_owned();
        let plan = plan_new(&GamesFile::default(), &presets(), &asked).unwrap();
        assert_eq!(plan.entry.slots[0].preset, "Arcade");
    }

    /// Two rows a human reads as one name must not both exist.
    #[test]
    fn a_duplicate_title_is_refused_case_insensitively() {
        let err = plan_new(
            &existing("Street Fighter"),
            &presets(),
            &spec("street fighter", "C:\\x.exe"),
        )
        .unwrap_err();
        assert!(matches!(err, ProfileError::Duplicate { .. }), "{err}");
    }

    #[test]
    fn a_slot_cannot_start_on_a_preset_that_is_not_there() {
        let mut asked = spec("SF", "C:\\x.exe");
        asked.preset = "Nope".to_owned();
        let err = plan_new(&GamesFile::default(), &presets(), &asked).unwrap_err();
        assert!(matches!(err, ProfileError::NoSuchPreset { .. }), "{err}");
        assert!(err.advice().contains("ksx preset new"), "{}", err.advice());
    }

    #[test]
    fn an_empty_title_or_path_is_refused_with_the_reason() {
        let err =
            plan_new(&GamesFile::default(), &presets(), &spec("  ", "C:\\x.exe")).unwrap_err();
        assert!(matches!(err, ProfileError::EmptyTitle), "{err}");
        let err = plan_new(&GamesFile::default(), &presets(), &spec("SF", "  ")).unwrap_err();
        assert!(matches!(err, ProfileError::EmptyPath { .. }), "{err}");
    }

    #[test]
    fn zero_slots_and_more_than_max_slots_are_both_refused() {
        let mut asked = spec("SF", "C:\\x.exe");
        asked.slots = 0;
        let err = plan_new(&GamesFile::default(), &presets(), &asked).unwrap_err();
        assert!(matches!(err, ProfileError::BadSlots { asked: 0 }), "{err}");
        asked.slots = MAX_SLOTS.saturating_add(1);
        let err = plan_new(&GamesFile::default(), &presets(), &asked).unwrap_err();
        assert!(matches!(err, ProfileError::BadSlots { .. }), "{err}");
        // The refusal names the ceiling, so a 16-slot cabinet's owner is not
        // left guessing what the limit is.
        assert!(err.to_string().contains(&MAX_SLOTS.to_string()), "{err}");
    }

    /// A path that is not there yet is NOT a refusal: the profile list
    /// preflights and marks it broken, which is a fixable row rather than a
    /// create button that will not click.
    #[test]
    fn a_path_that_does_not_exist_is_written_anyway() {
        let plan = plan_new(
            &GamesFile::default(),
            &presets(),
            &spec("MAME 4P", "C:\\gone\\mame.exe"),
        )
        .unwrap();
        assert_eq!(plan.entry.path, "C:\\gone\\mame.exe");
    }
}
