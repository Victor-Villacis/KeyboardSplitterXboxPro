//! `ksx run` — the end-to-end supervisor (M4).
//!
//! Resolves a plan ([`plan`]), then drives the capture → engine → output
//! pipeline ([`supervisor`]) with a latency histogram ([`latency`]).
//!
//! # Exit codes (stable, documented in `--help` and README)
//!
//! | code | meaning |
//! |---|---|
//! | 0 | clean stop — the Ctrl+Alt+Del escape, `--dry-run`, or Ctrl+C where it can be delivered |
//! | 1 | unexpected error |
//! | 2 | refused to start: invalid config, unknown `--game`, a missing driver, two keyboards sharing one hardware id, or any pad-plug failure (bus missing, no free slot, plug timeout, output thread died before the pads were ready). Nothing was plugged and no keyboard filter was ever set |
//! | 3 | started, then a runtime failure tore it down. Keyboards were released first |
//!
//! The 2/3 boundary is exactly "was a keyboard filter ever armed", not "which
//! component failed" — every pad-plug failure happens before capture is told to
//! capture anything, so it is always a 2.
//!
//! # Getting out (read this before the first cabinet session)
//!
//! **Ctrl+C cannot work once every keyboard is captured.** Interception
//! suppresses captured strokes below win32k, so Windows never generates a
//! `CTRL_C_EVENT` and the console handler installed by [`crate::ctrl_c`] never
//! runs. It works only from a keyboard ksx is *not* capturing, or before
//! blocking is enabled. What always works:
//!
//! - `LeftCtrl ×5` — toggle capture off (evaluated in the capture thread, so it
//!   survives a wedged engine, output thread, or driver call);
//! - `Ctrl+Alt+Del` — release the keyboards and stop emulation;
//! - `taskkill /f /im ksx.exe` — process death frees the filters with no
//!   cleanup, but you need an input device you can still act from. The mouse is
//!   never captured in M4, so a mouse-driven Task Manager always works.
//!
//! # Safety contract
//!
//! Blocking is enabled only for keyboards bound to a slot, only after the pads
//! are up, and only after the escape-hotkey banner has been printed. Every exit
//! path — clean, Ctrl+C, panic, thread death — uncaptures before it unplugs,
//! and process death needs no cleanup at all.

pub mod latency;
pub mod plan;
pub mod supervisor;

/// End-to-end pipeline tests. `ksx-app` is a bin-only crate, so integration
/// tests that need its internals live here rather than in `tests/`.
#[cfg(test)]
mod pipeline_tests;

use std::io::Write;

use plan::RunPlan;
use supervisor::{Outcome, RunOptions, Wiring};

/// The config/driver problem exit code, shared with `ksx devices`/`ksx pads`:
/// 2 always means "this cannot start".
pub const EXIT_CANNOT_START: i32 = 2;
/// Started, then torn down by a runtime failure (keyboards were released).
pub const EXIT_RUNTIME_FAILURE: i32 = 3;

/// CLI entry point for `ksx run`.
pub fn run(
    game: Option<String>,
    dry_run: bool,
    live_latency: bool,
    json: bool,
) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    let plan = match plan::resolve(&root, game.as_deref()) {
        Ok(plan) => plan,
        Err(err) => {
            if json {
                println!(
                    "{}",
                    crate::pads::error_json("run-cannot-start", &err.to_string())
                );
            } else {
                eprintln!("error: {err}");
            }
            std::process::exit(EXIT_CANNOT_START);
        }
    };

    if dry_run {
        if json {
            println!("{}", serde_json::to_string_pretty(&plan::plan_json(&plan))?);
        } else {
            print!("{}", plan::render_human(&plan));
            println!("dry run: nothing was plugged, no keyboard filter was set");
        }
        return Ok(());
    }

    run_live(plan, live_latency, json)
}

#[cfg(windows)]
fn run_live(plan: RunPlan, live_latency: bool, json: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_capture::{CaptureError, InterceptionBackend};
    use ksx_output::VigemBackend;

    // Pads first, even for the availability check: a missing bus must be found
    // before anything touches the keyboard stack.
    let pads = match VigemBackend::connect() {
        Ok(backend) => backend,
        Err(err) if err.is_bus_missing() => {
            refuse(json, "vigembus-missing", &err.to_string());
        }
        Err(err) => return Err(err).context("connecting to ViGEmBus"),
    };
    // Safe: creating the context sets no filter (see InterceptionBackend::new).
    let capture = match InterceptionBackend::new() {
        Ok(backend) => backend,
        Err(err @ CaptureError::DriverUnavailable) => {
            refuse(
                json,
                "interception-unavailable",
                &format!("{err}; run `ksx doctor` for driver diagnostics"),
            );
        }
        Err(err) => return Err(err).context("creating the Interception context"),
    };

    // The doctor's borrowed-time warning at startup, same code + message.
    for advice in ksx_platform::summarize(&ksx_platform::collect())
        .iter()
        .filter(|a| a.code == "interception-borrowed-time")
    {
        eprintln!("[WARN] {}: {}", advice.code, advice.message);
    }

    if !crate::ctrl_c::install() {
        tracing::warn!("SetConsoleCtrlHandler failed; Ctrl+C will abort without a clean teardown");
    }

    let options = RunOptions {
        live_latency,
        beep: true,
        stop: Box::new(crate::ctrl_c::requested),
        ..RunOptions::default()
    };
    let wiring = Wiring {
        capture: Box::new(capture),
        pads: Box::new(pads),
    };

    // In --json mode stdout carries exactly one object: the final summary.
    // Everything human — including the safety banner — goes to stderr.
    //
    // NOT `.lock()`. `StderrLock`/`StdoutLock` hold a process-wide reentrant
    // lock for as long as the value lives, and `supervise` lives for the whole
    // session. The capture thread logs through `tracing` to **stderr**, so with
    // `--json` the first capture-thread log line — the emergency-escape warning,
    // the stall-watchdog error, the slot-exhaustion error, or the panic
    // notice — would block forever on a lock the supervisor only releases when
    // the session ends. A blocked capture thread stops pumping
    // `interception_receive` with the keyboard filter still armed: every
    // keyboard on the machine dead until reboot, triggered by pressing the
    // escape hatch. Locking per write costs nothing (a handful of lines a
    // session) and cannot deadlock.
    let outcome = if json {
        let mut err = std::io::stderr();
        supervisor::supervise(&plan, wiring, &options, &mut err)?
    } else {
        let mut out = std::io::stdout();
        for note in &plan.notes {
            writeln!(out, "{note}")?;
        }
        supervisor::supervise(&plan, wiring, &options, &mut out)?
    };

    report(&outcome, json)?;
    let code = outcome.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_live(_plan: RunPlan, _live_latency: bool, _json: bool) -> anyhow::Result<()> {
    anyhow::bail!(
        "`ksx run` is Windows-only (it drives the Interception and ViGEmBus kernel drivers); \
         `ksx run --dry-run` works everywhere"
    )
}

#[cfg(windows)]
fn refuse(json: bool, code: &str, message: &str) -> ! {
    if json {
        println!("{}", crate::pads::error_json(code, message));
    } else {
        eprintln!("error: {message}");
    }
    std::process::exit(EXIT_CANNOT_START);
}

fn report(outcome: &Outcome, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome.to_json())?);
        return Ok(());
    }
    println!("{}", outcome.stop.message());
    println!(
        "session: {} event(s) in, {} pad update(s) out, capture exit '{}', {} escape toggle(s)",
        outcome.events, outcome.updates, outcome.capture_exit, outcome.toggles
    );
    println!("{}", outcome.latency.render());
    if outcome.health.dropped_events > 0 {
        println!(
            "[WARN] {} event(s) were dropped: the engine could not keep up with capture",
            outcome.health.dropped_events
        );
    }
    if outcome.health.reboot_required {
        println!(
            "[FAIL] Interception keyboard-slot exhaustion detected — REBOOT REQUIRED before \
             the affected board works again"
        );
    }
    if outcome.deltas_coalesced > 0 {
        println!(
            "[WARN] {} pad state(s) were superseded before the output thread took them: the \
             pads still hold the newest state, but the output path could not keep up",
            outcome.deltas_coalesced
        );
    }
    for (slot, reason) in &outcome.invalidated {
        println!("[FAIL] slot {slot} invalidated: {}", reason.explanation());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The README is user-facing documentation of a safety-critical escape
    /// path, so it is held to the same standard as `--help`: it must not claim
    /// Ctrl+C gets you out of a captured cabinet.
    #[test]
    fn readme_is_honest_about_ctrl_c() {
        let readme = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .join("README.md");
        let text = std::fs::read_to_string(&readme)
            .unwrap_or_else(|e| panic!("reading {}: {e}", readme.display()));

        assert!(
            text.contains("`Ctrl+C` does not work while your keyboards are captured"),
            "the README must state the Ctrl+C limitation plainly"
        );
        assert!(
            text.contains("not** capturing, or before blocking is enabled"),
            "the README must say when Ctrl+C DOES work"
        );
        assert!(
            text.contains("you need an input device you can still act"),
            "taskkill needs a usable input device — say so"
        );
        assert!(
            !text.contains("`Ctrl+C` and\n`taskkill /f` both return your keyboards too"),
            "the old claim that Ctrl+C always returns your keyboards must be gone"
        );
        assert!(
            text.contains("| 0 | clean stop — the `Ctrl+Alt+Del` escape"),
            "the exit-code table must not lead with Ctrl+C as the clean stop"
        );
    }

    /// A `StdoutLock`/`StderrLock` holds a **process-wide** reentrant lock for
    /// as long as the guard lives, and `supervise` lives for the entire session.
    /// The capture thread logs through `tracing` to stderr (the emergency-escape
    /// warning, the stall-watchdog error, slot exhaustion, the panic notice), so
    /// holding that lock here parks the capture thread at exactly the moment it
    /// reports an escape — with the Interception keyboard filter still armed and
    /// nobody pumping `interception_receive`. That is the every-keyboard-dead-
    /// until-reboot case, triggered by pressing the escape hatch.
    ///
    /// Asserted over the source because there is no way to observe "this handle
    /// is not a lock guard" from a unit test, and the mistake is a one-character
    /// edit away.
    #[test]
    fn the_session_writer_never_holds_a_std_stream_lock() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn run_live")
            .expect("run_live is defined here");
        let end = start
            + source[start..]
                .find("\nfn report")
                .expect("report() follows run_live");
        let body = &source[start..end];
        assert!(
            !body.contains("().lock()"),
            "run_live must pass UNLOCKED std handles to supervise(): a stream lock held \
             across the session deadlocks the capture thread's tracing write with the \
             keyboard filter armed"
        );
    }

    #[test]
    fn exit_codes_are_the_documented_values() {
        assert_eq!(super::EXIT_CANNOT_START, 2);
        assert_eq!(super::EXIT_RUNTIME_FAILURE, 3);
        // 2 means the same thing across every ksx command.
        assert_eq!(super::EXIT_CANNOT_START, crate::pads::EXIT_DRIVER_MISSING);
        assert_eq!(
            super::EXIT_CANNOT_START,
            crate::devices::EXIT_DRIVER_MISSING
        );
    }
}
