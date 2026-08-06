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
    #[error(
        "{0} — a macro step's duration is `ms = <n>` or `frames = <n>` (60 Hz), exactly one of          them; a step with no duration would be an input no game could sample"
    )]
    MacroStepDuration(String),
    #[error(
        "{0} — a turbo macro's rate is `turbo_hz = <n>` or `gap_ms = <n>`, exactly one of them; \
         guessing a rate would put an auto-fire on the wire that nobody asked for \
         (docs/INPUT-TRANSFORMS.md §1c)"
    )]
    MacroTurboRate(String),
    #[error("no macro called '{0}' is defined in this preset (add a [macros.{0}] table)")]
    UnknownMacro(String),
    #[error(
        "'macro.{0}' cannot carry a when/unless guard: a macro is started by a key, and a \
         chord that starts a sequence is not implemented (docs/INPUT-TRANSFORMS.md §1c)"
    )]
    GuardedMacroTrigger(String),
    #[error(
        "'macro.{0}' cannot carry `turbo_hz`: a macro repeats by saying `repeat = \"turbo\"` in \
         its own [macros.{0}] table, and a second spelling for the same thing would make \
         \"which one runs\" something a reader has to remember (docs/INPUT-TRANSFORMS.md §3)"
    )]
    TurboOnMacroTrigger(String),
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
    /// JSON interop: rendering a document failed. No path, because the usual
    /// destination is stdout.
    #[error("cannot render the JSON document: {message}")]
    JsonRender { message: String },
    #[error(
        "{path}: this JSON has no 'ksx_interop' field, so ksx cannot tell what it is; \
         say so with --what config|games|presets"
    )]
    UntaggedJson { path: PathBuf },
    #[error(
        "{path}: ksx_interop {found} is not supported by this build (current: {supported}); \
         a newer ksx wrote it"
    )]
    UnsupportedInteropVersion {
        path: PathBuf,
        found: u64,
        supported: u32,
    },
    #[error(
        "no usable configuration directory: no ksx.toml next to the exe and \
         no user config directory available"
    )]
    NoConfigDir,
}
