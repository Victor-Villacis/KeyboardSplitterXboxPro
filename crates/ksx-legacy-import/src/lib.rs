//! ksx-legacy-import -- reads the legacy app's UTF-16 XML and emits ksx TOML.
//!
//! Import fidelity is bit-exact against the legacy ID tables
//! (`legacy/VirtualXbox/Enums/*.cs`): XboxButton 0x0010..0x8000, triggers
//! 0x10000/0x20000, axes 1/2/4/8 with signed 16-bit values, dpad flags, and the
//! flat XboxCustomFunction space (expanded into plain [`ksx_core::Binding`]s).
//! Also honors the v1-schema upgrader semantics (`LeftTrigger`→`Left`,
//! `<pov>`→dpad, named enum attributes) from
//! `legacy/KeyboardSplitter/Presets/PresetUpgrader.cs` -- applied transparently
//! when v1 markers are detected.
//!
//! Unmappable entries never abort an import: they produce precise
//! [`Warning`]s (file, preset, entry, reason) and are skipped.
//!
//! Golden-file tests run against the real cabinet corpus in `tests/fixtures/`.
//! XML is import-only; no XML writer will ever exist here.

use std::fs;
use std::path::{Path, PathBuf};

mod decode;
mod games;
mod presets;
mod report;
mod settings;
mod xml;

pub use decode::decode_xml;
pub use games::{parse_games, ParsedGames};
pub use presets::{parse_presets, ParsedPresets};
pub use report::{json_escape, ImportReport, Warning};
pub use settings::{parse_settings, ParsedSettings};

/// Legacy file names, exactly as the legacy app wrote them (working-directory
/// relative in legacy; here resolved against the import directory).
pub const PRESETS_XML: &str = "splitter_presets.xml";
pub const GAMES_XML: &str = "splitter_games.xml";
pub const SETTINGS_XML: &str = "splitter_settings.xml";

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("legacy directory '{0}' does not exist")]
    MissingDir(PathBuf),
    #[error(
        "no legacy files found in '{0}' (expected {PRESETS_XML}, {GAMES_XML} or {SETTINGS_XML})"
    )]
    NoLegacyFiles(PathBuf),
    #[error("failed to read '{path}': {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write '{path}': {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("existing '{path}' could not be parsed -- refusing to overwrite it: {source}")]
    ExistingConfig {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("TOML serialization failed: {0}")]
    Toml(#[from] toml::ser::Error),
}

/// Everything one import run produced, ready to render/write as TOML.
pub struct LegacyImport {
    /// From `splitter_settings.xml`; `None` when that file was absent
    /// (config.toml is then left untouched).
    pub settings: Option<ksx_config::Settings>,
    pub presets: Vec<ksx_config::PresetFile>,
    /// From `splitter_games.xml`; `None` when that file was absent.
    pub games: Option<ksx_config::GamesFile>,
    pub report: ImportReport,
}

/// One output file, rendered as TOML. `path` is relative to the ksx config
/// root and uses `/` separators (e.g. `presets/IPAC P1.toml`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedFile {
    pub path: String,
    pub content: String,
}

/// Import all legacy XML files found in `dir`. Individual missing files are
/// skipped (recorded in [`ImportReport::skipped`]); it is an error only when
/// the directory is missing or contains none of the three files.
pub fn import_dir(dir: &Path) -> Result<LegacyImport, ImportError> {
    if !dir.is_dir() {
        return Err(ImportError::MissingDir(dir.to_owned()));
    }

    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut read = |name: &str| -> Result<Option<String>, ImportError> {
        let path = dir.join(name);
        if !path.is_file() {
            skipped.push(name.to_owned());
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|source| ImportError::Read {
            path: path.clone(),
            source,
        })?;
        let (text, had_errors) = decode_xml(&bytes);
        if had_errors {
            warnings.push(Warning::file_level(
                name,
                "malformed byte sequences were replaced during decoding",
            ));
        }
        Ok(Some(text))
    };

    let presets_text = read(PRESETS_XML)?;
    let games_text = read(GAMES_XML)?;
    let settings_text = read(SETTINGS_XML)?;
    if presets_text.is_none() && games_text.is_none() && settings_text.is_none() {
        return Err(ImportError::NoLegacyFiles(dir.to_owned()));
    }

    let presets = match &presets_text {
        Some(text) => {
            let parsed = parse_presets(text, PRESETS_XML);
            warnings.extend(parsed.warnings);
            parsed.presets
        }
        None => Vec::new(),
    };
    let games = games_text.map(|text| {
        let parsed = parse_games(&text, GAMES_XML);
        warnings.extend(parsed.warnings);
        parsed.games
    });
    let settings = settings_text.map(|text| {
        let parsed = parse_settings(&text, SETTINGS_XML);
        warnings.extend(parsed.warnings);
        parsed.settings
    });

    let report = ImportReport {
        presets: presets.len(),
        games: games.as_ref().map_or(0, |g| g.games.len()),
        skipped,
        warnings,
    };
    Ok(LegacyImport {
        settings,
        presets,
        games,
        report,
    })
}

impl LegacyImport {
    /// Render every output file as TOML, without touching the filesystem
    /// (drives `--dry-run` and the golden tests).
    pub fn rendered_files(&self) -> Result<Vec<RenderedFile>, ImportError> {
        let mut out = Vec::new();
        if let Some(settings) = &self.settings {
            let config = ksx_config::ConfigFile {
                settings: settings.clone(),
                ..Default::default()
            };
            out.push(RenderedFile {
                path: "config.toml".to_owned(),
                content: toml::to_string(&config)?,
            });
        }
        if let Some(games) = &self.games {
            out.push(RenderedFile {
                path: "games.toml".to_owned(),
                content: toml::to_string(games)?,
            });
        }
        for preset in &self.presets {
            out.push(RenderedFile {
                path: format!("presets/{}.toml", sanitize_file_stem(&preset.name)),
                content: toml::to_string(preset)?,
            });
        }
        Ok(out)
    }

    /// Write all outputs under `config_root` (created if missing). Presets and
    /// games.toml are overwritten (the import is their source of truth); an
    /// existing config.toml keeps its `[[device]]`/`[[slot]]` entries and only
    /// its `[settings]` section is replaced. Returns the paths written.
    pub fn write_outputs(&self, config_root: &Path) -> Result<Vec<PathBuf>, ImportError> {
        let mut written = Vec::new();
        for file in self.rendered_files()? {
            let mut target = config_root.to_path_buf();
            for part in file.path.split('/') {
                target.push(part);
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|source| ImportError::Write {
                    path: parent.to_owned(),
                    source,
                })?;
            }
            let content = if file.path == "config.toml" && target.is_file() {
                merge_settings_into_existing(&target, self.settings.as_ref().expect("rendered"))?
            } else {
                file.content
            };
            fs::write(&target, content).map_err(|source| ImportError::Write {
                path: target.clone(),
                source,
            })?;
            written.push(target);
        }
        Ok(written)
    }
}

/// Replace only the `[settings]` section of an existing config.toml,
/// preserving devices/slots. Refuses to overwrite a file it cannot parse.
fn merge_settings_into_existing(
    path: &Path,
    settings: &ksx_config::Settings,
) -> Result<String, ImportError> {
    let existing = fs::read_to_string(path).map_err(|source| ImportError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut config: ksx_config::ConfigFile =
        toml::from_str(&existing).map_err(|source| ImportError::ExistingConfig {
            path: path.to_owned(),
            source: Box::new(source),
        })?;
    config.settings = settings.clone();
    Ok(toml::to_string(&config)?)
}

/// Turn a preset name into a safe Windows file stem. Delegates to
/// [`ksx_config::preset_file_name`] so `Store::load_preset` derives the same
/// filename the importer wrote (divergent sanitizers = orphaned presets).
pub fn sanitize_file_stem(name: &str) -> String {
    ksx_config::preset_file_name(name).unwrap_or_else(|_| "preset".to_owned())
}

/// Default legacy directory: the folder the legacy KeyboardSplitter exe runs
/// from (it kept its XML next to the exe / in its working directory). On the
/// cabinet that is `C:\Users\Victor\KeyboardSplitter`; resolved via
/// `%USERPROFILE%` so it tracks the current user.
pub fn default_legacy_dir() -> PathBuf {
    match std::env::var_os("USERPROFILE") {
        Some(profile) => PathBuf::from(profile).join("KeyboardSplitter"),
        None => PathBuf::from(r"C:\Users\Victor\KeyboardSplitter"),
    }
}

/// Default ksx config root: `%APPDATA%\ksx` (the persistence layer will add
/// the portable `ksx.toml`-next-to-exe override; import always targets the
/// roaming root). `None` when `%APPDATA%` is unset.
pub fn default_config_root() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("ksx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stem_sanitization() {
        assert_eq!(sanitize_file_stem("IPAC P1"), "IPAC P1");
        assert_eq!(sanitize_file_stem("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_file_stem("dots.."), "dots");
        assert_eq!(sanitize_file_stem("  "), "preset");
        assert_eq!(sanitize_file_stem("tab\there"), "tab_here");
    }
}
