use std::path::PathBuf;

use ksx_core::InvalidSlotNumber;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("unknown function name '{0}' in preset bindings")]
    UnknownFunction(String),
    #[error(
        "unknown key name '{0}' (exact legacy InterceptionKey spelling required, \
         e.g. 'Eight', 'BackslashPipe', 'None')"
    )]
    UnknownKey(String),
    #[error("axis value '{0}' is not 'min', 'max' or a signed 16-bit integer")]
    InvalidAxisValue(String),
    #[error("unknown device alias '{0}' (no [[device]] entry has this alias)")]
    UnknownDeviceAlias(String),
    #[error(transparent)]
    InvalidSlotNumber(#[from] InvalidSlotNumber),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid UTF-8 (ksx config files are always UTF-8)")]
    NotUtf8 { path: PathBuf },
    #[error("cannot parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("cannot serialize {path}: {message}")]
    Serialize { path: PathBuf, message: String },
    #[error("{path}: missing or non-integer schema_version")]
    MissingSchemaVersion { path: PathBuf },
    #[error(
        "{path}: schema_version {found} is not supported by this build (current: {supported}); \
         a newer ksx probably wrote it"
    )]
    UnsupportedSchemaVersion {
        path: PathBuf,
        found: i64,
        supported: u32,
    },
    #[error("{path}: migration from schema_version {from} failed: {message}")]
    MigrationFailed {
        path: PathBuf,
        from: u32,
        message: String,
    },
    #[error("preset name '{0}' cannot be turned into a file name")]
    InvalidPresetName(String),
    #[error(
        "no usable configuration directory: no ksx.toml next to the exe and \
         no user config directory available"
    )]
    NoConfigDir,
}
