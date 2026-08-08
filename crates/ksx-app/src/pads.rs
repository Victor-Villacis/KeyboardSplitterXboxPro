//! `ksx pads` — plug N virtual pads, show each pad's XInput identity, run a
//! visible test pattern, then unplug cleanly.
//!
//! [`ceiling_warning`] runs BEFORE the first plug: an XInput persona past
//! Windows' four slots plugs pads no game can read, and this used to happen in
//! silence. It warns and proceeds — the convention this command keeps — on
//! stderr in both modes, so `--json` still puts exactly one object on stdout.
//!
//! Exit code [`EXIT_DRIVER_MISSING`] (2) = ViGEmBus is not installed, and
//! [`EXIT_REFUSED`] (also 2) = `--persona` names something this build cannot
//! create. Same code, different `--json` `code` — because only the first one
//! has an install that fixes it. The human-readable hint goes to stderr;
//! `--json` puts `{"error":{code,message}}` on stdout instead. Ctrl+C is handled by the
//! shared [`crate::ctrl_c`] latch the pattern loop polls — no ctrlc crate. A
//! `taskkill /f` mid-pattern runs no destructors; that case is covered by
//! driver-side client-handle cleanup (see `VigemBackend` docs).
//!
//! The pattern math is pure and cross-platform (tested everywhere); only
//! [`run`]'s driver plumbing is Windows-only.

// Off Windows only the stub `run` is reachable outside tests; the pure pattern
// + JSON helpers stay compiled (and tested) but would trip dead_code.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::Duration;

use ksx_core::pad::{XButtons, AXIS_MAX};
use ksx_core::PadState;
use ksx_platform::BusDriverReport;

/// Exit code when ViGEmBus is not installed (documented in `--help`).
pub const EXIT_DRIVER_MISSING: i32 = 2;

/// Exit code when `--persona` names something this build cannot create.
///
/// Shares 2 with [`EXIT_DRIVER_MISSING`] because both mean "did not start, and
/// not because anything broke". The `--json` `code` is what tells them apart,
/// and it has to: one is fixed by installing a driver and the other by nothing.
pub const EXIT_REFUSED: i32 = 2;

/// One pattern frame per tick. Coarse on purpose — this is the demo path, not
/// the input hot path.
pub const TICK: Duration = Duration::from_millis(100);

/// Each of A/B/X/Y is held this many ticks (400 ms).
const BUTTON_HOLD_TICKS: u64 = 4;
const BUTTON_CYCLE: [XButtons; 4] = [XButtons::A, XButtons::B, XButtons::X, XButtons::Y];
/// Full left-stick revolution every 20 ticks (2 s); right stick counter-rotates.
const STICK_PERIOD_TICKS: u64 = 20;
/// Trigger triangle-wave period (1.6 s); RT runs half a period behind LT.
const TRIGGER_PERIOD_TICKS: u64 = 16;

/// The state the test pattern shows at `tick` (one tick = [`TICK`]).
///
/// Pure and total: same tick, same state, on any platform.
fn pattern_state(tick: u64) -> PadState {
    let button = BUTTON_CYCLE[((tick / BUTTON_HOLD_TICKS) % 4) as usize];
    let angle =
        (tick % STICK_PERIOD_TICKS) as f64 / STICK_PERIOD_TICKS as f64 * std::f64::consts::TAU;
    let amp = f64::from(AXIS_MAX);
    PadState {
        buttons: button,
        lt: triangle(tick),
        rt: triangle(tick + TRIGGER_PERIOD_TICKS / 2),
        lx: (angle.cos() * amp) as i16,
        ly: (angle.sin() * amp) as i16,
        // Counter-rotating so the two sticks are visibly independent.
        rx: (angle.cos() * amp) as i16,
        ry: (-angle.sin() * amp) as i16,
    }
}

/// Triangle wave 0 → 255 → 0 over [`TRIGGER_PERIOD_TICKS`].
fn triangle(tick: u64) -> u8 {
    let pos = tick % TRIGGER_PERIOD_TICKS;
    let half = TRIGGER_PERIOD_TICKS / 2;
    let distance = if pos <= half {
        pos
    } else {
        TRIGGER_PERIOD_TICKS - pos
    };
    (distance * 255 / half) as u8
}

/// One plugged pad, as reported by `--json` (and mirrored by the human lines).
pub struct PadRow {
    /// 1-based plug order.
    pub slot: u8,
    /// Backend handle id ([`ksx_output::PadHandle::raw`]).
    pub handle: u32,
    /// Which controller the pad presents itself as.
    pub persona: ksx_core::Persona,
    /// XInput `dwUserIndex`; `None` beyond the 4 XInput slots, and always
    /// `None` for non-XInput personas.
    pub user_index: Option<u8>,
    /// LED number from the bus feedback, if one arrived during the drain.
    pub led_number: Option<u8>,
}

/// The single `--json` success object: `{driver, pads}`.
pub fn pads_json(driver: &BusDriverReport, pads: &[PadRow]) -> serde_json::Value {
    let pads: Vec<serde_json::Value> = pads
        .iter()
        .map(|p| {
            serde_json::json!({
                "slot": p.slot,
                "handle": p.handle,
                "persona": p.persona.as_str(),
                "user_index": p.user_index,
                "led_number": p.led_number,
            })
        })
        .collect();
    serde_json::json!({ "driver": driver, "pads": pads })
}

/// The single `--json` failure object: `{"error":{code,message}}`.
pub fn error_json(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

/// The persona this build can plug that costs no XInput slot — the remedy
/// [`ceiling_warning`] points at.
///
/// Derived from the roster, never spelled, for the same reason
/// [`surface::spawn_offer`] derives its own option list: a build that cannot
/// plug that persona must not be told to use it. `None` means this build has
/// no HID persona at all, and the warning then names the cost without offering
/// a command that would fail.
pub fn hid_persona() -> Option<ksx_core::Persona> {
    ksx_core::Persona::ALL
        .iter()
        .copied()
        .find(|p| p.can_plug() && !p.is_xinput())
}

/// What `ksx pads --count N --persona P` will hand a GAME, said before the
/// first pad is plugged. `None` when there is nothing to say.
///
/// A WARNING, not a refusal — the convention this command already keeps
/// (`--prune` refuses only what would take a pad out of a player's hands).
/// `ksx pads` is the test command: plugging four dead pads on purpose to see
/// what Windows does with them is a legitimate thing to ask for, and the
/// config layer is where the same ceiling REFUSES
/// (`ksx_config::Issue::TooManyXinputSlots`). What was missing was anyone
/// saying it out loud beforehand: eight pads plugged, the pattern ran on all
/// eight, and four of them were invisible to every game with nothing on screen
/// admitting it.
///
/// **What decides it**: Windows hands out exactly [`MAX_XINPUT_SLOTS`] XInput
/// slots, and an XInput persona needs one to be read at all. Whether THIS pad
/// gets one depends on how many are already taken when it plugs — read here
/// from XInput itself (real pads and other processes' pads count, which is why
/// the ViGEm child list cannot answer it) and phrased as a ceiling, because
/// another process can take a slot between this reading and the plug.
///
/// A HID persona does not enter that arithmetic at all, which is the fix.
pub fn ceiling_warning(
    count: u8,
    persona: ksx_core::Persona,
    xinput_in_use: Option<u8>,
) -> Option<String> {
    use ksx_core::MAX_XINPUT_SLOTS;

    if !persona.is_xinput() {
        // A HID pad takes no XInput slot, so this answer does not depend on
        // the reading — and a warning here would be crying wolf.
        return None;
    }
    // "…and the extras still plug": true in both definite arms, and the whole
    // reason this needs saying — a dead pad is indistinguishable from a live
    // one until a game fails to see it.
    let unread = "Those pads plug and run the pattern all the same; nothing reads them.";
    let mut text = match (xinput_in_use, surface::xinput_free(xinput_in_use)) {
        (Some(_), Some(free)) if count <= free => return None,
        (Some(in_use), Some(free)) => format!(
            "warning: {} of these {count} {persona} pad(s) will be invisible to every game. \
             Windows exposes exactly {MAX_XINPUT_SLOTS} XInput slots and no virtual bus can \
             create a fifth; {in_use} of them {} in use right now — by any pad on this machine, \
             real or virtual — so at most {free} of these will be readable. {unread}\n",
            count - free,
            if in_use == 1 { "is" } else { "are" },
        ),
        // A reading that failed is not a reading of zero — but it still leaves
        // one thing knowable: no machine has fewer than none in use, so
        // anything past the ceiling is invisible whatever the true occupancy
        // turns out to be. That is a floor, and it is said as one.
        _ if count > MAX_XINPUT_SLOTS => format!(
            "warning: at least {} of these {count} {persona} pad(s) will be invisible to every \
             game. Windows exposes exactly {MAX_XINPUT_SLOTS} XInput slots and no virtual bus \
             can create a fifth. ksx could not read how many are already in use, so the real \
             number can only be higher — that is a reading that failed, not an empty machine. \
             {unread}\n",
            count - MAX_XINPUT_SLOTS,
        ),
        _ => format!(
            "warning: ksx could not read how many of Windows' {MAX_XINPUT_SLOTS} XInput slots \
             are in use, so it cannot say how many of these {count} {persona} pad(s) a game will \
             see — that is a reading that failed, not an empty machine.\n"
        ),
    };
    if let Some(hid) = hid_persona() {
        // The fix that WORKS, measured rather than reasoned: ViGEm's DS4
        // targets are plain HID, so they never enter the XInput arithmetic —
        // not capped at four, and no second driver to install.
        text.push_str(&format!(
            "  The fix is a HID persona, which never enters that arithmetic: `ksx pads --count \
             {count} --persona {hid}` plugs {count} pads a game can read (HID/DirectInput — \
             MAME, RetroArch, SDL, \
             Steam Input) and takes none of the {MAX_XINPUT_SLOTS} XInput slots. Measured \
             2026-08-04 (docs/research/m6.5-ds4-findings.md): six {hid} targets enumerated \
             alongside four {persona} pads, with the XInput count unmoved.\n"
        ));
        text.push_str(&format!(
            "  On a real panel that is the mixed shape the same measurement ran: slots \
             1..={MAX_XINPUT_SLOTS} `persona = \"{persona}\"`, slots {}+ `persona = \"{hid}\"`, \
             in config.toml.\n",
            MAX_XINPUT_SLOTS + 1,
        ));
    }
    text.push_str("  Plugging anyway — this is a warning, not a refusal.\n");
    Some(text)
}

#[cfg(windows)]
pub fn run(
    count: u8,
    persona: ksx_core::Persona,
    hold_secs: u64,
    json: bool,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_output::{OutputError, RoutedBackend, VigemBackend, VirtualPadBackend as _};

    // Refused before ViGEmBus is opened. Connecting first would make a build
    // limitation arrive after a driver handshake, in the shape of a driver
    // problem — and `--json` would report it under a `vigembus`-flavoured code.
    if !persona.can_plug() {
        let err = OutputError::PersonaNotImplemented(persona);
        if json {
            println!(
                "{}",
                error_json("persona-not-implemented", &err.to_string())
            );
        } else {
            eprintln!("error: {err}");
        }
        std::process::exit(EXIT_REFUSED);
    }

    // BEFORE the bus is opened, so the sentence arrives while the answer is
    // still "don't", not as a post-mortem over eight pads that are already
    // animating. After the persona refusal above, because a persona this build
    // cannot create has no XInput arithmetic to warn about.
    //
    // stderr in BOTH modes: `--json` promises exactly one object on stdout,
    // and a warning that broke that promise would be a worse bug than the one
    // it is reporting. A machine reader loses nothing — the response already
    // carries `user_index: null` on every pad the slots ran out for.
    if let Some(warning) = ceiling_warning(count, persona, ksx_platform::xinput::slots_in_use()) {
        eprint!("{warning}");
    }

    let vigem = match VigemBackend::connect() {
        Ok(backend) => backend,
        Err(err) if err.is_bus_missing() => {
            if json {
                println!("{}", error_json("vigembus-missing", &err.to_string()));
            } else {
                eprintln!("error: {err}");
            }
            std::process::exit(EXIT_DRIVER_MISSING);
        }
        Err(err) => return Err(err).context("connecting to ViGEmBus"),
    };
    // Routed, so `ksx pads --persona dualsense` is a real test of the M8 path
    // rather than a `PersonaUnsupported` from the wrong backend.
    let mut backend = RoutedBackend::standard(Box::new(vigem));

    let mut handles = Vec::new();
    let mut rows = Vec::new();
    for slot in 1..=count {
        let handle = match backend.plug_persona(persona) {
            Ok(handle) => handle,
            // A missing HIDMaestro is its own exit code path, not a generic
            // failure: the fix is "install HIDMaestro", and the cabinet's own
            // personas are unaffected.
            Err(err) if err.is_hidmaestro_missing() => {
                if json {
                    println!("{}", error_json("hidmaestro-missing", &err.to_string()));
                } else {
                    eprintln!("error: {err}");
                }
                std::process::exit(EXIT_DRIVER_MISSING);
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("plugging {persona} pad {slot} of {count}"))
            }
        };
        let user_index = backend.user_index(handle);
        // Draining LED feedback on a persona with no feedback channel would
        // just burn the 600ms drain deadline per pad reporting nothing.
        let led_number = persona
            .has_feedback()
            .then(|| drain_led(&mut backend, handle))
            .flatten();
        if !json {
            if persona.is_xinput() {
                let ui = match user_index {
                    Some(i) => i.to_string(),
                    None => "none".to_string(),
                };
                let led = match led_number {
                    Some(n) => n.to_string(),
                    None => "?".to_string(),
                };
                println!("pad {slot}: user index {ui} (led {led})");
            } else {
                // No XInput index and no LED — by design, not by failure.
                println!("pad {slot}: {} (HID/DirectInput)", persona.label());
            }
        }
        handles.push(handle);
        rows.push(PadRow {
            slot,
            handle: handle.raw(),
            persona,
            user_index,
            led_number,
        });
    }

    if json {
        // The driver section reuses doctor's collector, scoped to ViGEmBus.
        let driver = ksx_platform::collect().vigembus;
        println!("{}", pads_json(&driver, &rows));
    } else {
        println!("running test pattern for {hold_secs}s (Ctrl+C to unplug and exit)...");
        animate(&mut backend, &handles, hold_secs);
    }

    for (handle, row) in handles.iter().zip(&rows) {
        backend
            .unplug(*handle)
            .with_context(|| format!("unplugging pad {}", row.slot))?;
    }
    drop(backend);
    if !json {
        println!("unplugged {count} pad(s)");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(
    _count: u8,
    _persona: ksx_core::Persona,
    _hold_secs: u64,
    _json: bool,
) -> anyhow::Result<()> {
    anyhow::bail!("`ksx pads` is Windows-only (it drives the ViGEmBus kernel driver)")
}

/// Polls feedback briefly after a plug so the bus's initial LED notification is
/// reported. Returns the last LED seen, or `None` if nothing arrived in time.
#[cfg(windows)]
fn drain_led(
    backend: &mut dyn ksx_output::VirtualPadBackend,
    handle: ksx_output::PadHandle,
) -> Option<u8> {
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    let mut led = None;
    loop {
        while let Some(feedback) = backend.poll_feedback(handle) {
            led = Some(feedback.led_number);
        }
        if led.is_some() || std::time::Instant::now() >= deadline {
            return led;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drives [`pattern_state`] on every pad until the hold expires or Ctrl+C.
#[cfg(windows)]
fn animate(
    backend: &mut dyn ksx_output::VirtualPadBackend,
    handles: &[ksx_output::PadHandle],
    hold_secs: u64,
) {
    if !crate::ctrl_c::install() {
        tracing::warn!("SetConsoleCtrlHandler failed; the pattern runs the full hold time");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(hold_secs);
    let mut tick: u64 = 0;
    while std::time::Instant::now() < deadline && !crate::ctrl_c::requested() {
        let state = pattern_state(tick);
        for &handle in handles {
            if let Err(err) = backend.update(handle, &state) {
                tracing::warn!(%err, ?handle, "pattern update failed");
            }
        }
        tick += 1;
        std::thread::sleep(TICK);
    }
    // Release everything before the unplug so nothing is left "pressed".
    for &handle in handles {
        let _ = backend.update(handle, &PadState::default());
    }
}

// ---------------------------------------------------------------------------
// `ksx pads --prune` — clearing pads that outlived whatever made them
// ---------------------------------------------------------------------------

/// What a prune would do, decided before anything is touched.
///
/// Pure so the whole decision surface — including the two refusals — is tested
/// in CI with no ViGEmBus anywhere near it, the same shape `winusb::plan_claim`
/// uses for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrunePlan {
    /// The bus has no children. Nothing to do, and saying so is the answer.
    Nothing,
    /// ViGEmBus is not installed, or exposes no devnode to restart.
    NoBus,
    /// A session is live. Its pads are the ones a player is holding, and a bus
    /// restart would yank them mid-game.
    SessionRunning { count: usize },
    /// Restart the bus devnode, which drops every child pad with it.
    Restart {
        bus_instance_id: String,
        count: usize,
    },
}

/// Decide, given the bus devnode, how many pads hang off it, and whether a
/// session is running.
///
/// `session_running` is the input the ghost heuristic could never have: doctor
/// matches an owner by PROCESS NAME, and the tray daemon is `ksx.exe` whether
/// or not it has a session — so on a cabinet, where the daemon runs all day,
/// stale pads are indistinguishable from working ones. Fifteen accumulated on
/// the reference cabinet that way. Asking the daemon what it is actually doing
/// is the difference between a guess and an answer.
pub fn plan_prune(bus_instance_id: Option<&str>, count: usize, session_running: bool) -> PrunePlan {
    if count == 0 {
        return PrunePlan::Nothing;
    }
    if session_running {
        return PrunePlan::SessionRunning { count };
    }
    match bus_instance_id {
        Some(bus) => PrunePlan::Restart {
            bus_instance_id: bus.to_owned(),
            count,
        },
        None => PrunePlan::NoBus,
    }
}

/// Why this render is or is not describing work that happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneMode {
    /// No `--yes`. The trailer tells them how to mean it.
    DryRun,
    /// They meant it, and something else stopped it — elevation, so far. The
    /// trailer must NOT tell them to add a flag they already passed.
    Blocked,
    /// Actually running.
    Doing,
}

impl PrunePlan {
    /// The command a user could run by hand instead. Printed in the dry run,
    /// because a driver operation nobody can reproduce without ksx is one
    /// nobody can undo without ksx either.
    pub fn command(&self) -> Option<String> {
        match self {
            PrunePlan::Restart {
                bus_instance_id, ..
            } => Some(format!("pnputil /restart-device \"{bus_instance_id}\"")),
            _ => None,
        }
    }

    pub fn render(&self, mode: PruneMode) -> String {
        let dry_run = mode != PruneMode::Doing;
        match self {
            PrunePlan::Nothing => "no virtual pads on the bus — nothing to prune\n".to_owned(),
            PrunePlan::NoBus => {
                "ViGEmBus exposes no devnode to restart. If joy.cpl still lists pads, reboot \
                 — there is nothing here to act on.\n"
                    .to_owned()
            }
            PrunePlan::SessionRunning { count } => format!(
                "REFUSED: a session is running, and those {count} pad(s) are the ones it is \
                 driving.\n\nPruning restarts the bus device, which unplugs every pad on it — \
                 mid-game, for whoever is holding one. Stop emulation first \
                 (`ksx session stop`), then prune.\n"
            ),
            PrunePlan::Restart { count, .. } => {
                let head = if dry_run {
                    format!("DRY RUN — would clear {count} virtual pad(s)")
                } else {
                    format!("clearing {count} virtual pad(s)")
                };
                let command = self.command().unwrap_or_default();
                format!(
                    "{head}\n\n  {command}\n\nRestarting the bus device drops every child pad \
                     with it. No session is running, so nothing is holding one.\n{}",
                    match mode {
                        // Telling someone to "re-run with --yes" when they just
                        // passed --yes is the kind of stale instruction that
                        // makes a user doubt the rest of the output.
                        PruneMode::DryRun =>
                            "\nNothing was changed. Re-run with --yes from an elevated prompt.\n",
                        PruneMode::Blocked => "\nNothing was changed.\n",
                        PruneMode::Doing => "",
                    }
                )
            }
        }
    }
}

/// Is a session running right now?
///
/// `None` means the daemon could not be asked — no daemon, or the pipe did not
/// answer. Treated as "not running" by the caller, deliberately: with no daemon
/// there is certainly no session, and a pruning refusal that fires because a
/// diagnostic could not reach a process that is not there would block the fix
/// exactly when the machine most needs it.
#[cfg(windows)]
fn session_running() -> Option<bool> {
    use crate::daemon::pipe::{client, PIPE_NAME};
    let response = client::request(PIPE_NAME, &serde_json::json!({ "verb": "status" })).ok()?;
    response
        .pointer("/session/running")
        .and_then(serde_json::Value::as_bool)
        .or_else(|| response["running"].as_bool())
}

/// `ksx pads --prune` — clear pads that outlived whatever made them.
#[cfg(windows)]
pub fn prune(yes: bool, json: bool) -> anyhow::Result<()> {
    let report = ksx_platform::collect();
    let pads = &report.virtual_pads;
    let running = session_running().unwrap_or(false);
    let plan = plan_prune(pads.bus_instance_id.as_deref(), pads.count, running);

    // Before narrating anything. `render(false)` opens with "clearing 15
    // virtual pad(s)" — present tense, because that is what a real run is
    // doing — and printing that above a refusal reads as though ksx acted and
    // then changed its mind. Elevation is a property of THIS process and is
    // knowable now, so it is answered before a word is said about clearing.
    let blocked_on_elevation = yes
        && matches!(plan, PrunePlan::Restart { .. })
        && ksx_platform::process::is_elevated() == Some(false);
    if blocked_on_elevation && !json {
        print!("{}", plan.render(PruneMode::Blocked));
        eprintln!(
            "\nREFUSED: restarting a bus device needs an administrator token. ksx never \
             self-elevates — open an elevated prompt and re-run the same command."
        );
        std::process::exit(crate::winusb::EXIT_REFUSED);
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "pads": pads.count,
                "session_running": running,
                "command": plan.command(),
                "dry_run": !yes || blocked_on_elevation,
                "refused": matches!(plan, PrunePlan::SessionRunning { .. })
                    || blocked_on_elevation,
                "refused_because": if matches!(plan, PrunePlan::SessionRunning { .. }) {
                    Some("session-running")
                } else if blocked_on_elevation {
                    Some("needs-elevation")
                } else {
                    None
                },
            })
        );
    } else {
        print!(
            "{}",
            plan.render(if yes {
                PruneMode::Doing
            } else {
                PruneMode::DryRun
            })
        );
    }

    let PrunePlan::Restart { .. } = &plan else {
        if matches!(plan, PrunePlan::SessionRunning { .. }) {
            std::process::exit(crate::winusb::EXIT_REFUSED);
        }
        return Ok(());
    };
    if !yes {
        return Ok(());
    }
    if blocked_on_elevation {
        // The --json path: the object above already carried the refusal.
        std::process::exit(crate::winusb::EXIT_REFUSED);
    }
    let Some(command) = plan.command() else {
        return Ok(());
    };
    let PrunePlan::Restart {
        bus_instance_id, ..
    } = &plan
    else {
        return Ok(());
    };
    let planned = ksx_platform::winusb::PlannedCommand::pnputil(
        &["/restart-device", bus_instance_id],
        "restart the bus, which drops every child pad with it",
    );
    match ksx_platform::winusb::run_command(&planned) {
        Ok(output) => {
            println!("\n{output}");
            println!("done — `ksx doctor` should now report no pads on the bus.");
            Ok(())
        }
        Err(err) => {
            eprintln!("\nFAILED: {err}");
            eprintln!(
                "\nRun it by hand from an elevated prompt:\n  {command}\n\nIf that also fails, \
                 a reboot clears every pad on the bus."
            );
            std::process::exit(crate::winusb::EXIT_APPLY_FAILED);
        }
    }
}

#[cfg(not(windows))]
pub fn prune(_yes: bool, _json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx pads --prune` restarts a Windows bus device and is Windows-only")
}

// ---------------------------------------------------------------------------
// The same two verbs, called by a SURFACE instead of a console
// ---------------------------------------------------------------------------

/// `ksx pads` and `ksx pads --prune` for a caller that has no stdout, no exit
/// code and no Ctrl+C.
///
/// Everything here is the DECISION half — pure, total, and tested in CI with
/// no ViGEmBus anywhere near it, exactly like [`plan_prune`] above. The only
/// Windows-only thing in the module is [`plug_and_hold`], which is the driver
/// plumbing the decision authorises.
///
/// Three properties the console versions got for free and a surface does not:
///
/// - **An end.** `ksx pads` holds the pattern until `--hold-secs` expires or
///   the user presses Ctrl+C. A web page has neither, so the hold is part of
///   the spec and every plug schedules its own unplug.
/// - **A judgement about the session.** The CLI trusts whoever typed the
///   command to know emulation is stopped. Nothing on a page carries that
///   knowledge, so [`plan_spawn`] refuses while a session is live — the same
///   refusal, and for the same reason, [`plan_prune`] already makes.
/// - **A total.** A console operator sees the pads accumulate and stops. A
///   button does not: five submits of `count=16` leave eighty pads on the bus
///   for two minutes, and the recovery — a prune — is refused to the
///   unelevated process a browser-launched Studio usually is. So
///   [`SpawnPlan::BusFull`] bounds the TOTAL, not the request, and
///   [`spawn_offer`] stops offering counts that would not fit.
///
/// What it deliberately does NOT do is refuse a count above the XInput
/// ceiling. `ksx pads --count 8 --persona xbox360` plugs eight pads and four
/// of them are invisible to every game; that stayed a WARNING when task #16
/// landed — plugging dead pads on purpose to see what Windows does with them
/// is a legitimate thing to ask a test command for, and the config layer is
/// where the same ceiling refuses. What this adds is the sentence saying so,
/// attached to the option that would cause it, so a surface can warn BEFORE
/// the click without knowing why four is the number — and NAMING the persona
/// it applies to, because the same click costs nothing on a HID pad. The
/// console's half of that same sentence is [`super::ceiling_warning`].
#[cfg_attr(not(any(feature = "studio", feature = "cabinet")), allow(dead_code))]
pub mod surface {
    use ksx_core::{Persona, MAX_SLOTS, MAX_XINPUT_SLOTS};

    use super::{PruneMode, PrunePlan};

    /// The longest hold a surface may ask for. Two minutes is long enough to
    /// walk to joy.cpl and back and short enough that a forgotten tab does not
    /// leave pads on the bus all afternoon.
    pub const MAX_HOLD_SECS: u64 = 120;

    /// The holds offered, in seconds, shortest first — so the default a
    /// `<select>` picks on its own is the CLI's own `--hold-secs 10`. One
    /// default for both surfaces beats a defensible reason for two.
    const HOLD_CHOICES: [u64; 4] = [10, 30, 60, 120];

    /// What a spawn would do, decided before ViGEmBus is opened.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SpawnPlan {
        /// Emulation is live. Those four XInput slots are the ones a player is
        /// using, and a test pad would take one out of their hands.
        SessionRunning,
        /// This build cannot create that persona at all
        /// ([`Persona::can_plug`]) — a driver install does not change it.
        PersonaNotImplemented { persona: Persona },
        /// Outside `1..=MAX_SLOTS`.
        BadCount { given: u8 },
        /// Outside `1..=MAX_HOLD_SECS`.
        BadHold { given: u64 },
        /// The bus already carries enough pads that this spawn would push the
        /// total past [`MAX_SLOTS`].
        ///
        /// A per-call bound is not a bound: nothing stopped five submits of
        /// `count=16` from leaving eighty pads on the bus for two minutes,
        /// and the only cleanup is a prune that an unelevated Studio cannot
        /// perform. So the ceiling is on the TOTAL, not on the request.
        BusFull { on_bus: usize, requested: u8 },
        /// Plug them, hold the pattern, unplug.
        Plug {
            count: u8,
            persona: Persona,
            hold_secs: u64,
            /// How many of the requested pads no game will be able to read,
            /// because the XInput slots ran out. `Some(0)` for HID personas,
            /// and `Some(0)` is the normal answer.
            ///
            /// `None` means the XInput slots could not be READ — which is a
            /// different sentence from "none are taken" and must render as a
            /// different sentence (see [`crate::pads::surface::xinput_line`]).
            unreadable: Option<u8>,
        },
    }

    /// Decide, given what the caller asked for and what the machine is doing.
    ///
    /// `xinput_in_use` is how many of Windows' four XInput slots hold a pad
    /// right now — from [`ksx_platform::xinput::slots_in_use`], which asks
    /// XInput itself rather than counting ViGEmBus children. `None` is
    /// "could not ask", never "none".
    ///
    /// `on_bus` is how many pads the ViGEm bus is already carrying, and it is
    /// what makes the ceiling a ceiling rather than a per-click suggestion.
    pub fn plan_spawn(
        count: u8,
        persona: Persona,
        hold_secs: u64,
        session_running: bool,
        xinput_in_use: Option<u8>,
        on_bus: usize,
    ) -> SpawnPlan {
        if session_running {
            return SpawnPlan::SessionRunning;
        }
        if count == 0 || count > MAX_SLOTS {
            return SpawnPlan::BadCount { given: count };
        }
        if hold_secs == 0 || hold_secs > MAX_HOLD_SECS {
            return SpawnPlan::BadHold { given: hold_secs };
        }
        if !persona.can_plug() {
            return SpawnPlan::PersonaNotImplemented { persona };
        }
        if on_bus.saturating_add(usize::from(count)) > usize::from(MAX_SLOTS) {
            return SpawnPlan::BusFull {
                on_bus,
                requested: count,
            };
        }
        SpawnPlan::Plug {
            count,
            persona,
            hold_secs,
            unreadable: unreadable_count(count, persona, xinput_free(xinput_in_use)),
        }
    }

    /// XInput slots still free for a NEW pad. `None` in, `None` out — a
    /// reading that failed cannot be turned into a free count by arithmetic.
    pub fn xinput_free(xinput_in_use: Option<u8>) -> Option<u8> {
        xinput_in_use.map(|used| MAX_XINPUT_SLOTS.saturating_sub(used))
    }

    /// How many pads a spawn may still add before the total on the bus passes
    /// [`MAX_SLOTS`].
    pub fn spawn_headroom(on_bus: usize) -> u8 {
        MAX_SLOTS.saturating_sub(u8::try_from(on_bus).unwrap_or(u8::MAX))
    }

    /// How many of `count` pads of this persona no game could read.
    fn unreadable_count(count: u8, persona: Persona, free: Option<u8>) -> Option<u8> {
        if !persona.is_xinput() {
            // Nothing to read and nothing to fail to read: a HID pad takes no
            // XInput slot, so this answer does not depend on the reading.
            return Some(0);
        }
        free.map(|free| count.saturating_sub(free))
    }

    impl SpawnPlan {
        /// The stable code a refusal carries; `None` when nothing is refused.
        pub fn code(&self) -> Option<&'static str> {
            match self {
                SpawnPlan::SessionRunning => Some("session-running"),
                SpawnPlan::PersonaNotImplemented { .. } => Some("persona-not-implemented"),
                SpawnPlan::BadCount { .. } => Some("bad-count"),
                SpawnPlan::BadHold { .. } => Some("bad-hold"),
                SpawnPlan::BusFull { .. } => Some("bus-full"),
                SpawnPlan::Plug { .. } => None,
            }
        }

        /// ONE SENTENCE. Studio caps a flash at 300 characters and truncates
        /// the rest silently, so nothing here is allowed to be a paragraph —
        /// the page renders the long form from [`spawn_offer`] instead.
        pub fn message(&self) -> String {
            match self {
                SpawnPlan::SessionRunning => "a session is running, and those pads are the ones \
                     it is driving — stop emulation first"
                    .to_owned(),
                SpawnPlan::PersonaNotImplemented { persona } => {
                    format!("this build of ksx cannot create a {} pad", persona.label())
                }
                SpawnPlan::BadCount { given } => {
                    format!("pad count must be 1..={MAX_SLOTS}, got {given}")
                }
                SpawnPlan::BadHold { given } => {
                    format!("hold must be 1..={MAX_HOLD_SECS} seconds, got {given}")
                }
                SpawnPlan::BusFull { on_bus, requested } => format!(
                    "the bus already carries {on_bus} pad(s) and ksx's ceiling is {MAX_SLOTS}, \
                     so {requested} more will not fit — prune first"
                ),
                SpawnPlan::Plug {
                    count,
                    persona,
                    hold_secs,
                    unreadable,
                } => {
                    let head = format!(
                        "{count} {} pad(s) plugged — they unplug themselves in {hold_secs}s",
                        persona.as_str()
                    );
                    match unreadable {
                        Some(0) => format!("{head}."),
                        Some(n) => format!(
                            "{head}. {n} of them are past Windows' {MAX_XINPUT_SLOTS} XInput \
                             slots and no game can read those."
                        ),
                        // Not "all of them are readable". ksx did not look.
                        None => format!(
                            "{head}. ksx could not read the XInput slots, so it cannot say how \
                             many of them a game will see."
                        ),
                    }
                }
            }
        }

        /// The command that carries this out where it is not refused, so a
        /// refusal is never a dead end.
        pub fn remedy(&self) -> Option<String> {
            match self {
                SpawnPlan::SessionRunning => Some("run `ksx session stop`".to_owned()),
                SpawnPlan::PersonaNotImplemented { .. } => None,
                SpawnPlan::BadCount { .. } | SpawnPlan::BadHold { .. } => {
                    Some("pick a value from the list".to_owned())
                }
                SpawnPlan::BusFull { .. } => {
                    Some("clear the bus first (`ksx pads --prune`), or ask for fewer".to_owned())
                }
                SpawnPlan::Plug { .. } => None,
            }
        }
    }

    /// The XInput ceiling, stated for THIS machine right now — the sentence
    /// task #16 existed because nothing said. (The console says it too now,
    /// from [`super::ceiling_warning`]; this is the page's spelling of the
    /// same fact.)
    ///
    /// `xinput_in_use` comes from XInput itself, not from the ViGEm bus's
    /// child list, and the difference is the whole point. The bus can only
    /// ever show ksx its OWN virtual pads (`ksx_platform::virtual_pads`'s
    /// module header says so); a cabinet with two real wired Xbox pads and no
    /// virtual ones would report zero, and "so four more will be readable"
    /// would be wrong by two with nothing on the page hedging it.
    ///
    /// `None` is a FAILED READING and says so. It does not become zero, and
    /// the sentence does not promise a free count it never obtained.
    ///
    /// Even a successful reading only supports "at most": another process can
    /// take a slot between this read and the plug.
    pub fn xinput_line(xinput_in_use: Option<u8>) -> String {
        let head = format!(
            "Windows exposes exactly {MAX_XINPUT_SLOTS} XInput slots and no virtual bus can \
             create a fifth."
        );
        let tail = "PlayStation pads are plain HID and use none of the four.";
        let Some(in_use) = xinput_in_use else {
            return format!(
                "{head} ksx could not read how many of them are in use, so it cannot say how \
                 many more xbox360 pad(s) a game would see — that is a reading that failed, not \
                 an empty machine. {tail}"
            );
        };
        let free = MAX_XINPUT_SLOTS.saturating_sub(in_use);
        let mut line = format!(
            "{head} {in_use} of them {} in use right now — by any pad on this machine, real or \
             virtual — so at most {free} more xbox360 pad(s) will be readable. {tail}",
            if in_use == 1 { "is" } else { "are" },
        );
        if free == 0 {
            line.push_str(" Anything asked for now still plugs — and no game will see it.");
        }
        line
    }

    /// One `<option>` label for a pad count, with its consequence attached.
    ///
    /// **The consequence names the persona**, because it is not the same
    /// consequence for both. A persona-blind "8 pads — 4 invisible to games"
    /// contradicts the persona `<select>` sitting beside it ("playstation —
    /// plain HID, takes no XInput slot"), the card paragraph above it, and
    /// [`SpawnPlan::message`], which reports no warning at all after an
    /// eight-pad PlayStation spawn. Four sentences on one screen cannot
    /// disagree about one click.
    ///
    /// Naming both personas rather than tracking the `<select>` is deliberate:
    /// the no-JS paint is this page's baseline, and a label that only became
    /// correct once JavaScript rewired it would be wrong exactly where the
    /// page promises to still work.
    pub fn count_label(
        count: u8,
        free: Option<u8>,
        xinput_name: Option<&str>,
        hid_name: Option<&str>,
    ) -> String {
        let noun = if count == 1 { "pad" } else { "pads" };
        let head = format!("{count} {noun}");
        // No XInput persona on offer means no count of any persona can be
        // unreadable, and a warning would be crying wolf.
        let Some(xinput) = xinput_name else {
            return head;
        };
        let Some(free) = free else {
            return format!(
                "{head} — ksx could not read the XInput slots, so it cannot say how many a game \
                 would see"
            );
        };
        if count <= free {
            return head;
        }
        let readable = match free {
            0 => format!("none readable as {xinput}"),
            n => format!("only {n} readable as {xinput}"),
        };
        match hid_name {
            Some(hid) => format!("{head} — {readable}, all {count} as {hid}"),
            None => format!("{head} — {readable}"),
        }
    }

    /// One `<option>` label for a persona.
    pub fn persona_label(persona: Persona) -> String {
        if persona.is_xinput() {
            format!(
                "{} — takes one of the {MAX_XINPUT_SLOTS} XInput slots",
                persona.as_str()
            )
        } else {
            format!("{} — plain HID, takes no XInput slot", persona.as_str())
        }
    }

    /// One `<option>` label for a hold.
    pub fn hold_label(secs: u64) -> String {
        format!("{secs} seconds")
    }

    /// One line: how many pads the bus is carrying.
    ///
    /// The zero case ends in a full stop and the others in a colon, and that
    /// is load-bearing: the colon introduces the list of pads underneath it,
    /// so "0 virtual pads:" above nothing reads as a list that failed to
    /// render rather than as an empty bus.
    pub fn summary_line(count: usize) -> String {
        match count {
            0 => "no virtual pads on the ViGEm bus".to_owned(),
            1 => "1 virtual pad on the ViGEm bus:".to_owned(),
            n => format!("{n} virtual pads on the ViGEm bus:"),
        }
    }

    /// The ViGEmBus devnode, or why there is not one.
    ///
    /// `None` is not an error and must not read like one: a machine that never
    /// installed ViGEmBus has no devnode and also has no pads, which is a
    /// perfectly healthy state for a cabinet that has not been set up yet.
    /// (A devnode ksx could not *look* for is a different thing entirely, and
    /// is carried by [`ksx_api::PadsView::unreadable`], not by this.)
    pub fn bus_line(bus: Option<&str>) -> String {
        match bus {
            Some(id) => id.to_owned(),
            None => "none present — ViGEmBus is not installed, or its devnode has gone".to_owned(),
        }
    }

    /// Who is holding the pads, with the heuristic's limit stated rather than
    /// hidden — the collector matches known splitter process NAMES, so a
    /// third-party ViGEm feeder is invisible to it and "no owner" would be a
    /// stronger claim than the evidence supports.
    pub fn owners_line(owners: &[String]) -> String {
        if owners.is_empty() {
            return "no known splitter process is alive — a third-party ViGEm feeder would be \
                    invisible here"
                .to_owned();
        }
        owners.join(", ")
    }

    /// Whether a prune can work from this process, said before the click
    /// rather than after the refusal.
    pub fn elevation_line(elevated: Option<bool>) -> String {
        match elevated {
            Some(true) => "ksx is running elevated — it can restart the bus itself.".to_owned(),
            Some(false) => "ksx is NOT running elevated, and ksx never self-elevates — this \
                            prune will be refused. Run the command below from an elevated prompt \
                            instead."
                .to_owned(),
            None => "whether ksx is elevated could not be determined; if the prune is refused, \
                     run the command below from an elevated prompt."
                .to_owned(),
        }
    }

    /// The confirm panel's lead. Says "every pad listed here", because a bus
    /// restart cannot remove one pad and keep the others and a user about to
    /// press this needs that to be the sentence, not a footnote.
    pub fn confirm_line(count: usize) -> String {
        format!(
            "This removes {count} pad(s) by restarting the ViGEmBus devnode. Every pad listed \
             here goes, at once:"
        )
    }

    /// The whole spawn offer, every option already labelled.
    ///
    /// The counts stop at the bus's remaining headroom, so the menu can never
    /// offer a click [`plan_spawn`] would refuse — the same invariant that
    /// keeps unimplementable personas out of the persona list.
    pub fn spawn_offer(
        session_running: bool,
        xinput_in_use: Option<u8>,
        on_bus: usize,
    ) -> ksx_api::SpawnOffer {
        let free = xinput_free(xinput_in_use);
        let headroom = spawn_headroom(on_bus);
        let offered: Vec<Persona> = Persona::ALL
            .iter()
            .copied()
            .filter(|p| p.can_plug())
            .collect();
        // Whichever personas this build can actually create decide the wording
        // — nothing here spells "xbox360" or "playstation" by hand.
        let xinput_name = offered.iter().find(|p| p.is_xinput()).map(|p| p.as_str());
        let hid_name = offered.iter().find(|p| !p.is_xinput()).map(|p| p.as_str());
        ksx_api::SpawnOffer {
            counts: (1..=headroom)
                .map(|n| ksx_api::SpawnOption {
                    value: n.to_string(),
                    label: count_label(n, free, xinput_name, hid_name),
                })
                .collect(),
            personas: offered
                .iter()
                .map(|p| ksx_api::SpawnOption {
                    value: p.as_str().to_owned(),
                    label: persona_label(*p),
                })
                .collect(),
            holds: HOLD_CHOICES
                .iter()
                .map(|s| ksx_api::SpawnOption {
                    value: s.to_string(),
                    label: hold_label(*s),
                })
                .collect(),
            note: format!(
                "A spawn is a TEST: the pads plug, run the A/B/X/Y + stick pattern so you can \
                 see them move in joy.cpl, then unplug themselves when the hold expires. \
                 Nothing is written to config, the longest hold is {MAX_HOLD_SECS}s, and the \
                 bus is never allowed past {MAX_SLOTS} pads in total."
            ),
            refused: if session_running {
                Some(SpawnPlan::SessionRunning.message())
            } else if headroom == 0 {
                Some(
                    SpawnPlan::BusFull {
                        on_bus,
                        requested: 1,
                    }
                    .message(),
                )
            } else {
                None
            },
        }
    }

    /// [`PrunePlan`] on the wire, so no surface re-decides whether a bus
    /// restart is allowed.
    pub fn prune_plan_view(plan: &PrunePlan) -> ksx_api::PrunePlanView {
        let (kind, count) = match plan {
            PrunePlan::Nothing => ("nothing", 0),
            PrunePlan::NoBus => ("no-bus", 0),
            PrunePlan::SessionRunning { count } => ("session-running", *count),
            PrunePlan::Restart { count, .. } => ("restart", *count),
        };
        ksx_api::PrunePlanView {
            kind: kind.to_owned(),
            count,
            command: plan.command(),
            detail: plan.render(PruneMode::DryRun),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// An idle machine with all four XInput slots free and an empty bus.
        const IDLE: (Option<u8>, usize) = (Some(0), 0);

        /// The refusal that matters, and the one the CLI never had to make:
        /// whoever typed `ksx pads` knew emulation was stopped. A page click
        /// carries no such knowledge, so the plan makes the judgement.
        #[test]
        fn a_running_session_refuses_a_spawn_before_the_bus_is_opened() {
            let plan = plan_spawn(4, Persona::Xbox360, 30, true, IDLE.0, IDLE.1);
            assert_eq!(plan, SpawnPlan::SessionRunning);
            assert_eq!(plan.code(), Some("session-running"));
            assert!(
                plan.remedy()
                    .is_some_and(|r| r.contains("ksx session stop")),
                "a refusal with no way forward is just an error message"
            );
        }

        /// Task #16, as the page sees it: over the ceiling is a WARNING, not
        /// a refusal — and that is how #16 landed on the console too, so the
        /// two faces still describe one click the same way
        /// (`the_cli_warning_and_the_surface_plan_agree_on_the_number`).
        #[test]
        fn eight_xbox_pads_are_allowed_and_four_of_them_are_named_unreadable() {
            let plan = plan_spawn(8, Persona::Xbox360, 30, false, IDLE.0, IDLE.1);
            assert_eq!(
                plan,
                SpawnPlan::Plug {
                    count: 8,
                    persona: Persona::Xbox360,
                    hold_secs: 30,
                    unreadable: Some(4),
                }
            );
            assert_eq!(plan.code(), None, "over the ceiling is not a refusal");
            let message = plan.message();
            assert!(message.contains('4'), "{message}");
            assert!(message.contains("no game can read"), "{message}");
        }

        /// Slots already taken — by ANY pad on the machine, which is why this
        /// number comes from XInput and not from the ViGEm bus's child list.
        #[test]
        fn xinput_slots_already_taken_reduce_what_a_new_spawn_can_be_read_as() {
            let plan = plan_spawn(4, Persona::Xbox360, 30, false, Some(2), 0);
            let SpawnPlan::Plug { unreadable, .. } = plan else {
                panic!("expected a plug: {plan:?}");
            };
            assert_eq!(unreadable, Some(2));
            assert_eq!(xinput_free(Some(2)), Some(2));
            assert_eq!(xinput_free(Some(9)), Some(0), "saturating, never wrapping");
        }

        /// **A reading that failed is not a reading of zero.**
        ///
        /// Fails against the version this replaced, which counted ViGEmBus
        /// children into a plain `u8`: there was no way to express "could not
        /// ask", so an unanswerable machine reported four free slots and the
        /// plan promised that four more pads would be readable.
        #[test]
        fn an_unreadable_xinput_ceiling_is_never_folded_into_zero_in_use() {
            assert_eq!(xinput_free(None), None, "unknown in, unknown out");

            let plan = plan_spawn(4, Persona::Xbox360, 30, false, None, 0);
            let SpawnPlan::Plug { unreadable, .. } = plan.clone() else {
                panic!("expected a plug: {plan:?}");
            };
            assert_eq!(
                unreadable, None,
                "an unread ceiling must not report 0 pads unreadable"
            );
            let message = plan.message();
            assert!(message.contains("could not read"), "{message}");

            let line = xinput_line(None);
            assert!(line.contains("could not read"), "{line}");
            assert!(
                !line.contains("at most 4 more"),
                "a failed reading must not promise four free slots: {line}"
            );

            // …and the count labels must not promise either.
            let offer = spawn_offer(false, None, 0);
            for option in &offer.counts {
                assert!(
                    option.label.contains("could not read"),
                    "an unread ceiling must be said, not assumed: {}",
                    option.label
                );
            }

            // A HID pad's answer never depended on the reading, so it stays
            // knowable even when the slots are not.
            let hid = plan_spawn(8, Persona::PlayStation, 30, false, None, 0);
            let SpawnPlan::Plug { unreadable, .. } = hid else {
                panic!("expected a plug: {hid:?}");
            };
            assert_eq!(unreadable, Some(0));
        }

        /// PlayStation pads are plain HID: eight of them is eight readable
        /// pads, which is the whole reason MAX_SLOTS and MAX_XINPUT_SLOTS are
        /// different numbers.
        #[test]
        fn playstation_pads_never_count_against_the_xinput_ceiling() {
            let plan = plan_spawn(8, Persona::PlayStation, 30, false, Some(4), 0);
            let SpawnPlan::Plug { unreadable, .. } = plan else {
                panic!("expected a plug: {plan:?}");
            };
            assert_eq!(unreadable, Some(0));
        }

        /// A persona this build cannot create is refused before ViGEmBus is
        /// opened — a build limitation must never arrive shaped like a driver
        /// problem (the same ordering `run` keeps).
        #[test]
        fn an_unimplementable_persona_is_refused_and_not_offered() {
            let plan = plan_spawn(1, Persona::DualSense, 30, false, IDLE.0, IDLE.1);
            assert_eq!(plan.code(), Some("persona-not-implemented"));
            let offered: Vec<String> = spawn_offer(false, IDLE.0, IDLE.1)
                .personas
                .into_iter()
                .map(|o| o.value)
                .collect();
            assert!(
                !offered.iter().any(|v| v == "dualsense"),
                "a menu must not offer what the plan refuses: {offered:?}"
            );
            assert!(offered.iter().any(|v| v == "xbox360"), "{offered:?}");
        }

        #[test]
        fn counts_and_holds_are_bounded() {
            assert_eq!(
                plan_spawn(0, Persona::Xbox360, 30, false, IDLE.0, IDLE.1).code(),
                Some("bad-count")
            );
            assert_eq!(
                plan_spawn(MAX_SLOTS + 1, Persona::Xbox360, 30, false, IDLE.0, IDLE.1).code(),
                Some("bad-count")
            );
            assert_eq!(
                plan_spawn(4, Persona::Xbox360, 0, false, IDLE.0, IDLE.1).code(),
                Some("bad-hold")
            );
            assert_eq!(
                plan_spawn(
                    4,
                    Persona::Xbox360,
                    MAX_HOLD_SECS + 1,
                    false,
                    IDLE.0,
                    IDLE.1
                )
                .code(),
                Some("bad-hold")
            );
        }

        /// **Nothing bounded what repeated Spawns left on the bus.**
        ///
        /// Fails against the version this replaced, where `plan_spawn` capped
        /// a SINGLE call at `1..=MAX_SLOTS` and nothing looked at the total:
        /// five submits of `count=16` left eighty pads on the bus for two
        /// minutes, and the only cleanup was a prune an unelevated Studio
        /// cannot perform. The ceiling has to be on the total.
        #[test]
        fn the_total_on_the_bus_is_bounded_not_just_one_request() {
            let plan = plan_spawn(
                MAX_SLOTS,
                Persona::Xbox360,
                30,
                false,
                Some(0),
                usize::from(MAX_SLOTS),
            );
            assert_eq!(plan.code(), Some("bus-full"), "{plan:?}");
            assert!(
                plan.remedy().is_some_and(|r| r.contains("prune")),
                "the way back must be named: {:?}",
                plan.remedy()
            );

            // One under the ceiling: exactly one more pad fits, and no more.
            let headroom = usize::from(MAX_SLOTS) - 1;
            assert_eq!(
                plan_spawn(1, Persona::Xbox360, 30, false, Some(0), headroom).code(),
                None
            );
            assert_eq!(
                plan_spawn(2, Persona::Xbox360, 30, false, Some(0), headroom).code(),
                Some("bus-full")
            );
        }

        /// The menu is the other half of that bound: a `<select>` must never
        /// offer a click the plan would refuse. Fails against the version that
        /// always offered `1..=MAX_SLOTS` whatever the bus was already
        /// carrying.
        #[test]
        fn the_menu_never_offers_a_count_the_plan_would_refuse() {
            for on_bus in [0usize, 1, 9, usize::from(MAX_SLOTS) - 1] {
                let offer = spawn_offer(false, Some(0), on_bus);
                assert_eq!(
                    offer.counts.len(),
                    usize::from(spawn_headroom(on_bus)),
                    "on_bus {on_bus}"
                );
                for option in &offer.counts {
                    let count: u8 = option.value.parse().unwrap();
                    assert_eq!(
                        plan_spawn(count, Persona::Xbox360, 30, false, Some(0), on_bus).code(),
                        None,
                        "on_bus {on_bus} offers {count}, which the plan refuses"
                    );
                }
                assert!(offer.refused.is_none(), "on_bus {on_bus}");
            }

            // A full bus offers nothing at all, and says why rather than
            // rendering an empty dropdown beside a live button.
            let full = spawn_offer(false, Some(0), usize::from(MAX_SLOTS));
            assert!(full.counts.is_empty());
            assert!(
                full.refused.is_some_and(|r| r.contains("prune")),
                "a full bus must refuse the offer in words"
            );
        }

        /// **The count label must name the persona its warning applies to.**
        ///
        /// Fails against the version this replaced, whose labels were
        /// persona-blind: with `persona=playstation, count=8` the dropdown
        /// said "8 pads — only 4 readable, 4 invisible to games (XInput)"
        /// while the persona option beside it said "playstation — plain HID,
        /// takes no XInput slot", the card above said eight players is eight
        /// readable pads, and the post-submit flash reported no warning at
        /// all. Four sentences, one click, three of them wrong.
        #[test]
        fn count_labels_name_the_persona_the_warning_applies_to() {
            let offer = spawn_offer(false, Some(0), 0);
            let hid: Vec<&str> = offer
                .personas
                .iter()
                .filter(|p| p.label.contains("plain HID"))
                .map(|p| p.value.as_str())
                .collect();
            assert!(!hid.is_empty(), "the fixture needs a HID persona on offer");

            for option in &offer.counts {
                let count: u8 = option.value.parse().unwrap();
                if count <= MAX_XINPUT_SLOTS {
                    assert_eq!(
                        option.label,
                        format!("{count} {}", if count == 1 { "pad" } else { "pads" }),
                        "a count every persona can serve must not cry wolf"
                    );
                    continue;
                }
                assert!(
                    option.label.contains("as xbox360"),
                    "the warning must name the persona it applies to: {}",
                    option.label
                );
                assert!(
                    hid.iter().any(|h| option.label.contains(*h)),
                    "…and must say the HID persona is unaffected: {}",
                    option.label
                );
                // The label and the plan cannot disagree about the same click.
                let xbox = plan_spawn(count, Persona::Xbox360, 30, false, Some(0), 0);
                let SpawnPlan::Plug { unreadable, .. } = xbox else {
                    panic!("expected a plug: {xbox:?}");
                };
                assert_eq!(unreadable, Some(count - MAX_XINPUT_SLOTS));
                let ps = plan_spawn(count, Persona::PlayStation, 30, false, Some(0), 0);
                let SpawnPlan::Plug { unreadable, .. } = ps else {
                    panic!("expected a plug: {ps:?}");
                };
                assert_eq!(
                    unreadable,
                    Some(0),
                    "the label promises the HID persona is readable; the plan must agree"
                );
            }
        }

        /// A full XInput ceiling makes every XInput count unreadable — and
        /// still must not claim a HID pad is invisible. Fails against the
        /// version this replaced, where a full bus labelled EVERY count "none
        /// readable (XInput slots are full)" including for the persona where
        /// every one of them is readable.
        #[test]
        fn a_full_xinput_ceiling_does_not_cry_wolf_about_the_hid_persona() {
            let offer = spawn_offer(false, Some(MAX_XINPUT_SLOTS), 0);
            for option in &offer.counts {
                let count: u8 = option.value.parse().unwrap();
                assert!(
                    option.label.contains("none readable as xbox360"),
                    "{}",
                    option.label
                );
                assert!(
                    option
                        .label
                        .contains(&format!("all {count} as playstation")),
                    "{}",
                    option.label
                );
            }
        }

        /// A live session must disable the whole panel, not just fail on
        /// submit — the offer says so itself.
        #[test]
        fn a_running_session_refuses_the_offer_not_just_the_submit() {
            assert!(spawn_offer(true, IDLE.0, IDLE.1).refused.is_some());
            assert!(spawn_offer(false, IDLE.0, IDLE.1).refused.is_none());
        }

        /// The ceiling sentence names the number, what is holding it and what
        /// is left — and never promises more than "at most", because another
        /// process can take a slot between this read and the plug.
        #[test]
        fn the_ceiling_sentence_states_the_numbers_and_promises_no_more_than_it_can() {
            let clean = xinput_line(Some(2));
            assert!(clean.contains('4'), "{clean}");
            assert!(clean.contains("2 of them are in use"), "{clean}");
            assert!(
                clean.contains("at most 2 more"),
                "a slot count is a ceiling, not a promise: {clean}"
            );

            let full = xinput_line(Some(4));
            assert!(full.contains("no game will see it"), "{full}");
        }

        /// Every plan's message is one line and fits a Studio flash (300
        /// chars, truncated silently past that).
        #[test]
        fn every_spawn_message_survives_a_flash() {
            for plan in [
                plan_spawn(4, Persona::Xbox360, 30, true, Some(0), 0),
                plan_spawn(0, Persona::Xbox360, 30, false, Some(0), 0),
                plan_spawn(4, Persona::Xbox360, 0, false, Some(0), 0),
                plan_spawn(1, Persona::DualSense, 30, false, Some(0), 0),
                plan_spawn(16, Persona::Xbox360, 120, false, Some(0), 0),
                plan_spawn(16, Persona::Xbox360, 120, false, None, 0),
                plan_spawn(16, Persona::Xbox360, 120, false, Some(0), 99),
            ] {
                let message = plan.message();
                assert!(message.chars().count() <= 300, "{message}");
                assert!(!message.contains('\n'), "{message}");
            }
        }

        /// The prune decision crosses to the wire unchanged, kind included —
        /// a surface keys its panels off `kind`, never off the prose.
        #[test]
        fn the_prune_plan_reaches_the_wire_with_its_kind_and_its_command() {
            let bus = r"ROOT\SYSTEM\0002";
            let view = prune_plan_view(&super::super::plan_prune(Some(bus), 15, false));
            assert_eq!(view.kind, "restart");
            assert_eq!(view.count, 15);
            assert!(view.command.is_some_and(|c| c.contains("pnputil")));
            assert!(view.detail.contains("15"), "{}", view.detail);

            let busy = prune_plan_view(&super::super::plan_prune(Some(bus), 4, true));
            assert_eq!(busy.kind, "session-running");
            assert_eq!(
                busy.command, None,
                "a refusal must not hand over the command that does it anyway"
            );

            assert_eq!(
                prune_plan_view(&super::super::plan_prune(Some(bus), 0, false)).kind,
                "nothing"
            );
            assert_eq!(
                prune_plan_view(&super::super::plan_prune(None, 3, false)).kind,
                "no-bus"
            );
        }

        /// An empty bus must not open a list it has no rows for. The colon is
        /// the tell: it introduces the pad tiles underneath, so a summary that
        /// ends in one above nothing reads as a render that broke.
        ///
        /// Fails against the obvious one-arm implementation
        /// (`format!("{n} virtual pads on the ViGEm bus:")`), which is what
        /// this replaced a test that just restated the three match arms with.
        #[test]
        fn an_empty_bus_summary_does_not_open_a_list_it_has_no_rows_for() {
            assert!(!summary_line(0).ends_with(':'), "{}", summary_line(0));
            for count in [1usize, 2, 15] {
                assert!(
                    summary_line(count).ends_with(':'),
                    "a non-empty bus introduces its rows: {}",
                    summary_line(count)
                );
                assert!(summary_line(count).contains(&count.to_string()));
            }
            // Singular/plural, because "1 virtual pads" is the kind of thing
            // that makes a user doubt the number itself.
            assert!(summary_line(1).contains("1 virtual pad on"));
            assert!(summary_line(7).contains("7 virtual pads on"));
        }

        /// The four worded helpers. Each has a case that must not read like an
        /// error, and one that must not read like an absence.
        #[test]
        fn the_worded_helpers_stay_honest_about_absence() {
            assert_eq!(bus_line(Some("ROOT\\X")), "ROOT\\X");
            assert!(bus_line(None).contains("not installed"));

            assert_eq!(owners_line(&["a".to_owned(), "b".to_owned()]), "a, b");
            assert!(
                owners_line(&[]).contains("no known splitter"),
                "never claim there is no owner — the collector matches process NAMES"
            );

            assert!(elevation_line(Some(true)).contains("is running elevated"));
            assert!(elevation_line(Some(false)).contains("NOT running elevated"));
            assert!(elevation_line(None).contains("could not be determined"));

            assert!(confirm_line(15).contains("15 pad(s)"));
            assert!(confirm_line(15).contains("at once"));
        }
    }

    /// Plug `count` pads, run the visible pattern for `hold_secs`, unplug —
    /// with no printing, no exit code and no Ctrl+C latch.
    ///
    /// [`super::run`] is the console command and cannot be reused: it prints
    /// its rows, it `std::process::exit`s on a missing driver, and its pattern
    /// loop polls a `SetConsoleCtrlHandler` latch that a long-lived GUI
    /// process must not have installed. This is the same three steps for a
    /// caller that lives inside another process.
    ///
    /// **The plug is synchronous and the hold is not.** A surface that
    /// returned "4 pads plugged" and then discovered ViGEmBus was missing on a
    /// background thread would have lied to the user; a surface that blocked
    /// for the whole hold would be unusable. So the driver is opened and every
    /// pad is plugged before this returns, and only the pattern + unplug run
    /// on the thread — which also means the handles outlive this call, because
    /// a pad exists exactly as long as the handle that made it.
    #[cfg(windows)]
    pub fn plug_and_hold(count: u8, persona: Persona, hold_secs: u64) -> Result<(), PlugError> {
        use std::time::{Duration, Instant};

        use ksx_core::PadState;
        use ksx_output::{OutputError, RoutedBackend, VigemBackend, VirtualPadBackend as _};

        // Refused before ViGEmBus is opened, exactly as `run` does it: a build
        // limitation must not arrive after a driver handshake wearing a driver
        // problem's clothes.
        if !persona.can_plug() {
            return Err(OutputError::PersonaNotImplemented(persona).into());
        }
        let vigem = VigemBackend::connect()?;
        let mut backend = RoutedBackend::standard(Box::new(vigem));
        let mut handles = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            match backend.plug_persona(persona) {
                Ok(handle) => handles.push(handle),
                Err(err) => {
                    // Half a spawn is worse than none: it leaves pads nobody
                    // asked for on the bus AND reports a failure. Undo, then
                    // fail.
                    for handle in handles {
                        let _ = backend.unplug(handle);
                    }
                    return Err(err.into());
                }
            }
        }

        std::thread::Builder::new()
            .name("ksx-pad-test".to_owned())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(hold_secs);
                let mut tick: u64 = 0;
                while Instant::now() < deadline {
                    let state = super::pattern_state(tick);
                    for &handle in &handles {
                        if let Err(err) = backend.update(handle, &state) {
                            tracing::warn!(%err, ?handle, "pad-test pattern update failed");
                        }
                    }
                    tick += 1;
                    std::thread::sleep(super::TICK);
                }
                for handle in handles {
                    // Released before the unplug so nothing is left "pressed",
                    // and unplugged even if the release failed — a pad left on
                    // the bus is precisely the ghost `--prune` exists to clear.
                    let _ = backend.update(handle, &PadState::default());
                    if let Err(err) = backend.unplug(handle) {
                        tracing::warn!(%err, ?handle, "pad-test unplug failed");
                    }
                }
            })
            // A spawn that cannot get a thread has already plugged the pads,
            // and nothing would ever unplug them. Say so, do not leave ghosts.
            .map_err(PlugError::HoldThread)?;
        Ok(())
    }

    /// Why [`plug_and_hold`] did not leave pads running.
    ///
    /// Two members and they are not the same news. Everything the driver
    /// refuses left the bus exactly as it found it; [`PlugError::HoldThread`]
    /// means the pads ARE on the bus with nothing scheduled to remove them,
    /// which is the one outcome that needs the prune command in its message.
    #[cfg(windows)]
    #[derive(Debug, thiserror::Error)]
    pub enum PlugError {
        #[error(transparent)]
        Output(#[from] ksx_output::OutputError),
        #[error(
            "the pads are plugged but the hold thread could not start ({0}) — run \
             `ksx pads --prune` to clear them"
        )]
        HoldThread(std::io::Error),
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    const BUS: &str = r"ROOT\SYSTEM\0002";

    /// The refusal that matters: those pads belong to whoever is playing.
    ///
    /// A prune restarts the bus, which unplugs EVERY pad on it. Doing that
    /// during a session takes the controller out of a player's hands mid-game,
    /// and "there were a lot of pads" is not a reason a player would accept.
    #[test]
    fn a_running_session_is_refused_before_anything_is_touched() {
        let plan = plan_prune(Some(BUS), 4, true);
        assert_eq!(plan, PrunePlan::SessionRunning { count: 4 });
        assert_eq!(
            plan.command(),
            None,
            "a refusal must not hand over a command that would do the thing anyway"
        );
        let text = plan.render(PruneMode::DryRun);
        assert!(text.contains("REFUSED"), "{text}");
        assert!(
            text.contains("ksx session stop"),
            "a refusal with no way forward is just an error message: {text}"
        );
    }

    /// The case on the reference cabinet: daemon up, no session, 15 pads.
    #[test]
    fn an_idle_daemon_with_pads_on_the_bus_is_prunable() {
        let plan = plan_prune(Some(BUS), 15, false);
        assert_eq!(
            plan,
            PrunePlan::Restart {
                bus_instance_id: BUS.to_owned(),
                count: 15,
            }
        );
        assert_eq!(
            plan.command().as_deref(),
            Some(r#"pnputil /restart-device "ROOT\SYSTEM\0002""#),
            "the dry run must print a command a user can run by hand"
        );
    }

    /// Telling someone to "re-run with --yes" when they just passed --yes is
    /// the kind of stale instruction that makes a user doubt the rest of the
    /// output — and this path is reached exactly when they DID mean it and
    /// something else stopped them.
    #[test]
    fn a_blocked_run_does_not_tell_you_to_pass_a_flag_you_already_passed() {
        let text = plan_prune(Some(BUS), 15, false).render(PruneMode::Blocked);
        assert!(text.contains("Nothing was changed"), "{text}");
        assert!(
            !text.contains("--yes"),
            "they passed --yes; saying it again is wrong: {text}"
        );
    }

    #[test]
    fn a_dry_run_says_it_changed_nothing_and_a_real_one_does_not() {
        let plan = plan_prune(Some(BUS), 15, false);
        assert!(plan
            .render(PruneMode::DryRun)
            .contains("Nothing was changed"));
        assert!(
            !plan
                .render(PruneMode::Doing)
                .contains("Nothing was changed"),
            "a real prune must not claim it did nothing"
        );
    }

    #[test]
    fn an_empty_bus_is_not_an_error() {
        assert_eq!(plan_prune(Some(BUS), 0, false), PrunePlan::Nothing);
        // And "no session" must not turn an empty bus into work.
        assert_eq!(plan_prune(Some(BUS), 0, true), PrunePlan::Nothing);
    }

    /// Pads counted but no devnode to restart is a hand-built report or a bus
    /// that vanished mid-collect. Nothing to act on, and saying so beats
    /// running pnputil against an empty string.
    #[test]
    fn pads_with_no_bus_devnode_has_nothing_to_restart() {
        assert_eq!(plan_prune(None, 3, false), PrunePlan::NoBus);
        assert_eq!(plan_prune(None, 3, false).command(), None);
    }
}

#[cfg(test)]
mod tests {
    use ksx_platform::{DriverFileReport, ServiceInfo, ServiceState, StartType};

    use super::*;

    #[test]
    fn exit_code_for_missing_driver_is_2() {
        assert_eq!(EXIT_DRIVER_MISSING, 2);
    }

    #[test]
    fn pattern_is_deterministic() {
        for tick in 0..200 {
            assert_eq!(pattern_state(tick), pattern_state(tick), "tick {tick}");
        }
    }

    #[test]
    fn buttons_cycle_a_b_x_y_at_400ms_each() {
        let expect = [
            (0, XButtons::A),
            (3, XButtons::A),
            (4, XButtons::B),
            (7, XButtons::B),
            (8, XButtons::X),
            (11, XButtons::X),
            (12, XButtons::Y),
            (15, XButtons::Y),
            (16, XButtons::A), // wraps
        ];
        for (tick, button) in expect {
            assert_eq!(pattern_state(tick).buttons, button, "tick {tick}");
        }
    }

    #[test]
    fn exactly_one_face_button_at_a_time() {
        for tick in 0..64 {
            let buttons = pattern_state(tick).buttons;
            assert_eq!(buttons.bits().count_ones(), 1, "tick {tick}: {buttons:?}");
            assert!(
                (XButtons::A | XButtons::B | XButtons::X | XButtons::Y).contains(buttons),
                "tick {tick}: {buttons:?}"
            );
        }
    }

    #[test]
    fn triggers_pulse_full_range_out_of_phase() {
        assert_eq!(triangle(0), 0);
        assert_eq!(triangle(8), 255);
        assert_eq!(triangle(16), 0);
        for tick in 0..64 {
            let state = pattern_state(tick);
            // RT is LT delayed by half a period.
            assert_eq!(state.rt, triangle(tick + 8), "tick {tick}");
            // Periodicity.
            assert_eq!(state.lt, pattern_state(tick + 16).lt, "tick {tick}");
        }
    }

    #[test]
    fn stick_sweep_is_circular_and_periodic() {
        for tick in 0..40 {
            let state = pattern_state(tick);
            // Constant magnitude (within integer-truncation slack).
            let mag = (f64::from(state.lx).powi(2) + f64::from(state.ly).powi(2)).sqrt();
            let amp = f64::from(AXIS_MAX);
            assert!((mag - amp).abs() < amp * 0.01, "tick {tick}: |l| = {mag}");
            // Right stick mirrors the left (counter-rotation). `ly` is never
            // i16::MIN (amplitude is AXIS_MAX = 32767), so the negation is safe.
            assert_eq!(state.rx, state.lx, "tick {tick}");
            assert_eq!(state.ry, -state.ly, "tick {tick}");
            // Periodicity.
            let next = pattern_state(tick + STICK_PERIOD_TICKS);
            assert_eq!((state.lx, state.ly), (next.lx, next.ly), "tick {tick}");
        }
    }

    #[test]
    fn stick_sweep_visits_all_four_quadrants() {
        let mut quadrants = [false; 4];
        for tick in 0..STICK_PERIOD_TICKS {
            let state = pattern_state(tick);
            if state.lx != 0 && state.ly != 0 {
                let q = match (state.lx > 0, state.ly > 0) {
                    (true, true) => 0,
                    (false, true) => 1,
                    (false, false) => 2,
                    (true, false) => 3,
                };
                quadrants[q] = true;
            }
        }
        assert_eq!(quadrants, [true; 4]);
    }

    fn fixture_driver() -> BusDriverReport {
        BusDriverReport {
            installed: true,
            service: Some(ServiceInfo {
                start_type: StartType::Demand,
                image_path: Some("System32\\drivers\\ViGEmBus.sys".into()),
                display_name: Some("Nefarius Virtual Gamepad Emulation Bus Driver".into()),
                state: ServiceState::Running,
            }),
            driver_file: Some(DriverFileReport {
                path: "C:\\Windows\\System32\\drivers\\ViGEmBus.sys".into(),
                file_version: Some("1.21.442.0".into()),
                file_version_string: None,
                company: Some("Nefarius Software Solutions e.U.".into()),
                description: None,
                signature: None,
            }),
        }
    }

    #[test]
    fn pads_json_shape() {
        let rows = [
            PadRow {
                slot: 1,
                handle: 0,
                persona: ksx_core::Persona::Xbox360,
                user_index: Some(0),
                led_number: Some(2),
            },
            PadRow {
                slot: 2,
                handle: 1,
                persona: ksx_core::Persona::PlayStation,
                user_index: None,
                led_number: None,
            },
        ];
        let v = pads_json(&fixture_driver(), &rows);
        insta::assert_snapshot!(serde_json::to_string_pretty(&v).unwrap());
    }

    // -----------------------------------------------------------------------
    // The ceiling warning `ksx pads` prints before it plugs anything (task #16)
    // -----------------------------------------------------------------------

    use ksx_core::{Persona, MAX_XINPUT_SLOTS};

    /// **Eight xbox360 pads, four of them dead, and nothing said so.**
    ///
    /// Fails against the version this replaced, where `run` printed nothing
    /// before the plug loop: `ksx pads --count 8 --persona xbox360` plugged
    /// eight pads, animated all eight, and four were invisible to every game
    /// with no line anywhere admitting it. The number has to be NAMED, and it
    /// has to arrive before the plug — a post-mortem over eight animating pads
    /// is not a warning.
    #[test]
    fn eight_xinput_pads_on_an_idle_machine_name_the_four_no_game_will_see() {
        let warning = ceiling_warning(8, Persona::Xbox360, Some(0))
            .expect("eight XInput pads past a four-slot ceiling must warn");
        assert!(warning.starts_with("warning:"), "{warning}");
        assert!(
            warning.contains(&format!("{} of these 8", 8 - MAX_XINPUT_SLOTS)),
            "the count of invisible pads must be named: {warning}"
        );
        assert!(warning.contains("invisible to every game"), "{warning}");
        assert!(
            warning.contains(&MAX_XINPUT_SLOTS.to_string()),
            "the ceiling itself must be named: {warning}"
        );
        // Warn and PROCEED — this command's convention, and the one thing a
        // reader must not have to guess after a paragraph of bad news.
        assert!(warning.contains("warning, not a refusal"), "{warning}");
    }

    /// The remedy is the measured one, and it is a command that works.
    ///
    /// Fails against a warning that only stated the problem: "4 of these will
    /// be invisible" with no way forward is the shape of message that gets
    /// read once and ignored after. The persona named is derived from the
    /// roster, so a build that cannot plug it cannot be told to use it.
    #[test]
    fn the_warning_hands_over_the_fix_that_was_actually_measured() {
        let hid = hid_persona().expect("this build plugs a HID persona");
        assert!(!hid.is_xinput(), "the fix must not take an XInput slot");
        assert!(hid.can_plug(), "a remedy this build cannot run is not one");

        let warning = ceiling_warning(8, Persona::Xbox360, Some(0)).unwrap();
        assert!(
            warning.contains(&format!("ksx pads --count 8 --persona {hid}")),
            "the fix must be a command, with the count they asked for: {warning}"
        );
        assert!(
            warning.contains("m6.5-ds4-findings.md"),
            "a claim about six HID pads is measured, not reasoned — cite it: {warning}"
        );
        assert!(warning.contains("XInput count unmoved"), "{warning}");
        // …and the shape that measurement actually ran, for a real panel.
        assert!(
            warning.contains(&format!("slots {}+", MAX_XINPUT_SLOTS + 1)),
            "{warning}"
        );
    }

    /// A HID persona never triggers it, at any count. Fails against a
    /// persona-blind warning keyed on `count > MAX_XINPUT_SLOTS` alone, which
    /// would tell someone plugging eight PlayStation pads that four of them
    /// are invisible — contradicting `--help`, the persona's own doc, and
    /// `surface::plan_spawn`, which reports no warning for that same spawn.
    #[test]
    fn a_hid_persona_is_never_warned_about_however_many_are_asked_for() {
        for count in [1u8, MAX_XINPUT_SLOTS, MAX_XINPUT_SLOTS + 1, 16] {
            for in_use in [Some(0), Some(MAX_XINPUT_SLOTS), None] {
                assert_eq!(
                    ceiling_warning(count, Persona::PlayStation, in_use),
                    None,
                    "count {count}, in use {in_use:?}"
                );
            }
        }
    }

    /// A count that fits is silent. Crying wolf on every `ksx pads` is how a
    /// warning stops being read.
    #[test]
    fn a_count_that_fits_the_free_slots_says_nothing() {
        for count in 1..=MAX_XINPUT_SLOTS {
            assert_eq!(ceiling_warning(count, Persona::Xbox360, Some(0)), None);
        }
        // …and "fits" is measured against what is FREE, not against the
        // ceiling: two slots already taken makes three pads two too many.
        assert_eq!(ceiling_warning(2, Persona::Xbox360, Some(2)), None);
        let warning = ceiling_warning(3, Persona::Xbox360, Some(2)).unwrap();
        assert!(warning.contains("1 of these 3"), "{warning}");
        assert!(warning.contains("2 of them are in use"), "{warning}");
        assert!(
            warning.contains("at most 2 of these will be readable"),
            "a slot count is a ceiling, not a promise: {warning}"
        );
    }

    /// **A reading that failed is not a reading of zero** — the rule
    /// `surface::xinput_free` exists for, applied to the CLI's own sentence.
    ///
    /// Fails against a warning that folded `None` into `0`: it would have
    /// announced "0 of them are in use right now" on a machine it never
    /// looked at, and stayed silent about four pads on a machine whose four
    /// slots were already full.
    #[test]
    fn an_unread_ceiling_is_said_and_never_assumed_to_be_empty() {
        // Past the ceiling, one thing stays knowable whatever the occupancy:
        // no machine has fewer than none in use, so the excess is a FLOOR.
        let over = ceiling_warning(8, Persona::Xbox360, None).unwrap();
        assert!(
            over.contains(&format!("at least {}", 8 - MAX_XINPUT_SLOTS)),
            "{over}"
        );
        assert!(over.contains("could not read"), "{over}");
        assert!(
            !over.contains("0 of them"),
            "an unread ceiling must not report an empty machine: {over}"
        );

        // Under the ceiling nothing is knowable at all, and silence would be
        // the claim that everything is fine — on four real pads it is not.
        let under = ceiling_warning(MAX_XINPUT_SLOTS, Persona::Xbox360, None)
            .expect("an unread ceiling must be said, not assumed");
        assert!(under.contains("could not read"), "{under}");
        assert!(
            !under.contains("invisible to every game"),
            "ksx did not look; it cannot claim any pad is dead: {under}"
        );
    }

    /// A full ceiling: every pad asked for is invisible, and the sentence
    /// still has to be arithmetic rather than a special case.
    #[test]
    fn a_full_ceiling_makes_every_requested_pad_unreadable() {
        let warning = ceiling_warning(2, Persona::Xbox360, Some(MAX_XINPUT_SLOTS)).unwrap();
        assert!(warning.contains("2 of these 2"), "{warning}");
        assert!(
            warning.contains("at most 0 of these will be readable"),
            "{warning}"
        );
    }

    /// The CLI warning and the surface plan describe the SAME click with the
    /// same number. Two faces of one backend cannot disagree about how many
    /// pads a game will see (docs/SURFACES.md §1).
    #[test]
    fn the_cli_warning_and_the_surface_plan_agree_on_the_number() {
        for (count, in_use) in [(8u8, 0u8), (5, 1), (6, 3), (4, 4)] {
            let surface::SpawnPlan::Plug { unreadable, .. } =
                surface::plan_spawn(count, Persona::Xbox360, 30, false, Some(in_use), 0)
            else {
                panic!("expected a plug for {count}/{in_use}");
            };
            let unreadable = unreadable.expect("a read ceiling gives a number");
            let warning = ceiling_warning(count, Persona::Xbox360, Some(in_use));
            if unreadable == 0 {
                assert_eq!(warning, None, "{count} pads, {in_use} in use");
                continue;
            }
            let warning = warning.expect("the plan says some are unreadable; the CLI must too");
            assert!(
                warning.contains(&format!("{unreadable} of these {count}")),
                "{count} pads, {in_use} in use: {warning}"
            );
        }
    }

    #[test]
    fn error_json_shape() {
        let v = error_json("vigembus-missing", "ViGEmBus is not installed");
        assert_eq!(
            v.pointer("/error/code"),
            Some(&serde_json::json!("vigembus-missing"))
        );
        assert_eq!(
            v.pointer("/error/message"),
            Some(&serde_json::json!("ViGEmBus is not installed"))
        );
    }
}
