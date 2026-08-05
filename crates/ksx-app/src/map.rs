//! `ksx map` — bind one preset function to one panel key, from a shell.
//!
//! The CLI face of [`crate::mapping::apply`] (the pipe's `map` verb and, over
//! it, Studio's mapper are the other two faces — one writer, three surfaces).
//! `--restore defaults|session-backup` is the same story for
//! [`crate::mapping::restore`] (pipe verb `map-restore`).
//!
//! Exit codes: 0 = written, 1 = error (I/O, unreadable config),
//! 2 = refused (unknown preset/function/key, a conflict without --force, or a
//! restore with nothing to restore — nothing was written).

use crate::mapping::{self, MapError, MapSpec, RestoreKind};

pub const EXIT_REFUSED: i32 = 2;

/// What `ksx map` was asked to do.
pub enum Action {
    Bind {
        function: String,
        /// `None` = clear (clap guarantees `--clear` was given).
        key: Option<String>,
        force: bool,
    },
    Restore(RestoreKind),
}

pub struct Options {
    pub preset: String,
    pub action: Action,
    pub json: bool,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    let store = ksx_config::Store::new(root);
    let outcome = match options.action {
        Action::Bind {
            function,
            key,
            force,
        } => mapping::apply(
            &store,
            &MapSpec {
                preset: options.preset,
                function,
                key,
                force,
            },
        )
        .map(|applied| {
            let json = serde_json::json!({
                "ok": true,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
                "function": applied.function,
                "key": applied.key,
                "stolen_from": applied.stolen_from,
                "conflicts": mapping::conflicts_json(&applied.overridden),
            });
            (applied.message(), applied.path, json)
        }),
        Action::Restore(kind) => mapping::restore(&store, &options.preset, kind).map(|applied| {
            let json = serde_json::json!({
                "ok": true,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
            });
            (applied.message(), applied.path, json)
        }),
    };
    match outcome {
        Ok((message, path, json)) => {
            if options.json {
                println!("{json}");
            } else {
                println!("{message}");
                println!("wrote {}", path.display());
                println!(
                    "a running session applies it after `ksx session reload` \
                     (or the next start)"
                );
            }
            Ok(())
        }
        Err(err) => {
            let refusal = matches!(
                err,
                MapError::UnknownPreset { .. }
                    | MapError::UnknownFunction(_)
                    | MapError::UnknownKey(_)
                    | MapError::Conflicts { .. }
                    | MapError::NoSessionBackup { .. }
                    | MapError::BadSessionBackup { .. }
            );
            if options.json {
                let conflicts = match &err {
                    MapError::Conflicts { conflicts, .. } => mapping::conflicts_json(conflicts),
                    _ => serde_json::json!([]),
                };
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "code": match &err {
                            MapError::UnknownPreset { .. } => "unknown-preset",
                            MapError::UnknownFunction(_) => "unknown-function",
                            MapError::UnknownKey(_) => "unknown-key",
                            MapError::Conflicts { .. } => "conflict",
                            MapError::NoSessionBackup { .. } => "no-session-backup",
                            MapError::BadSessionBackup { .. } => "bad-session-backup",
                            MapError::Config(_) => "config-error",
                        },
                        "error": err.to_string(),
                        "conflicts": conflicts,
                    })
                );
            } else {
                eprintln!("{err}");
            }
            if refusal {
                std::process::exit(EXIT_REFUSED);
            }
            Err(err.into())
        }
    }
}
