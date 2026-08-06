//! Config root discovery.
//!
//! Precedence (first match wins):
//! 1. **Portable** — a `ksx.toml` next to the running exe makes the exe's
//!    directory the config root and that very file the main config file
//!    (arcade cabs love portable installs).
//! 2. **Installed** — `%APPDATA%\ksx\` on Windows (the platform config dir
//!    elsewhere, via `directories`), main config file `config.toml`.
//!
//! In both modes, presets live in `<root>\presets\*.toml`, games in
//! `<root>\games.toml`, logs in `<root>\logs\`.
//!
//! Every config file has a JSON interop sibling (`config.json`, `games.json`,
//! `presets\<name>.json`) that ksx reads only when the canonical TOML is
//! absent. TOML wins; see [`crate::interop`] for why.
//!
//! [`ConfigRoot::discover`] performs the real lookup; [`ConfigRoot::resolve`]
//! is the same decision with every input injectable for tests.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// File whose presence next to the exe switches ksx into portable mode. It is
/// also the portable-mode main config file.
pub const PORTABLE_MARKER: &str = "ksx.toml";

/// A resolved configuration directory plus the naming convention inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigRoot {
    dir: PathBuf,
    portable: bool,
}

impl ConfigRoot {
    /// Installed-style root at an explicit directory (main file `config.toml`).
    /// This is the injectable entry point for tests and `--config-dir` flags.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            portable: false,
        }
    }

    /// Portable-style root (main file `ksx.toml`).
    pub fn portable(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            portable: true,
        }
    }

    /// The discovery decision, fully injectable: `exe_dir` is where the
    /// running exe lives (portable candidate), `installed` the fallback
    /// config dir. Returns `None` only when both are unavailable.
    pub fn resolve(exe_dir: Option<&Path>, installed: Option<PathBuf>) -> Option<Self> {
        if let Some(dir) = exe_dir {
            if dir.join(PORTABLE_MARKER).is_file() {
                return Some(Self::portable(dir));
            }
        }
        installed.map(Self::at)
    }

    /// Discover the real config root for this process (see module docs for
    /// precedence).
    pub fn discover() -> Result<Self, ConfigError> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        Self::resolve(exe_dir.as_deref(), installed_config_dir()).ok_or(ConfigError::NoConfigDir)
    }

    pub fn is_portable(&self) -> bool {
        self.portable
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Main config file: `<root>\config.toml`, or `<root>\ksx.toml` in
    /// portable mode.
    pub fn config_path(&self) -> PathBuf {
        let name = if self.portable {
            PORTABLE_MARKER
        } else {
            "config.toml"
        };
        self.dir.join(name)
    }

    /// The JSON interop sibling of [`ConfigRoot::config_path`]: `config.json`,
    /// or `ksx.json` in portable mode. It is the file ksx reads only when the
    /// canonical TOML is absent — see [`crate::interop`] for the precedence
    /// rule and why the two are never merged.
    pub fn config_json_path(&self) -> PathBuf {
        let name = if self.portable {
            "ksx.json"
        } else {
            "config.json"
        };
        self.dir.join(name)
    }

    pub fn presets_dir(&self) -> PathBuf {
        self.dir.join("presets")
    }

    pub fn games_path(&self) -> PathBuf {
        self.dir.join("games.toml")
    }

    /// JSON interop sibling of [`ConfigRoot::games_path`].
    pub fn games_json_path(&self) -> PathBuf {
        self.dir.join("games.json")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
}

/// `%APPDATA%\ksx` on Windows; the platform config dir elsewhere.
pub fn installed_config_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("ksx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn portable_marker_wins_over_installed() {
        let exe_dir = TempDir::new("portable");
        let installed = TempDir::new("installed");
        std::fs::write(exe_dir.path().join(PORTABLE_MARKER), "schema_version = 1\n").unwrap();

        let root = ConfigRoot::resolve(Some(exe_dir.path()), Some(installed.path().to_path_buf()))
            .unwrap();
        assert!(root.is_portable());
        assert_eq!(root.dir(), exe_dir.path());
        assert_eq!(root.config_path(), exe_dir.path().join("ksx.toml"));
    }

    #[test]
    fn no_marker_falls_back_to_installed() {
        let exe_dir = TempDir::new("no-marker");
        let installed = TempDir::new("installed2");

        let root = ConfigRoot::resolve(Some(exe_dir.path()), Some(installed.path().to_path_buf()))
            .unwrap();
        assert!(!root.is_portable());
        assert_eq!(root.dir(), installed.path());
        assert_eq!(root.config_path(), installed.path().join("config.toml"));
    }

    #[test]
    fn nothing_available_is_none() {
        assert_eq!(ConfigRoot::resolve(None, None), None);
    }

    #[test]
    fn layout_paths() {
        let root = ConfigRoot::at(r"C:\cfg\ksx");
        assert_eq!(
            root.config_path(),
            Path::new(r"C:\cfg\ksx").join("config.toml")
        );
        assert_eq!(root.presets_dir(), Path::new(r"C:\cfg\ksx").join("presets"));
        assert_eq!(
            root.games_path(),
            Path::new(r"C:\cfg\ksx").join("games.toml")
        );
        assert_eq!(root.logs_dir(), Path::new(r"C:\cfg\ksx").join("logs"));

        let portable = ConfigRoot::portable(r"D:\cab");
        assert_eq!(
            portable.config_path(),
            Path::new(r"D:\cab").join("ksx.toml")
        );
        assert_eq!(portable.presets_dir(), Path::new(r"D:\cab").join("presets"));
    }

    /// Every canonical file has a JSON interop sibling in the same place,
    /// portable mode included.
    #[test]
    fn json_interop_siblings_sit_next_to_the_canonical_files() {
        let root = ConfigRoot::at(r"C:\cfg\ksx");
        assert_eq!(
            root.config_json_path(),
            Path::new(r"C:\cfg\ksx").join("config.json")
        );
        assert_eq!(
            root.games_json_path(),
            Path::new(r"C:\cfg\ksx").join("games.json")
        );

        let portable = ConfigRoot::portable(r"D:\cab");
        assert_eq!(
            portable.config_json_path(),
            Path::new(r"D:\cab").join("ksx.json")
        );
    }
}
