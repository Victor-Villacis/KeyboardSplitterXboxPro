//! ksx-config — TOML configuration schema and persistence.
//!
//! Layout: `%APPDATA%\ksx\{config.toml, presets\*.toml, games.toml, logs\}` with a
//! portable override (`ksx.toml` next to the exe wins — arcade cabs love portable).
//! Lenient parsing: unknown keys warn, never fail. Timestamped backup before any
//! migrating write.
//!
//! Module map:
//! - [`config`] — `config.toml` (`schema_version`, `[settings]`, `[[device]]`, `[[slot]]`)
//! - [`preset`] — `presets/*.toml` (function-name → key-name bindings)
//! - [`games`] — `games.toml` (legacy `splitter_games.xml` content)
//! - [`function`] — the function-name vocabulary (`A`, `lt`, `"lx.-16384"`, `dpad.up`)
//! - [`paths`] — [`ConfigRoot`] discovery (portable override, `%APPDATA%\ksx`)
//! - [`store`] — [`Store`]: lenient loads with [`Warning`]s, atomic saves,
//!   schema migration with timestamped backups
//! - [`validate`] — structured cross-file [`Issue`]s
//! - [`error`] — [`ConfigError`]

pub mod config;
pub mod error;
pub mod function;
pub mod games;
pub mod paths;
pub mod persona_serde;
pub mod preset;
pub mod socd_serde;
pub mod store;
pub mod validate;

#[cfg(test)]
pub(crate) mod test_util;

pub use config::{Backend, ConfigFile, DeviceEntry, Settings, SlotEntry, SCHEMA_VERSION};
pub use error::ConfigError;
pub use function::{function_name, parse_function, CONSUME};
pub use games::{GameEntry, GameSlotEntry, GamesFile};
pub use paths::{installed_config_dir, ConfigRoot, PORTABLE_MARKER};
pub use preset::{BindingEntry, PresetFile};
pub use store::{
    preset_file_name, Loaded, MigrationStep, Migrations, Store, Timestamp, Warning, WarningKind,
};
pub use validate::{validate, validate_games, Issue};
