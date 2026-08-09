//! The M6 daemon contract, driven end to end: **one claim, many sessions**.
//!
//! These tests exist because the ones next to them did not catch the bug they
//! were written for. `daemon::tests` drives the control loop against a
//! `RecordingPanel` that only writes strings into a log, so it kept passing
//! while the production wiring handed the loop a `NoPanel` and every session
//! built (and dropped) its own WinUSB claim. Everything the panel is *for* —
//! the claim outliving the session, the typethrough typing in between, the
//! keys reaching the engine through a borrowed claim — was invisible to it.
//!
//! So nothing here is a stand-in for the thing under test:
//!
//! | piece | what runs |
//! |---|---|
//! | control loop | the real [`control_loop_with`] |
//! | panel | the real [`Panel`], with its real typethrough thread |
//! | session capture | the real `crate::capture::build_session`, borrowing that panel |
//! | session | the real `supervise()` — real engine, real teardown order |
//! | injector | [`RecordingInjector`] — a log instead of `SendInput` |
//! | pads | `ksx_output::MockBackend` |
//! | the claim | [`WireClaim`]: a `CaptureBackend` over a channel the test drives |
//!
//! `WireClaim` is the only substitution, and it substitutes exactly one thing:
//! the USB endpoint. It is opened once, counted, and observed for release —
//! which is precisely the property "the daemon owns the claim for its whole
//! lifetime" makes a claim about.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ksx_capture::{
    CaptureBackend, CaptureCtl, DeviceInfo, DeviceKind, EscapeHandle, EscapeWatch, ExitReason,
    Handles, HealthHandle, PresenceHandle,
};
use ksx_core::{DeviceId, Key, KeyEvent, Preset, ResolvedSlot, SlotSpec};
use ksx_output::{MockBackend, VirtualPadBackend};
use ksx_platform::inject::{InjectionLog, RecordingInjector};

use super::panel::{Panel, PanelKeyboardHandle};
use super::{
    control_loop_with, DaemonCommand, DaemonState, RunState, SessionFactory, SessionRunner,
    SessionSummary, SharedState,
};
use crate::run::plan::{PlanSource, RunPlan};
use crate::run::supervisor::{self, Outcome, RunOptions, Step, Trace, Wiring};

/// The cabinet's I-PAC keyboard interface, as the WinUSB backend identifies it:
/// a device *instance path*, not an Interception hardware id.
const PANEL: &str = r"USB\VID_D209&PID_0430&MI_00\7&1A2B3C4D&0&0000";

/// Long enough that a wedged thread fails the test instead of hanging CI,
/// short enough that a real failure is reported this decade. Nothing is timed
/// against it — it only converts "never" into "failed".
const DEADLINE: Duration = Duration::from_secs(10);

fn eventually(what: &str, mut pred: impl FnMut() -> bool) {
    let until = Instant::now() + DEADLINE;
    while Instant::now() < until {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out after {DEADLINE:?} waiting for: {what}");
}

fn event(key: Key, down: bool) -> KeyEvent {
    KeyEvent {
        device: DeviceId::from(PANEL),
        key,
        down,
        t: 0,
    }
}

// ---------------------------------------------------------------------------
// The claim
// ---------------------------------------------------------------------------

/// A [`CaptureBackend`] standing in for one claimed WinUSB interface.
///
/// Everything about it that matters is observable: how many times it was
/// *opened* (`runs` — a per-session claim would count more than one), and
/// whether it was released (`released`, set as its thread returns). Its event
/// source is a channel, so the test decides exactly when the panel produces a
/// keystroke and in which phase.
struct WireClaim {
    device: DeviceInfo,
    wire: Receiver<KeyEvent>,
    handles: Handles,
    presence: PresenceHandle,
    runs: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
}

impl WireClaim {
    fn new(
        wire: Receiver<KeyEvent>,
        handles: Handles,
        runs: Arc<AtomicUsize>,
        released: Arc<AtomicBool>,
    ) -> Self {
        let device = DeviceInfo {
            id: DeviceId::from(PANEL),
            // Claimed interfaces have no Interception slot — that is the point.
            interception_slot: None,
            friendly: Some("I-PAC (ksx WinUSB claim MI_00)".into()),
            kind: DeviceKind::Keyboard,
        };
        let presence = PresenceHandle::new();
        presence.publish(vec![device.id.clone()]);
        Self {
            device,
            wire,
            handles,
            presence,
            runs,
            released,
        }
    }
}

impl CaptureBackend for WireClaim {
    fn devices(&mut self) -> Vec<DeviceInfo> {
        vec![self.device.clone()]
    }

    fn health(&self) -> HealthHandle {
        self.handles.health.clone()
    }

    fn escapes(&self) -> EscapeHandle {
        self.handles.escapes.clone()
    }

    fn presence(&self) -> PresenceHandle {
        self.presence.clone()
    }

    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let WireClaim {
            wire,
            handles,
            presence,
            released,
            ..
        } = *self;
        std::thread::Builder::new()
            .name("ksx-test-claim".into())
            .spawn(move || {
                // Escapes are evaluated **here**, on the claim's own thread,
                // exactly as `ksx_capture::winusb` does — before anything is
                // forwarded and with no channel between the gesture and the
                // latch. That is what makes `LeftCtrl ×5` in these tests the
                // real gesture through the real detector rather than a poke at
                // an atomic, and it is the only way a test can tell a live
                // escape apart from a stale one.
                let mut escapes = EscapeWatch::new(handles.escapes.clone());
                let reason = loop {
                    crossbeam_channel::select! {
                        recv(ctl) -> message => match message {
                            Ok(CaptureCtl::Shutdown) | Err(_) => break ExitReason::Shutdown,
                            Ok(_) => {}
                        },
                        recv(wire) -> event => match event {
                            Ok(event) => {
                                escapes.observe(&event);
                                // Forwarded either way: suppression is about
                                // what Windows sees, never about what the
                                // engine sees.
                                match tx.try_send(event) {
                                    Ok(()) | Err(TrySendError::Full(_)) => {}
                                    Err(TrySendError::Disconnected(_)) => {
                                        break ExitReason::ChannelClosed
                                    }
                                }
                            }
                            Err(_) => break ExitReason::DeviceLost,
                        },
                    }
                };
                presence.publish(Vec::new());
                released.store(true, Ordering::SeqCst);
                reason
            })
    }
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

/// One claimed board bound to one slot, on an inert preset: these tests are
/// about who owns the keystrokes, not about what they map to.
fn panel_plan() -> RunPlan {
    let preset = Preset::builtin_empty();
    RunPlan {
        source: PlanSource::Config,
        config_path: PathBuf::from("test"),
        slots: vec![ResolvedSlot {
            spec: SlotSpec::new(1, Some(DeviceId::from(PANEL)), None, preset.name.clone())
                .expect("valid slot"),
            preset,
        }],
        block_keyboards: ksx_core::Blocking::Whole,
        block_mice: false,
        captureable: vec![DeviceId::from(PANEL)],
        // The whole point: this device is claimed, so the session must borrow
        // the daemon's claim rather than make one.
        winusb: vec![DeviceId::from(PANEL)],
        notes: Vec::new(),
    }
}

/// What every session reported, in order — the raw [`Outcome`], the
/// [`SessionSummary`] the daemon actually consumes, and the console output that
/// session produced.
///
/// Per **session**, deliberately: a shared buffer would make "session 2 printed
/// an `[ESC]` line for a gesture that predates it" indistinguishable from
/// "session 1 printed one", which is the whole question in R2.
#[derive(Clone, Default)]
struct Reports {
    outcomes: Arc<Mutex<Vec<Outcome>>>,
    summaries: Arc<Mutex<Vec<SessionSummary>>>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Reports {
    fn outcomes(&self) -> Vec<Outcome> {
        self.outcomes.lock().expect("outcomes").clone()
    }

    fn summaries(&self) -> Vec<SessionSummary> {
        self.summaries.lock().expect("summaries").clone()
    }

    fn logs(&self) -> Vec<String> {
        self.logs.lock().expect("logs").clone()
    }
}

struct PanelFactory {
    plan: RunPlan,
    panel: Arc<Panel>,
    trace: Trace,
    reports: Reports,
    makes: Arc<AtomicUsize>,
    /// Delay injected into every pad plug, to reproduce the real startup window
    /// where the panel is already muted but the supervisor is not up yet.
    slow_plug: Duration,
}

impl PanelFactory {
    fn new(panel: &Arc<Panel>, trace: &Trace, reports: &Reports) -> Self {
        Self {
            plan: panel_plan(),
            panel: Arc::clone(panel),
            trace: Arc::clone(trace),
            reports: reports.clone(),
            makes: Arc::new(AtomicUsize::new(0)),
            slow_plug: Duration::ZERO,
        }
    }
}

impl SessionFactory for PanelFactory {
    fn make(&mut self) -> anyhow::Result<Box<dyn SessionRunner>> {
        self.makes.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(PanelRunner {
            plan: self.plan.clone(),
            panel: Arc::clone(&self.panel),
            trace: Arc::clone(&self.trace),
            reports: self.reports.clone(),
            slow_plug: self.slow_plug,
        }))
    }

    fn config_dir(&self) -> PathBuf {
        PathBuf::from("test")
    }

    fn game(&self) -> Option<String> {
        None
    }

    fn set_game(&mut self, _game: Option<String>) {}
}

struct PanelRunner {
    plan: RunPlan,
    panel: Arc<Panel>,
    trace: Trace,
    reports: Reports,
    slow_plug: Duration,
}

/// `MockBackend` plugs instantly, which closes the very window this file needs
/// to test: on real hardware four ViGEm pads take hundreds of milliseconds to
/// seconds, and the panel is muted for all of it.
struct SlowPads {
    inner: MockBackend,
    delay: Duration,
}

impl VirtualPadBackend for SlowPads {
    fn plug(&mut self) -> Result<ksx_output::PadHandle, ksx_output::OutputError> {
        std::thread::sleep(self.delay);
        self.inner.plug()
    }
    fn persona(&self, handle: ksx_output::PadHandle) -> Option<ksx_core::Persona> {
        self.inner.persona(handle)
    }
    fn user_index(&self, handle: ksx_output::PadHandle) -> Option<u8> {
        self.inner.user_index(handle)
    }
    fn update(
        &mut self,
        handle: ksx_output::PadHandle,
        state: &ksx_core::PadState,
    ) -> Result<(), ksx_output::OutputError> {
        self.inner.update(handle, state)
    }
    fn poll_feedback(&mut self, handle: ksx_output::PadHandle) -> Option<ksx_output::Feedback> {
        self.inner.poll_feedback(handle)
    }
    fn unplug(&mut self, handle: ksx_output::PadHandle) -> Result<(), ksx_output::OutputError> {
        self.inner.unplug(handle)
    }
}

impl SessionRunner for PanelRunner {
    fn slots(&self) -> usize {
        self.plan.slots.len()
    }

    fn run(
        &mut self,
        stop: Arc<AtomicBool>,
        out: &mut dyn Write,
    ) -> anyhow::Result<SessionSummary> {
        // The real per-session wiring, including the refusal path for a device
        // the daemon never claimed.
        #[cfg(windows)]
        let capture = crate::capture::build_session(&self.plan, Some(&self.panel))?;
        #[cfg(not(windows))]
        let capture = self.panel.session_backend();

        let mut options = RunOptions {
            trace: Some(Arc::clone(&self.trace)),
            stop: Box::new(move || stop.load(Ordering::SeqCst)),
            plug_deadline: Duration::from_secs(10),
            ..RunOptions::default()
        };
        // This session's own console output, kept apart from every other
        // session's (see [`Reports`]) and then forwarded to the daemon's `out`
        // so nothing is hidden from the loop that normally sees it.
        let mut mine: Vec<u8> = Vec::new();
        let outcome = supervisor::supervise(
            &self.plan,
            Wiring {
                capture,
                pads: if self.slow_plug.is_zero() {
                    Box::new(MockBackend::new()) as Box<dyn VirtualPadBackend>
                } else {
                    Box::new(SlowPads {
                        inner: MockBackend::new(),
                        delay: self.slow_plug,
                    })
                },
            },
            &mut options,
            &mut mine,
        )?;
        let _ = out.write_all(&mine);
        // Exactly what `daemon::live` builds from an outcome: the reporting
        // path R1's secondary half is about.
        let summary = SessionSummary {
            stop_code: outcome.stop.code().to_owned(),
            message: outcome.stop.message(),
            exit_code: outcome.exit_code(),
            slots: outcome.pads.len(),
            reboot_required: outcome.health.reboot_required,
            watchdog_tripped: outcome.health.watchdog_tripped,
            dropped_events: outcome.health.dropped_events,
        };
        self.reports
            .outcomes
            .lock()
            .expect("outcomes")
            .push(outcome);
        self.reports
            .summaries
            .lock()
            .expect("summaries")
            .push(summary.clone());
        self.reports
            .logs
            .lock()
            .expect("logs")
            .push(String::from_utf8_lossy(&mine).into_owned());
        Ok(summary)
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Everything a test needs to play the user and watch the panel.
struct Rig {
    panel: Arc<Panel>,
    wire: Sender<KeyEvent>,
    typed: InjectionLog,
    /// The daemon-lifetime health + escape state, as the claim publishes into
    /// it. Held by the test because that is the whole point: it outlives every
    /// session, so a test can plant a first session's history and watch what a
    /// second session makes of it.
    handles: Handles,
    runs: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
}

fn rig() -> Rig {
    let (wire, wire_rx) = crossbeam_channel::unbounded::<KeyEvent>();
    let handles = Handles::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicBool::new(false));
    let claim = WireClaim::new(
        wire_rx,
        handles.clone(),
        Arc::clone(&runs),
        Arc::clone(&released),
    );
    let injector = RecordingInjector::new();
    let typed = injector.log();
    let panel = Panel::start(Box::new(claim), handles.clone(), injector).expect("panel starts");
    Rig {
        panel,
        wire,
        typed,
        handles,
        runs,
        released,
    }
}

fn press(wire: &Sender<KeyEvent>, key: Key) {
    wire.send(event(key, true)).expect("wire open");
    wire.send(event(key, false)).expect("wire open");
}

/// Press `key` and wait until **both** of its transitions have been typed.
///
/// The wait matters more than the press. Every "and now nothing may be typed"
/// assertion below starts by clearing the log, and it is only sound if the
/// panel's pipeline is empty when it does. Waiting for a single `Key+` is not
/// enough: its release is still in flight, and a release that lands after the
/// clear looks exactly like a mute that did not work. The channel is FIFO
/// through one thread, so seeing the *last* transition of the last key proves
/// everything before it has been processed.
fn press_and_drain(wire: &Sender<KeyEvent>, typed: &InjectionLog, key: Key) {
    press(wire, key);
    let want = format!("{n}+ {n}-", n = key.name());
    // `ends_with`, not `==`: anything already in flight is typed first, and
    // that is the point — when the fence's own release shows up at the end,
    // everything ahead of it has been processed.
    eventually(&format!("the panel to type {want} and drain"), || {
        typed.trace().ends_with(&want)
    });
    typed.clear();
}

fn run_state(state: &SharedState) -> RunState {
    state.lock().expect("state").run.clone()
}

/// Perform a real `LeftCtrl ×5` **on the wire**, and wait until the claim's own
/// escape watch has acted on it.
///
/// Not a poke at the latch: the strokes travel the same path a player's do, are
/// evaluated on the claim thread, and flip the shared latch there. Waiting for
/// the counter to move is what makes everything after this line ordered with
/// respect to the gesture.
fn escape_gesture(rig: &Rig) {
    let before = rig.handles.escapes.snapshot().toggles;
    for _ in 0..5 {
        press(&rig.wire, Key::LeftControl);
    }
    eventually("the LeftCtrl x5 gesture to reach the shared latch", || {
        rig.handles.escapes.snapshot().toggles > before
    });
}

/// A running daemon: the real [`control_loop_with`] on its own thread, with the
/// real [`Panel`] and the real `supervise()` behind it, driven by commands from
/// the test.
struct Daemon {
    commands: Sender<DaemonCommand>,
    state: SharedState,
    trace: Trace,
    reports: Reports,
    loop_handle: Option<std::thread::JoinHandle<Vec<u8>>>,
}

fn daemon(panel: &Arc<Panel>) -> Daemon {
    daemon_with_plug_delay(panel, Duration::ZERO)
}

fn daemon_with_plug_delay(panel: &Arc<Panel>, slow_plug: Duration) -> Daemon {
    let (commands, rx) = crossbeam_channel::unbounded::<DaemonCommand>();
    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    let reports = Reports::default();
    let loop_handle = std::thread::Builder::new()
        .name("ksx-test-daemon".into())
        .spawn({
            let panel = Arc::clone(panel);
            let state = Arc::clone(&state);
            let trace = Arc::clone(&trace);
            let reports = reports.clone();
            move || {
                let mut factory = PanelFactory::new(&panel, &trace, &reports);
                factory.slow_plug = slow_plug;
                let mut keyboard = PanelKeyboardHandle(panel);
                let mut out: Vec<u8> = Vec::new();
                control_loop_with(
                    rx,
                    state,
                    &mut factory,
                    &mut keyboard,
                    &super::NoUi,
                    &mut out,
                );
                out
            }
        })
        .expect("the control loop thread starts");
    Daemon {
        commands,
        state,
        trace,
        reports,
        loop_handle: Some(loop_handle),
    }
}

impl Daemon {
    /// Start the `n`th session and wait until the **real supervisor** has
    /// applied blocking — i.e. until "emulation owns the panel" is a fact
    /// rather than a command in flight.
    /// Send Start without waiting for the session to come up — the caller wants
    /// the startup window itself.
    fn start_asynchronously(&self) {
        self.commands
            .send(DaemonCommand::Start { game: None })
            .expect("the loop is alive");
    }

    fn wait_running(&self, n: usize) {
        eventually(&format!("session {n} to report Running"), || {
            matches!(run_state(&self.state), RunState::Running { .. })
        });
        eventually(
            &format!("session {n}: blocking to be enabled by the real supervisor"),
            || count(&self.trace, Step::BlockingEnabled) >= n,
        );
    }

    fn start_session(&self, n: usize) {
        self.commands
            .send(DaemonCommand::Start { game: None })
            .expect("the loop is alive");
        eventually(&format!("session {n} to report Running"), || {
            matches!(run_state(&self.state), RunState::Running { .. })
        });
        eventually(
            &format!("session {n}: blocking to be enabled by the real supervisor"),
            || count(&self.trace, Step::BlockingEnabled) >= n,
        );
    }

    fn stop_session(&self) {
        self.commands
            .send(DaemonCommand::Stop)
            .expect("the loop is alive");
        self.wait_session_ended();
    }

    /// Wait for the session to be reaped — whether the test stopped it or it
    /// ended on its own (a watchdog trip, a capture panic).
    fn wait_session_ended(&self) {
        eventually("the session to be reaped", || {
            matches!(
                run_state(&self.state),
                RunState::Stopped | RunState::Failed { .. }
            )
        });
    }

    /// Stop the daemon and collect everything it printed. Idempotent, because
    /// [`Drop`] calls it too.
    fn quit(&mut self) -> Vec<u8> {
        let _ = self.commands.send(DaemonCommand::Quit);
        let Some(handle) = self.loop_handle.take() else {
            return Vec::new();
        };
        match handle.join() {
            Ok(out) => out,
            // Already unwinding from a failed assertion: reporting a second
            // panic here would bury the one the test is about.
            Err(_) if std::thread::panicking() => Vec::new(),
            Err(_) => panic!("the daemon's control loop panicked"),
        }
    }
}

/// A failing assertion must not leave the control loop — and the session it is
/// holding — running behind the test that gave up on it.
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.quit();
    }
}

fn steps(trace: &Trace) -> Vec<Step> {
    trace.lock().expect("trace").clone()
}

fn count(trace: &Trace, step: Step) -> usize {
    steps(trace).iter().filter(|s| **s == step).count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// **The bug F1 was.** The daemon must claim once, keep the claim across
/// sessions, and put the panel's keystrokes back in between.
///
/// A per-session claim (what `live.rs` did) fails this three ways over: the
/// claim is opened twice, the panel is dead between the sessions, and the
/// typethrough never runs at all.
#[test]
fn one_claim_serves_two_sessions_and_the_panel_types_in_between() {
    let rig = rig();
    assert_eq!(
        rig.runs.load(Ordering::SeqCst),
        1,
        "the daemon opens the claim exactly once, before any session exists"
    );

    // Phase 1 — idle daemon, no session: the panel is a keyboard.
    press_and_drain(&rig.wire, &rig.typed, Key::Up);

    let (commands, rx) = crossbeam_channel::unbounded::<DaemonCommand>();
    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    let reports = Reports::default();
    let mut factory = PanelFactory::new(&rig.panel, &trace, &reports);
    let makes = Arc::clone(&factory.makes);
    let mut keyboard = PanelKeyboardHandle(Arc::clone(&rig.panel));

    // The user, on their own thread: the control loop owns this one.
    let user = std::thread::spawn({
        let wire = rig.wire.clone();
        let typed = rig.typed.clone();
        let state = Arc::clone(&state);
        let trace = Arc::clone(&trace);
        move || {
            let mut between = String::new();
            for session in 1..=2 {
                commands
                    .send(DaemonCommand::Start { game: None })
                    .expect("loop alive");
                eventually("the session to report Running", || {
                    matches!(run_state(&state), RunState::Running { .. })
                });
                // `Running` is set as the session thread is spawned; blocking
                // is applied further in. Wait for the supervisor's own step, so
                // "the session owns the panel" is a fact, not a sleep.
                eventually("blocking to be enabled by the real supervisor", || {
                    count(&trace, Step::BlockingEnabled) >= session
                });

                typed.clear();
                press(&wire, Key::A);
                // The session owns these keys. Give the typethrough every
                // chance to get it wrong before believing it.
                std::thread::sleep(Duration::from_millis(60));
                assert_eq!(
                    typed.trace(),
                    "",
                    "session {session}: emulation owns the panel, nothing may be typed"
                );

                commands.send(DaemonCommand::Stop).expect("loop alive");
                eventually("the session to be reaped", || {
                    matches!(
                        run_state(&state),
                        RunState::Stopped | RunState::Failed { .. }
                    )
                });

                if session == 1 {
                    // THE assertion: between two sessions, on a claim that was
                    // never released, the panel is a keyboard again.
                    //
                    // Retried rather than pressed once: `Stopped` is published
                    // by the reap, and the control loop hands the panel back
                    // immediately *after* it — a key pressed in that sliver is
                    // legitimately not typed, and the user simply presses again.
                    typed.clear();
                    eventually("the panel to type again between sessions", || {
                        press(&wire, Key::B);
                        typed.trace().contains("B+")
                    });
                    between = typed.trace();
                    // The retries above leave keys in flight. Drain them before
                    // the next session's "nothing may be typed" window, or one
                    // of them could land after the log is cleared and be read as
                    // a mute that failed.
                    press_and_drain(&wire, &typed, Key::Z);
                }
            }
            commands.send(DaemonCommand::Quit).expect("loop alive");
            between
        }
    });

    let mut out: Vec<u8> = Vec::new();
    control_loop_with(
        rx,
        Arc::clone(&state),
        &mut factory,
        &mut keyboard,
        &super::NoUi,
        &mut out,
    );
    let between = user.join().expect("the user script must not panic");

    assert!(
        between.contains("B+"),
        "the panel must type between sessions: {between:?}"
    );
    assert_eq!(makes.load(Ordering::SeqCst), 2, "two sessions ran");
    assert_eq!(
        rig.runs.load(Ordering::SeqCst),
        1,
        "the claim must be opened exactly ONCE for both sessions — a per-session claim \
         is the bug this test exists for"
    );
    assert!(
        !rig.released.load(Ordering::SeqCst),
        "the claim must still be held after the daemon's last session"
    );

    let outcomes = reports.outcomes();
    assert_eq!(outcomes.len(), 2, "both sessions reported");
    for (n, outcome) in outcomes.iter().enumerate() {
        assert!(
            outcome.events >= 2,
            "session {} received no keystrokes through the borrowed claim: {outcome:?}",
            n + 1
        );
        assert_eq!(outcome.capture_exit, "shutdown", "session {}", n + 1);
        assert_eq!(outcome.exit_code(), 0, "session {}", n + 1);
    }

    // ...and the claim is released when the daemon lets go of it, not before —
    // which means *every* holder: the control loop's keyboard view and the
    // session factory hold the same panel a real daemon's do.
    drop(keyboard);
    drop(factory);
    assert!(
        !rig.released.load(Ordering::SeqCst),
        "the claim outlives everything but the last holder"
    );
    drop(rig.panel);
    eventually("the claim to be released on drop", || {
        rig.released.load(Ordering::SeqCst)
    });
}

/// A key held when emulation starts must not stay held forever.
///
/// The player is holding P1-Left as the frontend launches the game. The
/// physical release is then consumed by the engine, so if nothing releases the
/// injected key Windows believes Left is down for the rest of the session — the
/// desktop scrolls sideways behind the game. This drives the real control loop,
/// so it pins the *wiring*, not `Typethrough`'s own unit test.
#[test]
fn a_key_held_into_emulation_is_released_and_not_latched() {
    let rig = rig();
    rig.wire.send(event(Key::Left, true)).expect("wire open");
    eventually("the held key to be typed while idle", || {
        rig.typed.trace() == "Left+"
    });

    let (commands, rx) = crossbeam_channel::unbounded::<DaemonCommand>();
    commands.send(DaemonCommand::Start { game: None }).unwrap();
    commands.send(DaemonCommand::Quit).unwrap();
    drop(commands);

    let mut factory = PanelFactory::new(
        &rig.panel,
        &(Arc::new(Mutex::new(Vec::new())) as Trace),
        &Reports::default(),
    );
    let mut keyboard = PanelKeyboardHandle(Arc::clone(&rig.panel));
    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
    let mut out: Vec<u8> = Vec::new();
    control_loop_with(
        rx,
        state,
        &mut factory,
        &mut keyboard,
        &super::NoUi,
        &mut out,
    );

    assert!(
        rig.typed.events().contains(&(Key::Left, false)),
        "entering emulation must release what the panel was holding: {}",
        rig.typed.trace()
    );
}

/// Quitting the daemon releases the claim and lets go of every key.
///
/// This is the teardown half of "the daemon owns the claim for its whole
/// lifetime": the lifetime has to end somewhere, and it has to end with the
/// board's keys not stuck down on a machine that now has one keyboard fewer.
#[test]
fn dropping_the_panel_releases_the_claim_and_every_held_key() {
    let rig = rig();
    rig.wire.send(event(Key::Space, true)).expect("wire open");
    eventually("the key to be typed", || rig.typed.trace() == "Space+");

    drop(rig.panel);

    eventually("the claim to be released", || {
        rig.released.load(Ordering::SeqCst)
    });
    assert!(
        rig.typed.events().contains(&(Key::Space, false)),
        "a claimed panel must not outlive the daemon with a key held down: {}",
        rig.typed.trace()
    );
}

/// Two sessions may not borrow the panel at once: one keystroke fanned into two
/// engines would drive two sets of pads from one press.
#[test]
fn the_panel_refuses_a_second_simultaneous_session() {
    let rig = rig();
    let first = rig.panel.session_backend();
    let (tx, _rx) = crossbeam_channel::bounded::<KeyEvent>(8);
    let (_ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<CaptureCtl>();
    let handle = first.run(tx, ctl_rx).expect("the first session attaches");

    let second = rig.panel.session_backend();
    let (tx2, _rx2) = crossbeam_channel::bounded::<KeyEvent>(8);
    let (_ctl_tx2, ctl_rx2) = crossbeam_channel::unbounded::<CaptureCtl>();
    let err = second
        .run(tx2, ctl_rx2)
        .expect_err("a second session must be refused");
    assert!(
        err.to_string().contains("already driving a session"),
        "{err}"
    );

    drop(_ctl_tx);
    assert_eq!(handle.join().expect("session thread"), ExitReason::Shutdown);
}

/// **The seam the bug lived in.** The keyboard the daemon's control loop drives
/// must be the daemon's own claim.
///
/// `daemon::run` builds this by asking the live factory — the same object that
/// hands the claim to every session — so "the loop mutes one panel while the
/// sessions use another" cannot be written. Before the fix the loop was handed
/// `panel_for(None)`, i.e. a `NoPanel`, and this test's first assertion is
/// exactly what that produces: a "muted" panel that types anyway.
#[test]
fn the_control_loops_keyboard_is_the_daemons_own_claim() {
    let rig = rig();
    let factory = super::live::LiveFactory {
        root: ksx_config::ConfigRoot::at(std::env::temp_dir().join("ksx-panel-seam-test")),
        game: None,
        no_launch: true,
        panel: Some(Arc::clone(&rig.panel)),
        feed: crate::feed::LiveSink::default(),
        staged: None,
    };
    let mut keyboard = factory.panel_keyboard();

    keyboard.set_emulating(true);
    rig.typed.clear();
    press(&rig.wire, Key::A);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        rig.typed.trace(),
        "",
        "the loop's keyboard must actually be the claimed panel — a `NoPanel` here \
         means a claimed board types into the desktop behind the game"
    );

    keyboard.set_emulating(false);
    eventually("the panel to type once emulation stops", || {
        press(&rig.wire, Key::B);
        rig.typed.trace().contains("B+")
    });

    // ...and with no claim there is nothing to drive, which is what keeps the
    // Interception path free of a second injector.
    let none = super::live::LiveFactory {
        root: ksx_config::ConfigRoot::at(std::env::temp_dir().join("ksx-panel-seam-test")),
        game: None,
        no_launch: true,
        panel: None,
        feed: crate::feed::LiveSink::default(),
        staged: None,
    };
    none.panel_keyboard().set_emulating(true);
}

// ---------------------------------------------------------------------------
// R1: health is latched for the daemon's lifetime; a session is not
// ---------------------------------------------------------------------------

/// The shared body of the two R1 tests: session 1 ends because the backend
/// published a fatal, **write-only** health flag; session 2 must then be a
/// perfectly ordinary session on the very same, still-latched handle.
///
/// Before the fix session 2 stopped on its first loop iteration — before a
/// single keystroke was translated — with session 1's stop code, and went on
/// doing that for every session until the daemon was restarted. On a cabinet
/// that is a machine that will not play another game until it is rebooted,
/// caused by a stall that already ended safely.
fn a_fatal_flag_from_session_one_does_not_end_session_two(
    publish: impl Fn(&HealthHandle),
    want_code: &str,
) {
    let rig = rig();
    let mut daemon = daemon(&rig.panel);

    // Session 1: running normally, then the backend reports it is not.
    daemon.start_session(1);
    publish(&rig.handles.health);
    daemon.wait_session_ended();

    // Session 2: the player starts the next game. The flag is still set — the
    // daemon never released the claim, so nothing cleared it and nothing can.
    daemon.start_session(2);
    rig.typed.clear();
    press(&rig.wire, Key::A);
    // Long enough for a session that was going to die on its first poll (50 ms)
    // to have died, and for the keystrokes to reach the engine.
    std::thread::sleep(Duration::from_millis(120));
    assert!(
        matches!(run_state(&daemon.state), RunState::Running { .. }),
        "session 2 must still be running after session 1's {want_code}"
    );
    assert_eq!(
        rig.typed.trace(),
        "",
        "session 2 owns the panel: a session that quietly died would let it type"
    );
    daemon.stop_session();
    daemon.quit();

    let outcomes = daemon.reports.outcomes();
    assert_eq!(outcomes.len(), 2, "two sessions ran");
    assert_eq!(
        outcomes[0].stop.code(),
        want_code,
        "session 1 ends on the flag it earned"
    );
    assert_eq!(
        outcomes[1].stop.code(),
        "ctrl-c",
        "session 2 must end because the user stopped it, not because of session 1"
    );
    assert_eq!(outcomes[1].exit_code(), 0, "session 2 ended cleanly");
    assert!(
        outcomes[1].events >= 2,
        "session 2 must translate its own keystrokes: {:?}",
        outcomes[1]
    );
    assert!(
        !outcomes[1].health.watchdog_tripped && !outcomes[1].health.panicked,
        "session 2 inherited a fatal flag it never earned: {:?}",
        outcomes[1].health
    );
}

/// R1: one watchdog trip used to kill every later session with
/// `watchdog-tripped` before it translated a keystroke.
#[test]
fn session_two_runs_normally_after_session_one_tripped_the_watchdog() {
    a_fatal_flag_from_session_one_does_not_end_session_two(
        HealthHandle::set_watchdog_tripped,
        "watchdog-tripped",
    );
}

/// R1, the same latch by another name: a caught capture panic.
#[test]
fn session_two_runs_normally_after_session_one_saw_a_capture_panic() {
    a_fatal_flag_from_session_one_does_not_end_session_two(
        HealthHandle::set_panicked,
        "capture-panicked",
    );
}

/// R1's reporting half, and the one deliberate exception to it.
///
/// `dropped_events` is a fact about **this** run's engine, so session 2 must
/// report its own (zero) rather than session 1's. `reboot_required` is a fact
/// about the **machine** — Interception slot exhaustion survives until Windows
/// restarts — so it is carried forward on purpose, and the tray tooltip must
/// still say so after a Stop and a Start.
#[test]
fn session_two_reports_its_own_drops_and_the_machines_reboot_flag() {
    let rig = rig();
    let mut daemon = daemon(&rig.panel);

    daemon.start_session(1);
    rig.handles.health.add_dropped(7);
    rig.handles.health.set_reboot_required();
    daemon.stop_session();

    daemon.start_session(2);
    daemon.stop_session();
    let tooltip = daemon.state.lock().expect("state").tooltip();
    daemon.quit();

    let reported = daemon.reports.summaries();
    assert_eq!(reported.len(), 2);
    assert_eq!(reported[0].dropped_events, 7, "session 1 dropped 7");
    assert!(reported[0].reboot_required);
    assert_eq!(
        reported[1].dropped_events, 0,
        "session 2 dropped nothing; counting session 1's would make every later \
         session look worse than the last"
    );
    assert!(
        !reported[1].watchdog_tripped,
        "session 2 never stalled: {reported:?}"
    );
    assert!(
        reported[1].reboot_required,
        "slot exhaustion is a property of the machine until it reboots — carried \
         across sessions on purpose"
    );
    assert!(
        tooltip.contains("REBOOT REQUIRED"),
        "the tray must keep saying it after the next session: {tooltip:?}"
    );
}

// ---------------------------------------------------------------------------
// R2: the escape latch frees a session, not the daemon
// ---------------------------------------------------------------------------

/// R2: `LeftCtrl ×5` escapes the game you are in — not every game after it.
///
/// The player escapes a wedged game, quits it, and starts the next one. Before
/// the fix the latch was still set when session 2 began, so `Typethrough::apply`
/// computed `emulating && !freed == false` and the panel typed onto the desktop
/// while the pads were being driven: double input on every press. The supervisor
/// made it worse by replaying the old gesture from a zero baseline — toggling
/// its own blocking back off and printing an `[ESC]` line for something that
/// happened in the previous game.
#[test]
fn an_escape_in_session_one_leaves_session_two_muted_and_silent_about_it() {
    let rig = rig();
    let mut daemon = daemon(&rig.panel);

    // Session 1: the game wedges and the player escapes it.
    daemon.start_session(1);
    escape_gesture(&rig);
    eventually("the escape to free the panel during session 1", || {
        press(&rig.wire, Key::B);
        rig.typed.trace().contains("B+")
    });
    daemon.stop_session();

    // They quit that game and start the next one.
    press_and_drain(&rig.wire, &rig.typed, Key::Z);
    daemon.start_session(2);
    assert!(
        !rig.handles.escapes.passthrough(),
        "a session starts armed: an escape from a game that is already over must \
         not still be freeing the panel"
    );

    rig.typed.clear();
    press(&rig.wire, Key::A);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        rig.typed.trace(),
        "",
        "session 2 must start muted — a panel that types here is double input on \
         every press, with the game receiving the pad events too"
    );

    daemon.stop_session();
    daemon.quit();

    let outcomes = daemon.reports.outcomes();
    assert_eq!(outcomes[0].toggles, 1, "session 1 saw exactly one gesture");
    assert_eq!(
        outcomes[1].toggles, 0,
        "session 2 saw none — counting from zero replays every gesture the daemon \
         has ever seen"
    );
    let logs = daemon.reports.logs();
    assert!(
        logs[0].contains("[ESC]"),
        "session 1 must report the gesture it saw: {:?}",
        logs[0]
    );
    assert!(
        !logs[1].contains("[ESC]"),
        "session 2 announced an escape that predates it: {:?}",
        logs[1]
    );
}

/// R2's other half: re-arming must not cost the escape hatch anything.
///
/// A gesture made *during* session 2 still frees the panel on the very next
/// key, from the capture thread, with no supervisor in the path — which is the
/// property the whole escape design exists for. Session 1 escapes first, so
/// this also proves the fix is a re-arm and not a disabling.
#[test]
fn an_escape_during_session_two_still_frees_the_panel_immediately() {
    let rig = rig();
    let mut daemon = daemon(&rig.panel);

    daemon.start_session(1);
    escape_gesture(&rig);
    daemon.stop_session();

    press_and_drain(&rig.wire, &rig.typed, Key::Z);
    daemon.start_session(2);
    rig.typed.clear();
    press(&rig.wire, Key::A);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(rig.typed.trace(), "", "session 2 starts muted");

    // The new game wedges too. This gesture is live.
    escape_gesture(&rig);
    eventually("the live escape to free the panel on the next key", || {
        press(&rig.wire, Key::B);
        rig.typed.trace().contains("B+")
    });

    daemon.stop_session();
    daemon.quit();

    let outcomes = daemon.reports.outcomes();
    assert_eq!(
        outcomes[1].toggles, 1,
        "exactly this session's gesture, neither replayed nor swallowed"
    );
    let logs = daemon.reports.logs();
    assert!(
        logs[1].contains("[ESC]  LeftCtrl x5 — keyboard capture OFF"),
        "the live gesture must be reported: {:?}",
        logs[1]
    );
}

/// The panic case: the panel goes dead the instant Start is pressed, but the
/// pads take a moment to plug. A player staring at a dead panel mashes P1
/// Button 1 — which on an I-PAC *is* `LeftCtrl`. That gesture happens while
/// the claim's capture thread is running but the supervisor is not yet up.
///
/// It must be honoured, not silently erased. Taking the escape baseline after
/// the pad phase (or arming there) absorbs it: the panel goes dead again with
/// no `[ESC]` line and no trace of the user's emergency escape.
#[cfg(windows)]
#[test]
fn an_escape_while_the_pads_are_still_plugging_is_honoured() {
    let rig = rig();
    // Long enough that the gesture lands mid-plug on any machine.
    let mut daemon = daemon_with_plug_delay(&rig.panel, Duration::from_millis(250));

    daemon.start_asynchronously();
    // The control loop mutes the panel as it accepts Start; wait for that, then
    // escape while the supervisor is still waiting on pads.
    eventually("the panel to be muted by the Start command", || {
        rig.typed.clear();
        press(&rig.wire, Key::Z);
        std::thread::sleep(Duration::from_millis(15));
        rig.typed.trace().is_empty()
    });
    escape_gesture(&rig);

    daemon.wait_running(1);
    rig.typed.clear();
    eventually("the mid-plug escape to have freed the panel", || {
        press(&rig.wire, Key::B);
        rig.typed.trace().contains("B+")
    });

    daemon.stop_session();
    daemon.quit();

    let logs = daemon.reports.logs();
    assert!(
        logs[0].contains("[ESC]  LeftCtrl x5 — keyboard capture OFF"),
        "the mid-plug gesture must be reported, not swallowed: {:?}",
        logs[0]
    );
    assert_eq!(
        daemon.reports.outcomes()[0].toggles,
        1,
        "the session owns the gesture made while it was starting"
    );
}

/// A device that appears in the configuration *after* the daemon claimed its
/// panel is refused with an explanation — never claimed behind the user's back.
/// Capture mode is a per-device choice, not something a reload performs.
#[cfg(windows)]
#[test]
fn a_device_the_daemon_never_claimed_is_refused_not_claimed() {
    let rig = rig();
    let other = DeviceId::from(r"USB\VID_D209&PID_0430&MI_00\7&5E6F7A8B&0&0000");
    let mut plan = panel_plan();
    plan.captureable.push(other.clone());
    plan.winusb.push(other.clone());

    let Err(err) = crate::capture::build_session(&plan, Some(&rig.panel)) else {
        panic!("claiming mid-session is not something ksx does");
    };
    assert_eq!(err.code(), "winusb-panel-missing");
    let text = err.to_string();
    assert!(text.contains("Restart the daemon"), "{text}");
    assert!(text.contains(other.as_str()), "{text}");
}
