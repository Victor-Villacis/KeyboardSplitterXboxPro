//! ksx-config — TOML configuration schema and persistence.
//!
//! Layout: `%APPDATA%\ksx\{config.toml, presets\*.toml, games.toml, logs\}` with a
//! portable override (`ksx.toml` next to the exe wins — arcade cabs love portable).
//! Lenient parsing: unknown keys warn, never fail. Timestamped backup before any
//! migrating write. Implemented in M1.
