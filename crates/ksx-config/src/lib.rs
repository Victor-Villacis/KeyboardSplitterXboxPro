//! ksx-config — TOML configuration schema and persistence.
//!
//! Layout: `%APPDATA%\ksx\{config.toml, presets\*.toml, games.toml, logs\}` with a
//! portable override (`ksx.toml` next to the exe wins — arcade cabs love portable).
//! Lenient parsing: unknown keys warn, never fail. Timestamped backup before any
//! migrating write.
//!
//! # Canonical TOML, interop JSON
//!
//! **TOML is canonical because it carries COMMENTS** — ksx config files are
//! annotated for the humans who maintain them and for the AI that reads them,
//! and JSON has no syntax that could keep those notes. **JSON exists for
//! machines**: preset sharing (M7), AI-generated configs (E5), anything that
//! wants a schema. Both formats go through the SAME serde types, so the two
//! cannot drift; where both spellings of a file exist, the TOML wins and the
//! JSON is ignored with a warning — never merged. The full argument, the
//! precedence rule and the one thing JSON does not get (migration) are in
//! [`interop`]; the CLI face is `ksx config export|import`.
//!
//! Module map:
//! - [`config`] — `config.toml` (`schema_version`, `[settings]`, `[[device]]`, `[[slot]]`)
//! - [`preset`] — `presets/*.toml` (function-name → key-name bindings)
//! - [`games`] — `games.toml` (legacy `splitter_games.xml` content)
//! - [`function`] — the function-name vocabulary (`A`, `lt`, `"lx.-16384"`, `dpad.up`)
//! - [`paths`] — [`ConfigRoot`] discovery (portable override, `%APPDATA%\ksx`)
//! - [`store`] — [`Store`]: lenient loads with [`Warning`]s, atomic saves,
//!   schema migration with timestamped backups, format preservation
//! - [`interop`] — JSON as a first-class interop format ([`Bundle`], [`Format`])
//! - [`validate`] — structured cross-file [`Issue`]s
//! - [`error`] — [`ConfigError`]

pub mod blocking_serde;
pub mod config;
pub mod error;
pub mod function;
pub mod games;
pub mod interop;
pub mod macro_serde;
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
pub use function::{
    function_name, macro_function_name, macro_name, parse_function, CONSUME, MACRO_PREFIX,
};
pub use games::{GameEntry, GameSlotEntry, GamesFile};
pub use interop::{Bundle, Format, JsonStyle, Part, INTEROP_VERSION};
pub use paths::{installed_config_dir, ConfigRoot, PORTABLE_MARKER};
pub use preset::{BindingEntry, MacroFile, MacroStepFile, PresetFile};
pub use store::{
    preset_file_name, Loaded, MigrationStep, Migrations, Source, Store, Timestamp, Warning,
    WarningKind,
};
pub use validate::{validate, validate_games, Issue};
