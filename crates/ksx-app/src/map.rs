//! `ksx map` — bind one preset function to one panel key, from a shell.
//!
//! The CLI face of [`crate::mapping::apply`] (the pipe's `map` verb and, over
//! it, Studio's mapper are the other two faces — one writer, three surfaces).
//! `--restore defaults|session-backup|latest-backup` is the same story for
//! [`crate::mapping::restore`] (pipe verb `map-restore`), and
//! `--list-backups` reads the timestamped backups every restore leaves behind.
//!
//! Exit codes: 0 = written, 1 = error (I/O, unreadable config),
//! 2 = refused (unknown preset/function/key, a CROSS-SLOT conflict without
//! --force, a `--move-from` that would unbind something the caller did not
//! mean, or a restore with nothing to restore — nothing was written). A key
//! already used by another control of the SAME preset is not a refusal at all:
//! it is a multi-bind, written and reported (`also_drives`).

use crate::mapping::{self, MapError, MapSpec, RestoreKind};

pub const EXIT_REFUSED: i32 = 2;

/// What `ksx map` was asked to do.
pub enum Action {
    Bind {
        function: String,
        /// `None` = clear (clap guarantees `--clear` was given).
        key: Option<String>,
        /// Bind anyway despite a CROSS-SLOT duplicate (removes nothing).
        force: bool,
        /// `--move-from B`: take the key off exactly that one function.
        move_from: Option<String>,
        /// CHORD: keys that must ALL be held too (`--when B,C`).
        when: Vec<String>,
        /// CHORD: keys that must NOT be held (`--unless LeftShift`).
        unless: Vec<String>,
    },
    Restore(RestoreKind),
    /// Unbind every function of the preset (a timestamped backup first).
    ClearAll,
    /// Read-only: what timestamped backups exist for this preset.
    ListBackups,
}

pub struct Options {
    pub preset: String,
    pub action: Action,
    pub json: bool,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    let store = ksx_config::Store::new(root);

    // The read-only verb answers before anything else: it writes nothing, so
    // it shares none of the write path's reporting.
    if let Action::ListBackups = options.action {
        return list_backups(&store, &options.preset, options.json);
    }

    let outcome = match options.action {
        Action::Bind {
            function,
            key,
            force,
            move_from,
            when,
            unless,
        } => mapping::apply(
            &store,
            &MapSpec {
                preset: options.preset,
                function,
                key,
                force,
                move_from,
                when,
                unless,
            },
        )
        .map(|applied| {
            let json = serde_json::json!({
                "ok": true,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
                "function": applied.function,
                "key": applied.key,
                // Guards: empty arrays for an ordinary binding, so a reader
                // can treat "chord" as `when.length || unless.length`.
                "when": applied.when,
                "unless": applied.unless,
                "chord": applied.chord(),
                // MULTI-BIND: the other controls of this preset the key drives
                // now. Information, never an error — the write succeeded and
                // none of them was touched (docs/INPUT-TRANSFORMS.md §1a).
                "also_drives": applied.also_drives,
                // What --move-from unbound, or null. The ONLY field that can
                // ever report a binding this verb removed on its own.
                "moved_from": mapping::moved_from_json(applied.moved_from.as_ref()),
                "conflicts": mapping::conflicts_json(&applied.overridden),
                "flash": mapping::flash_json(&applied.flash),
            });
            (applied.message(), applied.path, json)
        }),
        Action::ClearAll => mapping::clear_all(&store, &options.preset).map(whole_preset_json),
        Action::Restore(kind) => {
            mapping::restore(&store, &options.preset, kind).map(whole_preset_json)
        }
        // Handled above; kept exhaustive so a new action cannot fall through.
        Action::ListBackups => unreachable!("list-backups answered above"),
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
                    | MapError::UnknownMacro { .. }
                    | MapError::UnknownKey(_)
                    | MapError::InvalidGuard(_)
                    | MapError::BadMoveFrom(_)
                    | MapError::Conflicts { .. }
                    | MapError::NoSessionBackup { .. }
                    | MapError::NoBackup { .. }
                    | MapError::BadBackup { .. }
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
                        "code": error_code(&err),
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

/// The `--json` shape shared by every WHOLE-PRESET write (restore ×3, clear
/// all). `mode` and `wrote` are the honest pair: the machine word and the
/// sentence a human was shown before the write; `backup` is the road home.
fn whole_preset_json(
    applied: mapping::AppliedRestore,
) -> (String, std::path::PathBuf, serde_json::Value) {
    let json = serde_json::json!({
        "ok": true,
        "path": applied.path.display().to_string(),
        "preset": applied.preset,
        "mode": applied.kind.as_str(),
        "wrote": applied.kind.destination(),
        "backup": applied.backup.as_ref().map(|b| serde_json::json!({
            "stamp": b.stamp,
            "label": b.label(),
            "path": b.path.display().to_string(),
        })),
    });
    (applied.message(), applied.path, json)
}

/// The stable `--json` `code` for every refusal. One place, so the CLI, the
/// pipe and docs/CONTROL-SURFACE.md cannot drift.
pub fn error_code(err: &MapError) -> &'static str {
    match err {
        MapError::UnknownPreset { .. } => "unknown-preset",
        MapError::UnknownFunction(_) => "unknown-function",
        MapError::UnknownMacro { .. } => "unknown-macro",
        MapError::UnknownKey(_) => "unknown-key",
        MapError::InvalidGuard(_) => "invalid-guard",
        MapError::BadMoveFrom(_) => "bad-move-from",
        MapError::Conflicts { .. } => "conflict",
        MapError::NoSessionBackup { .. } => "no-session-backup",
        MapError::NoBackup { .. } => "no-backup",
        MapError::BadBackup { .. } => "bad-backup",
        MapError::Config(_) => "config-error",
    }
}

/// `ksx map --list-backups --preset X [--json]` — the restore points on disk,
/// newest first. Read-only: exit 0 even when the list is empty (an empty list
/// is an answer, not a refusal).
fn list_backups(store: &ksx_config::Store, preset: &str, json: bool) -> anyhow::Result<()> {
    let backups = mapping::list_backups(store, preset)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "preset": preset,
                "backups": backups.iter().map(|b| serde_json::json!({
                    "stamp": b.stamp,
                    "label": b.label(),
                    "path": b.path.display().to_string(),
                })).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    if backups.is_empty() {
        println!(
            "no timestamped backups for \"{preset}\" — one is written before every \
             restore, so the first restore has nothing older to go back to"
        );
        return Ok(());
    }
    println!(
        "{} backup(s) for \"{preset}\", newest first:",
        backups.len()
    );
    for backup in &backups {
        println!("  {}  {}", backup.label(), backup.path.display());
    }
    println!("restore the newest with: ksx map --preset \"{preset}\" --restore latest-backup");
    Ok(())
}
