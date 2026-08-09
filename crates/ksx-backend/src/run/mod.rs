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
//! | 0 | …and, with `--game`, the game exiting: that is what "the session ended as asked" means on a cabinet |
//! | 2 | refused to start: invalid config, unknown `--game`, a `--game` profile whose exe does not exist, a missing driver, two keyboards sharing one hardware id, or any pad-plug failure (bus missing, no free slot, plug timeout, output thread died before the pads were ready). Nothing was plugged and no keyboard filter was ever set |
//! | 3 | started, then a runtime failure tore it down — including a `--game` launch that failed after the pads were up. Keyboards were released first |
//!
//! The 2/3 boundary is exactly "was a keyboard filter ever armed", not "which
//! component failed" — every pad-plug failure happens before capture is told to
//! capture anything, so it is always a 2. A `--game` profile's missing exe is
//! checked in the same breath as the config, before a single pad is plugged, so
//! it is a 2; a launch that fails *at* launch time necessarily happens after
//! the pads are up (see [`supervisor::SessionHook`]), so it is a 3.
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

pub mod game;
pub mod latency;
pub mod plan;
pub mod resolve;
pub mod supervisor;

/// End-to-end pipeline tests. They reach this module's internals — the
/// `Wiring` seam and the fake backends behind it — so they live inside the
/// crate rather than in `tests/`, which sees only the public surface.
/// (Before the ksx-app split they had no choice: a bin crate has no
/// integration-test target at all.)
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
    no_launch: bool,
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

    // Resolve the launch *before* any driver is touched. A profile with a
    // typo'd path must refuse here, with every keyboard still normal and no pad
    // plugged, rather than tearing a live session down two seconds in.
    let launch = match game.as_deref() {
        Some(title) if !no_launch => match resolve_launch(&root, title) {
            Ok(spec) => Some(spec),
            Err(err) => {
                if json {
                    println!(
                        "{}",
                        crate::pads::error_json("game-not-launchable", &err.to_string())
                    );
                } else {
                    eprintln!("error: {err}");
                }
                std::process::exit(EXIT_CANNOT_START);
            }
        },
        _ => None,
    };

    if dry_run {
        if json {
            let mut value = plan::plan_json(&plan);
            value["launch"] = launch_json(launch.as_ref(), no_launch);
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print!("{}", plan::render_human(&plan));
            print!(
                "{}",
                render_launch(launch.as_ref(), no_launch, game.as_deref())
            );
            println!("dry run: nothing was plugged, no keyboard filter was set, nothing launched");
        }
        return Ok(());
    }

    run_live(plan, launch, root.games_path(), live_latency, json)
}

/// Find the profile again and turn it into a launch spec, refusing anything
/// that cannot possibly work.
pub(crate) fn resolve_launch(
    root: &ksx_config::ConfigRoot,
    title: &str,
) -> anyhow::Result<ksx_games::LaunchSpec> {
    let games = ksx_config::Store::new(root.clone()).load_games()?;
    let Some(entry) = games.value.games.iter().find(|g| g.title == title) else {
        // `plan::resolve` already refused this case with a better message; if
        // we get here the two disagree, which is worth saying out loud.
        anyhow::bail!("no game profile titled '{title}' in games.toml");
    };
    let spec = ksx_games::LaunchSpec::from_entry(entry);
    ksx_games::preflight(&spec)?;
    Ok(spec)
}

fn render_launch(
    launch: Option<&ksx_games::LaunchSpec>,
    no_launch: bool,
    game: Option<&str>,
) -> String {
    match (launch, no_launch, game) {
        (Some(spec), _, _) => {
            let tracking = match &spec.process_name {
                Some(name) => format!("track process '{name}' after a launcher hand-off"),
                None => "no process_name: a launcher hand-off cannot be followed".to_owned(),
            };
            format!(
                "  launch: {}\n          {tracking}\n",
                spec.target.describe()
            )
        }
        (None, true, Some(title)) => {
            format!("  launch: skipped (--no-launch); '{title}' profile applied only\n")
        }
        _ => String::new(),
    }
}

fn launch_json(launch: Option<&ksx_games::LaunchSpec>, no_launch: bool) -> serde_json::Value {
    match launch {
        Some(spec) => serde_json::json!({
            "will_launch": true,
            "target": spec.target.describe(),
            "protocol": spec.target.is_protocol(),
            "process_name": spec.process_name,
        }),
        None => serde_json::json!({ "will_launch": false, "no_launch": no_launch }),
    }
}

#[cfg(windows)]
fn run_live(
    plan: RunPlan,
    launch: Option<ksx_games::LaunchSpec>,
    games_toml: std::path::PathBuf,
    live_latency: bool,
    json: bool,
) -> anyhow::Result<()> {
    let hook: Box<dyn supervisor::SessionHook> = match launch {
        Some(spec) => Box::new(game::GameHook::new(
            spec,
            ksx_games::RealHost::new(),
            games_toml,
        )),
        None => Box::new(supervisor::NoHook),
    };
    // Backend selection is per device (`[[device]] backend = ...`). This claims
    // any WinUSB interfaces the plan names, and only creates an Interception
    // context if something still needs one — see `crate::capture`.
    live_session(&plan, crate::capture::build, hook, live_latency, json)
}

/// One live session, from "is the bus there" to the final report.
///
/// The capture backend arrives as a **closure**, not a value, and that is the
/// whole reason this is a seam rather than three arguments: pads are connected
/// first, deliberately (see below), so the backend cannot already exist by the
/// time this is called. `ksx run` passes `crate::capture::build`; `ksx play`
/// passes one that puts a recording in front of the same pipeline
/// (`crate::play`). Everything between those two lines — teardown order, the
/// escape banner, the exit codes — is shared by construction instead of by two
/// people remembering to keep it that way.
#[cfg(windows)]
pub(crate) fn live_session(
    plan: &RunPlan,
    capture: impl FnOnce(
        &RunPlan,
    )
        -> Result<Box<dyn ksx_capture::CaptureBackend>, crate::capture::SetupError>,
    hook: Box<dyn supervisor::SessionHook>,
    live_latency: bool,
    json: bool,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_output::{RoutedBackend, VigemBackend};

    // Pads first, even for the availability check: a missing bus must be found
    // before anything touches the keyboard stack.
    //
    // Wrapped in a `RoutedBackend` so a slot's *persona* picks its driver stack
    // (see `ksx_output::router`). ViGEmBus is still connected eagerly — it backs
    // xbox360/playstation, which is every configuration ksx ships — while
    // HIDMaestro is built only if some slot asks for a persona that needs it.
    // A cabinet with no such slot never probes for it and cannot be broken by
    // its absence.
    let vigem = match VigemBackend::connect() {
        Ok(backend) => backend,
        Err(err) if err.is_bus_missing() => {
            refuse(json, "vigembus-missing", &err.to_string());
        }
        Err(err) => return Err(err).context("connecting to ViGEmBus"),
    };
    let pads = RoutedBackend::standard(Box::new(vigem));
    // Every failure here is a refusal: no pad is plugged and no filter is armed.
    let capture = match capture(plan) {
        Ok(backend) => backend,
        Err(err) => {
            let hint = match &err {
                crate::capture::SetupError::Interception(_) => {
                    "; run `ksx doctor` for driver diagnostics"
                }
                _ => "",
            };
            refuse(json, err.code(), &format!("{err}{hint}"));
        }
    };

    // The doctor's borrowed-time warning at startup, same code + message — but
    // only when this session actually loads the end-of-life driver. A fully
    // migrated cabinet has earned its silence.
    if plan.needs_interception() {
        for advice in ksx_platform::summarize(&ksx_platform::collect())
            .iter()
            .filter(|a| a.code == "interception-borrowed-time")
        {
            eprintln!("[WARN] {}: {}", advice.code, advice.message);
        }
    }

    if !crate::ctrl_c::install() {
        tracing::warn!("SetConsoleCtrlHandler failed; Ctrl+C will abort without a clean teardown");
    }

    let mut options = RunOptions {
        live_latency,
        beep: true,
        stop: Box::new(crate::ctrl_c::requested),
        hook,
        ..RunOptions::default()
    };
    let wiring = Wiring {
        capture,
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
        supervisor::supervise(plan, wiring, &mut options, &mut err)?
    } else {
        let mut out = std::io::stdout();
        for note in &plan.notes {
            writeln!(out, "{note}")?;
        }
        supervisor::supervise(plan, wiring, &mut options, &mut out)?
    };

    // The hook outlives the session, so its final word is available here — and
    // "which state did game tracking end in" is exactly what you want when a
    // profile behaved oddly (`launcher handed off, looking for the game` is a
    // very different bug report from `exited`). It is merged INTO the summary,
    // never printed alongside it: `--json` promises exactly one object.
    report(&outcome, options.hook.status_line(), json)?;
    let code = outcome.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_live(
    _plan: RunPlan,
    _launch: Option<ksx_games::LaunchSpec>,
    _games_toml: std::path::PathBuf,
    _live_latency: bool,
    _json: bool,
) -> anyhow::Result<()> {
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

fn report(outcome: &Outcome, game_status: Option<String>, json: bool) -> anyhow::Result<()> {
    // Mirror the verdict into `tracing` before rendering it for a human.
    //
    // `ksx autostart` registers `ksx run`, not the daemon, so on a cabinet this
    // process has no console at logon: everything below is `println!` to a
    // stdout nobody reads. Without this the log file gets a startup banner and
    // then silence — no stop reason, no REBOOT REQUIRED, no dropped-event
    // count — which is exactly the 3am failure the log exists for. stdout stays
    // the human surface; the file gets the same facts.
    if outcome.health.reboot_required {
        tracing::error!(
            stop = outcome.stop.code(),
            "Interception keyboard-slot exhaustion — REBOOT REQUIRED before the affected board works again"
        );
    }
    if outcome.health.watchdog_tripped {
        tracing::warn!(
            stop = outcome.stop.code(),
            "the capture watchdog tripped: the event consumer stalled and keystrokes were passed through"
        );
    }
    if outcome.health.dropped_events > 0 {
        tracing::warn!(
            dropped = outcome.health.dropped_events,
            "events were dropped: the engine could not keep up with capture"
        );
    }
    tracing::info!(
        stop = outcome.stop.code(),
        events = outcome.events,
        updates = outcome.updates,
        capture_exit = %outcome.capture_exit,
        toggles = outcome.toggles,
        exit_code = outcome.exit_code(),
        "session ended: {}",
        outcome.stop.message()
    );

    if json {
        let mut value = outcome.to_json();
        value["game"] = match &game_status {
            Some(status) => serde_json::json!(status),
            None => serde_json::Value::Null,
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!("{}", outcome.stop.message());
    if let Some(status) = &game_status {
        println!("game: {status}");
    }
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
