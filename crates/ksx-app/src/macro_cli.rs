//! `ksx macro` — write (or delete) a preset's WHOLE `[macros.<name>]` table
//! from a shell.
//!
//! The CLI face of [`crate::mapping::save_macro`]; the pipe's `map-macro` verb
//! and, over it, Studio's macro editor are the other two — one writer, three
//! surfaces, exactly as `ksx map` / `map` / the mapper are for bindings.
//!
//! # Why the body is JSON on stdin and not a flag per field
//!
//! A macro is a TIMELINE: an ordered list of steps, each holding a SET of
//! functions for a duration in one of two units, plus an opt-out per step and
//! three policies for the whole thing. Spelling that as flags would need
//! either a positional mini-language (`--step "dpad.down:50"`) or repeated
//! flags whose ORDER is load-bearing — both of which are worse than the TOML
//! they would produce, and neither of which an editor could round-trip.
//!
//! So the body arrives as JSON, in the SAME serde shape the preset file uses
//! (`ksx_config::MacroFile`): the config interop types, not a parallel schema.
//! What `ksx config export` prints for a macro is what this verb accepts.
//!
//! ```text
//! ksx macro --preset "IPAC P1" --name hadouken <<'JSON'
//! { "steps": [
//!     { "hold": ["dpad.down"],                 "ms": 50 },
//!     { "hold": ["dpad.down", "dpad.right"],   "ms": 50 },
//!     { "hold": ["dpad.right"],                "ms": 50 },
//!     { "hold": ["A"],                         "frames": 3 } ] }
//! JSON
//! ```
//!
//! Exit codes: 0 = written, 1 = error (I/O, unreadable config, unreadable
//! JSON), 2 = refused (unknown preset, a macro body validation names — an
//! unknown function in a `hold` set, no steps, a step with two duration units
//! or none — or a `--delete` for a macro the preset does not define; nothing
//! was written and no backup was taken).

use std::io::Read as _;
use std::path::PathBuf;

use crate::map::EXIT_REFUSED;
use crate::mapping::{self, MacroSpec, MapError};

pub struct Options {
    pub preset: String,
    pub name: String,
    /// Read the body from this file instead of stdin.
    pub from_json: Option<PathBuf>,
    /// Delete the table (and the `macro.<name>` trigger rows that would
    /// otherwise dangle) instead of writing one. No body is read.
    pub delete: bool,
    /// `--enable` / `--disable`: switch the existing table on or off and
    /// change nothing else. No body is read.
    pub set_enabled: Option<bool>,
    pub json: bool,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    let store = ksx_config::Store::new(root);

    // Both --delete and --enable/--disable are whole-table operations that
    // name the table and say nothing about its contents, so neither reads a
    // body: a toggle that had to be handed a step list could not promise the
    // steps came back unchanged.
    let body = if options.delete || options.set_enabled.is_some() {
        ksx_config::MacroFile::default()
    } else {
        read_body(options.from_json.as_deref())?
    };

    match mapping::save_macro(
        &store,
        &MacroSpec {
            preset: options.preset,
            name: options.name,
            body,
            delete: options.delete,
            set_enabled: options.set_enabled,
        },
    ) {
        Ok(applied) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "path": applied.path.display().to_string(),
                        "preset": applied.preset,
                        "name": applied.name,
                        "steps": applied.steps,
                        "total_ms": applied.total_ms,
                        "deleted": applied.deleted,
                        // Does it RUN? `toggled` says this write was only
                        // --enable/--disable and touched nothing else.
                        "enabled": applied.enabled,
                        "toggled": applied.toggled,
                        // The keys that START it. Unchanged by this verb —
                        // `ksx map --function macro.<name>` writes those —
                        // except on a delete, where they went with the table.
                        "triggers": applied.triggers,
                        // Advisories from a write that SUCCEEDED: a step below
                        // the sampling floor was raised, or runs as written and
                        // may be missed. Never silent, never fatal.
                        "warnings": applied.warnings,
                        "backup": applied.backup.as_ref().map(|b| serde_json::json!({
                            "stamp": b.stamp,
                            "label": b.label(),
                            "path": b.path.display().to_string(),
                        })),
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
                    | MapError::UnknownMacro { .. }
                    | MapError::BadMacro { .. }
            );
            if options.json {
                let problems = match &err {
                    MapError::BadMacro { problems, .. } => problems.clone(),
                    _ => Vec::new(),
                };
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "code": crate::map::error_code(&err),
                        "error": err.to_string(),
                        "problems": problems,
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

/// The macro body: `--from-json <FILE>`, or stdin.
///
/// The parse error is passed through with the shape spelled out beside it,
/// because "invalid type: string" on its own tells nobody which field.
fn read_body(from_json: Option<&std::path::Path>) -> anyhow::Result<ksx_config::MacroFile> {
    let (text, source) = match from_json {
        Some(path) => (
            std::fs::read_to_string(path)
                .map_err(|err| anyhow::anyhow!("cannot read {}: {err}", path.display()))?,
            path.display().to_string(),
        ),
        None => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            (text, "stdin".to_owned())
        }
    };
    if text.trim().is_empty() {
        anyhow::bail!(
            "no macro body on {source} — give it JSON: \
             {{\"steps\":[{{\"hold\":[\"dpad.down\"],\"ms\":50}}]}} \
             (or pass --delete to remove the macro)"
        );
    }
    serde_json::from_str(&text).map_err(|err| {
        anyhow::anyhow!(
            "{source} is not a macro body: {err}\n\
             shape: {{\"steps\":[{{\"hold\":[\"dpad.down\"],\"ms\":50,\"allow_short\":false}}],\
             \"on_release\":\"finish|abort\",\"retrigger\":\"ignore|restart\",\
             \"interrupt\":\"none|any-input|opposing\"}}\n\
             each step gives exactly one of \"ms\" or \"frames\" (frames are 60 Hz); \
             \"hold\" holds pad function names (A, lt, dpad.up, \"lx.min\"), and an \
             empty hold is a deliberate neutral gap"
        )
    })
}
