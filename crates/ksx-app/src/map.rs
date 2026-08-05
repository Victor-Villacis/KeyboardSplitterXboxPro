//! `ksx map` — bind one preset function to one panel key, from a shell.
//!
//! The CLI face of [`crate::mapping::apply`] (the pipe's `map` verb and, over
//! it, Studio's mapper are the other two faces — one writer, three surfaces).
//!
//! Exit codes: 0 = written, 1 = error (I/O, unreadable config),
//! 2 = refused (unknown preset/function/key, or a conflict without --force —
//! nothing was written).

use crate::mapping::{self, MapError, MapSpec};

pub const EXIT_REFUSED: i32 = 2;

pub struct Options {
    pub preset: String,
    pub function: String,
    /// `None` with `clear` = clear; `Some` binds. Clap guarantees exactly one.
    pub key: Option<String>,
    pub force: bool,
    pub json: bool,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    let store = ksx_config::Store::new(root);
    let spec = MapSpec {
        preset: options.preset,
        function: options.function,
        key: options.key,
        force: options.force,
    };
    match mapping::apply(&store, &spec) {
        Ok(applied) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "path": applied.path.display().to_string(),
                        "preset": applied.preset,
                        "function": applied.function,
                        "key": applied.key,
                        "stolen_from": applied.stolen_from,
                        "conflicts": mapping::conflicts_json(&applied.overridden),
                    })
                );
            } else {
                println!("{}", applied.message());
                println!("wrote {}", applied.path.display());
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
