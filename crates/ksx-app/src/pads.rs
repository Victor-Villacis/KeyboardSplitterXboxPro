//! `ksx pads` — plug N virtual pads, show each pad's XInput identity, run a
//! visible test pattern, then unplug cleanly.
//!
//! Exit code [`EXIT_DRIVER_MISSING`] (2) = ViGEmBus is not installed: the
//! human-readable install hint goes to stderr; `--json` puts
//! `{"error":{code,message}}` on stdout instead. Ctrl+C is handled by the
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

#[cfg(windows)]
pub fn run(
    count: u8,
    persona: ksx_core::Persona,
    hold_secs: u64,
    json: bool,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_output::{RoutedBackend, VigemBackend, VirtualPadBackend as _};

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
