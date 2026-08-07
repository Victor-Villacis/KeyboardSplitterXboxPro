//! The `ksx run` supervisor: three threads, one teardown order, no tokio.
//!
//! ```text
//! [capture thread]  owned by ksx-capture. Receives strokes, evaluates the
//!                   emergency escapes (there, where nothing can starve them),
//!                   decides pass/suppress, re-sends what must pass, try_sends
//!                   KeyEvents.
//!        │ bounded crossbeam channel (1024; capture never blocks on it)
//!        ▼
//! [engine thread]   owns the ksx-core Engine. Maps events to pad deltas and
//!                   try_sends them, coalescing per slot — it never blocks.
//!        │ bounded channel of (PadDelta, capture timestamp)
//!        ▼
//! [output thread]   owns the VirtualPadBackend. Plugs the pads, applies deltas,
//!                   drains rumble/LED feedback, records the latency histogram.
//!
//! [supervisor]      this thread. Owns policy: capture on/off, teardown order,
//!                   hotplug invalidation, printing. May block freely — nothing
//!                   in the input path waits on it.
//! ```
//!
//! # The two orders that matter
//!
//! **Startup**: pads are plugged and their XInput slots resolved *first* (a
//! plug failure must not happen with keyboards already captured), then capture
//! starts in passthrough, then blocking is enabled for exactly the devices
//! bound to slots. Unassigned keyboards are never in that set.
//!
//! **Teardown**: uncapture *before* unplug, always. A [`CaptureGuard`] sends
//! `SetPassthrough` from `Drop`, so a panic anywhere in the supervisor still
//! frees the keyboards; and process death frees them with no cleanup at all
//! (the driver releases filters with the context handle). Every fatal condition
//! — Ctrl+C, thread death, capture panic, watchdog trip, output error — runs
//! the same path. The output thread never unplugs on its own initiative either:
//! a failing pad update is *reported*, and the supervisor drives teardown in
//! order (otherwise the pads vanish while the keyboards are still captured).
//!
//! # What the supervisor is NOT on the critical path for
//!
//! Emergency escapes. They are detected and acted on inside the capture thread
//! (`ksx_capture::escape`); this thread only *polls* the resulting counters to
//! mirror its own bookkeeping, print, and beep. If this thread stopped running
//! entirely, `LeftCtrl ×5` would still free the keyboards.
//!
//! # One session's worth of state, on state that outlives the session
//!
//! Everything below was written when the capture backend was born and died with
//! the session: health flags and escape counters started at zero because the
//! object holding them was new. Since M6 that is no longer true for a claimed
//! panel — the daemon claims once and keeps the claim (and its `Handles`) for
//! its whole lifetime, so a second session borrows state a first session already
//! wrote into. Latched flags would then be read as *this* session's, which turns
//! one watchdog trip into "every game after it refuses to start".
//!
//! So a session takes a **baseline** at [`supervise`]'s startup and reads
//! deltas: [`ksx_capture::HealthView`] for health, `seen_escapes` seeded from
//! the live counters for escapes. Two consequences worth stating plainly:
//!
//! - `ksx run` is bit-for-bit unaffected. It builds its own backend per run, so
//!   the baseline is all-zero and every delta is the value itself.
//! - The one piece of health that is *not* session-scoped is
//!   `reboot_required`: Interception slot exhaustion is a property of the
//!   machine until it restarts, not of a session (`ksx_capture::CaptureHealth::since`).
//!
//! The escape **latch** cannot be handled by a reader, because the capture
//! thread and the panel's typethrough act on it live. It is re-armed once, at
//! session start, by `EscapeHandle::arm` — starting a session is the user
//! asking for emulation *now*, and a gesture from a game they already quit must
//! not silently keep the panel typing onto the desktop behind the new one.

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{
    bounded, never, unbounded, Receiver, RecvTimeoutError, Sender, TrySendError,
};
use ksx_capture::{
    CaptureBackend, CaptureCtl, CaptureHealth, DeviceInfo, EscapeStatus, ExitReason, HealthView,
};
use ksx_core::{
    DeviceId, Engine, EngineTables, InvalidationReason, KeyEvent, PadState, Persona, ResolvedSlot,
};
use ksx_output::{PadHandle, VirtualPadBackend};

use super::latency::{Clock, LatencyRecorder, LatencySummary};
use super::plan::RunPlan;

/// Supervisor wake interval — bounds Ctrl+C, hotplug and teardown latency.
const POLL: Duration = Duration::from_millis(50);
/// Capture→engine channel depth (docs/ARCHITECTURE.md pipeline).
const EVENT_CHANNEL_CAPACITY: usize = 1024;
/// Engine→output channel depth. Blocking here is backpressure between two
/// non-hot threads; the capture thread's watchdog is what protects the OS.
const DELTA_CHANNEL_CAPACITY: usize = 1024;
/// `--latency` rolling report interval.
const LIVE_LATENCY_INTERVAL: Duration = Duration::from_secs(5);
/// How long to wait for all pads to plug before giving up.
const DEFAULT_PLUG_DEADLINE: Duration = Duration::from_secs(30);

/// Ordered lifecycle steps, recorded for tests (and useful in traces).
///
/// The teardown-order contract is an assertion over this sequence:
/// `BlockingDisabled` always precedes `PadsUnplugged`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    PadsPlugged,
    CaptureStarted,
    BlockingEnabled,
    BlockingDisabled,
    PadsUnplugged,
}

/// Shared ordered log of [`Step`]s.
pub type Trace = Arc<Mutex<Vec<Step>>>;

fn record(trace: &Option<Trace>, step: Step) {
    if let Some(trace) = trace {
        // A poisoned trace mutex is a test-harness bug, never a runtime path.
        if let Ok(mut log) = trace.lock() {
            log.push(step);
        }
    }
}

/// The two backends the pipeline is built from. Both are trait objects so the
/// integration tests drive the *real* supervisor with mock drivers.
pub struct Wiring {
    pub capture: Box<dyn CaptureBackend>,
    pub pads: Box<dyn VirtualPadBackend>,
}

// ---------------------------------------------------------------------------
// Session hooks (M5: game launching)
// ---------------------------------------------------------------------------

/// Why a hook wants the session to end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookStop {
    /// The launched game exited. A clean, expected end: exit 0.
    GameExited,
    /// The game could not be started. Emulation is already up by then, so this
    /// is a runtime failure: exit 3.
    LaunchFailed(String),
}

/// One conversion point, so a hook's vocabulary and the session's stay in sync.
impl From<HookStop> for StopReason {
    fn from(stop: HookStop) -> Self {
        match stop {
            HookStop::GameExited => StopReason::GameExited,
            HookStop::LaunchFailed(message) => StopReason::GameLaunchFailed(message),
        }
    }
}

/// Work that hangs off the session's lifecycle without being part of the input
/// path.
///
/// This exists for exactly one ordering problem. A game started **before** the
/// pads are plugged enumerates zero controllers, caches that answer, and spends
/// the rest of the session insisting no gamepad is connected — with four
/// perfectly good virtual pads sitting right behind it. So launching cannot be
/// something `ksx run` does around the supervisor; it has to be something the
/// supervisor does at a specific point *inside* startup, after
/// [`Step::BlockingEnabled`].
///
/// Every method has a no-op default, and [`RunOptions::default`] installs
/// [`NoHook`], so the M4 pipeline is untouched: a run with no `--game` behaves
/// exactly as it did before this trait existed, down to the step trace.
pub trait SessionHook: Send {
    /// Called once, after the pads are plugged, capture is running and blocking
    /// has been applied. `Err` ends the session with [`HookStop::LaunchFailed`].
    fn started(&mut self, _out: &mut dyn Write) -> Result<(), String> {
        Ok(())
    }

    /// Called on every supervisor poll (~50 ms). `Some` ends the session.
    fn poll(&mut self, _out: &mut dyn Write) -> Option<HookStop> {
        None
    }

    /// Called exactly once during teardown, on **every** path out — clean stop,
    /// escape, panic, thread death. Never kills anything; see
    /// `ksx_platform::process`' no-kill policy.
    fn finished(&mut self, _out: &mut dyn Write) {}

    /// One line for `--json` and the tray tooltip. `None` when there is nothing
    /// to say (the default hook).
    fn status_line(&self) -> Option<String> {
        None
    }
}

/// The default: a session with nothing hanging off it.
pub struct NoHook;

impl SessionHook for NoHook {}

// ---------------------------------------------------------------------------
// The binding hot-swap (Victor: "why does it need to disconnect to reconnect?")
// ---------------------------------------------------------------------------

/// Everything about a running session a preset edit is NOT allowed to change
/// without a restart.
///
/// The split is drawn where the *drivers* are: anything that would make the
/// output thread plug a different pad, or the capture thread block a different
/// device, has to go through a real teardown. Everything else is a key→function
/// table, which [`Engine::swap_tables`] can replace in place.
///
/// Deliberately NOT in the shape:
///
/// - **preset contents** — the whole point;
/// - **which preset a slot names** (`SlotSpec::preset`). Pointing slot 2 at a
///   different preset file changes the binding table and nothing else: same
///   pad, same persona, same keyboard. It is the one "structural-looking"
///   change that is genuinely hot;
/// - **plan notes / config path** — reporting, not wiring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionShape {
    /// `(slot number, persona, keyboard, mouse)` per slot, in plan order.
    slots: Vec<(u8, Persona, Option<DeviceId>, Option<DeviceId>)>,
    block_keyboards: bool,
    /// Exactly the `SetCaptured` set.
    captureable: Vec<DeviceId>,
    /// Which boards are WinUSB-claimed — a backend change per device.
    winusb: Vec<DeviceId>,
}

impl SessionShape {
    pub fn of(plan: &RunPlan) -> Self {
        Self {
            slots: plan
                .slots
                .iter()
                .map(|s| {
                    (
                        s.spec.number,
                        s.spec.persona,
                        s.spec.keyboard.clone(),
                        s.spec.mouse.clone(),
                    )
                })
                .collect(),
            block_keyboards: plan.block_keyboards,
            captureable: plan.captureable.clone(),
            winusb: plan.winusb.clone(),
        }
    }

    /// Why `next` cannot be hot-swapped onto this shape — `None` when it can.
    /// The string is user-facing: it says what changed, so "this restarts the
    /// pads" is never a mystery.
    pub fn bounce_reason(&self, next: &Self) -> Option<String> {
        if self.slots.len() != next.slots.len() {
            return Some(format!(
                "the slot count changed ({} → {})",
                self.slots.len(),
                next.slots.len()
            ));
        }
        for (before, after) in self.slots.iter().zip(&next.slots) {
            if before.0 != after.0 {
                return Some(format!("slot {} became slot {}", before.0, after.0));
            }
            if before.1 != after.1 {
                return Some(format!(
                    "slot {} changed persona ({} → {})",
                    before.0,
                    before.1.label(),
                    after.1.label()
                ));
            }
            if before.2 != after.2 || before.3 != after.3 {
                return Some(format!("slot {}'s input device changed", before.0));
            }
        }
        if self.block_keyboards != next.block_keyboards {
            return Some("keyboard blocking was turned on or off".to_owned());
        }
        if self.captureable != next.captureable {
            return Some("the set of captured keyboards changed".to_owned());
        }
        if self.winusb != next.winusb {
            return Some("a device's capture backend changed".to_owned());
        }
        None
    }
}

/// What an [`HotSwap::apply`] did — the two halves of the split, so the caller
/// can say which one happened in the response message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapVerdict {
    /// Bindings only: the tables were handed to the live engine. Pads stayed
    /// plugged, keyboards stayed captured, nothing re-enumerated.
    Applied,
    /// Structural: the caller must bounce the session. Carries the reason.
    NeedsRestart(String),
}

/// A handle onto the RUNNING session's engine thread. Published by
/// [`supervise`] once the engine exists; the daemon's control loop is the only
/// thing that holds one.
#[derive(Clone)]
pub struct HotSwap {
    ectl: Sender<EngineCtl>,
    shape: SessionShape,
}

impl HotSwap {
    /// Try to apply `plan`'s bindings to the running session in place.
    ///
    /// The new dispatch tables are built HERE — on the caller's thread, which
    /// is the daemon control loop and is allowed to block and allocate — so the
    /// engine thread only ever moves a pointer. `Err` means the engine is gone
    /// (the session ended underneath us) and the caller should treat it as a
    /// restart.
    pub fn apply(&self, plan: &RunPlan) -> Result<SwapVerdict, &'static str> {
        if let Some(reason) = self.shape.bounce_reason(&SessionShape::of(plan)) {
            return Ok(SwapVerdict::NeedsRestart(reason));
        }
        let tables = EngineTables::build(plan.slots.clone());
        self.ectl
            .send(EngineCtl::SwapTables(Box::new(tables)))
            .map_err(|_| "the session's engine thread has gone")?;
        Ok(SwapVerdict::Applied)
    }
}

/// Where a session publishes its [`HotSwap`], the same way [`SessionHook`]-less
/// health is published: taken by the caller BEFORE the session moves onto its
/// own thread, filled in by the session once the engine exists, and cleared on
/// teardown so nothing can hand tables to a dead engine.
#[derive(Clone, Debug, Default)]
pub struct HotSwapSlot(Arc<Mutex<Option<HotSwap>>>);

impl std::fmt::Debug for HotSwap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotSwap")
            .field("shape", &self.shape)
            .finish()
    }
}

impl HotSwapSlot {
    fn publish(&self, swap: HotSwap) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(swap);
        }
    }

    fn clear(&self) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = None;
        }
    }

    /// The handle, if a session is running and has an engine. Cloned out so
    /// the (blocking) table build never holds the lock.
    pub fn handle(&self) -> Option<HotSwap> {
        self.0.lock().ok()?.clone()
    }

    /// Stand in for a real session's engine thread: publish a handle over a
    /// fresh channel and return the receiving end, so a test can assert what
    /// the daemon control loop pushed through it.
    #[cfg(test)]
    pub(crate) fn publish_test_handle(&self, shape: SessionShape) -> Receiver<EngineCtl> {
        let (tx, rx) = unbounded();
        self.publish(HotSwap { ectl: tx, shape });
        rx
    }
}

/// Everything about a run that is not the plan.
pub struct RunOptions {
    /// `--latency`: print a rolling summary every 5 s.
    pub live_latency: bool,
    pub clock: Clock,
    /// Cooperative stop (the Ctrl+C latch in production).
    pub stop: Box<dyn Fn() -> bool + Send + Sync>,
    /// Play the legacy audio cue on an escape toggle.
    pub beep: bool,
    pub trace: Option<Trace>,
    pub plug_deadline: Duration,
    /// Test-only: stop the run once this many events have reached the engine.
    /// Never set from the CLI — a real session runs until told to stop.
    pub max_events: Option<u64>,
    /// Test-only: panic inside the engine thread after this many events, to
    /// prove teardown still frees the keyboards.
    pub panic_after_events: Option<u64>,
    /// Lifecycle hook — `--game` launching in M5. Defaults to [`NoHook`], so
    /// every pre-M5 caller and test is unaffected.
    pub hook: Box<dyn SessionHook>,
    /// Where this session publishes its [`HotSwap`] handle, so a binding edit
    /// can reach the live engine without bouncing the pads. Default is a slot
    /// nobody reads — `ksx run` has no control channel to swap through.
    pub hot_swap: HotSwapSlot,
    /// The live fan-out sink a surface watches the pipeline through
    /// (`crate::feed`).
    ///
    /// Default is a sink with no subscribers, and that is not merely a
    /// convenience: with nobody watching, every tap below is one relaxed atomic
    /// load and a return, so `ksx run` and a daemon with no cabinet window open
    /// run exactly the pipeline they ran before this existed.
    pub feed: crate::feed::LiveSink,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            live_latency: false,
            clock: Clock::platform(),
            stop: Box::new(|| false),
            beep: false,
            trace: None,
            plug_deadline: DEFAULT_PLUG_DEADLINE,
            max_events: None,
            panic_after_events: None,
            hook: Box::new(NoHook),
            hot_swap: HotSwapSlot::default(),
            feed: crate::feed::LiveSink::default(),
        }
    }
}

/// One plugged pad as the supervisor reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadInfo {
    /// Slot *number* (1..=MAX_SLOTS) — never an XInput user index.
    pub slot: u8,
    pub handle: u32,
    /// Which controller the pad presents itself as.
    pub persona: Persona,
    /// XInput `dwUserIndex`, discovered by the backend. `None` when all four
    /// XInput slots were already taken (e.g. by a real pad) — or, always, for
    /// a non-XInput persona, which never has one.
    pub user_index: Option<u8>,
}

/// Why the session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// Ctrl+C (or the host's stop latch).
    CtrlC,
    /// Ctrl+Alt+Del on a captured keyboard — the legacy emergency stop.
    EmergencyStop,
    /// Test-only `max_events` limit.
    EventLimit,
    /// The `--game` profile's game exited. The cabinet's normal end-of-session:
    /// the player quit, so emulation goes away and the frontend comes back.
    GameExited,
    /// The `--game` profile could not be started. This happens *after* the pads
    /// are up (see [`SessionHook`]), so it is a runtime failure, not a refusal.
    GameLaunchFailed(String),
    /// Pads could not be plugged; nothing was ever captured.
    PlugFailed {
        message: String,
        bus_missing: bool,
    },
    /// Two enumerated keyboards share one hardware id and at least one of them
    /// is bound to a slot. Refused before anything was plugged or captured.
    AmbiguousDevices {
        ids: Vec<DeviceId>,
    },
    /// The capture thread returned on its own (driver gone, channel closed).
    CaptureEnded(&'static str),
    /// The capture loop panicked. Its drop guard already reset the filters.
    CapturePanicked,
    /// The consumer stalled long enough that capture force-flipped to
    /// passthrough. Keyboards are safe, but the pipeline is not healthy.
    WatchdogTripped,
    EngineThreadDied,
    OutputThreadDied,
    OutputError(String),
}

impl StopReason {
    /// `true` when the session ended the way the user asked it to.
    pub fn is_clean(&self) -> bool {
        matches!(
            self,
            StopReason::CtrlC
                | StopReason::EmergencyStop
                | StopReason::EventLimit
                | StopReason::GameExited
        )
    }

    pub fn code(&self) -> &'static str {
        match self {
            StopReason::CtrlC => "ctrl-c",
            StopReason::EmergencyStop => "emergency-stop",
            StopReason::EventLimit => "event-limit",
            StopReason::GameExited => "game-exited",
            StopReason::GameLaunchFailed(_) => "game-launch-failed",
            StopReason::PlugFailed { .. } => "plug-failed",
            StopReason::AmbiguousDevices { .. } => "ambiguous-devices",
            StopReason::CaptureEnded(_) => "capture-ended",
            StopReason::CapturePanicked => "capture-panicked",
            StopReason::WatchdogTripped => "watchdog-tripped",
            StopReason::EngineThreadDied => "engine-thread-died",
            StopReason::OutputThreadDied => "output-thread-died",
            StopReason::OutputError(_) => "output-error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            StopReason::CtrlC => "stopped by Ctrl+C".to_owned(),
            StopReason::EmergencyStop => "stopped by the Ctrl+Alt+Del emergency escape".to_owned(),
            StopReason::EventLimit => "stopped at the configured event limit".to_owned(),
            StopReason::GameExited => {
                "the game exited; emulation stopped and the pads were unplugged".to_owned()
            }
            StopReason::GameLaunchFailed(err) => format!(
                "the game could not be started: {err}. Emulation was torn down, so the \
                 keyboards are back to normal"
            ),
            StopReason::PlugFailed { message, .. } => {
                format!("could not plug the virtual pads: {message}")
            }
            StopReason::AmbiguousDevices { ids } => ambiguous_devices_message(ids),
            StopReason::CaptureEnded(reason) => {
                format!("the capture thread ended on its own ({reason})")
            }
            StopReason::CapturePanicked => {
                "the capture loop panicked; its drop guard reset the class filters and \
                 keyboards kept working"
                    .to_owned()
            }
            StopReason::WatchdogTripped => {
                "the event consumer stalled: capture force-flipped to passthrough so \
                 keystrokes reached Windows. Emulation was torn down"
                    .to_owned()
            }
            StopReason::EngineThreadDied => {
                "the engine thread died; tearing down so no keyboard stays captured".to_owned()
            }
            StopReason::OutputThreadDied => {
                "the output thread died; tearing down so no keyboard stays captured".to_owned()
            }
            StopReason::OutputError(err) => format!("a pad update failed: {err}"),
        }
    }
}

/// The duplicate-hardware-id refusal, spelled out (risk review R2 / §3 items 3
/// and 4). Two boards of the same model report *the same* Interception hardware
/// id, so "capture this device" would capture both of them and either board
/// would drive every slot bound to that id.
fn ambiguous_devices_message(ids: &[DeviceId]) -> String {
    use std::fmt::Write as _;

    let mut out = String::from(
        "refusing to start: more than one connected keyboard reports the same hardware id, \
         and that id is bound to a slot:",
    );
    for id in ids {
        let _ = write!(out, "\n  {id}");
    }
    out.push_str(
        "\nThe Interception driver cannot tell identical boards apart, so ksx would capture \
         BOTH of them and let either one drive every slot bound to that id — including the \
         board you meant to leave typing. Nothing was plugged and no keyboard filter was set.\n\
         Fix it by unplugging the duplicate, binding a different model to the slot, or waiting \
         for the WinUSB backend (M6), which identifies boards by USB device path instead.",
    );
    out
}

/// Everything the CLI needs to report after a session.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub stop: StopReason,
    pub pads: Vec<PadInfo>,
    pub latency: LatencySummary,
    /// Health **as this session earned it** — a delta over the baseline taken
    /// at startup, so a daemon that has run five games does not report the
    /// first one's stall five times. The single exception is
    /// `reboot_required`, which describes the machine rather than the session
    /// and is carried forward on purpose
    /// (`ksx_capture::CaptureHealth::since`).
    pub health: CaptureHealth,
    pub events: u64,
    pub updates: u64,
    /// How the capture thread exited (`ExitReason`, or `join-panic`).
    pub capture_exit: &'static str,
    /// Escape-hotkey capture toggles during the session.
    pub toggles: u32,
    /// Slots invalidated by hotplug, in the order it happened.
    pub invalidated: Vec<(u8, InvalidationReason)>,
    /// Pad states superseded by a newer state for the same slot before the
    /// output thread could take them (level-triggered coalescing, never a lost
    /// transition — the newest state always wins).
    pub deltas_coalesced: u64,
    /// Pad states discarded because the output thread was gone.
    pub deltas_dropped: u64,
    /// The plan's non-fatal notes, echoed so `--json` consumers see them too.
    pub plan_notes: Vec<String>,
}

impl Outcome {
    /// 0 = the session ended as asked, 2 = it never started (see
    /// [`super::EXIT_CANNOT_START`]), 3 = it started and a runtime failure tore
    /// it down.
    ///
    /// The 2/3 split is about *blocking*, not about which component failed:
    /// every `PlugFailed` variant — bus missing, no free slot, plug timeout, the
    /// output thread dying before `PadsReady` — happens strictly before the
    /// capture thread is told to capture anything, so no keyboard filter was
    /// ever set and the correct answer is "could not start".
    pub fn exit_code(&self) -> i32 {
        match &self.stop {
            s if s.is_clean() => 0,
            StopReason::PlugFailed { .. } | StopReason::AmbiguousDevices { .. } => {
                super::EXIT_CANNOT_START
            }
            _ => super::EXIT_RUNTIME_FAILURE,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "stop": { "code": self.stop.code(), "message": self.stop.message() },
            "exit_code": self.exit_code(),
            "pads": self.pads.iter().map(|p| serde_json::json!({
                "slot": p.slot,
                "handle": p.handle,
                "persona": p.persona.as_str(),
                "user_index": p.user_index,
            })).collect::<Vec<_>>(),
            "events": self.events,
            "pad_updates": self.updates,
            "deltas_coalesced": self.deltas_coalesced,
            "deltas_dropped": self.deltas_dropped,
            "capture_exit": self.capture_exit,
            "capture_toggles": self.toggles,
            "notes": self.plan_notes,
            "invalidated_slots": self.invalidated.iter().map(|(slot, reason)| serde_json::json!({
                "slot": slot,
                "reason": format!("{reason:?}"),
                "explanation": reason.explanation(),
            })).collect::<Vec<_>>(),
            "latency": self.latency.to_json(),
            "health": {
                "dropped_events": self.health.dropped_events,
                "watchdog_tripped": self.health.watchdog_tripped,
                "reboot_required": self.health.reboot_required,
                "capture_panicked": self.health.panicked,
            },
        })
    }
}

/// The escape-hotkey banner, printed before any keyboard is ever captured.
///
/// It is deliberately explicit about Ctrl+C: Interception suppresses captured
/// strokes below win32k, so Windows never generates a `CTRL_C_EVENT` from a
/// captured keyboard and the console handler cannot fire. Telling a user in a
/// locked-up cabinet to "just press Ctrl+C" is the difference between an escape
/// hatch and a reboot.
pub fn escape_banner() -> String {
    let bar = "=".repeat(72);
    format!(
        "{bar}\n\
         EMERGENCY ESCAPES — these work even when a fullscreen game holds focus\n\
         \n  \
           LeftCtrl  x5    toggle keyboard capture off/on (high beep = on)\n  \
           RightCtrl x5    reserved for mouse capture — logged only in M4\n  \
           Ctrl+Alt+Del    stop emulation, unplug the pads, exit\n\
         \n\
         Press each hotkey 5 times with no other key in between. They are\n\
         evaluated inside the capture thread, so they keep working even if the\n\
         rest of ksx wedges.\n\
         \n\
         Ctrl+C does NOT work from a captured keyboard: its keystrokes are\n\
         suppressed before Windows sees them, so no console break event is ever\n\
         generated. Use it only from a keyboard ksx is not capturing, or before\n\
         blocking is enabled. `taskkill /f /im ksx.exe` also returns every\n\
         keyboard immediately — but you need a keyboard or mouse you can still\n\
         act from (the mouse is never captured in M4). Blocking needs no cleanup\n\
         to be undone: process death alone frees the keyboards.\n\
         {bar}\n"
    )
}

// ---------------------------------------------------------------------------
// Internal channel vocabulary
// ---------------------------------------------------------------------------

/// One pad transition plus the capture stamp it came from (latency accounting).
#[derive(Clone, Copy)]
struct OutMsg {
    slot: u8,
    state: PadState,
    t_capture: u64,
}

/// Level-triggered, non-blocking delta forwarding (engine → output).
///
/// [`PadState`] is a *snapshot*, not an edit: a newer state for a slot fully
/// supersedes an older one. So the engine never blocks on a stalled output
/// thread — it `try_send`s, and when the channel is full it keeps exactly one
/// pending state per slot (the newest) and flushes when the channel drains.
///
/// This is the F2 fix: a blocking `send()` here meant a wedged ViGEm IOCTL
/// stopped the engine, which stopped it draining key events — the mechanism
/// that made the whole pipeline (and, before F1, the escape hatch) hostage to
/// the output driver.
struct DeltaTx {
    tx: Sender<OutMsg>,
    /// At most one entry per slot, oldest first. Bounded by the slot count
    /// (≤ 4 in M4), so it allocates once and never grows on the hot path.
    pending: Vec<OutMsg>,
    coalesced: u64,
    dropped: u64,
    /// The live fan-out (`crate::feed`). Tapped at the TOP of [`Self::send`],
    /// before the coalescing below — a button held for one millisecond is a
    /// state this queue is entitled to collapse away, and the button check is
    /// not entitled to miss it.
    feed: crate::feed::LiveSink,
}

/// The output thread is gone; nothing can be delivered any more.
#[derive(Debug)]
struct OutputGone;

impl DeltaTx {
    fn new(tx: Sender<OutMsg>, feed: crate::feed::LiveSink) -> Self {
        Self {
            tx,
            pending: Vec::with_capacity(ksx_core::MAX_SLOTS as usize),
            coalesced: 0,
            dropped: 0,
            feed,
        }
    }

    /// Queue a delta and push as much as the channel will take. Never blocks.
    fn send(&mut self, msg: OutMsg) -> Result<(), OutputGone> {
        // The live tap, first, on the transition itself. With nobody watching
        // this is a relaxed load and a return.
        self.feed.pad(msg.slot, msg.state);
        match self.pending.iter_mut().find(|p| p.slot == msg.slot) {
            Some(slot) => {
                *slot = msg;
                self.coalesced += 1;
            }
            None => self.pending.push(msg),
        }
        self.flush()
    }

    fn flush(&mut self) -> Result<(), OutputGone> {
        while let Some(&msg) = self.pending.first() {
            match self.tx.try_send(msg) {
                Ok(()) => {
                    self.pending.remove(0);
                }
                // Still stalled: keep the newest state per slot and move on.
                Err(TrySendError::Full(_)) => break,
                Err(TrySendError::Disconnected(_)) => {
                    self.dropped += self.pending.len() as u64;
                    self.pending.clear();
                    return Err(OutputGone);
                }
            }
        }
        Ok(())
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

pub(crate) enum EngineCtl {
    /// A bound device vanished: release everything it was holding down.
    ReleaseDevice(DeviceId),
    /// The binding hot-swap: new dispatch tables, built by the SENDER on its
    /// own thread, to be moved into the running engine. The pads, their
    /// handles and the capture filters are untouched.
    SwapTables(Box<EngineTables>),
    /// Cancel every macro in flight and release what it held. Sent when the
    /// player stops driving these pads without the session ending — today that
    /// is the `LeftCtrl ×5` escape turning blocking off
    /// (docs/INPUT-TRANSFORMS.md §1c, "everything releases on the way out").
    CancelMacros,
    Stop,
}

enum OutputCtl {
    Stop,
}

/// Worker → supervisor.
///
/// Note what is *not* here any more: the emergency escapes. They used to travel
/// this channel from the engine thread, which put a starvable queue between the
/// gesture and the release. They now happen inside the capture thread and are
/// polled from `EscapeHandle` (F1).
enum Msg {
    PadsReady(Vec<PadInfo>),
    PlugFailed { message: String, bus_missing: bool },
    EventLimitReached,
    Latency(LatencySummary),
    OutputError(String),
}

/// Sends `SetPassthrough` exactly once — from teardown, or from `Drop` if the
/// supervisor unwinds. This is the "keyboards come back no matter what"
/// guarantee inside a living process; process death is covered by the capture
/// backend's own context guard.
struct CaptureGuard {
    ctl: Sender<CaptureCtl>,
    trace: Option<Trace>,
    released: AtomicBool,
}

impl CaptureGuard {
    fn new(ctl: Sender<CaptureCtl>, trace: Option<Trace>) -> Self {
        Self {
            ctl,
            trace,
            released: AtomicBool::new(false),
        }
    }

    fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        // Unconditional: releasing an already-passthrough backend is a no-op,
        // and second-guessing our own state is how keyboards stay captured.
        let _ = self.ctl.send(CaptureCtl::SetPassthrough);
        record(&self.trace, Step::BlockingDisabled);
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.release();
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// Run one emulation session to completion.
///
/// Returns `Err` only for failures that happen before anything is plugged or
/// captured; everything else comes back as an [`Outcome`] with a
/// [`StopReason`], because a session that started must always report how it
/// ended.
pub fn supervise(
    plan: &RunPlan,
    wiring: Wiring,
    opts: &mut RunOptions,
    out: &mut dyn Write,
) -> anyhow::Result<Outcome> {
    let Wiring {
        mut capture, pads, ..
    } = wiring;

    // The escape baseline is taken HERE, first statement, before pads are
    // plugged — not next to the health baseline further down.
    //
    // Why: under `ksx daemon` the panel is muted at the control loop
    // (`daemon::mod`'s Start arm) and pad plugging can take seconds. A player
    // staring at a suddenly-dead panel mashes P1 Button 1 — which on an I-PAC
    // *is* LeftCtrl. The claim's capture thread is daemon-lifetime and already
    // running, so it latches passthrough correctly and the panel starts typing.
    // Taking the baseline after that point would absorb the gesture: the panel
    // would go dead again with no `[ESC]` line and no trace, and a Ctrl+Alt+Del
    // in the same window would be discarded instead of stopping the session.
    // Escapes must never be undone by the machine (escape.rs module docs).
    let escape_baseline = capture.escapes().snapshot();

    // Startup order, step 0: what does the capture backend see right now? This
    // is both the hotplug baseline and the "you configured a device that isn't
    // plugged in" warning.
    let enumerated: Vec<DeviceInfo> = capture.devices();

    // ...and, before anything else, is any of it ambiguous? Two boards sharing
    // one hardware id cannot be told apart by Interception: capturing "that id"
    // captures both, and either board drives every slot bound to it. Refuse
    // while every keyboard is still perfectly normal (risk review §3 items 3-4).
    let ambiguous: Vec<DeviceId> = crate::devices::duplicate_hardware_ids(&enumerated)
        .into_iter()
        .filter(|id| plan.captureable.contains(id))
        .collect();
    if !ambiguous.is_empty() {
        let stop = StopReason::AmbiguousDevices { ids: ambiguous };
        let _ = writeln!(out, "{}", stop.message());
        let _ = out.flush();
        return Ok(Outcome {
            stop,
            pads: Vec::new(),
            latency: LatencySummary::default(),
            health: CaptureHealth::default(),
            events: 0,
            updates: 0,
            capture_exit: "not-started",
            toggles: 0,
            invalidated: Vec::new(),
            deltas_coalesced: 0,
            deltas_dropped: 0,
            plan_notes: plan.notes.clone(),
        });
    }

    let visible: BTreeSet<DeviceId> = enumerated.into_iter().map(|d| d.id).collect();
    let mut present: BTreeSet<DeviceId> = plan
        .captureable
        .iter()
        .filter(|d| visible.contains(*d))
        .cloned()
        .collect();
    for device in &plan.captureable {
        if !present.contains(device) {
            writeln!(
                out,
                "[WARN] slot(s) {:?} are bound to {device}, which the capture backend cannot \
                 see right now — {}",
                plan.slots_using(device),
                InvalidationReason::KeyboardUnplugged.explanation()
            )?;
        }
    }

    // The banner goes out BEFORE anything can block a keystroke.
    write!(out, "{}", escape_banner())?;
    out.flush()?;

    let (msg_tx, msg_rx) = unbounded::<Msg>();
    let (delta_tx, delta_rx) = bounded::<OutMsg>(DELTA_CHANNEL_CAPACITY);
    let (octl_tx, octl_rx) = unbounded::<OutputCtl>();
    let (ectl_tx, ectl_rx) = unbounded::<EngineCtl>();

    // Startup order, step 1: pads first. If the bus is missing or full we find
    // out while every keyboard is still perfectly normal.
    let slot_plugs: Vec<(u8, Persona)> = plan
        .slots
        .iter()
        .map(|s| (s.spec.number, s.spec.persona))
        .collect();
    let recorder = LatencyRecorder::new(opts.clock);
    let live = opts.live_latency.then_some(LIVE_LATENCY_INTERVAL);
    let output_msg = msg_tx.clone();
    let output_feed = opts.feed.clone();
    let output_handle = std::thread::Builder::new()
        .name("ksx-output".into())
        .spawn(move || {
            output_thread(
                pads,
                slot_plugs,
                delta_rx,
                octl_rx,
                output_msg,
                OutputReporting {
                    latency: recorder,
                    live,
                    feed: output_feed,
                },
            )
        })?;

    let pad_infos = match wait_for_pads(&msg_rx, &output_handle, opts.plug_deadline) {
        Ok(infos) => infos,
        Err(stop) => {
            let _ = octl_tx.send(OutputCtl::Stop);
            let outcome = output_handle.join().unwrap_or_default();
            return Ok(Outcome {
                stop,
                pads: Vec::new(),
                latency: outcome.latency,
                health: CaptureHealth::default(),
                events: 0,
                updates: 0,
                capture_exit: "not-started",
                toggles: 0,
                invalidated: Vec::new(),
                deltas_coalesced: 0,
                deltas_dropped: 0,
                plan_notes: plan.notes.clone(),
            });
        }
    };
    record(&opts.trace, Step::PadsPlugged);
    // From here on, writes are best-effort (`let _ =`). A `?` would return
    // early past the teardown block, leaving pads plugged and — worse, once
    // capture starts — keyboards captured, all because stdout hiccuped.
    let _ = writeln!(
        out,
        "pads plugged (slot number is NOT the XInput user index):"
    );
    for pad in &pad_infos {
        let _ = match (pad.user_index, pad.persona.is_xinput()) {
            (Some(index), _) => writeln!(
                out,
                "  slot {} -> XInput user index {index} (player {})",
                pad.slot,
                index + 1
            ),
            // Correct behavior, not a failure: this persona never gets an
            // XInput index. Printing the bus-full explanation here would send
            // someone debugging a working PlayStation pad.
            (None, false) => writeln!(
                out,
                "  slot {} -> {} (HID/DirectInput, no XInput index)",
                pad.slot,
                pad.persona.label()
            ),
            (None, true) => writeln!(
                out,
                "  slot {} -> no XInput slot ({})",
                pad.slot,
                InvalidationReason::XinputBusFull.explanation()
            ),
        };
    }
    let _ = out.flush();

    // Startup order, step 2: capture starts — in passthrough, always.
    //
    // The handles are grabbed here, and this is where the session's *health*
    // view begins (see the module docs) — taken BEFORE `capture.run` so nothing
    // this session's own capture thread publishes can be mistaken for history.
    //
    // The escape baseline is deliberately NOT taken here: it was captured as
    // the first statement of this function, before the pad phase, so a gesture
    // made while the panel was already muted and the pads were still plugging
    // still counts as this session's. Arming likewise belongs to whoever mutes
    // the panel — `ksx daemon` arms as it sets emulating, and `ksx run` builds a
    // fresh backend whose latch starts clear.
    let presence = capture.presence();
    let escapes = capture.escapes();
    let health = HealthView::new(capture.health());
    let (ev_tx, ev_rx) = bounded::<KeyEvent>(EVENT_CHANNEL_CAPACITY);
    let (ctl_tx, ctl_rx) = unbounded::<CaptureCtl>();
    let capture_handle = match capture.run(ev_tx, ctl_rx) {
        Ok(handle) => handle,
        Err(err) => {
            // Nothing is captured yet; unplug the pads and report upward.
            let _ = octl_tx.send(OutputCtl::Stop);
            let _ = output_handle.join();
            return Err(anyhow::Error::new(err).context("spawning the capture thread"));
        }
    };
    record(&opts.trace, Step::CaptureStarted);
    let guard = CaptureGuard::new(ctl_tx.clone(), opts.trace.clone());

    let engine_handle = std::thread::Builder::new()
        .name("ksx-engine".into())
        .spawn({
            let slots: Vec<ResolvedSlot> = plan.slots.clone();
            let msg = msg_tx.clone();
            let limits = Limits {
                max_events: opts.max_events,
                panic_after_events: opts.panic_after_events,
            };
            let feed = opts.feed.clone();
            move || engine_thread(slots, ev_rx, ectl_rx, delta_tx, msg, limits, feed)
        })?;

    // The pipeline is up: a surface watching the fan-out can now tell "nothing
    // is pressed" from "nothing is running". Cleared in teardown below, on
    // every path out, so a closed session never leaves a button check claiming
    // a live pipeline.
    //
    // `plan.captureable` goes with it: that is exactly "the keyboards bound to
    // a slot in THIS session", which is what makes the live feed's key column
    // mean the panel rather than every keyboard on the machine (`crate::feed`,
    // `LiveSink::key`).
    opts.feed.session_started(&plan.captureable);

    // The engine exists: a binding edit can now reach it in place. Published
    // here rather than at the top of the function because before this line
    // there is nothing to swap into — and cleared in teardown below, so a swap
    // can never be handed to an engine that has already joined.
    opts.hot_swap.publish(HotSwap {
        ectl: ectl_tx.clone(),
        shape: SessionShape::of(plan),
    });

    // Drop our own sender so a Disconnected on `msg_rx` genuinely means both
    // workers are gone.
    drop(msg_tx);

    // Startup order, step 3: enable blocking, for the bound devices only.
    let mut blocking = plan.block_keyboards;
    apply_capture(&ctl_tx, &opts.trace, blocking, &present);
    let _ = if blocking && !present.is_empty() {
        writeln!(
            out,
            "blocking enabled for {} assigned keyboard(s); every other keyboard keeps typing",
            present.len()
        )
    } else {
        writeln!(
            out,
            "running in passthrough: pads are driven and keystrokes still reach Windows"
        )
    };
    let _ = out.flush();

    // Startup order, step 4: the session hook. This is where `--game` launches
    // the game, and the ONLY place it may: the pads exist, they have their
    // XInput slots, capture is live and blocking is applied. A game started any
    // earlier sees zero controllers.
    //
    // A launch failure from here is exit 3, not 2 — emulation is up, so this
    // run has to be torn down, not merely refused.
    if let Err(message) = opts.hook.started(out) {
        tracing::error!(%message, "session hook failed to start; tearing down");
        let _ = writeln!(out, "[FAIL] {message}");
        let _ = out.flush();
        opts.hook.finished(out);
        opts.hot_swap.clear();
        opts.feed.session_ended();
        guard.release();
        let _ = ctl_tx.send(CaptureCtl::Shutdown);
        let capture_exit = match capture_handle.join() {
            Ok(reason) => exit_reason_str(reason),
            Err(_) => "join-panic",
        };
        let _ = ectl_tx.send(EngineCtl::Stop);
        let engine = engine_handle.join().unwrap_or_default();
        let _ = octl_tx.send(OutputCtl::Stop);
        let output = output_handle.join().unwrap_or_default();
        record(&opts.trace, Step::PadsUnplugged);
        return Ok(Outcome {
            stop: HookStop::LaunchFailed(message).into(),
            pads: pad_infos,
            latency: output.latency,
            health: health.snapshot(),
            events: engine.events,
            updates: output.updates,
            capture_exit,
            toggles: 0,
            invalidated: Vec::new(),
            deltas_coalesced: engine.coalesced,
            deltas_dropped: engine.dropped,
            plan_notes: plan.notes.clone(),
        });
    }

    // ---- supervisor loop ---------------------------------------------------
    let mut stop: Option<StopReason> = None;
    let mut toggles = 0u32;
    let mut invalidated: Vec<(u8, InvalidationReason)> = Vec::new();
    // Seeded from the baseline, NOT from zero. `mirror_escapes` replays every
    // gesture between `seen` and the live counter, and on a daemon-lifetime
    // handle "zero" means "every gesture since the daemon started" — an old
    // `LeftCtrl ×5` would toggle this session's blocking straight back off and
    // print an `[ESC]` line for something that happened two games ago.
    let mut seen_escapes = escape_baseline;

    while stop.is_none() {
        if (opts.stop)() {
            stop = Some(StopReason::CtrlC);
            break;
        }

        // Capture health FIRST, before any decision that could change capture
        // state. A tripped watchdog means keystrokes are already reaching
        // Windows again; we still tear down, because a pipeline that stalled
        // once is not a pipeline anyone should keep playing on — and nothing
        // below (hotplug, an escape mirror) may re-arm blocking on the way out.
        let snapshot = health.snapshot();
        if snapshot.panicked {
            stop = Some(StopReason::CapturePanicked);
            break;
        }
        if snapshot.watchdog_tripped {
            stop = Some(StopReason::WatchdogTripped);
            break;
        }

        // Emergency escapes. The capture thread has ALREADY acted by the time
        // these counters move — it flipped its own passthrough latch with no
        // channel in the way. All that is left here is bookkeeping, printing,
        // and mirroring the ctl-level state so the two agree. Toggles are
        // processed before the stop check so a gesture in the same 50 ms window
        // as the emergency stop is still recorded.
        let escape = escapes.snapshot();
        let was_blocking = blocking;
        mirror_escapes(
            &escape,
            &mut seen_escapes,
            &mut blocking,
            &mut toggles,
            opts.beep,
            Some((&ctl_tx, &opts.trace, &present)),
            out,
        );
        // The gesture just handed the keyboard back to Windows. A macro in
        // flight would keep driving the pad from a panel the player is no
        // longer using — so it is cancelled, and what it held is released
        // (docs/INPUT-TRANSFORMS.md §1c).
        if was_blocking && !blocking {
            let _ = ectl_tx.send(EngineCtl::CancelMacros);
        }
        if escape.stops > seen_escapes.stops {
            seen_escapes.stops = escape.stops;
            stop = Some(StopReason::EmergencyStop);
            break;
        }

        // Hotplug: presence is republished by the capture thread's idle path.
        if let Some(snapshot) = presence.snapshot() {
            let mut changed = false;
            for device in &plan.captureable {
                let now = snapshot.contains(device);
                if now == present.contains(device) {
                    continue;
                }
                changed = true;
                let slots = plan.slots_using(device);
                if now {
                    present.insert(device.clone());
                    invalidated.retain(|(slot, _)| !slots.contains(slot));
                    tracing::warn!(%device, ?slots, "bound device is back");
                    let _ = writeln!(out, "[OK]   device back: {device} -> slot(s) {slots:?}");
                } else {
                    present.remove(device);
                    // Stuck-key invariant: whatever it was holding must be
                    // released on every pad it fed.
                    let _ = ectl_tx.send(EngineCtl::ReleaseDevice(device.clone()));
                    for slot in &slots {
                        invalidated.push((*slot, InvalidationReason::KeyboardUnplugged));
                    }
                    tracing::error!(
                        %device, ?slots,
                        "bound device unplugged — slots invalidated; emulation continues"
                    );
                    let _ = writeln!(
                        out,
                        "[FAIL] device unplugged: {device} -> slot(s) {slots:?} invalidated. {}",
                        InvalidationReason::KeyboardUnplugged.explanation()
                    );
                }
            }
            if changed {
                apply_capture(&ctl_tx, &opts.trace, blocking, &present);
                let _ = out.flush();
            }
        }

        // The session hook: has the game exited? Polled after the health and
        // escape checks so a driver problem always outranks a game problem, and
        // before the worker checks so a clean game exit is reported as such
        // even if a worker is finishing in the same 50 ms window.
        if let Some(hook_stop) = opts.hook.poll(out) {
            stop = Some(hook_stop.into());
            break;
        }

        // A dead worker is always fatal: nothing else can guarantee the
        // keyboards get released.
        if capture_handle.is_finished() {
            stop = Some(StopReason::CaptureEnded("thread exited"));
            break;
        }
        if engine_handle.is_finished() {
            stop = Some(StopReason::EngineThreadDied);
            break;
        }
        if output_handle.is_finished() {
            stop = Some(StopReason::OutputThreadDied);
            break;
        }

        match msg_rx.recv_timeout(POLL) {
            Ok(Msg::EventLimitReached) => stop = Some(StopReason::EventLimit),
            Ok(Msg::OutputError(err)) => stop = Some(StopReason::OutputError(err)),
            Ok(Msg::Latency(summary)) => {
                let _ = writeln!(out, "{}", summary.render());
                let _ = out.flush();
            }
            Ok(Msg::PadsReady(_) | Msg::PlugFailed { .. }) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => stop = Some(StopReason::EngineThreadDied),
        }
    }

    // A gesture in the last few milliseconds — after the final poll, before the
    // loop broke — still happened, and the report must say so. Counting only:
    // re-arming capture on the way out would be absurd.
    mirror_escapes(
        &escapes.snapshot(),
        &mut seen_escapes,
        &mut blocking,
        &mut toggles,
        false,
        None,
        out,
    );

    // The hook is told the session is over before anything is torn down, on
    // every path out — clean stop, emergency escape, watchdog, thread death.
    // It never kills the game; it only stops tracking it (see
    // `ksx_platform::process`' no-kill policy). A game the player is still in
    // keeps running, with a keyboard that types again.
    opts.hook.finished(out);
    // Nothing may hand tables to an engine that is about to join.
    opts.hot_swap.clear();
    // ...and no surface may go on claiming a live pipeline once there is not
    // one. Both teardown paths clear it, so a button check that was open when
    // the game exited says "no session is running" rather than "nothing is
    // pressed". The panel set goes with it: between sessions there is no bound
    // device, so nothing is the panel.
    opts.feed.session_ended();

    // ---- teardown: uncapture, THEN unplug ----------------------------------
    //
    // This order is also what makes the worst case survivable. If a wedged
    // driver made `unplug` (or a join) hang forever, everything that frees the
    // keyboards has already happened: the process would hang with ghost pads
    // and perfectly working keyboards. The reverse order would hang with the
    // keyboards still captured, which is the one outcome that needs a reboot.
    guard.release();
    let _ = ctl_tx.send(CaptureCtl::Shutdown);
    let capture_exit = match capture_handle.join() {
        Ok(reason) => exit_reason_str(reason),
        Err(_) => "join-panic",
    };

    let _ = ectl_tx.send(EngineCtl::Stop);
    let engine = engine_handle.join().unwrap_or_default();

    let _ = octl_tx.send(OutputCtl::Stop);
    let output = output_handle.join().unwrap_or_default();
    record(&opts.trace, Step::PadsUnplugged);

    Ok(Outcome {
        stop: stop.unwrap_or(StopReason::CtrlC),
        pads: pad_infos,
        latency: output.latency,
        health: health.snapshot(),
        events: engine.events,
        updates: output.updates,
        capture_exit,
        toggles,
        invalidated,
        deltas_coalesced: engine.coalesced,
        deltas_dropped: engine.dropped,
        plan_notes: plan.notes.clone(),
    })
}

fn wait_for_pads(
    msg_rx: &Receiver<Msg>,
    output: &std::thread::JoinHandle<OutputOutcome>,
    deadline: Duration,
) -> Result<Vec<PadInfo>, StopReason> {
    let until = Instant::now() + deadline;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(StopReason::PlugFailed {
                message: format!("timed out after {deadline:?} waiting for the pads to plug"),
                bus_missing: false,
            });
        }
        // A panicking output thread never sends anything; without this we would
        // sit out the whole deadline for a failure we already know about.
        if output.is_finished() {
            return Err(StopReason::PlugFailed {
                message: "the output thread ended before any pad was plugged".to_owned(),
                bus_missing: false,
            });
        }
        match msg_rx.recv_timeout(left.min(POLL)) {
            Ok(Msg::PadsReady(infos)) => return Ok(infos),
            Ok(Msg::PlugFailed {
                message,
                bus_missing,
            }) => {
                return Err(StopReason::PlugFailed {
                    message,
                    bus_missing,
                })
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(StopReason::PlugFailed {
                    message: "the output thread ended before any pad was plugged".to_owned(),
                    bus_missing: false,
                })
            }
        }
    }
}

/// Mirror the capture thread's escape counters into the supervisor's own state:
/// count the toggles, flip `blocking` to match, print, beep.
///
/// The capture thread has already acted — this can only ever be bookkeeping.
/// `apply` carries the ctl mirror (so the ctl-level state agrees with the latch,
/// which is what lets the *next* gesture re-arm capture); it is `None` for the
/// final reconciliation after the loop has decided to stop.
fn mirror_escapes(
    status: &EscapeStatus,
    seen: &mut EscapeStatus,
    blocking: &mut bool,
    toggles: &mut u32,
    beep_enabled: bool,
    apply: Option<(&Sender<CaptureCtl>, &Option<Trace>, &BTreeSet<DeviceId>)>,
    out: &mut dyn Write,
) {
    if status.mouse_escapes > seen.mouse_escapes {
        seen.mouse_escapes = status.mouse_escapes;
        tracing::info!(
            "emergency escape: RightCtrl x5 (mouse capture) — ignored, M4 never sets \
             the mouse class filter"
        );
        let _ = writeln!(
            out,
            "[ESC]  RightCtrl x5 — mouse capture is not implemented in M4 (ignored)"
        );
    }
    while seen.toggles < status.toggles {
        seen.toggles += 1;
        *blocking = !*blocking;
        *toggles += 1;
        if let Some((ctl, trace, present)) = apply {
            apply_capture(ctl, trace, *blocking, present);
        }
        let state = if *blocking { "ON" } else { "OFF" };
        tracing::warn!(
            blocking = *blocking,
            "emergency escape: keyboard capture toggled"
        );
        let _ = writeln!(out, "[ESC]  LeftCtrl x5 — keyboard capture {state}");
        if beep_enabled {
            beep(*blocking);
        }
    }
    let _ = out.flush();
}

/// The single place capture state is changed. Blocking is only ever enabled for
/// devices that are BOTH bound to a slot AND currently present.
fn apply_capture(
    ctl: &Sender<CaptureCtl>,
    trace: &Option<Trace>,
    blocking: bool,
    present: &BTreeSet<DeviceId>,
) {
    if blocking && !present.is_empty() {
        let _ = ctl.send(CaptureCtl::SetCaptured(present.iter().cloned().collect()));
        record(trace, Step::BlockingEnabled);
    } else {
        let _ = ctl.send(CaptureCtl::SetPassthrough);
        record(trace, Step::BlockingDisabled);
    }
}

fn exit_reason_str(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::Shutdown => "shutdown",
        ExitReason::ChannelClosed => "channel-closed",
        ExitReason::ScriptExhausted => "script-exhausted",
        ExitReason::Panicked => "panicked",
        // M6: a claimed board was unplugged. Deliberately its own string and
        // not folded into "shutdown" — the session ended because hardware
        // left, which is a different thing to explain to a user than "you
        // asked me to stop".
        ExitReason::DeviceLost => "device-lost",
    }
}

/// The legacy audio cue (`Sounds.Connected`/`Disconnected`), as a console beep.
/// Detached so the supervisor never waits on it — and the hot path never sees
/// it at all, since escapes are policy, not translation.
#[cfg(windows)]
fn beep(on: bool) {
    let freq = if on { 880 } else { 440 };
    let _ = std::thread::Builder::new()
        .name("ksx-beep".into())
        .spawn(move || {
            // SAFETY: kernel32 Beep takes two scalars and cannot fail unsafely.
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::Beep(freq, 150) };
        });
}

#[cfg(not(windows))]
fn beep(_on: bool) {}

// ---------------------------------------------------------------------------
// Engine thread
// ---------------------------------------------------------------------------

struct Limits {
    max_events: Option<u64>,
    panic_after_events: Option<u64>,
}

/// Raises the Windows system timer resolution to 1 ms **while a macro is
/// playing back**, and puts it straight back afterwards.
///
/// Why it is needed: the default timer resolution is ~15.6 ms, and a macro step
/// is 33 ms. Waking 15 ms late every step is not a correctness bug — the
/// absolute schedule in `ksx_core` corrects for it and the late-wake rule never
/// lets a step go unpublished — but it turns a crisp quarter-circle into a
/// mushy one, and on a fighting cabinet that is the whole point of the feature.
///
/// Why it is scoped: `timeBeginPeriod` is process-wide and, on Windows, its
/// effect on the *system* is real. `ksx daemon` lives for the whole session, so
/// holding 1 ms for its entire life to serve a macro somebody presses twice an
/// hour would tax every other process's power management for nothing. It goes
/// up when the first deadline is armed and comes down when the last one clears
/// — and `Drop` puts it back on every exit path, panic included, so no ksx
/// crash can leave the system timer raised.
/// `pub(crate)` so `ksx macro trace` raises it the same way, from the same
/// code: a tracer measuring the scheduler under a coarser timer than the daemon
/// runs would be measuring the wrong machine.
#[derive(Default)]
pub(crate) struct TimerResolution {
    raised: bool,
}

impl TimerResolution {
    /// `wanted` is "a macro is armed right now".
    pub(crate) fn set(&mut self, wanted: bool) {
        if wanted == self.raised {
            return;
        }
        self.raised = wanted;
        #[cfg(windows)]
        {
            // SAFETY: timeBeginPeriod/timeEndPeriod take a scalar and are
            // documented to be called in matched pairs, which `raised` is what
            // guarantees. Failure is reported, never unsafe.
            unsafe {
                if wanted {
                    windows_sys::Win32::Media::timeBeginPeriod(1);
                } else {
                    windows_sys::Win32::Media::timeEndPeriod(1);
                }
            }
        }
        tracing::debug!(raised = wanted, "macro playback timer resolution");
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// What the engine thread reports back at join.
#[derive(Default)]
struct EngineOutcome {
    events: u64,
    coalesced: u64,
    dropped: u64,
}

/// Owns the [`Engine`]. Consumes key events, emits coalesced pad deltas.
///
/// This thread must never block: it is the only consumer of the capture
/// channel, and a blocked consumer is what trips the capture watchdog (and,
/// before F1, killed the escape hatch outright). Every path out of here is
/// `try_send` (see [`DeltaTx`]).
///
/// Escapes are **not** evaluated here any more — they live in the capture
/// thread, upstream of every queue (risk review §3 item 2).
fn engine_thread(
    slots: Vec<ResolvedSlot>,
    events: Receiver<KeyEvent>,
    ctl: Receiver<EngineCtl>,
    deltas: Sender<OutMsg>,
    msg: Sender<Msg>,
    limits: Limits,
    feed: crate::feed::LiveSink,
) -> EngineOutcome {
    let mut engine = Engine::new(slots);
    let mut deltas = DeltaTx::new(deltas, feed.clone());
    let mut seen: u64 = 0;
    let mut limit_signalled = false;
    // The macro clock: milliseconds since this thread started. Monotonic, and
    // supplied to the engine rather than read by it, so the whole scheduler
    // stays a pure function of (events, clock) and the core tests can drive it
    // with a fake one (docs/INPUT-TRANSFORMS.md §1c).
    let started = Instant::now();
    let clock = move || started.elapsed().as_millis() as u64;
    // Raised only while a macro is actually playing; `Drop` restores it on
    // every exit path, panic included.
    let mut timer_resolution = TimerResolution::default();

    macro_rules! finish {
        () => {
            return EngineOutcome {
                events: seen,
                coalesced: deltas.coalesced,
                dropped: deltas.dropped + deltas.pending.len() as u64,
            }
        };
    }

    loop {
        // Retry a stalled flush promptly, but idle at the normal poll rate when
        // there is nothing waiting...
        let mut idle = if deltas.has_pending() {
            Duration::from_millis(1)
        } else {
            POLL
        };
        // ...and never sleep past a macro step. This is the whole scheduler
        // integration on this side: the loop already woke on input and on a
        // timeout, so a macro is one more reason to wake, not a thread.
        let next_deadline = engine.next_deadline();
        timer_resolution.set(next_deadline.is_some());
        if let Some(deadline) = next_deadline {
            idle = idle.min(Duration::from_millis(deadline.saturating_sub(clock())));
        }
        crossbeam_channel::select! {
            recv(events) -> event => {
                let Ok(event) = event else {
                    // Capture is gone; the supervisor sees the thread finish.
                    finish!();
                };
                // The live tap's other half: what the PANEL sent, before the
                // engine has an opinion about it. That ordering is the point —
                // a key that arrives here and lights nothing downstream is an
                // unbound key, and the button check can only say so if it sees
                // both columns. Free when nobody is watching.
                feed.key(&event.device, event.key, event.down);
                for delta in engine.handle_at(&event, clock()) {
                    if deltas.send(OutMsg {
                        slot: delta.slot,
                        state: delta.state,
                        t_capture: event.t,
                    }).is_err() {
                        finish!(); // output thread is gone
                    }
                }
                seen += 1;
                if limits.panic_after_events.is_some_and(|n| seen >= n) {
                    panic!("ksx test hook: deliberate engine-thread panic after {seen} event(s)");
                }
                if !limit_signalled && limits.max_events.is_some_and(|n| seen >= n) {
                    limit_signalled = true;
                    let _ = msg.send(Msg::EventLimitReached);
                }
            }
            recv(ctl) -> command => match command {
                Ok(EngineCtl::ReleaseDevice(device)) => {
                    for delta in engine.release_device(&device) {
                        if deltas.send(OutMsg {
                            slot: delta.slot,
                            state: delta.state,
                            // No originating keystroke: attribute the release to
                            // "now" so it never pollutes the latency histogram
                            // with a synthetic sample.
                            t_capture: u64::MAX,
                        }).is_err() {
                            finish!();
                        }
                    }
                }
                // The binding hot-swap. The tables arrive fully built (the
                // sender did that work on its own thread), so this is a move
                // and nothing else. `swap_tables` returns the neutral states of
                // any control that was HELD across the edit — forwarding them
                // is what stops a rebind stranding a pressed virtual button.
                Ok(EngineCtl::SwapTables(tables)) => {
                    for delta in engine.swap_tables(*tables) {
                        if deltas.send(OutMsg {
                            slot: delta.slot,
                            state: delta.state,
                            t_capture: u64::MAX,
                        }).is_err() {
                            finish!();
                        }
                    }
                }
                Ok(EngineCtl::CancelMacros) => {
                    for delta in engine.cancel_macros() {
                        if deltas.send(OutMsg {
                            slot: delta.slot,
                            state: delta.state,
                            t_capture: u64::MAX,
                        }).is_err() {
                            finish!();
                        }
                    }
                }
                Ok(EngineCtl::Stop) | Err(_) => {
                    // Everything releases on the way out. The pads are about to
                    // be unplugged anyway, but a macro half-way through a
                    // quarter-circle must not be what the last frame says.
                    for delta in engine.cancel_macros() {
                        if deltas.send(OutMsg {
                            slot: delta.slot,
                            state: delta.state,
                            t_capture: u64::MAX,
                        }).is_err() {
                            finish!();
                        }
                    }
                    let _ = deltas.flush();
                    finish!();
                }
            },
            default(idle) => {
                if deltas.flush().is_err() {
                    finish!();
                }
            }
        }

        // Macro deadlines, checked after EVERY wake — an input event, a poll,
        // or the deadline itself. `tick` is a no-op with nothing armed, which
        // is what keeps a macro-free session bit-for-bit what it was.
        for delta in engine.tick(clock()) {
            if deltas
                .send(OutMsg {
                    slot: delta.slot,
                    state: delta.state,
                    // No originating keystroke: a scheduled step is not a latency
                    // sample (`u64::MAX` is the output thread's "synthetic" mark).
                    t_capture: u64::MAX,
                })
                .is_err()
            {
                finish!();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Output thread
// ---------------------------------------------------------------------------

#[derive(Default)]
struct OutputOutcome {
    latency: LatencySummary,
    updates: u64,
}

/// Everything the output thread REPORTS with, as opposed to everything it
/// drives. Grouped because the three of them arrive together, are read on the
/// same 50 ms service tick, and none of them can affect a pad.
struct OutputReporting {
    latency: LatencyRecorder,
    /// `--latency`: how often to emit a rolling summary. `None` = never.
    live: Option<Duration>,
    /// The live fan-out (`crate::feed`) — this thread's half is the game's
    /// feedback (E8's stream).
    feed: crate::feed::LiveSink,
}

/// Owns the pad backend for its whole life: plugs at startup, applies deltas,
/// drains feedback, unplugs on `Stop`. Nothing else ever touches the driver.
///
/// It never unplugs on its own initiative *after* the session is running: a
/// failing `update` is reported to the supervisor, which uncaptures the
/// keyboards and only then sends `Stop`. Unplugging here first would open a
/// window with no pads and captured keyboards — the ordering the teardown
/// contract exists to prevent. (Before `PadsReady`, cleanup here is the right
/// thing: nothing is captured yet and nobody else knows about those handles.)
fn output_thread(
    mut pads: Box<dyn VirtualPadBackend>,
    slots: Vec<(u8, Persona)>,
    deltas: Receiver<OutMsg>,
    ctl: Receiver<OutputCtl>,
    msg: Sender<Msg>,
    reporting: OutputReporting,
) -> OutputOutcome {
    let OutputReporting {
        mut latency,
        live,
        feed,
    } = reporting;
    let mut handles: Vec<(u8, PadHandle)> = Vec::with_capacity(slots.len());
    for &(slot, persona) in &slots {
        match pads.plug_persona(persona) {
            Ok(handle) => handles.push((slot, handle)),
            Err(err) => {
                tracing::error!(%err, slot, %persona, "plugging a virtual pad failed");
                for (_, handle) in handles.drain(..) {
                    let _ = pads.unplug(handle);
                }
                let _ = msg.send(Msg::PlugFailed {
                    message: err.to_string(),
                    bus_missing: err.is_bus_missing(),
                });
                return OutputOutcome::default();
            }
        }
    }

    let infos: Vec<PadInfo> = handles
        .iter()
        .map(|&(slot, handle)| PadInfo {
            slot,
            handle: handle.raw(),
            persona: pads.persona(handle).unwrap_or_default(),
            user_index: pads.user_index(handle),
        })
        .collect();
    if msg.send(Msg::PadsReady(infos)).is_err() {
        for (_, handle) in handles.drain(..) {
            let _ = pads.unplug(handle);
        }
        return OutputOutcome::default();
    }

    let idle = never::<OutMsg>();
    let mut deltas = deltas;
    let mut updates = 0u64;
    let mut last_service = Instant::now();
    let mut last_live = Instant::now();
    // Set once a pad update fails: we stop touching the driver and wait for the
    // supervisor to run teardown in the right order.
    let mut failed = false;

    'outer: loop {
        crossbeam_channel::select! {
            recv(deltas) -> message => match message {
                Ok(OutMsg { slot, state, t_capture }) => {
                    // Once `failed` is set the supervisor has been told and is
                    // running teardown; we keep draining (so nothing upstream
                    // blocks) but never touch the driver again.
                    let target = if failed {
                        None
                    } else {
                        handles.iter().find(|(n, _)| *n == slot).copied()
                    };
                    if let Some((_, handle)) = target {
                        match pads.update(handle, &state) {
                            Ok(()) => {
                                updates += 1;
                                // u64::MAX marks a synthetic release (device
                                // unplugged): no capture stamp to measure against.
                                if t_capture != u64::MAX {
                                    latency.record_since(t_capture);
                                }
                            }
                            Err(err) => {
                                tracing::error!(%err, slot, "pad update failed");
                                failed = true;
                                // Report and let the supervisor uncapture FIRST.
                                // Unplugging here would strand captured keyboards.
                                if msg.send(Msg::OutputError(err.to_string())).is_err() {
                                    // No supervisor left to order the teardown.
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                // The engine is gone. Keep serving ctl so teardown still runs
                // in order — but never spin on a closed channel.
                Err(_) => deltas = idle.clone(),
            },
            recv(ctl) -> command => match command {
                Ok(OutputCtl::Stop) | Err(_) => break 'outer,
            },
            default(POLL) => {}
        }

        if last_service.elapsed() >= POLL {
            last_service = Instant::now();
            for &(slot, handle) in &handles {
                while let Some(feedback) = pads.poll_feedback(handle) {
                    tracing::debug!(
                        slot,
                        large_motor = feedback.large_motor,
                        small_motor = feedback.small_motor,
                        led = feedback.led_number,
                        "pad feedback"
                    );
                    // E8's stream, on E7's bus. This queue used to be drained
                    // and discarded — "today that data is discarded: nothing
                    // consumes the queue" (docs/ENHANCEMENTS.md E8). It now has
                    // exactly one consumer shape, the same lossy fan-out the
                    // button check reads, so a lamp driver is a subscriber
                    // rather than a second tap into the output thread.
                    feed.feedback(ksx_api::PadFeedback {
                        slot,
                        large_motor: feedback.large_motor,
                        small_motor: feedback.small_motor,
                        led_number: feedback.led_number,
                    });
                }
            }
        }
        if let Some(interval) = live {
            if last_live.elapsed() >= interval {
                last_live = Instant::now();
                let _ = msg.send(Msg::Latency(latency.summary()));
            }
        }
    }

    // Never leave a pad holding a button: neutral first, then unplug in slot
    // order (the reverse of plug order would reshuffle XInput indices for
    // anything watching).
    for &(_, handle) in &handles {
        let _ = pads.update(handle, &PadState::default());
    }
    for (slot, handle) in handles.drain(..) {
        if let Err(err) = pads.unplug(handle) {
            tracing::warn!(%err, slot, "unplugging a virtual pad failed");
        }
    }
    drop(pads);

    OutputOutcome {
        latency: latency.summary(),
        updates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_names_every_escape_before_blocking_can_start() {
        let banner = escape_banner();
        assert!(banner.contains("LeftCtrl  x5"), "{banner}");
        assert!(banner.contains("RightCtrl x5"), "{banner}");
        assert!(banner.contains("Ctrl+Alt+Del"), "{banner}");
        assert!(banner.contains("taskkill"), "{banner}");
    }

    /// The banner is the last thing a user reads before their keyboards can be
    /// taken away, so it must not imply Ctrl+C is one of the ways back.
    #[test]
    fn banner_is_honest_about_ctrl_c() {
        let banner = escape_banner();
        assert!(
            banner.contains("Ctrl+C does NOT work from a captured keyboard"),
            "{banner}"
        );
        assert!(
            banner.contains("keyboard ksx is not capturing")
                || banner.contains("keyboard ksx is not capturing, or before"),
            "{banner}"
        );
        assert!(
            banner.contains("you need a keyboard or mouse you can still"),
            "taskkill needs an input device you can act from: {banner}"
        );
        assert!(
            banner.contains("mouse is never captured"),
            "the mouse is the reliable way to reach Task Manager: {banner}"
        );
    }

    fn outcome(stop: StopReason) -> Outcome {
        Outcome {
            stop,
            pads: Vec::new(),
            latency: LatencySummary::default(),
            health: CaptureHealth::default(),
            events: 0,
            updates: 0,
            capture_exit: "shutdown",
            toggles: 0,
            invalidated: Vec::new(),
            deltas_coalesced: 0,
            deltas_dropped: 0,
            plan_notes: Vec::new(),
        }
    }

    /// Every failure that happens BEFORE the capture thread is told to capture
    /// anything is a 2, not a 3: no filter was ever set, so nothing was ever at
    /// risk. Only `BusNotFound` used to qualify, which mis-reported a plug
    /// timeout, a full bus, or a dead output thread as "started then failed".
    #[test]
    fn every_pre_capture_failure_exits_2() {
        for message in [
            "timed out after 30s waiting for the pads to plug",
            "the output thread ended before any pad was plugged",
            "ViGEmBus target operation failed",
        ] {
            let stop = StopReason::PlugFailed {
                message: message.to_owned(),
                // The distinguishing detail is deliberately NOT the exit code's
                // input: nothing was plugged either way.
                bus_missing: false,
            };
            assert_eq!(
                outcome(stop.clone()).exit_code(),
                super::super::EXIT_CANNOT_START,
                "'{message}' happens before any keyboard is captured"
            );
            assert!(outcome(stop).stop.message().contains(message));
        }
        assert_eq!(
            outcome(StopReason::AmbiguousDevices {
                ids: vec![DeviceId::from("HID\\VID_D209&PID_0430&REV_0056&MI_00")],
            })
            .exit_code(),
            super::super::EXIT_CANNOT_START
        );
    }

    #[test]
    fn ambiguous_devices_message_names_the_id_and_the_way_out() {
        let id = DeviceId::from("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let text = StopReason::AmbiguousDevices {
            ids: vec![id.clone()],
        }
        .message();
        assert!(text.contains(id.as_str()), "{text}");
        assert!(text.contains("refusing to start"), "{text}");
        assert!(text.contains("Nothing was plugged"), "{text}");
        assert!(text.contains("WinUSB"), "{text}");
    }

    #[test]
    fn exit_codes_split_clean_refused_and_failed() {
        assert_eq!(outcome(StopReason::CtrlC).exit_code(), 0);
        assert_eq!(outcome(StopReason::EmergencyStop).exit_code(), 0);
        assert_eq!(outcome(StopReason::EventLimit).exit_code(), 0);
        assert_eq!(
            outcome(StopReason::PlugFailed {
                message: "no bus".into(),
                bus_missing: true
            })
            .exit_code(),
            super::super::EXIT_CANNOT_START
        );
        for reason in [
            StopReason::WatchdogTripped,
            StopReason::EngineThreadDied,
            StopReason::OutputThreadDied,
            StopReason::CapturePanicked,
            StopReason::CaptureEnded("thread exited"),
            StopReason::OutputError("boom".into()),
        ] {
            assert_eq!(
                outcome(reason.clone()).exit_code(),
                super::super::EXIT_RUNTIME_FAILURE,
                "{reason:?}"
            );
            assert!(!reason.message().is_empty());
            assert!(!reason.code().is_empty());
        }
    }

    #[test]
    fn outcome_json_carries_the_plan_notes_and_coalescing_counts() {
        // `--json` used to drop the plan notes entirely: they were printed on
        // the human path only, so an automated caller never saw "block_mice is
        // ignored" or "slot 3 skipped".
        let mut o = outcome(StopReason::CtrlC);
        o.plan_notes = vec!["[WARN] block_mice is set but ignored in M4".to_owned()];
        o.deltas_coalesced = 7;
        o.deltas_dropped = 2;
        let v = o.to_json();
        assert_eq!(
            v.pointer("/notes/0"),
            Some(&serde_json::json!(
                "[WARN] block_mice is set but ignored in M4"
            )),
            "{v}"
        );
        assert_eq!(v.pointer("/deltas_coalesced"), Some(&serde_json::json!(7)));
        assert_eq!(v.pointer("/deltas_dropped"), Some(&serde_json::json!(2)));
    }

    /// F2: the engine must never block on the output thread. A full channel
    /// coalesces per slot — the newest state wins, older ones are counted, and
    /// `send` returns immediately no matter what.
    #[test]
    fn delta_tx_coalesces_per_slot_instead_of_blocking() {
        let (tx, rx) = bounded::<OutMsg>(1);
        let deltas = DeltaTx::new(tx, crate::feed::LiveSink::default());
        fn state(lt: u8) -> PadState {
            PadState {
                lt,
                ..PadState::default()
            }
        }
        fn msg(slot: u8, lt: u8) -> OutMsg {
            OutMsg {
                slot,
                state: state(lt),
                t_capture: 0,
            }
        }

        // The first send fits in the channel; everything after finds it full
        // and queues — but never blocks and never loses the newest state.
        // Run them on a worker with a deadline: a blocking implementation must
        // FAIL this test, not hang CI forever.
        let (done_tx, done_rx) = bounded::<DeltaTx>(1);
        std::thread::spawn(move || {
            let mut deltas = deltas;
            for m in [msg(1, 1), msg(1, 2), msg(1, 3), msg(1, 4), msg(2, 9)] {
                deltas.send(m).expect("output alive");
            }
            let _ = done_tx.send(deltas);
        });
        let mut deltas = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the engine must never block on a full delta channel");

        assert_eq!(
            deltas.coalesced, 2,
            "slot 1 superseded its pending state twice; slot 2 never did"
        );
        assert!(deltas.has_pending());

        // The consumer wakes up: the channel drains, and slot 1 delivers the
        // NEWEST state, never a stale one.
        assert_eq!(rx.recv().unwrap().state, state(1));
        deltas.flush().expect("output alive");
        let a = rx.recv().unwrap();
        deltas.flush().expect("output alive");
        let b = rx.recv().unwrap();
        assert_eq!((a.slot, a.state), (1, state(4)));
        assert_eq!((b.slot, b.state), (2, state(9)));
        assert!(!deltas.has_pending());
        assert_eq!(deltas.dropped, 0);
    }

    /// Is `have` a subsequence of `want` (order preserved, gaps allowed)?
    fn is_subsequence(have: &[PadState], want: &[PadState]) -> bool {
        let mut it = want.iter();
        have.iter().all(|h| it.any(|w| w == h))
    }

    /// F2's real safety property, replayed against the recorded cabinet session
    /// (`crates/ksx-capture/tests/corpus/ipac-session-001.jsonl`).
    ///
    /// Coalescing is only ever allowed to drop *intermediate* states: a burst
    /// that got squashed must leave every pad holding exactly the state an
    /// un-squashed burst would have left it in. If that ever stopped being true,
    /// a busy moment would strand a pad with a button held down (or released)
    /// forever — the one failure a level-triggered design must not have.
    ///
    /// Swept across channel depths and consumer schedules, from "drains after
    /// every send" (no coalescing at all) to "never drains until the end" (worst
    /// case), so the invariant is checked at both extremes and in between.
    #[test]
    fn coalescing_preserves_the_final_pad_state_of_the_recorded_session() {
        use std::collections::BTreeMap;

        use super::super::pipeline_tests::{cabinet_slots, load_corpus};

        // The uncoalesced truth: exactly what a bare Engine emits, in order.
        let events = load_corpus();
        let mut engine = Engine::new(cabinet_slots());
        let truth: Vec<(u8, PadState)> = events
            .iter()
            .enumerate()
            .flat_map(|(i, e)| {
                engine
                    .handle(&KeyEvent {
                        device: e.device.clone(),
                        key: e.key,
                        down: e.down,
                        t: i as u64,
                    })
                    .into_iter()
                    .map(|d| (d.slot, d.state))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(truth.len() > 100, "expected a busy recorded session");

        let mut want_of: BTreeMap<u8, Vec<PadState>> = BTreeMap::new();
        for (slot, state) in &truth {
            want_of.entry(*slot).or_default().push(*state);
        }
        assert_eq!(want_of.len(), 4, "all four panels must be represented");

        for capacity in [1usize, 2, 7, 64] {
            for stride in [Some(1usize), Some(3), Some(29), None] {
                let (tx, rx) = bounded::<OutMsg>(capacity);
                let mut deltas = DeltaTx::new(tx, crate::feed::LiveSink::default());
                let mut got: Vec<(u8, PadState)> = Vec::new();
                let drain = |rx: &Receiver<OutMsg>, got: &mut Vec<(u8, PadState)>| {
                    while let Ok(m) = rx.try_recv() {
                        got.push((m.slot, m.state));
                    }
                };

                for (i, (slot, state)) in truth.iter().enumerate() {
                    deltas
                        .send(OutMsg {
                            slot: *slot,
                            state: *state,
                            t_capture: i as u64,
                        })
                        .expect("the consumer is alive");
                    if stride.is_some_and(|s| i % s == 0) {
                        drain(&rx, &mut got);
                        deltas.flush().expect("the consumer is alive");
                    }
                }
                // The engine thread's idle path: retry the flush until nothing
                // is left pending.
                while deltas.has_pending() {
                    drain(&rx, &mut got);
                    deltas.flush().expect("the consumer is alive");
                }
                drain(&rx, &mut got);

                let label = format!("capacity={capacity} stride={stride:?}");
                if stride == Some(1) {
                    assert_eq!(
                        deltas.coalesced, 0,
                        "{label}: a consumer that keeps up must never coalesce"
                    );
                    assert_eq!(
                        got, truth,
                        "{label}: with no backpressure the stream must be byte-identical"
                    );
                }
                for (slot, want) in &want_of {
                    let have: Vec<PadState> = got
                        .iter()
                        .filter(|(s, _)| s == slot)
                        .map(|(_, st)| *st)
                        .collect();
                    assert!(
                        is_subsequence(&have, want),
                        "{label}: slot {slot} emitted a state the engine never produced, \
                         or delivered two of them out of order"
                    );
                    assert_eq!(
                        have.last(),
                        want.last(),
                        "{label}: slot {slot} settled on the wrong final state — coalescing \
                         may drop intermediate states, never the LAST one"
                    );
                }
            }
        }
    }

    #[test]
    fn delta_tx_reports_a_dead_output_thread_and_counts_the_loss() {
        let (tx, rx) = bounded::<OutMsg>(1);
        let mut deltas = DeltaTx::new(tx, crate::feed::LiveSink::default());
        let msg = |slot: u8| OutMsg {
            slot,
            state: PadState::default(),
            t_capture: 0,
        };
        deltas.send(msg(1)).expect("output alive");
        deltas.send(msg(2)).expect("output alive"); // pends: channel is full
        drop(rx);
        assert!(deltas.send(msg(3)).is_err(), "the output thread is gone");
        assert_eq!(deltas.dropped, 2, "both pending states are lost, counted");
        assert!(!deltas.has_pending());
    }

    #[test]
    fn capture_guard_releases_once_on_drop_and_on_explicit_release() {
        let (tx, rx) = unbounded();
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        {
            let guard = CaptureGuard::new(tx, Some(trace.clone()));
            guard.release();
            guard.release();
        } // Drop must not send a third time.
        let sent: Vec<CaptureCtl> = rx.try_iter().collect();
        assert_eq!(sent.len(), 1, "exactly one SetPassthrough");
        assert!(matches!(sent[0], CaptureCtl::SetPassthrough));
        assert_eq!(*trace.lock().unwrap(), vec![Step::BlockingDisabled]);
    }

    #[test]
    fn capture_guard_releases_on_unwind() {
        let (tx, rx) = unbounded();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = CaptureGuard::new(tx, None);
            panic!("supervisor exploded");
        }));
        assert!(result.is_err());
        let sent: Vec<CaptureCtl> = rx.try_iter().collect();
        assert!(
            matches!(sent.as_slice(), [CaptureCtl::SetPassthrough]),
            "an unwinding supervisor MUST still free the keyboards"
        );
    }

    // -- the hot-swap eligibility split (FIX 3) -----------------------------

    fn shape_plan(slots: Vec<ResolvedSlot>) -> RunPlan {
        let captureable: Vec<DeviceId> = {
            let mut ids: Vec<DeviceId> = slots
                .iter()
                .filter_map(|s| s.spec.keyboard.clone())
                .collect();
            ids.dedup();
            ids
        };
        RunPlan {
            source: super::super::plan::PlanSource::Config,
            config_path: std::path::PathBuf::from("test"),
            slots,
            block_keyboards: true,
            block_mice: false,
            captureable,
            winusb: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn shape_slot(number: u8, device: &str, preset: ksx_core::Preset) -> ResolvedSlot {
        ResolvedSlot {
            spec: ksx_core::SlotSpec::new(
                number,
                Some(DeviceId::from(device)),
                None,
                preset.name.clone(),
            )
            .expect("valid slot"),
            preset,
        }
    }

    const BOARD: &str = r"HID\VID_D209&PID_0430&REV_0056&MI_00";
    const OTHER: &str = r"HID\VID_046D&PID_C31C&REV_6402&MI_00";

    /// The whole point of the split: a preset EDIT is invisible to the shape,
    /// so it takes the hot path. Which preset a slot names is invisible too —
    /// it changes the binding table and nothing a driver can see.
    #[test]
    fn binding_only_changes_are_hot_swappable() {
        let base = shape_plan(vec![shape_slot(
            1,
            BOARD,
            ksx_core::Preset::builtin_empty(),
        )]);

        let mut edited_preset = ksx_core::Preset::builtin_empty();
        edited_preset.entries.push((
            ksx_core::Key::G,
            ksx_core::Binding::Button(ksx_core::XButton::A),
        ));
        let edited = shape_plan(vec![shape_slot(1, BOARD, edited_preset)]);
        assert_eq!(
            SessionShape::of(&base).bounce_reason(&SessionShape::of(&edited)),
            None,
            "editing a binding must never bounce the pads"
        );

        let mut renamed = ksx_core::Preset::builtin_default();
        renamed.name = "Another Preset".to_owned();
        let repointed = shape_plan(vec![shape_slot(1, BOARD, renamed)]);
        assert_eq!(
            SessionShape::of(&base).bounce_reason(&SessionShape::of(&repointed)),
            None,
            "pointing a slot at a different preset is still just a table swap"
        );
    }

    /// …and everything a driver can see bounces, each with a reason a user can
    /// read on the page ("this change restarts the pads").
    #[test]
    fn structural_changes_bounce_and_say_what_changed() {
        let empty = ksx_core::Preset::builtin_empty;
        let base = shape_plan(vec![
            shape_slot(1, BOARD, empty()),
            shape_slot(2, BOARD, empty()),
        ]);
        let shape = SessionShape::of(&base);

        let fewer = shape_plan(vec![shape_slot(1, BOARD, empty())]);
        assert!(shape
            .bounce_reason(&SessionShape::of(&fewer))
            .is_some_and(|r| r.contains("slot count")));

        let renumbered = shape_plan(vec![
            shape_slot(1, BOARD, empty()),
            shape_slot(3, BOARD, empty()),
        ]);
        assert!(shape
            .bounce_reason(&SessionShape::of(&renumbered))
            .is_some_and(|r| r.contains("became slot")));

        let mut personas = base.slots.clone();
        personas[1].spec.persona = Persona::PlayStation;
        let repersona = shape_plan(personas);
        let reason = shape
            .bounce_reason(&SessionShape::of(&repersona))
            .expect("a persona change replugs a different device");
        assert!(reason.contains("persona"), "{reason}");

        let moved = shape_plan(vec![
            shape_slot(1, BOARD, empty()),
            shape_slot(2, OTHER, empty()),
        ]);
        assert!(shape
            .bounce_reason(&SessionShape::of(&moved))
            .is_some_and(|r| r.contains("input device")));

        let mut passthrough = shape_plan(base.slots.clone());
        passthrough.block_keyboards = false;
        assert!(shape
            .bounce_reason(&SessionShape::of(&passthrough))
            .is_some_and(|r| r.contains("blocking")));

        let mut winusb = shape_plan(base.slots.clone());
        winusb.winusb = vec![DeviceId::from(BOARD)];
        assert!(shape
            .bounce_reason(&SessionShape::of(&winusb))
            .is_some_and(|r| r.contains("backend")));

        // A plan identical in everything a driver sees stays hot even when the
        // reporting fields differ.
        let mut noisy = shape_plan(base.slots.clone());
        noisy.notes = vec!["[WARN] something".to_owned()];
        noisy.config_path = std::path::PathBuf::from("elsewhere");
        assert_eq!(shape.bounce_reason(&SessionShape::of(&noisy)), None);
    }

    /// A handle whose engine has gone must report it rather than pretending the
    /// swap landed — the caller then bounces, which is always safe.
    #[test]
    fn a_swap_onto_a_dead_engine_is_an_error_not_a_silent_success() {
        let plan = shape_plan(vec![shape_slot(
            1,
            BOARD,
            ksx_core::Preset::builtin_empty(),
        )]);
        let (tx, rx) = unbounded::<EngineCtl>();
        let swap = HotSwap {
            ectl: tx,
            shape: SessionShape::of(&plan),
        };
        assert_eq!(swap.apply(&plan), Ok(SwapVerdict::Applied));
        drop(rx);
        assert!(swap.apply(&plan).is_err());
    }

    /// The slot is empty until a session publishes into it, and empty again
    /// afterwards — nothing may hand tables to a joined engine.
    #[test]
    fn the_hot_swap_slot_is_empty_before_and_after_a_session() {
        let plan = shape_plan(vec![shape_slot(
            1,
            BOARD,
            ksx_core::Preset::builtin_empty(),
        )]);
        let slot = HotSwapSlot::default();
        assert!(slot.handle().is_none());
        let (tx, _rx) = unbounded::<EngineCtl>();
        slot.publish(HotSwap {
            ectl: tx,
            shape: SessionShape::of(&plan),
        });
        assert!(slot.handle().is_some());
        slot.clear();
        assert!(slot.handle().is_none());
    }

    #[test]
    fn apply_capture_never_captures_an_empty_or_absent_set() {
        let (tx, rx) = unbounded();
        let present = BTreeSet::new();
        apply_capture(&tx, &None, true, &present);
        assert!(matches!(rx.try_recv().unwrap(), CaptureCtl::SetPassthrough));

        let present: BTreeSet<DeviceId> = [DeviceId::from("A")].into_iter().collect();
        apply_capture(&tx, &None, false, &present);
        assert!(matches!(rx.try_recv().unwrap(), CaptureCtl::SetPassthrough));
        apply_capture(&tx, &None, true, &present);
        match rx.try_recv().unwrap() {
            CaptureCtl::SetCaptured(ids) => assert_eq!(ids, vec![DeviceId::from("A")]),
            other => panic!("expected SetCaptured, got {other:?}"),
        }
    }
}
