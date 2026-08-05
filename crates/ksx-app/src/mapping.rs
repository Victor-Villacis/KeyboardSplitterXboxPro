//! The one mapping writer: `ksx map`, the daemon's pipe `map` verb and (through
//! it) Studio's mapper all call [`apply`] — no surface gets a private editing
//! path (docs/CONTROL-SURFACE.md's standing rule).
//!
//! Semantics:
//!
//! - **Replace per function.** `map --function A --key G` makes G the ONLY key
//!   bound to A — the mapper's tile shows one tag per control, and the file
//!   says what the tile says. (Multi-key fan-in stays expressible by hand;
//!   this verb does not author it.)
//! - **`--clear`** leaves the function in the file bound to the inert `"None"`
//!   placeholder — the same convention as the built-in empty preset, so a
//!   cleared control stays visible instead of silently vanishing.
//! - **Conflicts block, the caller decides** (the PadForge gap this closes —
//!   docs/research/padforge-code-audit.md §1.2 "Conflict handling: none"). Two
//!   scopes are checked: the key already bound to ANOTHER function in the same
//!   preset, and the key bound in another slot's preset within any games.toml
//!   profile that uses the target preset. `force` proceeds: same-preset
//!   conflicts are stolen (the key is removed from the old function, which
//!   keeps a `"None"` placeholder if emptied); **other presets are never
//!   edited** — a cross-profile conflict under `force` is written anyway and
//!   reported, because silently rewriting a file the caller did not name is
//!   worse than a double binding the response spells out.
//! - **Writes are canonical.** The store serializes `PresetFile` afresh:
//!   bindings come back sorted with flat quoted dotted keys (`"dpad.up"`), and
//!   hand-written comments do not survive. That is the documented trade for
//!   atomic, validated writes (store.rs); the file remains hand-editable.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ksx_config::{parse_function, ConfigError, PresetFile, Store};
use ksx_core::Key;

/// One requested edit.
#[derive(Clone, Debug)]
pub struct MapSpec {
    /// Preset name (the `name` field, e.g. `"IPAC P1"`), not a file name.
    pub preset: String,
    /// Function name, any case (`A`, `dpad.up`, `lx.min`, `lx.-16384`).
    pub function: String,
    /// `Some(key name)` binds, `None` clears.
    pub key: Option<String>,
    /// Proceed despite conflicts (see module docs for exactly what that does).
    pub force: bool,
}

/// Where a conflicting binding lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictScope {
    /// Another function in the SAME preset. `force` steals from it.
    Preset,
    /// A different slot's preset inside a games.toml profile that also uses
    /// the target preset. Never auto-edited; reported for the caller.
    Profile,
}

/// One conflicting binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapConflict {
    pub scope: ConflictScope,
    /// Preset holding the conflicting binding.
    pub preset: String,
    /// Canonical function name the key is bound to there.
    pub function: String,
    /// Profile title (Profile scope only).
    pub profile: Option<String>,
    /// Slot number inside that profile (Profile scope only).
    pub slot: Option<u8>,
}

impl MapConflict {
    /// One human line, e.g. `G is "IPAC P2"'s A (slot 2 of "Steam")`.
    pub fn describe(&self, key: &str) -> String {
        match self.scope {
            ConflictScope::Preset => format!("{key} is already this preset's {}", self.function),
            ConflictScope::Profile => format!(
                "{key} is \"{}\"'s {}{}",
                self.preset,
                self.function,
                match (&self.profile, self.slot) {
                    (Some(profile), Some(slot)) => format!(" (slot {slot} of \"{profile}\")"),
                    (Some(profile), None) => format!(" (\"{profile}\")"),
                    _ => String::new(),
                }
            ),
        }
    }
}

/// What a successful [`apply`] did.
#[derive(Clone, Debug)]
pub struct AppliedMap {
    /// The file that was written.
    pub path: PathBuf,
    pub preset: String,
    /// Canonical function spelling (what the file now says).
    pub function: String,
    /// Canonical key name, `None` for a clear.
    pub key: Option<String>,
    /// Same-preset functions the key was stolen from (`force`).
    pub stolen_from: Vec<String>,
    /// Cross-profile conflicts that were overridden by `force` — written
    /// anyway, reported so the caller can say so.
    pub overridden: Vec<MapConflict>,
}

impl AppliedMap {
    /// The one-line confirmation every surface prints.
    pub fn message(&self) -> String {
        let mut line = match &self.key {
            Some(key) => format!("\"{}\": {} = {}", self.preset, self.function, key),
            None => format!("\"{}\": {} cleared", self.preset, self.function),
        };
        if !self.stolen_from.is_empty() {
            line.push_str(&format!(" (taken from {})", self.stolen_from.join(", ")));
        }
        if let Some(key) = &self.key {
            for conflict in &self.overridden {
                line.push_str(&format!("; still {}", conflict.describe(key)));
            }
        }
        line
    }
}

/// Why an [`apply`] (or [`restore`]) refused or failed.
#[derive(Debug)]
pub enum MapError {
    /// No preset with that name — nothing is guessed, nothing is created.
    UnknownPreset {
        name: String,
        known: Vec<String>,
    },
    UnknownFunction(String),
    UnknownKey(String),
    /// Conflicts found and `force` not given. The write did NOT happen.
    Conflicts {
        key: String,
        conflicts: Vec<MapConflict>,
    },
    /// `restore session-backup` with no backup on disk: nothing was mapped
    /// through the daemon this session, so there is nothing to undo.
    NoSessionBackup {
        preset: String,
    },
    /// `restore latest-backup` with no `*.toml.bak-*` file on disk: nothing has
    /// ever been restored for this preset, so no timestamped backup exists.
    NoBackup {
        preset: String,
    },
    /// A backup file exists but does not parse/validate — restoring it would
    /// trade a good file for a bad one, so it is refused. `source` names which
    /// backup ("the session-start backup", a file name).
    BadBackup {
        preset: String,
        source: String,
        reason: String,
    },
    Config(ConfigError),
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapError::UnknownPreset { name, known } => {
                write!(f, "unknown preset \"{name}\"")?;
                if !known.is_empty() {
                    write!(f, " (presets on disk: {})", known.join(", "))?;
                }
                Ok(())
            }
            MapError::UnknownFunction(name) => write!(
                f,
                "unknown function \"{name}\" (buttons A B X Y start back guide lb rb \
                 lthumb rthumb, triggers lt rt, axes lx/ly/rx/ry with .min/.max/.<i16>, \
                 dpad.up/.down/.left/.right)"
            ),
            MapError::UnknownKey(name) => write!(
                f,
                "unknown key \"{name}\" — key names use the legacy spelling \
                 (`ksx monitor` shows the name for any key you press)"
            ),
            MapError::Conflicts { key, conflicts } => {
                write!(f, "refusing to bind {key}: ")?;
                for (i, conflict) in conflicts.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}", conflict.describe(key))?;
                }
                write!(
                    f,
                    " — use --force to bind anyway (same-preset conflicts are \
                     stolen; other presets are never edited)"
                )
            }
            MapError::NoSessionBackup { preset } => write!(
                f,
                "no session backup for \"{preset}\" — nothing has been mapped through the \
                 daemon this session, so there is nothing to undo"
            ),
            MapError::NoBackup { preset } => write!(
                f,
                "no timestamped backup for \"{preset}\" — one is written next to the preset \
                 (\"<preset>.toml.bak-YYYYMMDD-HHMMSS\") before every restore, so the first \
                 restore of a preset has nothing older to go back to"
            ),
            MapError::BadBackup {
                preset,
                source,
                reason,
            } => write!(
                f,
                "{source} for \"{preset}\" is unreadable ({reason}) — refusing to replace a \
                 good preset with it"
            ),
            MapError::Config(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for MapError {}

impl From<ConfigError> for MapError {
    fn from(err: ConfigError) -> Self {
        Self::Config(err)
    }
}

/// Load, validate, conflict-check, edit, write. See the module docs for the
/// exact semantics of every step.
pub fn apply(store: &Store, spec: &MapSpec) -> Result<AppliedMap, MapError> {
    // Validate the function and key names FIRST — a typo must not depend on
    // which preset it was aimed at.
    let binding = parse_function(&spec.function)
        .map_err(|_| MapError::UnknownFunction(spec.function.clone()))?;
    let canonical = ksx_config::function_name(&binding);
    let key = spec.key.as_deref().map(resolve_key).transpose()?;

    // The preset must exist; `ksx map` creates bindings, never presets.
    let file = load_preset_by_name(store, &spec.preset)?;
    let mut entries = file.to_core()?.entries;

    let mut stolen_from = Vec::new();
    let mut overridden = Vec::new();
    if let Some(key) = key {
        let conflicts = find_conflicts(store, &spec.preset, &entries, key, &canonical);
        if !conflicts.is_empty() && !spec.force {
            return Err(MapError::Conflicts {
                key: key.name().to_owned(),
                conflicts,
            });
        }
        for conflict in conflicts {
            match conflict.scope {
                ConflictScope::Preset => stolen_from.push(conflict.function.clone()),
                ConflictScope::Profile => overridden.push(conflict),
            }
        }
        // Steal (same preset only): drop the key wherever else it appears,
        // leaving a "None" placeholder if a function would end up empty.
        if !stolen_from.is_empty() {
            let victims: Vec<ksx_core::Binding> = entries
                .iter()
                .filter(|(k, b)| *k == key && ksx_config::function_name(b) != canonical)
                .map(|(_, b)| *b)
                .collect();
            entries.retain(|(k, b)| !(*k == key && ksx_config::function_name(b) != canonical));
            for victim in victims {
                if !entries.iter().any(|(_, b)| {
                    ksx_config::function_name(b) == ksx_config::function_name(&victim)
                }) {
                    entries.push((Key::None, victim));
                }
            }
        }
    }

    // Replace-per-function: out with every old key for this function...
    entries.retain(|(_, b)| ksx_config::function_name(b) != canonical);
    // ...in with the new one (or the inert placeholder for a clear).
    entries.push((key.unwrap_or(Key::None), binding));

    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries,
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedMap {
        path,
        preset: file.name,
        function: canonical,
        key: key.map(|k| k.name().to_owned()),
        stolen_from,
        overridden,
    })
}

// ---------------------------------------------------------------------------
// Restore: the mapper's THREE destinations (docs/CONTROL-SURFACE.md
// "map-restore"). All of them go through the same store writer as `apply` — no
// private editing path — and all of them take a timestamped backup first.
//
// The three are deliberately different distances back, and the UI must name
// them by their DESTINATION, never by the word "defaults" (MAPPER-UX
// commandment 5: honest labels, guaranteed road home). "Defaults" is the one
// that surprised Victor: it does NOT mean "how this preset shipped" — it means
// the LEGACY GENERIC KEYBOARD layout (S=A, D=B, A=X, W=Y, Q/E triggers, arrows
// = left stick, Esc=Start), which on an arcade cabinet replaces an I-PAC
// panel map with a desktop-keyboard map. It stays available because it is the
// always-there floor, but it is spelled out everywhere it appears.
// ---------------------------------------------------------------------------

/// Which safety net [`restore`] pulls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreKind {
    /// Rewrite the preset's bindings to `ksx_core::Preset::builtin_default()` —
    /// the LEGACY GENERIC KEYBOARD layout, not "this preset as it shipped".
    /// Keeps the preset's name.
    Defaults,
    /// Rewrite the preset from its `<file>.session-bak` — the snapshot
    /// [`take_session_backup`] made before the daemon's FIRST map write to
    /// that preset this daemon lifetime ("undo everything since the daemon
    /// started").
    SessionBackup,
    /// Rewrite the preset from the NEWEST `<file>.bak-YYYYMMDD-HHMMSS` — the
    /// snapshot taken automatically before the previous restore. This is the
    /// undo for a restore itself.
    LatestBackup,
    /// Not a restore at all: [`clear_all`]'s destination, sharing this type so
    /// every whole-preset write reports the same way (and takes the same
    /// backup). Deliberately NOT parseable from a `--restore`/`"mode"` word —
    /// "clear everything" is its own verb, never a spelling of "restore".
    ClearAll,
}

impl RestoreKind {
    /// The wire word (`ksx map --restore <mode>`, pipe `map-restore` `"mode"`,
    /// and the `"mode"` field of every whole-preset response).
    pub fn as_str(self) -> &'static str {
        match self {
            RestoreKind::Defaults => "defaults",
            RestoreKind::SessionBackup => "session-backup",
            RestoreKind::LatestBackup => "latest-backup",
            RestoreKind::ClearAll => "clear-all",
        }
    }

    /// Parse a RESTORE mode. `None` for anything else — callers report the
    /// three valid spellings rather than guessing, and `clear-all` is
    /// deliberately not one of them.
    pub fn parse(mode: &str) -> Option<Self> {
        match mode {
            "defaults" => Some(RestoreKind::Defaults),
            "session-backup" => Some(RestoreKind::SessionBackup),
            "latest-backup" => Some(RestoreKind::LatestBackup),
            _ => None,
        }
    }

    /// What this destination WRITES, in one clause — the sentence every
    /// confirm dialog and every CLI line is built from. Never the bare word
    /// "defaults".
    pub fn destination(self) -> &'static str {
        match self {
            RestoreKind::Defaults => {
                "the generic keyboard layout (S=A, D=B, A=X, W=Y, Q/E triggers, arrow keys \
                 = left stick, Esc=Start) — NOT this preset's original panel map"
            }
            RestoreKind::SessionBackup => {
                "this preset as it was before the daemon's first change this session"
            }
            RestoreKind::LatestBackup => {
                "this preset as it was before the most recent restore (the newest \
                 timestamped backup)"
            }
            RestoreKind::ClearAll => {
                "an empty preset — every control still listed, none of them bound"
            }
        }
    }
}

/// One `<preset>.toml.bak-YYYYMMDD-HHMMSS` on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetBackup {
    pub path: PathBuf,
    /// `YYYYMMDD-HHMMSS` — sortable, and the file name's suffix verbatim.
    pub stamp: String,
}

impl PresetBackup {
    /// The stamp spelled for a human: `2026-08-05 14:32:07 UTC`.
    ///
    /// UTC, like every other timestamp ksx prints (the studio pages, the
    /// snapshot lines): converting to local time needs a Win32 call that would
    /// make this module platform-specific for a cosmetic gain, and a mixed
    /// UTC/local page is worse than a consistent UTC one.
    pub fn label(&self) -> String {
        let (date, time) = match self.stamp.split_once('-') {
            Some(split) => split,
            None => return self.stamp.clone(),
        };
        if date.len() != 8 || time.len() < 6 {
            return self.stamp.clone();
        }
        format!(
            "{}-{}-{} {}:{}:{} UTC",
            &date[0..4],
            &date[4..6],
            &date[6..8],
            &time[0..2],
            &time[2..4],
            &time[4..6],
        )
    }
}

/// The suffix that marks a timestamped backup: `<preset file>.bak-<stamp>`.
const BACKUP_MARK: &str = ".bak-";

/// `YYYYMMDD-HHMMSS`, UTC — sortable as a plain string, which is what makes
/// "newest" a lexicographic max rather than a filesystem-mtime guess.
fn stamp_now() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// Every timestamped backup of `preset_name`, NEWEST FIRST.
///
/// Backups are never pruned: they are small text files, a restore is a rare
/// deliberate act, and silently deleting a user's only copy of a panel map to
/// save a few kilobytes is not a trade ksx gets to make. The mapper shows the
/// newest one; the rest sit next to the preset for anyone who needs them.
pub fn list_backups(store: &Store, preset_name: &str) -> Result<Vec<PresetBackup>, MapError> {
    let preset_path = store.preset_path(preset_name)?;
    let Some(dir) = preset_path.parent() else {
        return Ok(Vec::new());
    };
    let Some(file_name) = preset_path.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let prefix = format!("{file_name}{BACKUP_MARK}");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new()); // no presets dir yet = no backups, not an error
    };
    let mut backups: Vec<PresetBackup> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_owned();
            let stamp = name.strip_prefix(&prefix)?.to_owned();
            (!stamp.is_empty()).then_some(PresetBackup { path, stamp })
        })
        .collect();
    // Newest first. The stamp is fixed-width and zero-padded, so a plain
    // descending string sort IS chronological (and the collision suffix
    // `-2`, `-3`… sorts after the bare stamp of the same second).
    backups.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(backups)
}

/// Copy the preset file to `<file>.bak-YYYYMMDD-HHMMSS`.
///
/// Called before EVERY restore (see [`restore`]) — that is the whole point:
/// no restore can be the last word, because the thing it overwrote is on disk
/// with a timestamp on it. `Ok(None)` means there was no preset file to copy
/// (a caller that is about to fail with `UnknownPreset` anyway).
///
/// Two restores inside one second get `-2`, `-3`… appended, so a backup is
/// never silently overwritten by the restore that follows it.
pub fn take_backup(store: &Store, preset_name: &str) -> Result<Option<PresetBackup>, MapError> {
    let source = store.preset_path(preset_name)?;
    if !source.exists() {
        return Ok(None);
    }
    let base = stamp_now();
    let (path, stamp) = (1u32..)
        .map(|n| {
            let stamp = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            (
                PathBuf::from(format!("{}{BACKUP_MARK}{stamp}", source.display())),
                stamp,
            )
        })
        .find(|(path, _)| !path.exists())
        .expect("an unbounded suffix search always finds a free name");
    std::fs::copy(&source, &path).map_err(|err| MapError::BadBackup {
        preset: preset_name.to_owned(),
        source: "the pre-restore backup".to_owned(),
        reason: format!("could not write {}: {err}", path.display()),
    })?;
    Ok(Some(PresetBackup { path, stamp }))
}

/// What a successful [`restore`] did.
#[derive(Clone, Debug)]
pub struct AppliedRestore {
    pub path: PathBuf,
    pub preset: String,
    pub kind: RestoreKind,
    /// The timestamped backup taken immediately before the write — the road
    /// back from this very restore. `None` only when there was no file to copy.
    pub backup: Option<PresetBackup>,
}

impl AppliedRestore {
    /// The one-line confirmation every surface prints. It names what was
    /// WRITTEN and what was BACKED UP, in that order — a restore that does not
    /// say both is a restore somebody will be afraid of.
    pub fn message(&self) -> String {
        let wrote = match self.kind {
            RestoreKind::Defaults => format!(
                "\"{}\": bindings reset to the generic keyboard layout (S/D/A/W…)",
                self.preset
            ),
            RestoreKind::SessionBackup => format!(
                "\"{}\": bindings restored from the session-start backup",
                self.preset
            ),
            RestoreKind::LatestBackup => format!(
                "\"{}\": bindings restored from the newest timestamped backup",
                self.preset
            ),
            RestoreKind::ClearAll => format!(
                "\"{}\": every binding cleared (all controls still listed, none bound)",
                self.preset
            ),
        };
        match &self.backup {
            Some(backup) => format!(
                "{wrote} — the previous file is backed up as {}",
                backup.stamp
            ),
            None => wrote,
        }
    }
}

/// `<preset file>.session-bak`, next to the preset itself.
pub fn session_backup_path(store: &Store, preset_name: &str) -> Result<PathBuf, MapError> {
    let path = store.preset_path(preset_name)?;
    Ok(PathBuf::from(format!("{}.session-bak", path.display())))
}

/// Snapshot the preset file to `<file>.session-bak` — called by the daemon's
/// map writer before its FIRST write to that preset in this daemon lifetime
/// (the caller keeps the once-per-lifetime set; this function just copies).
/// A missing preset file is not an error here: `apply` will name it properly.
pub fn take_session_backup(store: &Store, preset_name: &str) -> Result<(), MapError> {
    let Ok(source) = store.preset_path(preset_name) else {
        return Ok(());
    };
    if !source.exists() {
        return Ok(());
    }
    let backup = PathBuf::from(format!("{}.session-bak", source.display()));
    std::fs::copy(&source, &backup).map_err(|err| MapError::BadBackup {
        preset: preset_name.to_owned(),
        source: "the session-start backup".to_owned(),
        reason: format!("could not write {}: {err}", backup.display()),
    })?;
    Ok(())
}

/// Read a backup file and validate it the way every other write is validated —
/// a hand-damaged backup must never be swapped in for a good preset.
fn bindings_from_backup(
    path: &std::path::Path,
    preset_name: &str,
    source: &str,
) -> Result<Vec<(Key, ksx_core::Binding)>, MapError> {
    let bad = |reason: String| MapError::BadBackup {
        preset: preset_name.to_owned(),
        source: source.to_owned(),
        reason,
    };
    let text = std::fs::read_to_string(path).map_err(|err| bad(err.to_string()))?;
    let parsed: PresetFile = toml::from_str(&text).map_err(|err| bad(err.to_string()))?;
    let core = parsed.to_core().map_err(|err| bad(err.to_string()))?;
    Ok(core.entries)
}

/// Restore a preset's bindings from one of the three destinations.
///
/// The order is the safety property: the replacement is resolved and validated
/// FIRST (so a refusal leaves no pointless backup lying around), then the
/// current file is copied to `<file>.bak-YYYYMMDD-HHMMSS`, and only then is the
/// preset overwritten. The write is canonical (same serializer as [`apply`]);
/// the preset must already exist — restore edits presets, it never creates them.
pub fn restore(
    store: &Store,
    preset_name: &str,
    kind: RestoreKind,
) -> Result<AppliedRestore, MapError> {
    let file = load_preset_by_name(store, preset_name)?;
    let entries = match kind {
        RestoreKind::Defaults => ksx_core::Preset::builtin_default().entries,
        RestoreKind::SessionBackup => {
            let backup = session_backup_path(store, preset_name)?;
            if !backup.exists() {
                return Err(MapError::NoSessionBackup {
                    preset: preset_name.to_owned(),
                });
            }
            bindings_from_backup(&backup, preset_name, "the session-start backup")?
        }
        RestoreKind::LatestBackup => {
            let newest = list_backups(store, preset_name)?
                .into_iter()
                .next()
                .ok_or_else(|| MapError::NoBackup {
                    preset: preset_name.to_owned(),
                })?;
            let source = format!("the backup from {}", newest.label());
            bindings_from_backup(&newest.path, preset_name, &source)?
        }
        // `clear-all` is its own verb ([`clear_all`]) precisely so it cannot be
        // reached by anything spelled "restore".
        RestoreKind::ClearAll => ksx_core::Preset::builtin_empty().entries,
    };

    // Everything below this line WILL write: take the road home first.
    let backup = take_backup(store, preset_name)?;
    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries,
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedRestore {
        path,
        preset: file.name,
        kind,
        backup,
    })
}

/// Unbind every function of a preset, keeping the file structurally valid.
///
/// This writes the `empty` built-in's SHAPE — every function present, keyed
/// `Key::None` — rather than deleting rows, which is the same convention
/// `--clear` uses for one function: a cleared control stays visible in the file
/// and in the mapper's legend instead of silently vanishing.
///
/// Like every whole-preset write it takes a timestamped backup first, so
/// "Clear all bindings" has the same one-click road home as a restore.
pub fn clear_all(store: &Store, preset_name: &str) -> Result<AppliedRestore, MapError> {
    let file = load_preset_by_name(store, preset_name)?;
    let backup = take_backup(store, preset_name)?;
    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries: ksx_core::Preset::builtin_empty().entries,
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedRestore {
        path,
        preset: file.name,
        kind: RestoreKind::ClearAll,
        backup,
    })
}

/// Exact legacy spelling first; a UNIQUE case-insensitive match is accepted
/// (panel keys get typed as `g` at a shell); anything else is refused.
fn resolve_key(name: &str) -> Result<Key, MapError> {
    if let Some(key) = Key::from_name(name) {
        return Ok(key);
    }
    let mut matches = Key::ALL
        .iter()
        .copied()
        .filter(|k| k.name().eq_ignore_ascii_case(name));
    match (matches.next(), matches.next()) {
        (Some(key), None) => Ok(key),
        _ => Err(MapError::UnknownKey(name.to_owned())),
    }
}

/// Find the preset by its `name` field. The file name is derived storage
/// (store.rs), so this scans `load_presets` rather than guessing a path.
fn load_preset_by_name(store: &Store, name: &str) -> Result<PresetFile, MapError> {
    let loaded = store.load_presets()?;
    let known: Vec<String> = loaded.value.iter().map(|p| p.name.clone()).collect();
    loaded
        .value
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| MapError::UnknownPreset {
            name: name.to_owned(),
            known,
        })
}

/// Both conflict scopes for `key` aimed at `canonical` in `preset_name`.
fn find_conflicts(
    store: &Store,
    preset_name: &str,
    entries: &[(Key, ksx_core::Binding)],
    key: Key,
    canonical: &str,
) -> Vec<MapConflict> {
    let mut conflicts = Vec::new();

    // Same preset: the key on any OTHER function.
    for (k, b) in entries {
        let function = ksx_config::function_name(b);
        if *k == key && function != canonical {
            conflicts.push(MapConflict {
                scope: ConflictScope::Preset,
                preset: preset_name.to_owned(),
                function,
                profile: None,
                slot: None,
            });
        }
    }

    // Profiles that use this preset: the key in any OTHER slot's preset.
    // games.toml being unreadable is not a mapping error — the preset write
    // stands on its own; an unreadable profile list just cannot warn.
    let Ok(games) = store.load_games() else {
        return conflicts;
    };
    let mut cache: BTreeMap<String, Vec<(Key, String)>> = BTreeMap::new();
    let mut seen: Vec<(String, String)> = Vec::new(); // dedupe (preset, function)
    for game in &games.value.games {
        if !game.slots.iter().any(|s| s.preset == preset_name) {
            continue;
        }
        for slot in &game.slots {
            if slot.preset == preset_name {
                continue; // same-preset scope already covers it
            }
            let bound = cache.entry(slot.preset.clone()).or_insert_with(|| {
                store
                    .load_preset(&slot.preset)
                    .ok()
                    .flatten()
                    .and_then(|loaded| loaded.value.to_core().ok())
                    .map(|core| {
                        core.entries
                            .iter()
                            .map(|(k, b)| (*k, ksx_config::function_name(b)))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            for (k, function) in bound.iter() {
                if *k != key {
                    continue;
                }
                let dedupe = (slot.preset.clone(), function.clone());
                if seen.contains(&dedupe) {
                    continue;
                }
                seen.push(dedupe);
                conflicts.push(MapConflict {
                    scope: ConflictScope::Profile,
                    preset: slot.preset.clone(),
                    function: function.clone(),
                    profile: Some(game.title.clone()),
                    slot: Some(slot.number),
                });
            }
        }
    }
    conflicts
}

/// The conflicts as pipe/Studio JSON rows — one shape everywhere.
pub fn conflicts_json(conflicts: &[MapConflict]) -> serde_json::Value {
    serde_json::Value::Array(
        conflicts
            .iter()
            .map(|c| {
                serde_json::json!({
                    "scope": match c.scope {
                        ConflictScope::Preset => "preset",
                        ConflictScope::Profile => "profile",
                    },
                    "preset": c.preset,
                    "function": c.function,
                    "profile": c.profile,
                    "slot": c.slot,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::{ConfigRoot, GamesFile};

    struct TempRoot(std::path::PathBuf);
    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-mapping-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> Store {
            Store::new(ConfigRoot::at(&self.0))
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn preset(store: &Store, name: &str, toml: &str) {
        let file: PresetFile =
            toml::from_str(&format!("name = \"{name}\"\n[bindings]\n{toml}")).unwrap();
        store.save_preset(&file).unwrap();
    }

    fn games(store: &Store, toml: &str) {
        let file: GamesFile = toml::from_str(toml).unwrap();
        store.save_games(&file).unwrap();
    }

    fn spec(preset: &str, function: &str, key: Option<&str>, force: bool) -> MapSpec {
        MapSpec {
            preset: preset.into(),
            function: function.into(),
            key: key.map(str::to_owned),
            force,
        }
    }

    #[test]
    fn a_clean_bind_writes_canonical_toml() {
        let root = TempRoot::new("clean");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");

        let applied = apply(&store, &spec("P1", "a", Some("g"), false)).unwrap();
        assert_eq!(applied.function, "A", "function name canonicalized");
        assert_eq!(applied.key.as_deref(), Some("G"), "key name canonicalized");
        assert!(applied.stolen_from.is_empty());
        assert_eq!(applied.message(), "\"P1\": A = G");

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(
            !on_disk.contains("\"S\""),
            "replace-per-function: {on_disk}"
        );
    }

    #[test]
    fn replace_per_function_collapses_multi_key_bindings() {
        let root = TempRoot::new("multi");
        let store = root.store();
        preset(&store, "P1", "A = [\"S\", \"Enter\"]\n");
        let applied = apply(&store, &spec("P1", "A", Some("G"), false)).unwrap();
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(!on_disk.contains("Enter"), "{on_disk}");
    }

    #[test]
    fn clear_leaves_the_inert_placeholder() {
        let root = TempRoot::new("clear");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");
        let applied = apply(&store, &spec("P1", "A", None, false)).unwrap();
        assert_eq!(applied.key, None);
        assert_eq!(applied.message(), "\"P1\": A cleared");
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"None\""), "{on_disk}");
        assert!(
            on_disk.contains("B = \"D\""),
            "untouched sibling: {on_disk}"
        );
    }

    #[test]
    fn dotted_functions_write_the_flat_quoted_form() {
        let root = TempRoot::new("dotted");
        let store = root.store();
        preset(&store, "P1", "dpad.up = \"I\"\n");
        let applied = apply(&store, &spec("P1", "DPAD.UP", Some("W"), false)).unwrap();
        assert_eq!(applied.function, "dpad.up");
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("\"dpad.up\" = \"W\""), "{on_disk}");
        // And the rewrite still parses back through the store.
        assert_eq!(store.load_preset("P1").unwrap().unwrap().value.name, "P1");
    }

    #[test]
    fn unknown_names_are_refused_before_any_write() {
        let root = TempRoot::new("unknown");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        assert!(matches!(
            apply(&store, &spec("Nope", "A", Some("G"), false)),
            Err(MapError::UnknownPreset { .. })
        ));
        assert!(matches!(
            apply(&store, &spec("P1", "warp", Some("G"), false)),
            Err(MapError::UnknownFunction(_))
        ));
        assert!(matches!(
            apply(&store, &spec("P1", "A", Some("NotAKey"), false)),
            Err(MapError::UnknownKey(_))
        ));
        let after = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert_eq!(before, after, "no refusal may leave a changed file");
    }

    #[test]
    fn unknown_preset_lists_what_exists() {
        let root = TempRoot::new("known-list");
        let store = root.store();
        preset(&store, "IPAC P1", "A = \"S\"\n");
        let err = apply(&store, &spec("IPAC P9", "A", Some("G"), false)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("IPAC P9"), "{text}");
        assert!(text.contains("IPAC P1"), "{text}");
    }

    #[test]
    fn a_same_preset_conflict_blocks_without_force_and_steals_with_it() {
        let root = TempRoot::new("steal");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\n");

        // Without force: refused, file untouched.
        let err = apply(&store, &spec("P1", "A", Some("G"), false)).unwrap_err();
        let MapError::Conflicts { key, conflicts } = &err else {
            panic!("expected conflicts, got {err:?}");
        };
        assert_eq!(key, "G");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].scope, ConflictScope::Preset);
        assert_eq!(conflicts[0].function, "B");
        assert!(err.to_string().contains("--force"), "{err}");

        // With force: stolen, and the victim keeps a "None" placeholder.
        let applied = apply(&store, &spec("P1", "A", Some("G"), true)).unwrap();
        assert_eq!(applied.stolen_from, vec!["B".to_owned()]);
        assert!(
            applied.message().contains("taken from B"),
            "{}",
            applied.message()
        );
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(on_disk.contains("B = \"None\""), "{on_disk}");
    }

    #[test]
    fn a_cross_profile_conflict_reports_but_never_edits_the_other_preset() {
        let root = TempRoot::new("profile");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        preset(&store, "P2", "A = \"G\"\n");
        games(
            &store,
            r#"
[[game]]
title = "Steam"
path = "C:\\steam.exe"
[[game.slot]]
number = 1
preset = "P1"
[[game.slot]]
number = 2
preset = "P2"
"#,
        );

        let err = apply(&store, &spec("P1", "B", Some("G"), false)).unwrap_err();
        let MapError::Conflicts { conflicts, .. } = &err else {
            panic!("expected conflicts, got {err:?}");
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].scope, ConflictScope::Profile);
        assert_eq!(conflicts[0].preset, "P2");
        assert_eq!(conflicts[0].function, "A");
        assert_eq!(conflicts[0].profile.as_deref(), Some("Steam"));
        assert_eq!(conflicts[0].slot, Some(2));
        assert!(err.to_string().contains("\"P2\"'s A"), "{err}");

        // Force writes the target, reports the override, leaves P2 alone.
        let p2_before = std::fs::read_to_string(store.preset_path("P2").unwrap()).unwrap();
        let applied = apply(&store, &spec("P1", "B", Some("G"), true)).unwrap();
        assert_eq!(applied.overridden.len(), 1);
        assert!(
            applied.message().contains("still G is \"P2\"'s A"),
            "{}",
            applied.message()
        );
        let p2_after = std::fs::read_to_string(store.preset_path("P2").unwrap()).unwrap();
        assert_eq!(p2_before, p2_after, "other presets are never edited");
    }

    #[test]
    fn profiles_not_using_the_target_preset_are_not_conflict_scope() {
        let root = TempRoot::new("scope");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        preset(&store, "Other", "A = \"G\"\n");
        games(
            &store,
            r#"
[[game]]
title = "Solo"
path = "C:\\solo.exe"
[[game.slot]]
number = 1
preset = "Other"
"#,
        );
        // "Solo" does not use P1, so Other's G is not in scope.
        assert!(apply(&store, &spec("P1", "B", Some("G"), false)).is_ok());
    }

    #[test]
    fn keys_resolve_case_insensitively_when_unique() {
        assert_eq!(resolve_key("g").unwrap(), Key::G);
        assert_eq!(resolve_key("enter").unwrap(), Key::Enter);
        assert_eq!(resolve_key("Left").unwrap(), Key::Left);
        assert!(resolve_key("not a key").is_err());
    }

    #[test]
    fn restore_defaults_rewrites_the_bindings_and_keeps_the_name() {
        let root = TempRoot::new("restore-defaults");
        let store = root.store();
        preset(&store, "P1", "A = \"Q\"\nB = \"W\"\n");

        let applied = restore(&store, "P1", RestoreKind::Defaults).unwrap();
        assert_eq!(applied.preset, "P1");
        // The label names the DESTINATION, never the bare word "defaults" —
        // Victor's cabinet would otherwise read "restore defaults" as "put my
        // I-PAC map back" and get a desktop-keyboard layout instead.
        let message = applied.message();
        assert!(message.contains("generic keyboard layout"), "{message}");
        assert!(message.contains("S/D/A/W"), "{message}");
        assert!(
            message.contains("backed up as"),
            "a restore must say where the old file went: {message}"
        );
        let reloaded = store.load_preset("P1").unwrap().unwrap().value;
        assert_eq!(reloaded.name, "P1", "name survives a defaults restore");
        let defaults = PresetFile::from_core(&ksx_core::Preset {
            name: "P1".into(),
            entries: ksx_core::Preset::builtin_default().entries,
            protected: false,
        });
        assert_eq!(
            reloaded.bindings, defaults.bindings,
            "bindings are exactly the built-in default layout"
        );
    }

    #[test]
    fn restore_refuses_unknown_presets() {
        let root = TempRoot::new("restore-unknown");
        let store = root.store();
        assert!(matches!(
            restore(&store, "Nope", RestoreKind::Defaults),
            Err(MapError::UnknownPreset { .. })
        ));
    }

    #[test]
    fn session_backup_round_trip_undoes_later_writes() {
        let root = TempRoot::new("session-bak");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");

        // No backup yet: restore says so and writes nothing.
        let err = restore(&store, "P1", RestoreKind::SessionBackup).unwrap_err();
        assert!(matches!(err, MapError::NoSessionBackup { .. }), "{err:?}");
        assert!(err.to_string().contains("nothing to undo"), "{err}");

        // The daemon's first-write snapshot, then two edits.
        take_session_backup(&store, "P1").unwrap();
        apply(&store, &spec("P1", "A", Some("G"), false)).unwrap();
        apply(&store, &spec("P1", "B", None, false)).unwrap();
        let edited = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert!(edited.contains("A = \"G\""), "{edited}");
        assert!(edited.contains("B = \"None\""), "{edited}");

        // Undo this session: both edits gone.
        let applied = restore(&store, "P1", RestoreKind::SessionBackup).unwrap();
        assert!(applied.message().contains("session-start backup"));
        let restored = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert!(restored.contains("A = \"S\""), "{restored}");
        assert!(restored.contains("B = \"D\""), "{restored}");
    }

    #[test]
    fn a_corrupt_session_backup_is_refused_not_written() {
        let root = TempRoot::new("session-bak-corrupt");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        std::fs::write(
            session_backup_path(&store, "P1").unwrap(),
            "this is not a preset",
        )
        .unwrap();
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        let err = restore(&store, "P1", RestoreKind::SessionBackup).unwrap_err();
        assert!(matches!(err, MapError::BadBackup { .. }), "{err:?}");
        let after = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert_eq!(before, after, "refusal must not touch the preset");
        // A refusal must not leave a pointless timestamped backup behind
        // either — the copy happens only once the write is certain.
        assert!(list_backups(&store, "P1").unwrap().is_empty());
    }

    // -- timestamped backups (FIX 2) ---------------------------------------

    /// The road home from a restore. "Reset to the generic keyboard layout"
    /// is the most destructive button on the page; `latest-backup` is what
    /// makes pressing it survivable.
    #[test]
    fn every_restore_backs_the_preset_up_first_and_latest_backup_undoes_it() {
        let root = TempRoot::new("bak-undo");
        let store = root.store();
        preset(&store, "IPAC P1", "A = \"G\"\nB = \"F\"\n");
        let original = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();

        // Nothing to go back to yet: the refusal names the mechanism.
        let err = restore(&store, "IPAC P1", RestoreKind::LatestBackup).unwrap_err();
        assert!(matches!(err, MapError::NoBackup { .. }), "{err:?}");
        assert!(err.to_string().contains("bak-YYYYMMDD-HHMMSS"), "{err}");

        // The scary one: reset to the generic keyboard layout.
        let applied = restore(&store, "IPAC P1", RestoreKind::Defaults).unwrap();
        let backup = applied.backup.expect("a restore always backs up first");
        assert!(backup.path.exists(), "{}", backup.path.display());
        assert_eq!(
            std::fs::read_to_string(&backup.path).unwrap(),
            original,
            "the backup must be the file as it was before the restore"
        );
        let generic = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        // The legacy generic layout: S (and Enter) on A, W on Y, arrows on the
        // left stick — a desktop keyboard, nothing like the panel map above.
        assert!(generic.contains(r#"A = ["S", "Enter"]"#), "{generic}");
        assert!(generic.contains(r#""lx.min" = "Left""#), "{generic}");

        // …and one click back to the panel map.
        let undone = restore(&store, "IPAC P1", RestoreKind::LatestBackup).unwrap();
        assert!(
            undone.message().contains("newest timestamped backup"),
            "{}",
            undone.message()
        );
        let back = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(back.contains("A = \"G\""), "{back}");
        assert!(back.contains("B = \"F\""), "{back}");
        // Two restores, two backups, newest first.
        let backups = list_backups(&store, "IPAC P1").unwrap();
        assert_eq!(backups.len(), 2, "{backups:?}");
        assert!(backups[0].stamp >= backups[1].stamp, "{backups:?}");
    }

    /// Two restores inside one second must not overwrite each other's backup —
    /// the whole chain would collapse to one entry.
    #[test]
    fn backups_taken_in_the_same_second_do_not_collide() {
        let root = TempRoot::new("bak-collide");
        let store = root.store();
        preset(&store, "P1", "A = \"G\"\n");
        for _ in 0..3 {
            take_backup(&store, "P1").unwrap().expect("preset exists");
        }
        let backups = list_backups(&store, "P1").unwrap();
        assert_eq!(backups.len(), 3, "{backups:?}");
        let mut stamps: Vec<&str> = backups.iter().map(|b| b.stamp.as_str()).collect();
        stamps.sort_unstable();
        stamps.dedup();
        assert_eq!(stamps.len(), 3, "stamps must be distinct: {backups:?}");
    }

    /// Backups sit next to the preset with a non-`.toml` extension, so the
    /// store's preset scan must never pick one up as a second preset.
    #[test]
    fn backups_are_invisible_to_the_preset_loader() {
        let root = TempRoot::new("bak-invisible");
        let store = root.store();
        preset(&store, "P1", "A = \"G\"\n");
        take_backup(&store, "P1").unwrap();
        take_session_backup(&store, "P1").unwrap();
        let loaded = store.load_presets().unwrap();
        assert_eq!(loaded.value.len(), 1, "{:?}", loaded.value);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    /// "Clear all bindings" empties the preset without breaking it — every
    /// function is still listed (as the inert `"None"`), so the mapper's
    /// legend keeps 25 rows and the file keeps parsing.
    #[test]
    fn clear_all_empties_the_preset_and_leaves_a_backup() {
        let root = TempRoot::new("clear-all");
        let store = root.store();
        preset(&store, "IPAC P1", "A = \"G\"\nB = \"F\"\n");

        let applied = clear_all(&store, "IPAC P1").unwrap();
        assert_eq!(applied.kind, RestoreKind::ClearAll);
        assert!(applied.backup.is_some(), "clearing must be undoable");
        let message = applied.message();
        assert!(message.contains("every binding cleared"), "{message}");
        assert!(message.contains("backed up as"), "{message}");

        let on_disk = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(!on_disk.contains("\"G\""), "{on_disk}");
        assert!(on_disk.contains("A = \"None\""), "{on_disk}");
        assert!(on_disk.contains("\"dpad.up\" = \"None\""), "{on_disk}");
        // Still a valid preset, and every function is still present.
        let core = store
            .load_preset("IPAC P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert_eq!(core.entries.len(), 25);

        // …and one click back to the panel map.
        restore(&store, "IPAC P1", RestoreKind::LatestBackup).unwrap();
        let back = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(back.contains("A = \"G\""), "{back}");
    }

    #[test]
    fn clear_all_refuses_an_unknown_preset() {
        let root = TempRoot::new("clear-all-unknown");
        assert!(matches!(
            clear_all(&root.store(), "Nope"),
            Err(MapError::UnknownPreset { .. })
        ));
    }

    #[test]
    fn a_backup_stamp_spells_itself_out_for_a_human() {
        let backup = PresetBackup {
            path: PathBuf::from("x"),
            stamp: "20260805-143207".to_owned(),
        };
        assert_eq!(backup.label(), "2026-08-05 14:32:07 UTC");
        // Anything unexpected degrades to the raw stamp, never to a lie.
        let odd = PresetBackup {
            path: PathBuf::from("x"),
            stamp: "nonsense".to_owned(),
        };
        assert_eq!(odd.label(), "nonsense");
    }

    /// The wire words are contract: CLI `--restore`, pipe `"mode"`, Studio's
    /// three buttons all speak the same three strings.
    #[test]
    fn restore_kinds_round_trip_their_wire_words_and_name_their_destination() {
        for kind in [
            RestoreKind::Defaults,
            RestoreKind::SessionBackup,
            RestoreKind::LatestBackup,
        ] {
            assert_eq!(RestoreKind::parse(kind.as_str()), Some(kind));
            assert!(!kind.destination().is_empty());
        }
        assert_eq!(RestoreKind::parse("yolo"), None);
        // "clear everything" is its own verb, never a spelling of "restore" —
        // otherwise `--restore clear-all` would read as a way BACK.
        assert_eq!(RestoreKind::parse("clear-all"), None);
        assert_eq!(RestoreKind::ClearAll.as_str(), "clear-all");
        // The one label that must never be vague.
        assert!(RestoreKind::Defaults
            .destination()
            .contains("generic keyboard layout"));
        assert!(RestoreKind::Defaults
            .destination()
            .contains("NOT this preset's original panel map"));
    }

    #[test]
    fn conflicts_serialize_to_the_documented_rows() {
        let rows = conflicts_json(&[MapConflict {
            scope: ConflictScope::Profile,
            preset: "P2".into(),
            function: "A".into(),
            profile: Some("Steam".into()),
            slot: Some(2),
        }]);
        assert_eq!(
            rows,
            serde_json::json!([{
                "scope": "profile", "preset": "P2", "function": "A",
                "profile": "Steam", "slot": 2
            }])
        );
    }
}
