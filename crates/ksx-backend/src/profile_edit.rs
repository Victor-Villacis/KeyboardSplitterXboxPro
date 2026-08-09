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
//! `ConfigFile`, `GamesFile`, and preset names rather than reaching for a
//! store, so every refusal below is exercised in CI on any platform, with no
//! config root and no disk.
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
//! # A profile starts from the setup that already works
//!
//! Each seeded game slot inherits the matching base `[[slot]]`'s keyboard and
//! mouse selectors. `None` is not "any keyboard" in the run planner: a slot
//! with neither selector is skipped as `NoInputDeviceSelected`, and a profile
//! made entirely of those slots cannot play. The persona, SOCD mode, macro
//! switch, and config-wide blocking choices come along too; those are parts of
//! the working controller setup and the create form does not ask replacements.
//! The preset is the exception: the form explicitly chooses it, so that answer
//! is applied to every seeded slot.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use ksx_config::{ConfigError, ConfigFile, GameEntry, GameSlotEntry, GamesFile, Store};
use ksx_core::MAX_SLOTS;

/// Profile writes are read-modify-write operations on one shared games file.
/// Studio may dispatch more than one blocking machine call at once, so the
/// read, stale check, backup, and atomic replacement must be one process-local
/// critical section rather than four individually safe operations.
static PROFILE_WRITER: Mutex<()> = Mutex::new(());

fn profile_writer() -> MutexGuard<'static, ()> {
    PROFILE_WRITER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One "create a profile", as any surface spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProfileSpec {
    pub title: String,
    /// A full path to the program, or a launcher URL (`steam://…`). A single
    /// matching pair of surrounding quotes is accepted for paths pasted from a
    /// shell or Explorer and removed before the profile is stored.
    pub path: String,
    pub arguments: String,
    /// `[[game.slot]]` entries to seed, `1..=MAX_SLOTS`.
    pub slots: u8,
    /// The preset each seeded slot starts on.
    pub preset: String,
}

/// One edit of an existing profile. `original_title` remains the lookup key
/// when `title` asks for a rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateProfileSpec {
    pub original_title: String,
    /// Opaque revision returned by [`profile_revision`] when this edit form
    /// was drawn. It is deliberately not a timestamp or a file offset: the
    /// complete entry is the concurrency boundary.
    pub revision: String,
    pub title: String,
    pub path: String,
    pub arguments: String,
    pub slots: u8,
    pub preset: String,
    /// Keep existing game-slot selectors when false. When true, replace every
    /// resulting slot's keyboard/mouse selectors from the matching base slot.
    pub rebase_devices: bool,
}

/// One exact profile removal. It never names presets or controller setup,
/// because those are independent resources and deletion must not imply them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteProfileSpec {
    pub title: String,
    /// Opaque revision returned by [`profile_revision`] for the row.
    pub revision: String,
}

/// Everything `new` decided, before a byte is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProfilePlan {
    /// The entry that will be appended, whole.
    pub entry: GameEntry,
}

/// A complete profile replacement, including the value seen while planning so
/// the writer can refuse a concurrent edit instead of overwriting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateProfilePlan {
    pub original: GameEntry,
    pub replacement: GameEntry,
}

/// The exact profile a delete intends to remove. Carrying the whole entry is a
/// concurrency guard, not a promise to remove anything it references.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteProfilePlan {
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
    #[error("a saved game needs a game name")]
    EmptyTitle,
    #[error(
        "saved game \"{title}\" needs a program: the game's .exe, or a launcher URL like \
         steam://rungameid/620"
    )]
    EmptyPath { title: String },
    #[error(
        "saved game \"{title}\" has malformed surrounding quotes around its program: use one \
         matching pair, or no quotes"
    )]
    MalformedPathQuotes { title: String },
    #[error("a saved game called \"{title}\" already exists")]
    Duplicate { title: String },
    #[error("no controller layout called \"{name}\" is available")]
    NoSuchPreset { name: String },
    #[error("a saved game supports 1..={MAX_SLOTS} players; asked for {asked}")]
    BadSlots { asked: u8 },
    #[error(
        "saved game \"{title}\" needs player {number}, but that player's controller is not set up yet"
    )]
    MissingBaseSlot { title: String, number: u8 },
    #[error(
        "saved game \"{title}\" cannot use player {number}: that player has no keyboard or mouse selected"
    )]
    UnwiredBaseSlot { title: String, number: u8 },
    #[error("choose the saved game to change or delete")]
    EmptyTarget,
    #[error("no saved game called \"{title}\" exists")]
    UnknownProfile { title: String },
    #[error(
        "more than one saved game matches \"{title}\" ({matches} matches), so ksx cannot safely choose one"
    )]
    AmbiguousProfile { title: String, matches: usize },
    #[error(
        "saved game \"{title}\" has more than one player numbered {number}, so ksx cannot safely choose one"
    )]
    AmbiguousSlot { title: String, number: u8 },
    #[error(
        "saved game \"{title}\" cannot preserve player {number}: that player has no keyboard or mouse selected"
    )]
    UnwiredProfileSlot { title: String, number: u8 },
    #[error(
        "saved game \"{title}\" cannot use player {number}: device \"{device}\" is no longer in Controller Setup"
    )]
    UnknownDevice {
        title: String,
        number: u8,
        device: String,
    },
    #[error(
        "saved game \"{title}\" cannot be saved because its program is not available on this computer"
    )]
    ProgramUnavailable { title: String },
    #[error(
        "saved game \"{title}\" changed while this edit was being prepared; nothing was written"
    )]
    Changed { title: String },
    #[error("Saved Games could not be read or written")]
    Config(#[from] ConfigError),
}

impl ProfileError {
    /// What to do about it. A refusal with no way forward is just an error
    /// message.
    pub fn advice(&self) -> String {
        match self {
            Self::EmptyTitle => {
                "give the saved game a short, distinct game name you will recognize in the list"
                    .to_owned()
            }
            Self::EmptyPath { .. } => {
                "choose the game's executable or enter its launcher URL; an empty value cannot \
                 be started"
                    .to_owned()
            }
            Self::MalformedPathQuotes { .. } => {
                "paste the program location without quotes, or wrap the whole value in one matching pair \
                 of single quotes or double quotes."
                    .to_owned()
            }
            Self::Duplicate { title } => {
                format!("choose another game name, or update the existing saved game \"{title}\"")
            }
            Self::NoSuchPreset { .. } => {
                "choose an existing controller layout, or create one before saving this game"
                    .to_owned()
            }
            Self::BadSlots { .. } => {
                format!("choose one controller per player, from 1 through {MAX_SLOTS}")
            }
            Self::MissingBaseSlot { number, .. } | Self::UnwiredBaseSlot { number, .. } => {
                format!("finish player {number} in Controller Setup, save it, then try again")
            }
            Self::EmptyTarget | Self::UnknownProfile { .. } => {
                "refresh Saved Games and choose the saved game again".to_owned()
            }
            Self::AmbiguousProfile { .. } | Self::AmbiguousSlot { .. } => {
                "Saved Games needs its duplicate game names or player numbers resolved \
                 before this operation can safely continue"
                    .to_owned()
            }
            Self::UnwiredProfileSlot { number, .. } => format!(
                "choose to use the current Controller Setup for devices, or finish player \
                 {number} before saving"
            ),
            Self::UnknownDevice { number, .. } => format!(
                "finish player {number} in Controller Setup, or choose to use the current device choices before saving"
            ),
            Self::ProgramUnavailable { .. } => {
                "choose a program that is on this computer, or enter a launcher link such as steam://rungameid/620"
                    .to_owned()
            }
            Self::Changed { .. } => "refresh Saved Games and try the edit again".to_owned(),
            Self::Config(_) => {
                "refresh Saved Games and try again; if it still fails, make sure ksx can access its saved data"
                    .to_owned()
            }
        }
    }
}

/// Trim form whitespace and remove one unambiguous shell-style quote pair.
///
/// A quote at only one edge, or different quote characters at the two edges,
/// is not guessed: stripping either side could turn a typo into a different
/// executable than the user selected.
fn normalize_pasted_path(path: &str) -> Option<&str> {
    let path = path.trim();
    let starts_quoted = path.starts_with('\'') || path.starts_with('"');
    let ends_quoted = path.ends_with('\'') || path.ends_with('"');

    match (starts_quoted, ends_quoted) {
        (false, false) => Some(path),
        (true, true) if path.len() >= 2 && path.as_bytes().first() == path.as_bytes().last() => {
            let inner = path[1..path.len() - 1].trim();
            // Strip exactly one shell/Explorer pair. A second pair is not a
            // second convenience wrapper; it would be persisted as literal
            // quote characters and can never name the selected program.
            if inner.starts_with(['\'', '"']) || inner.ends_with(['\'', '"']) {
                None
            } else {
                Some(inner)
            }
        }
        _ => None,
    }
}

/// Opaque identity-and-content revision for a Saved Games row.
///
/// The whole serialized entry participates, including fields the current form
/// does not edit. That is intentional: a second tool changing notes, process
/// tracking, blocking, or one player's detailed setup must make an older form
/// stale instead of being silently overwritten. FNV-1a 128 keeps this helper
/// dependency-free and deterministic; the value is a change detector, not an
/// authentication token.
pub fn profile_revision(entry: &GameEntry) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let bytes = serde_json::to_vec(entry).unwrap_or_else(|_| format!("{entry:?}").into_bytes());
    let hash = bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u128::from(*byte)).wrapping_mul(PRIME)
    });
    format!("g1-{hash:032x}")
}

fn require_revision(entry: &GameEntry, supplied: &str) -> Result<(), ProfileError> {
    if supplied.trim().is_empty() || supplied != profile_revision(entry) {
        return Err(ProfileError::Changed {
            title: entry.title.clone(),
        });
    }
    Ok(())
}

fn device_error(title: &str, number: u8, err: ConfigError) -> ProfileError {
    match err {
        ConfigError::UnknownDeviceAlias(device) => ProfileError::UnknownDevice {
            title: title.to_owned(),
            number,
            device,
        },
        other => ProfileError::Config(other),
    }
}

/// The shared form validation for create and update. Returning the canonical
/// preset spelling here keeps both writers byte-for-byte aligned.
fn normalized_fields<'a>(
    title: &'a str,
    path: &'a str,
    slots: u8,
    preset: &str,
    presets: &[String],
) -> Result<(&'a str, &'a str, String), ProfileError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ProfileError::EmptyTitle);
    }
    let path = normalize_pasted_path(path).ok_or_else(|| ProfileError::MalformedPathQuotes {
        title: title.to_owned(),
    })?;
    if path.is_empty() {
        return Err(ProfileError::EmptyPath {
            title: title.to_owned(),
        });
    }
    if slots == 0 || slots > MAX_SLOTS {
        return Err(ProfileError::BadSlots { asked: slots });
    }
    let preset = preset.trim();
    let Some(preset) = presets
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(preset))
    else {
        return Err(ProfileError::NoSuchPreset {
            name: preset.to_owned(),
        });
    };
    Ok((title, path, preset.clone()))
}

/// Find one human profile identity. Case-insensitive matching is the same rule
/// create uses for collisions; more than one match is refused rather than
/// letting update/delete silently pick the first legacy duplicate.
fn profile_index(games: &GamesFile, title: &str) -> Result<usize, ProfileError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ProfileError::EmptyTarget);
    }
    let matches: Vec<usize> = games
        .games
        .iter()
        .enumerate()
        .filter_map(|(index, game)| {
            game.title
                .trim()
                .eq_ignore_ascii_case(title)
                .then_some(index)
        })
        .collect();
    match matches.as_slice() {
        [] => Err(ProfileError::UnknownProfile {
            title: title.to_owned(),
        }),
        [index] => Ok(*index),
        _ => Err(ProfileError::AmbiguousProfile {
            title: title.to_owned(),
            matches: matches.len(),
        }),
    }
}

fn check_title_available(
    games: &GamesFile,
    title: &str,
    except: Option<usize>,
) -> Result<(), ProfileError> {
    if games
        .games
        .iter()
        .enumerate()
        .any(|(index, game)| Some(index) != except && game.title.trim().eq_ignore_ascii_case(title))
    {
        return Err(ProfileError::Duplicate {
            title: title.to_owned(),
        });
    }
    Ok(())
}

/// Clone one saved base controller into game-profile form. This is the only
/// path that introduces a slot during create/update, so it is also the single
/// guard against writing the device-less rows the run planner discards.
fn slot_from_base(
    base: &ConfigFile,
    title: &str,
    number: u8,
    preset: &str,
) -> Result<GameSlotEntry, ProfileError> {
    let source = base
        .slots
        .iter()
        .find(|slot| slot.number == number)
        .ok_or_else(|| ProfileError::MissingBaseSlot {
            title: title.to_owned(),
            number,
        })?;
    if source.keyboard.is_none() && source.mouse.is_none() {
        return Err(ProfileError::UnwiredBaseSlot {
            title: title.to_owned(),
            number,
        });
    }
    // Resolve aliases through the same ConfigFile method the run planner uses.
    // A non-empty string is not enough: an alias removed from Controller Setup
    // would otherwise create a saved game that immediately refuses Play.
    base.slot_spec(source)
        .map_err(|err| device_error(title, number, err))?;
    Ok(GameSlotEntry {
        number,
        user_index: None,
        keyboard: source.keyboard.clone(),
        mouse: source.mouse.clone(),
        preset: preset.to_owned(),
        persona: source.persona,
        socd: source.socd,
        macros: source.macros,
    })
}

fn profile_slot(profile: &GameEntry, number: u8) -> Result<Option<&GameSlotEntry>, ProfileError> {
    let mut matches = profile.slots.iter().filter(|slot| slot.number == number);
    let first = matches.next();
    if first.is_some() && matches.next().is_some() {
        return Err(ProfileError::AmbiguousSlot {
            title: profile.title.clone(),
            number,
        });
    }
    Ok(first)
}

/// Decide the whole entry. Pure: no store, no disk, no platform.
///
/// `presets` is the list of preset names on disk. It is checked HERE rather
/// than at launch for the same reason the profile list preflights its paths:
/// a slot naming a preset that is not there starts a session that refuses,
/// long after the moment anyone could connect the two facts.
pub fn plan_new(
    base: &ConfigFile,
    games: &GamesFile,
    presets: &[String],
    spec: &NewProfileSpec,
) -> Result<NewProfilePlan, ProfileError> {
    let (title, path, preset) =
        normalized_fields(&spec.title, &spec.path, spec.slots, &spec.preset, presets)?;
    // Case-insensitively, because two profiles differing only in case are two
    // rows a human reads as one, and `--game` matching is not the place to
    // discover that.
    check_title_available(games, title, None)?;

    let slots = (1..=spec.slots)
        .map(|number| slot_from_base(base, title, number, &preset))
        .collect::<Result<Vec<_>, ProfileError>>()?;

    Ok(NewProfilePlan {
        entry: GameEntry {
            title: title.to_owned(),
            notes: String::new(),
            path: path.to_owned(),
            arguments: spec.arguments.trim().to_owned(),
            process_name: None,
            launcher_grace_ms: None,
            block_keyboards: base.settings.block_keyboards,
            block_mice: base.settings.block_mice,
            slots,
        },
    })
}

/// Decide a complete replacement without touching the store.
///
/// Existing slots preserve their device selectors and per-game persona/SOCD/
/// macro settings. Newly added slot numbers come from the matching base slot.
/// `rebase_devices` replaces keyboard/mouse selectors for every resulting slot
/// from that base, but deliberately leaves existing per-game settings alone.
/// In both modes the explicitly selected preset applies to every result.
pub fn plan_update(
    base: &ConfigFile,
    games: &GamesFile,
    presets: &[String],
    spec: &UpdateProfileSpec,
) -> Result<UpdateProfilePlan, ProfileError> {
    let index = profile_index(games, &spec.original_title)?;
    let original = &games.games[index];
    require_revision(original, &spec.revision)?;
    let (title, path, preset) =
        normalized_fields(&spec.title, &spec.path, spec.slots, &spec.preset, presets)?;
    check_title_available(games, title, Some(index))?;

    let slots = (1..=spec.slots)
        .map(|number| {
            let existing = profile_slot(original, number)?;
            let mut slot = match existing {
                Some(slot) => slot.clone(),
                None => slot_from_base(base, title, number, &preset)?,
            };
            if spec.rebase_devices {
                let rebased = slot_from_base(base, title, number, &preset)?;
                slot.keyboard = rebased.keyboard;
                slot.mouse = rebased.mouse;
            } else if slot.keyboard.is_none() && slot.mouse.is_none() {
                return Err(ProfileError::UnwiredProfileSlot {
                    title: original.title.clone(),
                    number,
                });
            }
            slot.number = number;
            slot.preset = preset.clone();
            base.game_slot_spec(&slot)
                .map_err(|err| device_error(title, number, err))?;
            Ok(slot)
        })
        .collect::<Result<Vec<_>, ProfileError>>()?;

    let mut replacement = original.clone();
    replacement.title = title.to_owned();
    replacement.path = path.to_owned();
    replacement.arguments = spec.arguments.trim().to_owned();
    replacement.slots = slots;
    Ok(UpdateProfilePlan {
        original: original.clone(),
        replacement,
    })
}

/// Identify exactly one profile for removal. Pure and deliberately carries the
/// whole entry so [`apply_delete`] can detect a concurrent change.
pub fn plan_delete(
    games: &GamesFile,
    spec: &DeleteProfileSpec,
) -> Result<DeleteProfilePlan, ProfileError> {
    let index = profile_index(games, &spec.title)?;
    require_revision(&games.games[index], &spec.revision)?;
    Ok(DeleteProfilePlan {
        entry: games.games[index].clone(),
    })
}

fn validate_program(entry: &GameEntry) -> Result<(), ProfileError> {
    ksx_games::preflight(&ksx_games::LaunchSpec::from_entry(entry)).map_err(|_| {
        ProfileError::ProgramUnavailable {
            title: entry.title.clone(),
        }
    })
}

fn validate_layouts(store: &Store, entry: &GameEntry) -> Result<(), ProfileError> {
    let mut checked = Vec::<&str>::new();
    for slot in &entry.slots {
        if checked
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&slot.preset))
        {
            continue;
        }
        checked.push(&slot.preset);
        let loaded = store.load_preset(&slot.preset)?;
        let usable = loaded
            .as_ref()
            .is_some_and(|loaded| loaded.value.to_core().is_ok());
        if !usable {
            return Err(ProfileError::NoSuchPreset {
                name: slot.preset.clone(),
            });
        }
    }
    Ok(())
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

#[derive(Clone, Debug)]
pub struct UpdateProfileOutcome {
    pub backup: Option<PathBuf>,
    pub plan: UpdateProfilePlan,
    /// True when the requested replacement was already on disk. No backup or
    /// write is made in this case.
    pub unchanged: bool,
}

#[derive(Clone, Debug)]
pub struct DeleteProfileOutcome {
    pub backup: Option<PathBuf>,
    pub plan: DeleteProfilePlan,
}

/// Write the entry `plan` describes.
///
/// Re-reads games.toml rather than trusting the copy the plan was made
/// against: between the read that drew the page and this write, `ksx setup` or
/// a hand edit may have added a profile, and appending to a stale in-memory
/// file would silently delete it.
pub fn apply_new(store: &Store, plan: &NewProfilePlan) -> Result<NewProfileOutcome, ProfileError> {
    let _writer = profile_writer();
    let source = store.games_source();
    let mut games = store.load_games_from_source(&source)?.value;
    if games
        .games
        .iter()
        .any(|g| g.title.trim().eq_ignore_ascii_case(plan.entry.title.trim()))
    {
        return Err(ProfileError::Duplicate {
            title: plan.entry.title.clone(),
        });
    }
    validate_program(&plan.entry)?;
    validate_layouts(store, &plan.entry)?;
    games.games.push(plan.entry.clone());
    let backup = store.backup(&source.path)?;
    store.save_games_to_source(&source, &games)?;
    Ok(NewProfileOutcome {
        backup,
        plan: plan.clone(),
    })
}

fn current_index(games: &GamesFile, expected: &GameEntry) -> Result<usize, ProfileError> {
    match profile_index(games, &expected.title) {
        Ok(index) if games.games[index] == *expected => Ok(index),
        Ok(_) | Err(ProfileError::UnknownProfile { .. }) => Err(ProfileError::Changed {
            title: expected.title.clone(),
        }),
        Err(err) => Err(err),
    }
}

/// Replace one profile behind a timestamped whole-file backup. The file is
/// re-read and compared with the value planning saw so an external edit cannot
/// be overwritten by a stale form submission.
pub fn apply_update(
    store: &Store,
    plan: &UpdateProfilePlan,
) -> Result<UpdateProfileOutcome, ProfileError> {
    let _writer = profile_writer();
    let source = store.games_source();
    let mut games = store.load_games_from_source(&source)?.value;
    let index = current_index(&games, &plan.original)?;
    check_title_available(&games, &plan.replacement.title, Some(index))?;
    validate_program(&plan.replacement)?;
    validate_layouts(store, &plan.replacement)?;
    if games.games[index] == plan.replacement {
        return Ok(UpdateProfileOutcome {
            backup: None,
            plan: plan.clone(),
            unchanged: true,
        });
    }
    games.games[index] = plan.replacement.clone();
    let backup = store.backup(&source.path)?;
    store.save_games_to_source(&source, &games)?;
    Ok(UpdateProfileOutcome {
        backup,
        plan: plan.clone(),
        unchanged: false,
    })
}

/// Remove the one exact profile [`plan_delete`] selected. No preset, device,
/// base slot, or second similarly named row is touched.
pub fn apply_delete(
    store: &Store,
    plan: &DeleteProfilePlan,
) -> Result<DeleteProfileOutcome, ProfileError> {
    let _writer = profile_writer();
    let source = store.games_source();
    let mut games = store.load_games_from_source(&source)?.value;
    let index = current_index(&games, &plan.entry)?;
    games.games.remove(index);
    let backup = store.backup(&source.path)?;
    store.save_games_to_source(&source, &games)?;
    Ok(DeleteProfileOutcome {
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
            "created saved game \"{}\" — {} player(s) using controller layout \"{}\"{}",
            plan.entry.title,
            plan.entry.slots.len(),
            plan.entry
                .slots
                .first()
                .map_or("(none)", |s| s.preset.as_str()),
            if self.backup.is_some() {
                " (backup saved first)"
            } else {
                ""
            },
        )
    }
}

impl UpdateProfileOutcome {
    pub fn message(&self) -> String {
        if self.unchanged {
            return format!(
                "saved game \"{}\" is already up to date",
                self.plan.replacement.title
            );
        }
        format!(
            "updated saved game \"{}\"{} — {} player(s) using controller layout \"{}\"{}",
            self.plan.original.title,
            if self.plan.original.title == self.plan.replacement.title {
                String::new()
            } else {
                format!(" → \"{}\"", self.plan.replacement.title)
            },
            self.plan.replacement.slots.len(),
            self.plan
                .replacement
                .slots
                .first()
                .map_or("(none)", |slot| slot.preset.as_str()),
            if self.backup.is_some() {
                " (backup saved first)"
            } else {
                ""
            },
        )
    }
}

impl DeleteProfileOutcome {
    pub fn message(&self) -> String {
        format!(
            "deleted saved game \"{}\" — controller layouts were kept{}",
            self.plan.entry.title,
            if self.backup.is_some() {
                " (backup saved first)"
            } else {
                ""
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ConfigFile {
        let mut base = ConfigFile::default();
        base.settings.block_keyboards = ksx_core::Blocking::BoundKeys;
        base.settings.block_mice = true;
        base.devices = vec![
            ksx_config::DeviceEntry {
                id: "usb:d209:0430:00".parse().unwrap(),
                alias: "P1 Panel".to_owned(),
                backend: Default::default(),
            },
            ksx_config::DeviceEntry {
                id: "usb:d209:0431:00".parse().unwrap(),
                alias: "P2 Panel".to_owned(),
                backend: Default::default(),
            },
            ksx_config::DeviceEntry {
                id: "usb:d209:0432:00".parse().unwrap(),
                alias: "P2 Trackball".to_owned(),
                backend: Default::default(),
            },
        ];
        base.slots = vec![
            ksx_config::SlotEntry {
                number: 1,
                keyboard: Some("P1 Panel".to_owned()),
                mouse: None,
                preset: "Player 1".to_owned(),
                persona: ksx_core::Persona::PlayStation,
                socd: ksx_core::Socd::UpPriority,
                macros: ksx_core::MacroSwitch::Off,
            },
            ksx_config::SlotEntry {
                number: 2,
                keyboard: Some("P2 Panel".to_owned()),
                mouse: Some("P2 Trackball".to_owned()),
                preset: "Player 2".to_owned(),
                persona: Default::default(),
                socd: Default::default(),
                macros: Default::default(),
            },
        ];
        base
    }

    /// Most planner tests are about validation unrelated to the base setup.
    /// Give them the same known-good two-controller cabinet while keeping the
    /// production signature explicit about that dependency.
    fn plan_new(
        games: &GamesFile,
        presets: &[String],
        spec: &NewProfileSpec,
    ) -> Result<NewProfilePlan, ProfileError> {
        super::plan_new(&base(), games, presets, spec)
    }

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

    fn update_spec(games: &GamesFile, original_title: &str) -> UpdateProfileSpec {
        UpdateProfileSpec {
            original_title: original_title.to_owned(),
            revision: games
                .games
                .iter()
                .find(|game| game.title.eq_ignore_ascii_case(original_title))
                .map(profile_revision)
                .unwrap_or_default(),
            title: original_title.to_owned(),
            path: "steam://rungameid/620".to_owned(),
            arguments: String::new(),
            slots: 2,
            preset: "Arcade".to_owned(),
            rebase_devices: false,
        }
    }

    fn playable_profile(title: &str) -> GameEntry {
        super::plan_new(
            &base(),
            &GamesFile::default(),
            &presets(),
            &spec(title, "C:\\games\\original.exe"),
        )
        .unwrap()
        .entry
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

    fn assert_saved_games_copy(line: &str) {
        let words = line
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        for internal_word in ["profile", "preset", "slot", "toml", "cli", "path"] {
            assert!(
                !words.iter().any(|word| word == internal_word),
                "customer copy leaked internal word {internal_word:?}: {line}"
            );
        }
    }

    #[test]
    fn errors_remedies_and_outcomes_use_saved_games_vocabulary() {
        let errors = [
            ProfileError::EmptyTitle,
            ProfileError::EmptyPath {
                title: "Street Fighter".to_owned(),
            },
            ProfileError::MalformedPathQuotes {
                title: "Street Fighter".to_owned(),
            },
            ProfileError::Duplicate {
                title: "Street Fighter".to_owned(),
            },
            ProfileError::NoSuchPreset {
                name: "Arcade".to_owned(),
            },
            ProfileError::BadSlots { asked: 0 },
            ProfileError::MissingBaseSlot {
                title: "Street Fighter".to_owned(),
                number: 2,
            },
            ProfileError::UnwiredBaseSlot {
                title: "Street Fighter".to_owned(),
                number: 2,
            },
            ProfileError::EmptyTarget,
            ProfileError::UnknownProfile {
                title: "Street Fighter".to_owned(),
            },
            ProfileError::AmbiguousProfile {
                title: "Street Fighter".to_owned(),
                matches: 2,
            },
            ProfileError::AmbiguousSlot {
                title: "Street Fighter".to_owned(),
                number: 1,
            },
            ProfileError::UnwiredProfileSlot {
                title: "Street Fighter".to_owned(),
                number: 1,
            },
            ProfileError::UnknownDevice {
                title: "Street Fighter".to_owned(),
                number: 1,
                device: "Old Panel".to_owned(),
            },
            ProfileError::ProgramUnavailable {
                title: "Street Fighter".to_owned(),
            },
            ProfileError::Changed {
                title: "Street Fighter".to_owned(),
            },
        ];
        for error in errors {
            assert_saved_games_copy(&error.to_string());
            assert_saved_games_copy(&error.advice());
        }
        let storage_error = ProfileError::Config(ConfigError::Io {
            path: PathBuf::from("games.toml"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied"),
        });
        assert_saved_games_copy(&storage_error.to_string());
        assert_saved_games_copy(&storage_error.advice());

        let created_plan = super::plan_new(
            &base(),
            &GamesFile::default(),
            &presets(),
            &spec("Street Fighter", "C:\\games\\sf.exe"),
        )
        .unwrap();
        let created = NewProfileOutcome {
            backup: None,
            plan: created_plan,
        };
        let games = GamesFile {
            games: vec![created.plan.entry.clone()],
        };
        let updated_plan = plan_update(
            &base(),
            &games,
            &presets(),
            &update_spec(&games, "Street Fighter"),
        )
        .unwrap();
        let deleted_plan = plan_delete(
            &games,
            &DeleteProfileSpec {
                title: "Street Fighter".to_owned(),
                revision: profile_revision(&games.games[0]),
            },
        )
        .unwrap();
        for line in [
            created.message(),
            UpdateProfileOutcome {
                backup: None,
                plan: updated_plan,
                unchanged: false,
            }
            .message(),
            DeleteProfileOutcome {
                backup: None,
                plan: deleted_plan,
            }
            .message(),
        ] {
            assert_saved_games_copy(&line);
            assert!(line.contains("saved game"), "{line}");
        }
    }

    #[test]
    fn a_plan_inherits_the_working_setup_but_uses_the_selected_preset() {
        let base = base();
        let before = base.clone();
        let plan = super::plan_new(
            &base,
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
        }
        assert_eq!(plan.entry.slots[0].keyboard.as_deref(), Some("P1 Panel"));
        assert_eq!(plan.entry.slots[0].mouse, None);
        assert_eq!(plan.entry.slots[0].persona, ksx_core::Persona::PlayStation);
        assert_eq!(plan.entry.slots[0].socd, ksx_core::Socd::UpPriority);
        assert_eq!(plan.entry.slots[0].macros, ksx_core::MacroSwitch::Off);
        assert_eq!(plan.entry.slots[1].keyboard.as_deref(), Some("P2 Panel"));
        assert_eq!(plan.entry.slots[1].mouse.as_deref(), Some("P2 Trackball"));
        assert_eq!(plan.entry.block_keyboards, ksx_core::Blocking::BoundKeys);
        assert!(plan.entry.block_mice);
        assert_eq!(base, before, "pure planning must not mutate the base setup");

        // The release regression in its runtime form: the entry produced by
        // Create survives the same planner Play uses, with two live slots
        // instead of both being skipped as NoInputDeviceSelected.
        let mut runnable_config = base;
        runnable_config.slots.clear();
        let games = GamesFile {
            games: vec![plan.entry],
        };
        let preset = ksx_config::PresetFile {
            name: "Arcade".to_owned(),
            bindings: Default::default(),
            macros: Default::default(),
        };
        let runnable = crate::run::plan::build_plan(
            &runnable_config,
            &games,
            &[preset],
            Some("Street Fighter"),
        )
        .expect("a newly created profile must be immediately plannable");
        assert_eq!(runnable.slots.len(), 2);
    }

    #[test]
    fn missing_or_unwired_base_slots_are_refused_instead_of_creating_a_dead_profile() {
        let asked = spec("Street Fighter", "C:\\games\\sf.exe");
        let mut one_slot = base();
        one_slot.slots.retain(|slot| slot.number == 1);
        let err =
            super::plan_new(&one_slot, &GamesFile::default(), &presets(), &asked).unwrap_err();
        assert!(
            matches!(err, ProfileError::MissingBaseSlot { number: 2, .. }),
            "{err}"
        );

        let mut unwired = base();
        unwired.slots[1].keyboard = None;
        unwired.slots[1].mouse = None;
        let err = super::plan_new(&unwired, &GamesFile::default(), &presets(), &asked).unwrap_err();
        assert!(
            matches!(err, ProfileError::UnwiredBaseSlot { number: 2, .. }),
            "{err}"
        );
        assert!(err.advice().contains("player 2"), "{}", err.advice());
    }

    #[test]
    fn inherited_and_preserved_device_aliases_must_still_exist() {
        let asked = spec("Street Fighter", "steam://rungameid/620");
        let mut missing_base_alias = base();
        missing_base_alias
            .devices
            .retain(|device| device.alias != "P1 Panel");
        let before = missing_base_alias.clone();
        let err = super::plan_new(
            &missing_base_alias,
            &GamesFile::default(),
            &presets(),
            &asked,
        )
        .unwrap_err();
        assert!(
            matches!(err, ProfileError::UnknownDevice { number: 1, .. }),
            "{err}"
        );
        assert_eq!(missing_base_alias, before, "a refusal mutates no setup");

        let mut original = playable_profile("Street Fighter");
        original.slots[0].keyboard = Some("Retired Panel".to_owned());
        let games = GamesFile {
            games: vec![original],
        };
        let before = games.clone();
        let mut preserve = update_spec(&games, "Street Fighter");
        let err = plan_update(&base(), &games, &presets(), &preserve).unwrap_err();
        assert!(
            matches!(err, ProfileError::UnknownDevice { number: 1, .. }),
            "{err}"
        );
        assert_eq!(games, before, "a refused edit mutates no saved game");

        preserve.rebase_devices = true;
        let repaired = plan_update(&base(), &games, &presets(), &preserve)
            .expect("the explicit current-device choice repairs a retired alias");
        assert_eq!(
            repaired.replacement.slots[0].keyboard.as_deref(),
            Some("P1 Panel")
        );
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
        assert!(err.advice().contains("create one"), "{}", err.advice());
    }

    #[test]
    fn an_empty_title_or_path_is_refused_with_the_reason() {
        let err =
            plan_new(&GamesFile::default(), &presets(), &spec("  ", "C:\\x.exe")).unwrap_err();
        assert!(matches!(err, ProfileError::EmptyTitle), "{err}");
        let err = plan_new(&GamesFile::default(), &presets(), &spec("SF", "  ")).unwrap_err();
        assert!(matches!(err, ProfileError::EmptyPath { .. }), "{err}");
        let err = plan_new(&GamesFile::default(), &presets(), &spec("SF", "  \"\"  ")).unwrap_err();
        assert!(
            matches!(err, ProfileError::EmptyPath { .. }),
            "a quoted empty string is still an empty path: {err}"
        );
    }

    /// Regression for the QA profile: copying a quoted path from Explorer or
    /// PowerShell stored the quote characters in games.toml, so an existing
    /// executable was immediately reported as missing.
    #[test]
    fn matching_quotes_and_outer_whitespace_are_removed_from_a_pasted_path() {
        for (pasted, expected) in [
            (
                "  \"C:\\Program Files (x86)\\Steam\\StreetFighterV.exe\"  ",
                "C:\\Program Files (x86)\\Steam\\StreetFighterV.exe",
            ),
            (
                "\t'C:\\Games\\Street Fighter.exe'\r\n",
                "C:\\Games\\Street Fighter.exe",
            ),
        ] {
            let plan = plan_new(
                &GamesFile::default(),
                &presets(),
                &spec("Street Fighter", pasted),
            )
            .unwrap();
            assert_eq!(plan.entry.path, expected, "pasted path: {pasted:?}");
        }
    }

    /// A lone quote or mixed quote pair is ambiguous. The broken behavior this
    /// catches is trying to be helpful by stripping whichever edge happens to
    /// exist and silently selecting a path the user did not enter.
    #[test]
    fn unmatched_or_mixed_path_quotes_are_refused_without_guessing() {
        for pasted in [
            "\"C:\\Games\\sf.exe",
            "C:\\Games\\sf.exe\"",
            "'C:\\Games\\sf.exe\"",
            "\"C:\\Games\\sf.exe'",
            "\"\"C:\\Games\\sf.exe\"\"",
            "''C:\\Games\\sf.exe''",
            "\"'C:\\Games\\sf.exe'\"",
            "'\"C:\\Games\\sf.exe\"'",
            "'",
        ] {
            let err = plan_new(
                &GamesFile::default(),
                &presets(),
                &spec("Street Fighter", pasted),
            )
            .unwrap_err();
            assert!(
                matches!(&err, ProfileError::MalformedPathQuotes { .. }),
                "malformed pasted path {pasted:?} must be refused, not normalized: {err}"
            );
        }
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

    /// Planning remains pure, but the writer refuses a missing local program
    /// before backup or mutation. Launcher links are the explicit exception.
    #[test]
    fn a_missing_local_program_is_refused_before_write_but_launcher_links_are_allowed() {
        let missing = std::env::temp_dir()
            .join(format!("ksx-missing-program-{}.exe", std::process::id()))
            .display()
            .to_string();
        let plan = plan_new(
            &GamesFile::default(),
            &presets(),
            &spec("MAME 4P", &missing),
        )
        .unwrap();
        assert_eq!(plan.entry.path, missing);

        let root = TempRoot::new("missing-program");
        let store = root.store(&base(), &GamesFile::default());
        let before = std::fs::read(store.root().games_path()).unwrap();
        let files = std::fs::read_dir(&root.0).unwrap().count();
        let err = apply_new(&store, &plan).unwrap_err();
        assert!(
            matches!(err, ProfileError::ProgramUnavailable { .. }),
            "{err}"
        );
        assert_eq!(std::fs::read(store.root().games_path()).unwrap(), before);
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), files);

        let launcher = super::plan_new(
            &base(),
            &GamesFile::default(),
            &presets(),
            &spec("Portal 2", "steam://rungameid/620"),
        )
        .unwrap();
        apply_new(&store, &launcher).expect("registered launcher links are intentionally allowed");
    }

    #[test]
    fn a_layout_that_no_longer_builds_is_refused_immediately_before_write() {
        let root = TempRoot::new("invalid-layout");
        let store = root.store(&base(), &GamesFile::default());
        let invalid: ksx_config::PresetFile =
            toml::from_str("name = \"Arcade\"\n[bindings]\nwarp = \"S\"\n").unwrap();
        assert!(invalid.to_core().is_err());
        store.save_preset(&invalid).unwrap();

        let plan = super::plan_new(
            &base(),
            &GamesFile::default(),
            &presets(),
            &spec("Street Fighter", "steam://rungameid/620"),
        )
        .unwrap();
        let before = std::fs::read(store.root().games_path()).unwrap();
        let files = std::fs::read_dir(&root.0).unwrap().count();
        let err = apply_new(&store, &plan).unwrap_err();
        assert!(matches!(err, ProfileError::NoSuchPreset { .. }), "{err}");
        assert_eq!(std::fs::read(store.root().games_path()).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(&root.0).unwrap().count(),
            files,
            "a refused layout creates no backup"
        );
    }

    #[test]
    fn update_preserves_profile_specific_setup_and_normalizes_the_edit() {
        let mut original = playable_profile("Street Fighter");
        original.notes = "tournament cabinet".to_owned();
        original.process_name = Some("StreetFighterV.exe".to_owned());
        original.launcher_grace_ms = Some(15_000);
        original.block_keyboards = ksx_core::Blocking::Off;
        original.block_mice = false;
        original.slots[0].keyboard = Some(r"HID\LEGACY\P1".to_owned());
        original.slots[0].persona = ksx_core::Persona::SwitchPro;
        original.slots[0].socd = ksx_core::Socd::Neutral;
        original.slots[0].macros = ksx_core::MacroSwitch::Off;
        let games = GamesFile {
            games: vec![original.clone()],
        };
        let before = games.clone();
        let mut asked = update_spec(&games, "street fighter");
        asked.title = "  Street Fighter V  ".to_owned();
        asked.path = "  \"C:\\Program Files\\StreetFighterV.exe\"  ".to_owned();
        asked.arguments = "  -fullscreen  ".to_owned();
        asked.preset = "DEFAULT".to_owned();

        let plan = plan_update(&base(), &games, &presets(), &asked).unwrap();
        assert_eq!(plan.replacement.title, "Street Fighter V");
        assert_eq!(
            plan.replacement.path,
            "C:\\Program Files\\StreetFighterV.exe"
        );
        assert_eq!(plan.replacement.arguments, "-fullscreen");
        assert_eq!(plan.replacement.notes, "tournament cabinet");
        assert_eq!(
            plan.replacement.process_name.as_deref(),
            Some("StreetFighterV.exe")
        );
        assert_eq!(plan.replacement.launcher_grace_ms, Some(15_000));
        assert_eq!(plan.replacement.block_keyboards, ksx_core::Blocking::Off);
        assert!(!plan.replacement.block_mice);
        assert_eq!(
            plan.replacement.slots[0].keyboard.as_deref(),
            Some(r"HID\LEGACY\P1")
        );
        assert_eq!(
            plan.replacement.slots[0].persona,
            ksx_core::Persona::SwitchPro
        );
        assert_eq!(plan.replacement.slots[0].socd, ksx_core::Socd::Neutral);
        assert_eq!(plan.replacement.slots[0].macros, ksx_core::MacroSwitch::Off);
        assert!(
            plan.replacement
                .slots
                .iter()
                .all(|slot| slot.preset == "default"),
            "the explicitly selected preset, in its on-disk spelling, wins"
        );
        assert_eq!(games, before, "pure update planning must not mutate input");
    }

    #[test]
    fn update_rebases_only_devices_and_uses_base_for_new_players() {
        let mut original = playable_profile("Street Fighter");
        original.slots.truncate(1);
        original.slots[0].keyboard = Some(r"HID\OLD\P1".to_owned());
        original.slots[0].mouse = None;
        original.slots[0].persona = ksx_core::Persona::PlayStation;
        let games = GamesFile {
            games: vec![original.clone()],
        };

        let preserved = plan_update(
            &base(),
            &games,
            &presets(),
            &update_spec(&games, "Street Fighter"),
        )
        .unwrap();
        assert_eq!(
            preserved.replacement.slots[0].keyboard.as_deref(),
            Some(r"HID\OLD\P1")
        );
        assert_eq!(
            preserved.replacement.slots[1].keyboard.as_deref(),
            Some("P2 Panel"),
            "a newly added player has no profile slot to preserve"
        );

        let mut asked = update_spec(&games, "Street Fighter");
        asked.rebase_devices = true;
        let rebased = plan_update(&base(), &games, &presets(), &asked).unwrap();
        assert_eq!(
            rebased.replacement.slots[0].keyboard.as_deref(),
            Some("P1 Panel")
        );
        assert_eq!(
            rebased.replacement.slots[0].persona,
            ksx_core::Persona::PlayStation,
            "rebase_devices changes selectors, not per-game persona"
        );
        assert_eq!(
            rebased.replacement.slots[1].mouse.as_deref(),
            Some("P2 Trackball")
        );

        let mut broken = original;
        broken.slots[0].keyboard = None;
        broken.slots[0].mouse = None;
        let games = GamesFile {
            games: vec![broken],
        };
        let mut asked = update_spec(&games, "Street Fighter");
        asked.rebase_devices = true;
        let err = plan_update(
            &base(),
            &games,
            &presets(),
            &update_spec(&games, "Street Fighter"),
        )
        .unwrap_err();
        assert!(
            matches!(err, ProfileError::UnwiredProfileSlot { number: 1, .. }),
            "{err}"
        );
        assert!(
            plan_update(&base(), &games, &presets(), &asked).is_ok(),
            "the explicit rebase is the repair path for a legacy device-less slot"
        );
    }

    #[test]
    fn update_and_delete_refuse_unknown_colliding_or_ambiguous_names() {
        let games = GamesFile {
            games: vec![playable_profile("Alpha"), playable_profile("Bravo")],
        };
        let mut rename = update_spec(&games, "Alpha");
        rename.title = "bravo".to_owned();
        let err = plan_update(&base(), &games, &presets(), &rename).unwrap_err();
        assert!(matches!(err, ProfileError::Duplicate { .. }), "{err}");

        let err =
            plan_update(&base(), &games, &presets(), &update_spec(&games, "Missing")).unwrap_err();
        assert!(matches!(err, ProfileError::UnknownProfile { .. }), "{err}");

        let ambiguous = GamesFile {
            games: vec![playable_profile("Steam"), playable_profile("steam")],
        };
        let err = plan_delete(
            &ambiguous,
            &DeleteProfileSpec {
                title: "STEAM".to_owned(),
                revision: String::new(),
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ProfileError::AmbiguousProfile { matches: 2, .. }),
            "{err}"
        );
        assert_eq!(ambiguous.games.len(), 2, "a refusal removes nothing");
    }

    #[test]
    fn stale_or_missing_form_revisions_refuse_before_planning_any_change() {
        let original = playable_profile("Street Fighter");
        let old_revision = profile_revision(&original);
        assert!(!old_revision.is_empty());

        let mut games = GamesFile {
            games: vec![original],
        };
        let mut update = update_spec(&games, "Street Fighter");
        let mut delete = DeleteProfileSpec {
            title: "Street Fighter".to_owned(),
            revision: old_revision.clone(),
        };

        // A field the current form does not expose still invalidates it. This
        // proves the revision covers the complete entry, not only visible
        // inputs or the title used for lookup.
        games.games[0].notes = "changed by another surface".to_owned();
        let before = games.clone();
        assert!(matches!(
            plan_update(&base(), &games, &presets(), &update),
            Err(ProfileError::Changed { .. })
        ));
        assert!(matches!(
            plan_delete(&games, &delete),
            Err(ProfileError::Changed { .. })
        ));
        assert_eq!(games, before, "stale form refusals mutate nothing");

        update.revision.clear();
        delete.revision.clear();
        assert!(matches!(
            plan_update(&base(), &games, &presets(), &update),
            Err(ProfileError::Changed { .. })
        ));
        assert!(matches!(
            plan_delete(&games, &delete),
            Err(ProfileError::Changed { .. })
        ));
        assert_ne!(old_revision, profile_revision(&games.games[0]));
    }

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-profile-edit-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn store(&self, config: &ConfigFile, games: &GamesFile) -> Store {
            let store = Store::new(ksx_config::ConfigRoot::at(&self.0));
            store.save_config(config).unwrap();
            store.save_games(games).unwrap();
            store
                .save_preset(&ksx_config::PresetFile {
                    name: "Arcade".to_owned(),
                    bindings: Default::default(),
                    macros: Default::default(),
                })
                .unwrap();
            store
        }

        fn json_store(&self, config: &ConfigFile, games: &GamesFile) -> Store {
            let store = Store::new(ksx_config::ConfigRoot::at(&self.0));
            store.save_config(config).unwrap();
            let document = ksx_config::interop::to_json(games, ksx_config::JsonStyle::Pretty)
                .expect("games JSON renders");
            std::fs::write(store.root().games_json_path(), document).unwrap();
            store
                .save_preset(&ksx_config::PresetFile {
                    name: "Arcade".to_owned(),
                    bindings: Default::default(),
                    macros: Default::default(),
                })
                .unwrap();
            store
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn update_and_delete_each_backup_then_touch_exactly_one_profile() {
        let base = base();
        let games = GamesFile {
            games: vec![playable_profile("Alpha"), playable_profile("Bravo")],
        };
        let root = TempRoot::new("apply");
        let store = root.store(&base, &games);
        let preset_path = store
            .save_preset(&ksx_config::PresetFile {
                name: "Arcade".to_owned(),
                bindings: Default::default(),
                macros: Default::default(),
            })
            .unwrap();

        let mut asked = update_spec(&games, "Alpha");
        asked.title = "Alpha Prime".to_owned();
        let plan = plan_update(&base, &games, &presets(), &asked).unwrap();
        let updated = apply_update(&store, &plan).unwrap();
        assert!(updated.backup.is_some());
        let after_update = store.load_games().unwrap().value;
        assert_eq!(after_update.games.len(), 2);
        assert_eq!(after_update.games[0].title, "Alpha Prime");
        assert_eq!(after_update.games[1], games.games[1]);

        let delete = plan_delete(
            &after_update,
            &DeleteProfileSpec {
                title: "Bravo".to_owned(),
                revision: profile_revision(
                    after_update
                        .games
                        .iter()
                        .find(|game| game.title == "Bravo")
                        .unwrap(),
                ),
            },
        )
        .unwrap();
        let deleted = apply_delete(&store, &delete).unwrap();
        assert!(deleted.backup.is_some());
        let after_delete = store.load_games().unwrap().value;
        assert_eq!(after_delete.games.len(), 1);
        assert_eq!(after_delete.games[0].title, "Alpha Prime");
        assert!(preset_path.is_file(), "deleting a profile keeps its preset");
        assert_eq!(
            store.load_config().unwrap().value,
            base,
            "neither writer changes the base controller setup"
        );
    }

    #[test]
    fn a_concurrent_change_is_not_overwritten_or_backed_up() {
        let base = base();
        let games = GamesFile {
            games: vec![playable_profile("Alpha")],
        };
        let root = TempRoot::new("stale");
        let store = root.store(&base, &games);
        let plan = plan_update(&base, &games, &presets(), &update_spec(&games, "Alpha")).unwrap();

        let mut changed = games;
        changed.games[0].notes = "edited somewhere else".to_owned();
        store.save_games(&changed).unwrap();
        let path = store.root().games_path();
        let bytes_before = std::fs::read(&path).unwrap();
        let files_before = std::fs::read_dir(&root.0).unwrap().count();
        let err = apply_update(&store, &plan).unwrap_err();
        assert!(matches!(err, ProfileError::Changed { .. }), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
        assert_eq!(
            std::fs::read_dir(&root.0).unwrap().count(),
            files_before,
            "a refused stale write must not leave a backup"
        );
    }

    #[test]
    fn json_only_create_update_and_delete_each_back_up_the_file_they_write() {
        let base = base();
        let initial = GamesFile {
            games: vec![playable_profile("Alpha"), playable_profile("Bravo")],
        };
        let root = TempRoot::new("json-apply");
        let store = root.json_store(&base, &initial);
        let json = store.root().games_json_path();
        let toml = store.root().games_path();
        assert_eq!(store.games_source().path, json);
        assert!(!toml.exists(), "a JSON-only root must stay JSON-only");

        let assert_backup = |backup: Option<PathBuf>, before: &[u8]| {
            let backup = backup.expect("an existing games.json gets a backup");
            assert!(
                backup
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("games.json.bak-")),
                "the active JSON source, not the absent TOML sibling, is backed up: {}",
                backup.display()
            );
            assert_eq!(std::fs::read(backup).unwrap(), before);
        };

        let before_create = std::fs::read(&json).unwrap();
        let create = super::plan_new(
            &base,
            &initial,
            &presets(),
            &spec("Charlie", "steam://rungameid/777"),
        )
        .unwrap();
        let created = apply_new(&store, &create).unwrap();
        assert_backup(created.backup, &before_create);
        let after_create = store.load_games().unwrap().value;
        assert_eq!(after_create.games.len(), 3);

        let before_update = std::fs::read(&json).unwrap();
        let mut request = update_spec(&after_create, "Alpha");
        request.title = "Alpha Prime".to_owned();
        let update = plan_update(&base, &after_create, &presets(), &request).unwrap();
        let updated = apply_update(&store, &update).unwrap();
        assert_backup(updated.backup, &before_update);
        let after_update = store.load_games().unwrap().value;
        assert_eq!(after_update.games[0].title, "Alpha Prime");

        let before_delete = std::fs::read(&json).unwrap();
        let delete = plan_delete(
            &after_update,
            &DeleteProfileSpec {
                title: "Bravo".to_owned(),
                revision: profile_revision(
                    after_update
                        .games
                        .iter()
                        .find(|game| game.title == "Bravo")
                        .unwrap(),
                ),
            },
        )
        .unwrap();
        let deleted = apply_delete(&store, &delete).unwrap();
        assert_backup(deleted.backup, &before_delete);
        let after_delete = store.load_games().unwrap().value;
        assert_eq!(
            after_delete
                .games
                .iter()
                .map(|game| game.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha Prime", "Charlie"]
        );
        assert!(json.is_file());
        assert!(!toml.exists(), "CRUD must not create a shadowing TOML twin");
    }

    #[test]
    fn profile_apply_waits_for_the_single_process_writer() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = TempRoot::new("writer-lock");
        let store = root.store(&base(), &GamesFile::default());
        let plan = super::plan_new(
            &base(),
            &GamesFile::default(),
            &presets(),
            &spec("Alpha", "steam://rungameid/888"),
        )
        .unwrap();

        let held = super::profile_writer();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(apply_new(&store, &plan)).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "a second profile writer must wait for the active critical section"
        );
        drop(held);
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the waiting writer resumes after the lock is released")
            .unwrap();
        writer.join().unwrap();
    }
}
