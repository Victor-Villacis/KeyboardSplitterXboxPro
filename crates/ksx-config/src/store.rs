//! File persistence: lenient loads, atomic saves, schema migration + backup.
//!
//! Rules (each maps to a legacy defect):
//! - **Lenient parsing** — unknown keys become [`Warning`]s the caller can
//!   log, never errors (legacy threw on any unknown XML node and users hated
//!   it). Missing files mean clean defaults, not errors.
//! - **Atomic writes** — tmp file + rename in the same directory, so a crash
//!   mid-save never leaves a truncated config. UTF-8 always (a leading BOM is
//!   tolerated on read, never written).
//! - **Migration backup** — before any migrating write, the original file is
//!   copied to `<name>.bak-YYYYMMDD-HHMMSS` next to it. The clock is
//!   injectable so tests pin exact backup names.
//! - **Format preservation** — a file loaded from `.json` is saved back as
//!   `.json`; everything else is canonical TOML. Where both spellings exist,
//!   **TOML wins and the JSON is ignored** with a
//!   [`WarningKind::ShadowedByCanonicalToml`] — never merged. The whole
//!   canonical-vs-interop argument lives in [`crate::interop`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::{ConfigFile, SCHEMA_VERSION};
use crate::error::ConfigError;
use crate::games::GamesFile;
use crate::interop::{self, Format, JsonStyle};
use crate::paths::ConfigRoot;
use crate::preset::PresetFile;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Civil UTC timestamp used for backup file names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Timestamp {
    pub fn now_utc() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self::from_unix(secs)
    }

    pub fn from_unix(secs: u64) -> Self {
        let (year, month, day) = civil_from_days((secs / 86_400) as i64);
        let tod = secs % 86_400;
        Self {
            year,
            month,
            day,
            hour: (tod / 3600) as u8,
            minute: (tod % 3600 / 60) as u8,
            second: (tod % 60) as u8,
        }
    }

    /// `YYYYMMDD-HHMMSS`.
    pub fn backup_suffix(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

// Howard Hinnant's civil_from_days; exact for the whole unix range used here.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m as u8, d as u8)
}

/// A non-fatal problem found while loading; callers decide how to log it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub file: PathBuf,
    #[serde(flatten)]
    pub kind: WarningKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WarningKind {
    /// An unknown key or table entry was ignored (dotted path, e.g.
    /// `settings.frobnicate`, `device.0.extra`).
    UnknownKey { path: String },
    /// A `presets/*.toml` could not be loaded and was skipped; the file is
    /// left untouched on disk.
    SkippedPreset { error: String },
    /// A JSON interop file was IGNORED because the canonical TOML for the same
    /// name sits next to it. Never a merge: one winner, said out loud (see
    /// [`crate::interop`]).
    ShadowedByCanonicalToml { toml: PathBuf },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            WarningKind::UnknownKey { path } => {
                write!(f, "{}: unknown key '{path}' ignored", self.file.display())
            }
            WarningKind::SkippedPreset { error } => {
                write!(f, "{}: preset skipped: {error}", self.file.display())
            }
            WarningKind::ShadowedByCanonicalToml { toml } => write!(
                f,
                "{} ignored: {} is canonical and wins (the two are never merged)",
                self.file.display(),
                toml.display()
            ),
        }
    }
}

/// A loaded value plus everything worth logging about how it loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded<T> {
    pub value: T,
    pub warnings: Vec<Warning>,
}

impl<T> Loaded<T> {
    fn clean(value: T) -> Self {
        Self {
            value,
            warnings: Vec::new(),
        }
    }
}

/// One in-place schema upgrade step (`from` → `from + 1`) applied to the raw
/// TOML document before typed parsing. Returns a human-readable failure.
pub type MigrationStep = fn(&mut toml::Table) -> Result<(), String>;

/// Registry of schema upgrade steps, keyed by the version they upgrade FROM.
/// Version 1 is the first schema, so the registry ships empty; it exists as
/// the hook point for future versions (and lets tests exercise the
/// backup-then-rewrite path today).
#[derive(Clone, Default)]
pub struct Migrations {
    steps: BTreeMap<u32, MigrationStep>,
}

impl Migrations {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn register(&mut self, from: u32, step: MigrationStep) {
        self.steps.insert(from, step);
    }

    fn step(&self, from: u32) -> Option<MigrationStep> {
        self.steps.get(&from).copied()
    }
}

/// Which spelling of a config file this store will read and write, and what
/// the other spelling cost.
///
/// Resolution is the same three lines everywhere: canonical TOML if it exists,
/// otherwise the JSON interop file if THAT exists, otherwise the canonical
/// TOML path (so a brand-new file is born canonical).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The file that will actually be read and written.
    pub path: PathBuf,
    pub format: Format,
    /// The JSON file that was ignored because `path` is a canonical TOML next
    /// to it. Callers surface it; nothing merges it.
    pub shadowed: Option<Warning>,
}

impl Source {
    fn resolve(toml_path: PathBuf, json_path: PathBuf) -> Self {
        let has_toml = toml_path.is_file();
        let has_json = json_path.is_file();
        match (has_toml, has_json) {
            (true, true) => Self {
                shadowed: Some(Warning {
                    file: json_path,
                    kind: WarningKind::ShadowedByCanonicalToml {
                        toml: toml_path.clone(),
                    },
                }),
                path: toml_path,
                format: Format::Toml,
            },
            (false, true) => Self {
                path: json_path,
                format: Format::Json,
                shadowed: None,
            },
            // Missing entirely: the canonical path, so the first write is TOML.
            _ => Self {
                path: toml_path,
                format: Format::Toml,
                shadowed: None,
            },
        }
    }

    fn warnings(&self) -> Vec<Warning> {
        self.shadowed.clone().into_iter().collect()
    }
}

/// Persistence for one [`ConfigRoot`].
pub struct Store {
    root: ConfigRoot,
    clock: Box<dyn Fn() -> Timestamp + Send + Sync>,
    migrations: Migrations,
}

impl Store {
    pub fn new(root: ConfigRoot) -> Self {
        Self {
            root,
            clock: Box::new(Timestamp::now_utc),
            migrations: Migrations::none(),
        }
    }

    /// Replace the backup-name clock (tests pin exact backup file names).
    pub fn with_clock(mut self, clock: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    pub fn with_migrations(mut self, migrations: Migrations) -> Self {
        self.migrations = migrations;
        self
    }

    pub fn root(&self) -> &ConfigRoot {
        &self.root
    }

    /// Which file the main config resolves to, and the JSON sibling (if any)
    /// that the canonical TOML shadowed.
    pub fn config_source(&self) -> Source {
        Source::resolve(self.root.config_path(), self.root.config_json_path())
    }

    /// Which file `games` resolves to. See [`Store::config_source`].
    pub fn games_source(&self) -> Source {
        Source::resolve(self.root.games_path(), self.root.games_json_path())
    }

    /// Which file the named preset resolves to. See [`Store::config_source`].
    pub fn preset_source(&self, name: &str) -> Result<Source, ConfigError> {
        let stem = preset_file_name(name)?;
        let dir = self.root.presets_dir();
        Ok(Source::resolve(
            dir.join(format!("{stem}.toml")),
            dir.join(format!("{stem}.json")),
        ))
    }

    /// Load `config.toml` (or the portable `ksx.toml`, or the `config.json`
    /// interop file when no TOML exists). Missing file → defaults. An outdated
    /// `schema_version` with a registered migration path is backed up,
    /// migrated, and rewritten before parsing — TOML only; a JSON config at
    /// another schema is refused (see [`crate::interop`]).
    pub fn load_config(&self) -> Result<Loaded<ConfigFile>, ConfigError> {
        let source = self.config_source();
        let mut warnings = source.warnings();
        let Some(raw) = read_utf8(&source.path)? else {
            return Ok(Loaded {
                value: ConfigFile::default(),
                warnings,
            });
        };
        let mut loaded = match source.format {
            Format::Toml => {
                let content = self.migrate_config(&source.path, raw)?;
                parse_lenient::<ConfigFile>(&source.path, &content)?
            }
            Format::Json => {
                interop::json_schema_gate(&source.path, &raw)?;
                interop::parse_lenient::<ConfigFile>(&source.path, &raw)?
            }
        };
        warnings.append(&mut loaded.warnings);
        Ok(Loaded {
            value: loaded.value,
            warnings,
        })
    }

    /// Atomic save, in the format the file already had (TOML for a new one).
    /// `schema_version` is normalized to the current [`SCHEMA_VERSION`] so this
    /// build never writes a file it would refuse to load.
    pub fn save_config(&self, config: &ConfigFile) -> Result<PathBuf, ConfigError> {
        let mut config = config.clone();
        config.schema_version = SCHEMA_VERSION;
        let source = self.config_source();
        write_atomic(&source.path, &render(&config, &source)?)?;
        Ok(source.path)
    }

    /// Load `games.toml` (or `games.json`). Missing file → empty games list.
    pub fn load_games(&self) -> Result<Loaded<GamesFile>, ConfigError> {
        let source = self.games_source();
        let mut warnings = source.warnings();
        let Some(raw) = read_utf8(&source.path)? else {
            return Ok(Loaded {
                value: GamesFile::default(),
                warnings,
            });
        };
        let mut loaded = read_typed::<GamesFile>(&source, &raw)?;
        warnings.append(&mut loaded.warnings);
        Ok(Loaded {
            value: loaded.value,
            warnings,
        })
    }

    pub fn save_games(&self, games: &GamesFile) -> Result<PathBuf, ConfigError> {
        let source = self.games_source();
        write_atomic(&source.path, &render(games, &source)?)?;
        Ok(source.path)
    }

    /// Load every `presets/*.toml` and every `presets/*.json` with no TOML of
    /// the same name, sorted by file name for determinism. Missing directory →
    /// empty. A file that fails to load is skipped with a
    /// [`WarningKind::SkippedPreset`] — one corrupt preset must not take down
    /// the whole config load (legacy invalidated every slot on parse failure).
    /// A `.json` shadowed by a `.toml` of the same stem is ignored with a
    /// [`WarningKind::ShadowedByCanonicalToml`], never merged.
    pub fn load_presets(&self) -> Result<Loaded<Vec<PresetFile>>, ConfigError> {
        let dir = self.root.presets_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Loaded::clean(Vec::new()))
            }
            Err(source) => return Err(ConfigError::Io { path: dir, source }),
        };
        // Group by file STEM, case-insensitively: on Windows `Foo.toml` and
        // `FOO.json` are the same preset name, and the shadow rule has to see
        // them as a pair or it would load the preset twice.
        let mut by_stem: BTreeMap<String, BTreeMap<Format, PathBuf>> = BTreeMap::new();
        for path in entries.filter_map(|entry| entry.ok().map(|e| e.path())) {
            let Some(format) = Format::of(&path) else {
                continue;
            };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            by_stem
                .entry(stem.to_ascii_lowercase())
                .or_default()
                .insert(format, path);
        }

        let mut value = Vec::new();
        let mut warnings = Vec::new();
        for pair in by_stem.into_values() {
            let source = match (pair.get(&Format::Toml), pair.get(&Format::Json)) {
                (Some(toml_path), json) => Source {
                    path: toml_path.clone(),
                    format: Format::Toml,
                    shadowed: json.map(|json_path| Warning {
                        file: json_path.clone(),
                        kind: WarningKind::ShadowedByCanonicalToml {
                            toml: toml_path.clone(),
                        },
                    }),
                },
                (None, Some(json_path)) => Source {
                    path: json_path.clone(),
                    format: Format::Json,
                    shadowed: None,
                },
                (None, None) => continue,
            };
            warnings.extend(source.warnings());
            let outcome = read_utf8(&source.path).and_then(|raw| match raw {
                None => Ok(None), // vanished mid-scan
                Some(raw) => read_typed::<PresetFile>(&source, &raw).map(Some),
            });
            match outcome {
                Ok(Some(loaded)) => {
                    warnings.extend(loaded.warnings);
                    value.push(loaded.value);
                }
                Ok(None) => {}
                Err(error) => warnings.push(Warning {
                    file: source.path,
                    kind: WarningKind::SkippedPreset {
                        error: error.to_string(),
                    },
                }),
            }
        }
        Ok(Loaded { value, warnings })
    }

    /// Load one preset by name (file looked up via [`preset_file_name`]).
    /// Missing file → `Ok(None)`.
    pub fn load_preset(&self, name: &str) -> Result<Option<Loaded<PresetFile>>, ConfigError> {
        let source = self.preset_source(name)?;
        let Some(raw) = read_utf8(&source.path)? else {
            return Ok(None);
        };
        let mut loaded = read_typed::<PresetFile>(&source, &raw)?;
        let mut warnings = source.warnings();
        warnings.append(&mut loaded.warnings);
        Ok(Some(Loaded {
            value: loaded.value,
            warnings,
        }))
    }

    pub fn save_preset(&self, preset: &PresetFile) -> Result<PathBuf, ConfigError> {
        let source = self.preset_source(&preset.name)?;
        write_atomic(&source.path, &render(preset, &source)?)?;
        Ok(source.path)
    }

    /// Where a preset with this name is stored. The `name` field inside the
    /// file is the identity; the file name is derived storage.
    ///
    /// This is the RESOLVED path — the `.json` interop file when that is the
    /// only spelling on disk — so every caller that reads, writes or backs up
    /// a preset lands on the same file this store would.
    pub fn preset_path(&self, name: &str) -> Result<PathBuf, ConfigError> {
        Ok(self.preset_source(name)?.path)
    }

    /// The canonical TOML path for a preset name, whatever is on disk. Use it
    /// only when you specifically mean "the canonical file"; [`preset_path`]
    /// is what reads and writes should use.
    ///
    /// [`preset_path`]: Store::preset_path
    pub fn canonical_preset_path(&self, name: &str) -> Result<PathBuf, ConfigError> {
        Ok(self
            .root
            .presets_dir()
            .join(format!("{}.toml", preset_file_name(name)?)))
    }

    /// Copy `path` to `<file>.bak-YYYYMMDD-HHMMSS` — the timestamped backup
    /// taken before anything overwrites a file the user did not write.
    ///
    /// `Ok(None)` means there was nothing there to copy. Two backups inside one
    /// second get `-2`, `-3`… appended, so a backup is never silently
    /// overwritten by the write that follows it. Same convention (and same
    /// name shape) as the mapper's preset backups, so `ksx map --list-backups`
    /// finds the ones an import leaves behind.
    pub fn backup(&self, path: &Path) -> Result<Option<PathBuf>, ConfigError> {
        if !path.is_file() {
            return Ok(None);
        }
        let base = (self.clock)().backup_suffix();
        let target = (1u32..)
            .map(|n| {
                let stamp = if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                };
                PathBuf::from(format!("{}.bak-{stamp}", path.display()))
            })
            .find(|candidate| !candidate.exists())
            .expect("an unbounded suffix search always finds a free name");
        fs::copy(path, &target).map_err(|source| ConfigError::Io {
            path: target.clone(),
            source,
        })?;
        Ok(Some(target))
    }

    /// Returns the (possibly migrated) content to parse. On migration the
    /// original bytes are backed up next to the file BEFORE anything is
    /// rewritten.
    fn migrate_config(&self, path: &Path, raw: String) -> Result<String, ConfigError> {
        let mut doc: toml::Table =
            raw.parse()
                .map_err(|e: toml::de::Error| ConfigError::Parse {
                    path: path.to_owned(),
                    message: e.to_string(),
                })?;
        let found = match doc.get("schema_version") {
            Some(toml::Value::Integer(v)) => *v,
            _ => {
                return Err(ConfigError::MissingSchemaVersion {
                    path: path.to_owned(),
                })
            }
        };
        if found == i64::from(SCHEMA_VERSION) {
            return Ok(raw);
        }
        let unsupported = || ConfigError::UnsupportedSchemaVersion {
            path: path.to_owned(),
            found,
            supported: SCHEMA_VERSION,
        };
        let from = u32::try_from(found).map_err(|_| unsupported())?;
        if from > SCHEMA_VERSION {
            return Err(unsupported());
        }
        // Refuse before touching anything unless a full migration path exists.
        for v in from..SCHEMA_VERSION {
            if self.migrations.step(v).is_none() {
                return Err(unsupported());
            }
        }

        self.backup(path)?;

        for v in from..SCHEMA_VERSION {
            let step = self.migrations.step(v).expect("checked above");
            step(&mut doc).map_err(|message| ConfigError::MigrationFailed {
                path: path.to_owned(),
                from: v,
                message,
            })?;
        }
        doc.insert(
            "schema_version".to_owned(),
            toml::Value::Integer(i64::from(SCHEMA_VERSION)),
        );
        let migrated = to_toml(&doc, path)?;
        write_atomic(path, &migrated)?;
        Ok(migrated)
    }
}

/// Read a whole file as UTF-8 (BOM tolerated). `Ok(None)` when missing.
fn read_utf8(path: &Path) -> Result<Option<String>, ConfigError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.to_owned(),
                source,
            })
        }
    };
    let content = bytes.strip_prefix(UTF8_BOM).unwrap_or(&bytes);
    std::str::from_utf8(content)
        .map(|s| Some(s.to_owned()))
        .map_err(|_| ConfigError::NotUtf8 {
            path: path.to_owned(),
        })
}

/// Write via tmp file + rename in the same directory (std's rename replaces
/// existing files on Windows). Parent directories are created as needed.
fn write_atomic(path: &Path, contents: &str) -> Result<(), ConfigError> {
    let io = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{tmp_name}.tmp"));
    {
        let mut file = fs::File::create(&tmp).map_err(io)?;
        file.write_all(contents.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    fs::rename(&tmp, path).map_err(io)
}

/// Typed parse that records ignored keys as warnings instead of failing.
fn parse_lenient<T: DeserializeOwned>(
    path: &Path,
    content: &str,
) -> Result<Loaded<T>, ConfigError> {
    let mut warnings = Vec::new();
    let de = toml::Deserializer::new(content);
    let value = serde_ignored::deserialize(de, |ignored| {
        warnings.push(Warning {
            file: path.to_owned(),
            kind: WarningKind::UnknownKey {
                path: ignored.to_string(),
            },
        });
    })
    .map_err(|e: toml::de::Error| ConfigError::Parse {
        path: path.to_owned(),
        message: e.to_string(),
    })?;
    Ok(Loaded { value, warnings })
}

fn to_toml<T: Serialize>(value: &T, path: &Path) -> Result<String, ConfigError> {
    toml::to_string(value).map_err(|e| ConfigError::Serialize {
        path: path.to_owned(),
        message: e.to_string(),
    })
}

/// Parse in whichever format the resolved [`Source`] is. Both arms are lenient
/// about unknown keys, and both go through the SAME serde type — that is the
/// whole no-drift argument in two lines of code.
fn read_typed<T: DeserializeOwned>(source: &Source, raw: &str) -> Result<Loaded<T>, ConfigError> {
    match source.format {
        Format::Toml => parse_lenient(&source.path, raw),
        Format::Json => interop::parse_lenient(&source.path, raw),
    }
}

/// Render for the resolved [`Source`]. JSON is written PRETTY: an interop file
/// still ends up in a diff, in a chat window, or under someone's eyes.
fn render<T: Serialize>(value: &T, source: &Source) -> Result<String, ConfigError> {
    match source.format {
        Format::Toml => to_toml(value, &source.path),
        Format::Json => interop::to_json(value, JsonStyle::Pretty).map(|mut text| {
            text.push('\n');
            text
        }),
    }
}

/// Windows-safe file stem for a preset name: illegal characters map to `_`,
/// trailing dots/spaces are trimmed, reserved device stems get a `_` suffix.
/// Errors only when nothing usable remains.
pub fn preset_file_name(name: &str) -> Result<String, ConfigError> {
    const ILLEGAL: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_control() || ILLEGAL.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim_matches([' ', '.']);
    if stem.is_empty() {
        return Err(ConfigError::InvalidPresetName(name.to_owned()));
    }
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.ends_with(|c: char| c.is_ascii_digit()));
    Ok(if reserved {
        format!("{stem}_")
    } else {
        stem.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DeviceEntry, Settings, SlotEntry};
    use crate::games::{GameEntry, GameSlotEntry};
    use crate::test_util::TempDir;

    fn store(dir: &TempDir) -> Store {
        Store::new(ConfigRoot::at(dir.path()))
    }

    fn sample_config() -> ConfigFile {
        ConfigFile {
            schema_version: SCHEMA_VERSION,
            settings: Settings {
                block_keyboards: true,
                block_mice: true,
                mouse_move_deadzone: 7,
                starting_user_index: 2,
            },
            devices: vec![DeviceEntry {
                id: r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000".into(),
                alias: "P1 I-PAC".into(),
                backend: crate::config::Backend::Interception,
            }],
            slots: vec![SlotEntry {
                number: 1,
                keyboard: Some("P1 I-PAC".into()),
                mouse: None,
                preset: "street-fighter-p1".into(),
                persona: ksx_core::Persona::default(),
                socd: ksx_core::Socd::default(),
                macros: Default::default(),
            }],
        }
    }

    #[test]
    fn missing_files_load_as_clean_defaults() {
        let dir = TempDir::new("defaults");
        let store = store(&dir);
        let cfg = store.load_config().unwrap();
        assert_eq!(cfg.value, ConfigFile::default());
        assert!(cfg.warnings.is_empty());
        let games = store.load_games().unwrap();
        assert_eq!(games.value, GamesFile::default());
        assert!(games.warnings.is_empty());
        let presets = store.load_presets().unwrap();
        assert!(presets.value.is_empty());
        assert!(presets.warnings.is_empty());
        assert_eq!(store.load_preset("nope").unwrap(), None);
    }

    #[test]
    fn config_round_trips_through_disk() {
        let dir = TempDir::new("config-rt");
        let store = store(&dir);
        let cfg = sample_config();
        store.save_config(&cfg).unwrap();
        let loaded = store.load_config().unwrap();
        assert_eq!(loaded.value, cfg);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn games_round_trip_through_disk() {
        let dir = TempDir::new("games-rt");
        let store = store(&dir);
        let games = GamesFile {
            games: vec![GameEntry {
                title: "Steam".into(),
                notes: String::new(),
                path: r"C:\Program Files (x86)\Steam\steam.exe".into(),
                arguments: "-silent".into(),
                process_name: Some("portal2.exe".into()),
                launcher_grace_ms: None,
                block_keyboards: true,
                block_mice: false,
                slots: vec![GameSlotEntry {
                    number: 1,
                    user_index: Some(1),
                    keyboard: Some(r"HID\VID_D209&PID_0430&REV_0056&MI_00".into()),
                    mouse: None,
                    preset: "IPAC P1".into(),
                    persona: ksx_core::Persona::default(),
                    socd: ksx_core::Socd::default(),
                    macros: Default::default(),
                }],
            }],
        };
        store.save_games(&games).unwrap();
        let loaded = store.load_games().unwrap();
        assert_eq!(loaded.value, games);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn presets_round_trip_and_scan_sorted() {
        let dir = TempDir::new("presets-rt");
        let store = store(&dir);
        let a = PresetFile::from_core(&ksx_core::Preset::builtin_default());
        let mut b = a.clone();
        b.name = "alpha".into();
        store.save_preset(&a).unwrap();
        store.save_preset(&b).unwrap();

        let all = store.load_presets().unwrap();
        assert!(all.warnings.is_empty());
        // Sorted by file name: alpha.toml before default.toml.
        assert_eq!(all.value.len(), 2);
        assert_eq!(all.value[0].name, "alpha");
        assert_eq!(all.value[1].name, "default");
        assert_eq!(all.value[1], a);

        let one = store.load_preset("alpha").unwrap().unwrap();
        assert_eq!(one.value, b);
    }

    #[test]
    fn unknown_keys_warn_with_dotted_paths() {
        let dir = TempDir::new("lenient");
        let store = store(&dir);
        std::fs::write(
            store.root().config_path(),
            r#"
schema_version = 1
future_option = "ignored"

[settings]
block_keyboards = false
frobnicate = 3

[[device]]
id = "USB\\X"
alias = "p1"
extra = true
"#,
        )
        .unwrap();
        let loaded = store.load_config().unwrap();
        assert!(!loaded.value.settings.block_keyboards);
        assert_eq!(loaded.value.devices.len(), 1);
        let paths: Vec<&str> = loaded
            .warnings
            .iter()
            .map(|w| match &w.kind {
                WarningKind::UnknownKey { path } => path.as_str(),
                other => panic!("unexpected warning {other:?}"),
            })
            .collect();
        assert_eq!(
            paths,
            vec!["future_option", "settings.frobnicate", "device.0.extra"]
        );
    }

    #[test]
    fn corrupt_preset_is_skipped_with_warning() {
        let dir = TempDir::new("bad-preset");
        let store = store(&dir);
        let good = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        store.save_preset(&good).unwrap();
        std::fs::create_dir_all(store.root().presets_dir()).unwrap();
        std::fs::write(
            store.root().presets_dir().join("broken.toml"),
            "name = [not toml",
        )
        .unwrap();

        let all = store.load_presets().unwrap();
        assert_eq!(all.value.len(), 1);
        assert_eq!(all.value[0].name, "empty");
        assert_eq!(all.warnings.len(), 1);
        assert!(matches!(
            all.warnings[0].kind,
            WarningKind::SkippedPreset { .. }
        ));
        // The broken file is preserved for the user to inspect.
        assert!(store.root().presets_dir().join("broken.toml").exists());
    }

    fn fixed_clock() -> Timestamp {
        Timestamp {
            year: 2026,
            month: 8,
            day: 3,
            hour: 12,
            minute: 34,
            second: 56,
        }
    }

    fn rename_migration(doc: &mut toml::Table) -> Result<(), String> {
        // Fictional v0: settings.block was split into block_keyboards/mice.
        if let Some(toml::Value::Table(settings)) = doc.get_mut("settings") {
            if let Some(v) = settings.remove("block") {
                settings.insert("block_keyboards".into(), v);
            }
        }
        Ok(())
    }

    #[test]
    fn migration_backs_up_before_rewriting() {
        let dir = TempDir::new("migrate");
        let mut migrations = Migrations::none();
        migrations.register(0, rename_migration);
        let store = Store::new(ConfigRoot::at(dir.path()))
            .with_clock(fixed_clock)
            .with_migrations(migrations);

        let original = "schema_version = 0\n\n[settings]\nblock = false\n";
        std::fs::write(store.root().config_path(), original).unwrap();

        let loaded = store.load_config().unwrap();
        assert!(!loaded.value.settings.block_keyboards);
        assert_eq!(loaded.value.schema_version, SCHEMA_VERSION);

        let backup = dir.path().join("config.toml.bak-20260803-123456");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

        // The file on disk is now current; a second load neither migrates nor
        // writes another backup.
        let again = store.load_config().unwrap();
        assert_eq!(again.value, loaded.value);
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".bak-")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn unknown_schema_versions_are_refused() {
        let dir = TempDir::new("versions");
        let store = store(&dir);

        std::fs::write(store.root().config_path(), "schema_version = 99\n").unwrap();
        assert!(matches!(
            store.load_config(),
            Err(ConfigError::UnsupportedSchemaVersion { found: 99, .. })
        ));

        // v0 with no registered migration path: refused, file untouched.
        std::fs::write(store.root().config_path(), "schema_version = 0\n").unwrap();
        assert!(matches!(
            store.load_config(),
            Err(ConfigError::UnsupportedSchemaVersion { found: 0, .. })
        ));

        std::fs::write(store.root().config_path(), "[settings]\n").unwrap();
        assert!(matches!(
            store.load_config(),
            Err(ConfigError::MissingSchemaVersion { .. })
        ));
    }

    #[test]
    fn saves_are_atomic_and_leave_no_tmp_files() {
        let dir = TempDir::new("atomic");
        let store = store(&dir);
        store.save_config(&sample_config()).unwrap();
        // Overwrite: second save wins, no residue.
        let mut cfg2 = sample_config();
        cfg2.settings.mouse_move_deadzone = 3;
        store.save_config(&cfg2).unwrap();
        assert_eq!(store.load_config().unwrap().value, cfg2);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.toml"]);
    }

    #[test]
    fn utf8_bom_tolerated_and_invalid_utf8_refused() {
        let dir = TempDir::new("utf8");
        let store = store(&dir);
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"schema_version = 1\n");
        std::fs::write(store.root().config_path(), &bytes).unwrap();
        assert_eq!(store.load_config().unwrap().value, ConfigFile::default());

        std::fs::write(store.root().config_path(), [0xFF, 0xFE, 0x00, 0x41]).unwrap();
        assert!(matches!(
            store.load_config(),
            Err(ConfigError::NotUtf8 { .. })
        ));
    }

    #[test]
    fn preset_file_names_are_windows_safe() {
        assert_eq!(preset_file_name("IPAC P1").unwrap(), "IPAC P1");
        assert_eq!(preset_file_name("a/b:c*d").unwrap(), "a_b_c_d");
        assert_eq!(preset_file_name("dots...").unwrap(), "dots");
        assert_eq!(preset_file_name("CON").unwrap(), "CON_");
        assert_eq!(preset_file_name("com1").unwrap(), "com1_");
        assert!(matches!(
            preset_file_name("..."),
            Err(ConfigError::InvalidPresetName(_))
        ));
        assert!(matches!(
            preset_file_name(""),
            Err(ConfigError::InvalidPresetName(_))
        ));
    }

    // ---- JSON interop on the load/save path (see `crate::interop`) --------

    fn write_json<T: Serialize>(path: &Path, value: &T) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, interop::to_json(value, JsonStyle::Pretty).unwrap()).unwrap();
    }

    /// A root spelled entirely in JSON loads exactly like a TOML one, and
    /// saving keeps it JSON — the format a file HAS is the format it keeps.
    #[test]
    fn a_json_only_root_loads_and_saves_as_json() {
        let dir = TempDir::new("json-root");
        let store = store(&dir);
        let cfg = sample_config();
        let preset = PresetFile::from_core(&ksx_core::Preset::builtin_default());
        write_json(&store.root().config_json_path(), &cfg);
        write_json(&store.root().games_json_path(), &GamesFile::default());
        write_json(&store.root().presets_dir().join("default.json"), &preset);

        assert_eq!(store.load_config().unwrap().value, cfg);
        assert_eq!(store.load_games().unwrap().value, GamesFile::default());
        assert_eq!(store.load_presets().unwrap().value, vec![preset.clone()]);
        assert_eq!(store.load_preset("default").unwrap().unwrap().value, preset);

        // Saving preserves the format, and writes no TOML twin.
        let mut edited = cfg.clone();
        edited.settings.mouse_move_deadzone = 1;
        let path = store.save_config(&edited).unwrap();
        assert_eq!(path, store.root().config_json_path());
        assert!(!store.root().config_path().exists());
        assert_eq!(store.load_config().unwrap().value, edited);
        assert_eq!(
            store.save_preset(&preset).unwrap(),
            store.root().presets_dir().join("default.json")
        );
    }

    /// Both spellings side by side: **TOML wins, JSON is ignored**, and the
    /// ignored file is NAMED. Never a merge — see `crate::interop`.
    #[test]
    fn a_canonical_toml_shadows_its_json_twin() {
        let dir = TempDir::new("shadow");
        let store = store(&dir);
        let canonical = sample_config();
        store.save_config(&canonical).unwrap();
        let mut impostor = canonical.clone();
        impostor.settings.mouse_move_deadzone = 12;
        write_json(&store.root().config_json_path(), &impostor);

        let loaded = store.load_config().unwrap();
        assert_eq!(loaded.value, canonical, "the TOML must win");
        assert_eq!(loaded.warnings.len(), 1);
        match &loaded.warnings[0].kind {
            WarningKind::ShadowedByCanonicalToml { toml } => {
                assert_eq!(*toml, store.root().config_path());
                assert_eq!(loaded.warnings[0].file, store.root().config_json_path());
            }
            other => panic!("unexpected warning {other:?}"),
        }
        // ...and a save still lands on the canonical file, not the shadow.
        assert_eq!(
            store.save_config(&canonical).unwrap(),
            store.root().config_path()
        );

        // Same rule per PRESET name, case-insensitively (Windows file names).
        let preset = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        store.save_preset(&preset).unwrap();
        let mut other = preset.clone();
        other.bindings.clear();
        write_json(&store.root().presets_dir().join("EMPTY.json"), &other);
        let presets = store.load_presets().unwrap();
        assert_eq!(presets.value, vec![preset], "one preset, not two");
        assert!(presets
            .warnings
            .iter()
            .any(|w| matches!(w.kind, WarningKind::ShadowedByCanonicalToml { .. })));
    }

    /// A directory holding both spellings of DIFFERENT presets loads both.
    #[test]
    fn json_and_toml_presets_coexist_when_they_are_different_presets() {
        let dir = TempDir::new("mixed-presets");
        let store = store(&dir);
        let mut a = PresetFile::from_core(&ksx_core::Preset::builtin_default());
        a.name = "alpha".into();
        let mut b = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        b.name = "beta".into();
        store.save_preset(&a).unwrap();
        write_json(&store.root().presets_dir().join("beta.json"), &b);

        let loaded = store.load_presets().unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.value, vec![a, b]);
    }

    /// A `.json` config is not the migration substrate: an off-schema one is
    /// refused with the same structured error the TOML path produces.
    #[test]
    fn a_json_config_at_another_schema_is_refused() {
        let dir = TempDir::new("json-schema");
        let store = store(&dir);
        fs::write(store.root().config_json_path(), r#"{"schema_version":99}"#).unwrap();
        assert!(matches!(
            store.load_config(),
            Err(ConfigError::UnsupportedSchemaVersion { found: 99, .. })
        ));
        fs::write(store.root().config_json_path(), r#"{"settings":{}}"#).unwrap();
        assert!(matches!(
            store.load_config(),
            Err(ConfigError::MissingSchemaVersion { .. })
        ));
    }

    /// A corrupt JSON preset is skipped like a corrupt TOML one — one bad file
    /// must not take down the whole load.
    #[test]
    fn a_corrupt_json_preset_is_skipped_with_a_warning() {
        let dir = TempDir::new("bad-json-preset");
        let store = store(&dir);
        let good = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        store.save_preset(&good).unwrap();
        fs::write(store.root().presets_dir().join("broken.json"), "{not json").unwrap();

        let loaded = store.load_presets().unwrap();
        assert_eq!(loaded.value, vec![good]);
        assert!(loaded
            .warnings
            .iter()
            .any(|w| matches!(w.kind, WarningKind::SkippedPreset { .. })));
        assert!(store.root().presets_dir().join("broken.json").exists());
    }

    /// The backup helper every overwriting verb shares: the mapper's name
    /// shape, and a collision suffix so one second can hold two of them.
    #[test]
    fn backups_are_timestamped_and_never_collide() {
        let dir = TempDir::new("store-backup");
        let store = store(&dir).with_clock(fixed_clock);
        let path = dir.path().join("config.toml");
        assert_eq!(store.backup(&path).unwrap(), None, "nothing to copy");

        fs::write(&path, "schema_version = 1\n").unwrap();
        let first = store.backup(&path).unwrap().unwrap();
        assert!(
            first.ends_with("config.toml.bak-20260803-123456"),
            "{first:?}"
        );
        assert_eq!(fs::read_to_string(&first).unwrap(), "schema_version = 1\n");
        let second = store.backup(&path).unwrap().unwrap();
        assert!(
            second.ends_with("config.toml.bak-20260803-123456-2"),
            "{second:?}"
        );
        assert!(first.exists() && second.exists());
    }

    /// Backups are invisible to the preset loader (their extension is
    /// `.bak-…`, which is neither `toml` nor `json`).
    #[test]
    fn backups_do_not_load_as_presets() {
        let dir = TempDir::new("bak-invisible");
        let store = store(&dir).with_clock(fixed_clock);
        let preset = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        let path = store.save_preset(&preset).unwrap();
        store.backup(&path).unwrap().unwrap();
        assert_eq!(store.load_presets().unwrap().value, vec![preset]);
    }

    #[test]
    fn timestamp_civil_conversion_is_exact() {
        let epoch = Timestamp::from_unix(0);
        assert_eq!(epoch.backup_suffix(), "19700101-000000");
        // 2000-02-29 00:00:00Z (leap day in a leap century year).
        assert_eq!(
            Timestamp::from_unix(951_782_400).backup_suffix(),
            "20000229-000000"
        );
        // 2100-01-01 00:00:00Z (2100 is NOT a leap year).
        assert_eq!(
            Timestamp::from_unix(4_102_444_800).backup_suffix(),
            "21000101-000000"
        );
        assert_eq!(
            Timestamp::from_unix(4_102_444_800 - 1).backup_suffix(),
            "20991231-235959"
        );
    }
}
