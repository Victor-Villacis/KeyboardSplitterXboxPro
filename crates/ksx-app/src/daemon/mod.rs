//! `ksx daemon` — ksx that stays resident, with a tray icon.
//!
//! # Shape
//!
//! ```text
//! [tray thread]     its own window + message pump. Owns the Shell_NotifyIconW
//!                   icon and the popup menu. Sends DaemonCommands. Never
//!                   touches the pipeline, the config, or a driver.
//!        │ crossbeam channel
//!        ▼
//! [control thread]  this module. Owns the session lifecycle: start, stop,
//!                   reload, quit. Blocks freely.
//!        │ spawns
//!        ▼
//! [session thread]  one `supervise()` call = one emulation session, exactly
//!                   the M4 pipeline with nothing added.
//! ```
//!
//! The separation is the point. The tray is a Win32 message pump, and a message
//! pump is precisely the thing that killed the legacy app: it dispatched every
//! keystroke onto the WPF UI thread, so a stalled UI froze every keyboard on
//! the machine until reboot (legacy inventory §8.6). Here the tray thread has
//! no path to the capture thread at all — it can only enqueue a command. If it
//! hangs, the session keeps running and every emergency escape still works,
//! because escapes are evaluated inside the capture thread.
//!
//! # Headless
//!
//! `--headless` skips the tray and offers the identical control surface on
//! stdin (`start`, `stop`, `reload`, `config`, `status`, `quit`). Same control
//! loop, same commands, same state — the tray is a front end for it, not a
//! parallel implementation. That is what makes the tray droppable if it ever
//! misbehaves, and what makes the control loop testable in CI.
//!
//! A third front end, the [`pipe`] named-pipe server, gives OTHER processes
//! (`ksx session`, Studio) exactly the tray's reach and no more: enqueue a
//! [`DaemonCommand`], read the [`DaemonState`] snapshot. See that module for
//! the protocol and the trust model.
//!
//! # M6: the daemon owns the panel
//!
//! With the Interception backend the daemon is a convenience — between sessions
//! every keyboard is an ordinary keyboard whether ksx runs or not. With a
//! WinUSB-claimed panel that is no longer true: the claimed interface is not in
//! the keyboard stack, so **something has to put its keystrokes back**, and the
//! only thing that can is ksx. [`typethrough::TypethroughService`] does it, and
//! this control loop decides when: [`PanelKeyboard::set_emulating`] is called
//! `true` before a session starts and `false` after it is reaped. That ordering
//! is contractual and asserted in CI — see
//! [`tests::the_panel_stops_typing_before_a_session_starts_and_resumes_after_it_ends`].
//!
//! The claim itself is made **once**, by [`run`], before this loop starts, and
//! lives in a [`panel::Panel`] for the whole life of the process. Sessions
//! borrow it through `Panel::session_backend` (`crate::capture::build_session`);
//! `supervise()` starts and stops that borrowed view per session while the claim
//! underneath never moves. Interception-backed devices are unchanged: their
//! backend is still created and destroyed per session, because between sessions
//! the OS owns them and ksx has nothing to do.
//!
//! End to end, that path is asserted in [`panel_tests`]: two real sessions over
//! one mock-backed claim, with the panel typing in between.

pub mod learn;
pub mod live;
pub mod panel;
pub mod pipe;
#[cfg(windows)]
pub mod tray;
pub mod typethrough;

#[cfg(test)]
mod panel_tests;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

/// Everything the tray, stdin or the control pipe can ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonCommand {
    /// Start emulation if it is not already running. `game: Some(title)` makes
    /// this and every later session use that games.toml profile (the pipe's
    /// `start --game`); `None` keeps whatever the daemon is configured with —
    /// which is what the tray and stdin always send.
    Start { game: Option<String> },
    /// Stop the current session. The game, if any, keeps running.
    Stop,
    /// Stop, re-read the configuration, start again.
    Reload,
    /// Apply the configuration on disk to the RUNNING session the cheapest way
    /// that is correct: a binding-only edit is hot-swapped into the live engine
    /// (pads stay plugged, keyboards stay captured, Windows plays no
    /// disconnect chime); anything structural falls back to the same bounce
    /// [`DaemonCommand::Reload`] does. The verdict lands in
    /// [`DaemonState::apply`] so the pipe can report which happened.
    ///
    /// This is what a mapper save asks for. `Reload` stays the blunt
    /// "restart it, whatever changed" verb the tray offers.
    ApplyBindings,
    /// Open the config folder in Explorer.
    OpenConfigFolder,
    /// Open the 10-foot cabinet panel — a fourth thread in this process, with
    /// exactly this channel's reach and no more (`crate::cabinet`).
    ///
    /// A command rather than something the tray thread does for itself,
    /// because the tray's one structural property is that its ONLY outbound
    /// edge is a `Sender<DaemonCommand>` (see [`tray`]'s module docs). A tray
    /// that could spawn a window would be a tray that could do something the
    /// control loop cannot see.
    ///
    /// **Closing that window does not quit the daemon.** Quit stays here, on
    /// the tray.
    OpenCabinet,
    /// Open ksx Studio in a browser, starting one if nothing is listening.
    OpenStudio,
    /// Print the current state (headless mode's `status`).
    Status,
    /// Stop everything and exit the process.
    Quit,
}

impl DaemonCommand {
    /// Parse a headless-mode line. Deliberately forgiving about case and
    /// whitespace; deliberately unforgiving about anything else.
    pub fn parse(line: &str) -> Option<Self> {
        match line.trim().to_ascii_lowercase().as_str() {
            "start" | "s" => Some(Self::Start { game: None }),
            "stop" | "x" => Some(Self::Stop),
            "reload" | "r" => Some(Self::Reload),
            "config" | "c" => Some(Self::OpenConfigFolder),
            "cabinet" | "ui" => Some(Self::OpenCabinet),
            "studio" | "web" => Some(Self::OpenStudio),
            "status" | "?" => Some(Self::Status),
            "quit" | "q" | "exit" => Some(Self::Quit),
            _ => None,
        }
    }

    /// The one-line help shown at startup and on an unrecognised line.
    pub fn help() -> &'static str {
        "commands: start | stop | reload | cabinet | studio | config | status | quit"
    }
}

/// What the daemon is doing, as the tooltip and `status` report it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    Stopped,
    Starting,
    Running {
        slots: usize,
    },
    /// The last session ended badly and nothing is running now.
    Failed {
        message: String,
    },
    Quitting,
}

/// Health carried over from the last finished session, so a problem that ended
/// the session is still visible afterwards — a tray icon that forgets why it
/// stopped is a tray icon nobody trusts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastSession {
    pub stop_code: String,
    pub message: String,
    pub reboot_required: bool,
    pub watchdog_tripped: bool,
    pub dropped_events: u64,
    pub exit_code: i32,
}

impl LastSession {
    /// The one health line to show for a session that is **over**.
    fn note(&self) -> Option<String> {
        if self.reboot_required {
            Some(REBOOT_NOTE.to_owned())
        } else if self.watchdog_tripped {
            Some("[!] capture watchdog tripped last session".to_owned())
        } else if self.dropped_events > 0 {
            Some(format!(
                "[!] {} event(s) dropped last session",
                self.dropped_events
            ))
        } else {
            None
        }
    }
}

/// Slot exhaustion phrased identically whether it is happening now or happened
/// then: it is a fact about the machine either way, and it is the message a user
/// is most likely to be searching for.
const REBOOT_NOTE: &str = "[!] REBOOT REQUIRED (Interception slot exhaustion)";

/// How long [`apply_bindings`] waits for a just-started session to publish its
/// hot-swap handle before giving up and bouncing. Long enough to cover pad
/// plugging on a cold ViGEm bus, short enough that a runner which will never
/// publish does not park the control loop.
const SWAP_HANDLE_GRACE: Duration = Duration::from_secs(3);

/// Capture health of the session running **right now**.
///
/// [`LastSession`] is written by [`reap`], which by definition runs after the
/// session is over. That left the worst failures invisible for exactly as long
/// as they mattered: a REBOOT REQUIRED or a watchdog trip halfway through a
/// two-hour game showed up nowhere until the player quit. This is the same
/// three facts, sampled *while* the session runs.
///
/// Plain data, deliberately: [`DaemonState::tooltip`] stays a pure function of
/// state, and the tray keeps reading one `Mutex` for the length of a `clone()`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveHealth {
    pub reboot_required: bool,
    pub watchdog_tripped: bool,
    pub dropped_events: u64,
}

impl LiveHealth {
    /// The single worst thing to say about the running session, worst first.
    /// `None` means there is nothing wrong, which is the common case and must
    /// stay silent — a tooltip that always has a `[!]` in it trains people to
    /// ignore the `[!]`.
    fn note(&self) -> Option<String> {
        if self.reboot_required {
            Some(REBOOT_NOTE.to_owned())
        } else if self.watchdog_tripped {
            Some("[!] capture watchdog TRIPPED — passthrough forced".to_owned())
        } else if self.dropped_events > 0 {
            Some(format!("[!] {} event(s) dropped", self.dropped_events))
        } else {
            None
        }
    }
}

/// Where a running session publishes the health the control loop polls.
///
/// One `Mutex<Option<HealthView>>`, written **once** per session by the session
/// thread the moment its capture backend exists, and read by the control loop
/// on the idle tick it already wakes on. The capture thread is not involved and
/// gains nothing: it goes on setting the lock-free atomics inside the
/// [`ksx_capture::HealthHandle`] this view reads, exactly as it did before. The
/// tray is one further step removed — it never touches this at all, only the
/// `DaemonState` snapshot the control loop refreshes from it.
///
/// A [`ksx_capture::HealthView`] rather than the handle, because the daemon's
/// WinUSB claim keeps one handle alive across every session: only a view with a
/// baseline taken at session start can answer "is *this* session in trouble"
/// (see that type's docs).
#[derive(Clone, Debug, Default)]
pub struct HealthSlot(Arc<Mutex<Option<ksx_capture::HealthView>>>);

impl HealthSlot {
    /// Publish this session's view. Called by the session thread once its
    /// capture backend is up.
    pub fn publish(&self, view: ksx_capture::HealthView) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(view);
        }
    }

    /// Sample it. `None` until the session's backend exists — during which time
    /// there is genuinely nothing to report, not "nothing wrong".
    pub fn poll(&self) -> Option<LiveHealth> {
        let slot = self.0.lock().ok()?;
        let snapshot = slot.as_ref()?.snapshot();
        Some(LiveHealth {
            reboot_required: snapshot.reboot_required,
            watchdog_tripped: snapshot.watchdog_tripped,
            dropped_events: snapshot.dropped_events,
        })
    }
}

/// What the last [`DaemonCommand::ApplyBindings`] did.
///
/// The pipe cannot ask the control loop a question — it enqueues commands and
/// reads this snapshot, which is exactly the reach the tray has (module docs).
/// So the verdict travels back here, keyed by a generation the caller compares
/// against the one it saw before enqueuing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Bumped once per handled `ApplyBindings`. A caller that still sees its
    /// baseline generation is looking at somebody else's answer.
    pub generation: u64,
    pub ok: bool,
    /// The new bindings went into the live engine — pads untouched.
    pub hot: bool,
    /// The session was torn down and started again.
    pub restarted: bool,
    /// One human sentence, already saying which of the two happened.
    pub message: String,
}

/// The state the tray polls. Small, cloneable, no borrows of anything live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DaemonState {
    pub run: RunState,
    pub game: Option<String>,
    pub last: Option<LastSession>,
    /// Refreshed by the control loop while a session runs; cleared when it is
    /// reaped, at which point [`Self::last`] is the truth again.
    pub live: Option<LiveHealth>,
    /// The verdict of the last binding-apply. Never cleared: a caller
    /// identifies its own answer by generation, not by presence.
    pub apply: Option<ApplyReport>,
}

impl DaemonState {
    /// The tray tooltip. Windows truncates it at 128 UTF-16 units, so the most
    /// important thing goes first and the health note is appended only if there
    /// is something to say.
    pub fn tooltip(&self) -> String {
        let mut text = match &self.run {
            RunState::Stopped => "ksx — stopped".to_owned(),
            RunState::Starting => "ksx — starting…".to_owned(),
            RunState::Running { slots } => format!("ksx — running, {slots} pad(s)"),
            RunState::Failed { message } => format!("ksx — stopped: {message}"),
            RunState::Quitting => "ksx — quitting…".to_owned(),
        };
        if let Some(game) = &self.game {
            text.push_str(&format!("\ngame: {game}"));
        }
        if let Some(note) = self.health_note() {
            text.push('\n');
            text.push_str(&note);
        }
        truncate_utf16(&text, 127)
    }

    /// The one health line the tooltip carries.
    ///
    /// **The running session wins.** What is wrong now outranks what was wrong
    /// then, and a tooltip is 128 UTF-16 units — there is room for one line, so
    /// it has to be the actionable one. A healthy running session falls through
    /// to the last finished one rather than hiding it: `reboot_required` in
    /// particular describes the *machine* and stays true until Windows
    /// restarts, so forgetting it because a fresh session looks fine would be
    /// the same lie in the other direction.
    fn health_note(&self) -> Option<String> {
        self.live
            .as_ref()
            .and_then(LiveHealth::note)
            .or_else(|| self.last.as_ref().and_then(LastSession::note))
    }

    /// Menu item labels + whether each is enabled right now.
    ///
    /// **"Open cabinet UI" is first**, and that is the point of the M9 pass:
    /// the tray's primary action is now the surface you can drive from the
    /// machine, with Studio — the authoring surface, which needs a keyboard —
    /// second. Start/Stop/Reload keep their place below both, because the
    /// people who use those from the tray are already at a desk.
    pub fn menu(&self) -> Vec<(DaemonCommand, &'static str, bool)> {
        let running = matches!(self.run, RunState::Running { .. } | RunState::Starting);
        vec![
            (DaemonCommand::OpenCabinet, "Open cabinet UI", true),
            (DaemonCommand::OpenStudio, "Open Studio", true),
            (
                DaemonCommand::Start { game: None },
                "Start emulation",
                !running,
            ),
            (DaemonCommand::Stop, "Stop emulation", running),
            (DaemonCommand::Reload, "Reload config", true),
            (DaemonCommand::OpenConfigFolder, "Open config folder", true),
            (DaemonCommand::Quit, "Quit", true),
        ]
    }
}

/// A tooltip longer than `NOTIFYICONDATAW.szTip` is silently truncated by
/// Windows — sometimes mid-surrogate. Cut it ourselves, on a char boundary.
fn truncate_utf16(text: &str, max_units: usize) -> String {
    if text.encode_utf16().count() <= max_units {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        let width = c.len_utf16();
        if units + width > max_units.saturating_sub(1) {
            break;
        }
        out.push(c);
        units += width;
    }
    out.push('…');
    out
}

pub type SharedState = Arc<Mutex<DaemonState>>;

/// One emulation session's outcome, distilled for the daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSummary {
    pub stop_code: String,
    pub message: String,
    pub exit_code: i32,
    pub slots: usize,
    pub reboot_required: bool,
    pub watchdog_tripped: bool,
    pub dropped_events: u64,
}

/// Runs one session to completion, honouring `stop`.
pub trait SessionRunner: Send {
    fn run(&mut self, stop: Arc<AtomicBool>, out: &mut dyn Write)
        -> anyhow::Result<SessionSummary>;
    /// Pads the plan will ask for — reported while starting, before any driver
    /// call, so the tooltip is useful during the slow part.
    fn slots(&self) -> usize;

    /// Where this session will publish its capture health.
    ///
    /// Taken by [`start`] **before** the runner is moved onto its own thread,
    /// so the control loop holds the slot for the whole session and the runner
    /// fills it in as soon as its backend exists.
    ///
    /// Defaulted to a slot nothing ever fills: a runner with no capture backend
    /// has no health to report, and the tooltip should then say nothing rather
    /// than invent a clean bill of it.
    fn health_slot(&self) -> HealthSlot {
        HealthSlot::default()
    }

    /// Where this session will publish its binding hot-swap handle. Taken by
    /// [`start`] before the runner moves onto its own thread, exactly like
    /// [`Self::health_slot`].
    ///
    /// Defaulted to a slot nothing ever fills, which reads as "this runner
    /// cannot hot-swap" — and the control loop then bounces, which is always
    /// correct, only louder.
    fn hot_swap_slot(&self) -> crate::run::supervisor::HotSwapSlot {
        crate::run::supervisor::HotSwapSlot::default()
    }
}

/// Makes a fresh runner per session, re-reading configuration each time. That
/// is what "Reload config" means: not a hot-patch of a live pipeline, but a
/// clean stop and a clean start from whatever is on disk now.
pub trait SessionFactory: Send {
    fn make(&mut self) -> anyhow::Result<Box<dyn SessionRunner>>;
    fn config_dir(&self) -> std::path::PathBuf;
    fn game(&self) -> Option<String>;
    /// Repoint every FUTURE session at this games.toml profile (`None` = the
    /// plain `[[slot]]` config). Validation stays where it always was — in
    /// [`Self::make`]'s plan resolution — so a pipe `start --game X` fails
    /// exactly like a tray Start with a broken config, not via a second path.
    fn set_game(&mut self, game: Option<String>);

    /// Re-resolve the plan from disk WITHOUT building a runner — the input to
    /// the hot-swap eligibility check.
    ///
    /// Defaulted to a refusal so every existing factory (and every test one)
    /// keeps compiling and simply never hot-swaps: the control loop reads
    /// `Err` as "I cannot tell what changed" and bounces, which is the safe
    /// direction to be wrong in.
    fn resolve_plan(&self) -> anyhow::Result<crate::run::plan::RunPlan> {
        anyhow::bail!("this session factory cannot re-resolve a plan")
    }
}

/// The keystroke behaviour of a WinUSB-claimed panel between sessions.
///
/// Implemented for real by [`typethrough::TypethroughService`]; [`NoPanel`] is
/// the correct implementation for every backend whose devices are still
/// keyboards in their own right (Interception, RawInput), where the OS is
/// already doing this and ksx doing it too would double every keystroke.
pub trait PanelKeyboard: Send {
    /// `true` = emulation owns the panel, inject nothing. `false` = the panel
    /// is a keyboard again.
    ///
    /// Implementations must have applied the change by the time this returns
    /// (see [`typethrough::TypethroughService::set_emulating`]): a queued mode
    /// change races the keystroke stream.
    fn set_emulating(&mut self, emulating: bool);

    /// Clear the escape passthrough latch, leaving the counters alone.
    ///
    /// Called in the same breath as muting: the instant the panel goes dead is
    /// the instant a fresh `LeftCtrl x5` must be able to revive it. Arming any
    /// later — once the supervisor is up, after pads have finished plugging —
    /// silently erases a gesture made during those seconds, which is exactly
    /// when a user staring at a dead panel reaches for one.
    fn arm_escapes(&mut self) {}
}

/// Opening the two surfaces a tray click can ask for.
///
/// Behind a trait for the same reason [`PanelKeyboard`] is: the control loop
/// is tested with no window, no browser and no GPU anywhere near it, and a
/// loop that called `eframe::run_native` directly could not be. [`NoUi`] is
/// what every test and every headless daemon gets.
pub trait UiHost: Send + Sync {
    /// Open the 10-foot cabinet panel (a fourth thread in this process).
    /// Returns immediately; closing that window must never end the daemon.
    fn open_cabinet(&self, out: &mut dyn Write) {
        let _ = writeln!(
            out,
            "this build has no cabinet window (rebuild with `--features cabinet`)"
        );
    }

    /// Open ksx Studio in a browser.
    fn open_studio(&self, out: &mut dyn Write) {
        let _ = writeln!(
            out,
            "this build has no Studio (rebuild with `--features studio`)"
        );
    }
}

/// No surfaces at all — the honest host for a build with neither UI feature,
/// and for every test. Both defaults SAY they cannot act rather than doing
/// nothing, which is the same obligation every refusal on the control surface
/// carries.
///
/// A build with BOTH features never constructs one outside its tests, which is
/// exactly what "this build has every surface" means; it stays here because it
/// is the type the control loop is specified against, and because the shape of
/// this module must not depend on which features happen to be on.
#[cfg_attr(any(feature = "cabinet", feature = "studio"), allow(dead_code))]
pub struct NoUi;

impl UiHost for NoUi {}

/// The panel needs no help — its devices are still keyboards to Windows.
pub struct NoPanel;

impl PanelKeyboard for NoPanel {
    fn set_emulating(&mut self, _emulating: bool) {}
}

/// Choose the daemon's [`PanelKeyboard`].
///
/// Given the claim the daemon made at startup, the panel types between sessions
/// through its [`typethrough::TypethroughService`]. Given `None`, every bound
/// device is still a keyboard in its own right (the Interception backend, or a
/// machine with nothing claimed) and ksx injecting as well would double every
/// keystroke, so the panel is left alone.
///
/// This is the one seam where the WinUSB backend plugs into the daemon: the
/// **daemon** owns the claim for its whole lifetime and hands each session a
/// borrowed view of it, instead of the M4/M5 arrangement where `supervise()`
/// created and destroyed the capture backend per session. That inversion is
/// required, not stylistic — releasing a WinUSB claim between sessions would not
/// give the panel back to Windows, only stop anything reading it. See
/// `docs/ARCHITECTURE.md` §M6.
pub fn panel_for(panel: Option<Arc<panel::Panel>>) -> Box<dyn PanelKeyboard> {
    match panel {
        Some(panel) => Box::new(panel::PanelKeyboardHandle(panel)),
        None => Box::new(NoPanel),
    }
}

/// The control loop. Returns when [`DaemonCommand::Quit`] is handled or the
/// command channel closes.
///
/// Blocking here is free: nothing in the input path waits on this thread.
///
/// The two calls into `panel` are the whole M6 daemon contract:
///
/// - **before** a session is spawned, `set_emulating(true)` — otherwise the
///   first frames of a game are both translated into pad state *and* typed onto
///   the desktop behind it;
/// - **after** a session is reaped, `set_emulating(false)` — otherwise the
///   frontend the player just returned to has a dead panel, which is the exact
///   failure mode a WinUSB claim introduces.
///
/// Both orderings are asserted in CI against a recording panel.
pub fn control_loop_with(
    commands: Receiver<DaemonCommand>,
    state: SharedState,
    factory: &mut dyn SessionFactory,
    panel: &mut dyn PanelKeyboard,
    ui: &dyn UiHost,
    out: &mut dyn Write,
) {
    let mut session: Option<LiveSession> = None;
    let mut apply_generation: u64 = 0;
    set_game(&state, factory.game());

    loop {
        // Reap a session that ended on its own (the game exited, an escape, a
        // driver failure) so the tray stops claiming it is running.
        if let Some(live) = &session {
            if live.handle.as_ref().is_some_and(|h| h.is_finished()) {
                let finished = session.take().expect("checked");
                reap(finished, &state, out);
                panel.set_emulating(false);
            }
        }

        // Live capture health, sampled on the tick this loop already wakes on.
        // Nothing is added to the hot path: the capture thread publishes into
        // lock-free atomics exactly as before, and this reads them. Without it
        // a REBOOT REQUIRED or a watchdog trip *during* a two-hour game is
        // visible nowhere until the player quits — which is the moment it stops
        // being useful.
        if let Some(live) = &mut session {
            live.refresh_health(&state);
        }

        match commands.recv_timeout(Duration::from_millis(200)) {
            Ok(DaemonCommand::Start { game }) => {
                if session.is_some() {
                    let _ = writeln!(out, "already running");
                    continue;
                }
                // A per-start profile override must not outlive a start that
                // never started: a typo'd `--game` would otherwise repoint
                // every later tray Start at the broken title.
                let previous_game = factory.game();
                if let Some(game) = game {
                    factory.set_game(Some(game));
                    set_game(&state, factory.game());
                }
                // Mute the panel BEFORE the pipeline exists, not after — and
                // re-arm the escape latch in the same breath. Muting and
                // arming are one event: the moment the panel goes dead is the
                // moment a fresh LeftCtrl x5 must be able to bring it back.
                // Arming later (in the supervisor, after pads plug) would
                // silently erase a gesture made during those seconds.
                panel.arm_escapes();
                panel.set_emulating(true);
                session = start(factory, &state, out);
                if session.is_none() {
                    // Nothing started, so nothing owns the panel: give it back
                    // rather than leaving a dead panel behind a failed start.
                    panel.set_emulating(false);
                    factory.set_game(previous_game);
                    set_game(&state, factory.game());
                }
            }
            Ok(DaemonCommand::Stop) => match session.take() {
                Some(live) => {
                    let _ = writeln!(out, "stopping…");
                    live.stop.store(true, Ordering::SeqCst);
                    reap(live, &state, out);
                    panel.set_emulating(false);
                }
                None => {
                    let _ = writeln!(out, "not running");
                }
            },
            Ok(DaemonCommand::Reload) => {
                let _ = writeln!(out, "reloading configuration…");
                restart(&mut session, factory, &state, panel, out);
            }
            // The mapper's save path. Cheap when it can be, honest when it
            // cannot: the pads only bounce for changes that genuinely move a
            // driver, and the report says which happened either way.
            Ok(DaemonCommand::ApplyBindings) => {
                apply_generation += 1;
                let report =
                    apply_bindings(apply_generation, &mut session, factory, &state, panel, out);
                let _ = writeln!(out, "{}", report.message);
                if let Ok(mut s) = state.lock() {
                    s.apply = Some(report);
                }
            }
            // Both windows are opened by the HOST, on a thread of its own, and
            // the control loop does not wait for either. A window that cannot
            // be created logs and says so; it never stops emulation, and
            // closing one later never does either — Quit is the tray's alone.
            Ok(DaemonCommand::OpenCabinet) => ui.open_cabinet(out),
            Ok(DaemonCommand::OpenStudio) => ui.open_studio(out),
            Ok(DaemonCommand::OpenConfigFolder) => {
                let dir = factory.config_dir();
                let _ = writeln!(out, "opening {}", dir.display());
                if let Err(err) = ksx_platform::process::open_folder(&dir) {
                    let _ = writeln!(out, "[FAIL] could not open {}: {err}", dir.display());
                }
            }
            Ok(DaemonCommand::Status) => {
                let snapshot = state.lock().map(|s| s.clone()).unwrap_or_default();
                let _ = writeln!(out, "{}", snapshot.tooltip());
            }
            Ok(DaemonCommand::Quit) => {
                if let Some(live) = session.take() {
                    let _ = writeln!(out, "stopping before exit…");
                    live.stop.store(true, Ordering::SeqCst);
                    reap(live, &state, out);
                }
                // The daemon is going away and the claim goes with it. Handing
                // the panel back on the way out is what makes a `quit` while a
                // key is held not leave that key latched down on the desktop —
                // the typethrough's own Drop is the second line of defence.
                panel.set_emulating(false);
                set_run(&state, RunState::Quitting);
                let _ = writeln!(out, "bye");
                let _ = out.flush();
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The only sender is gone (the tray thread died, or stdin closed).
            // Treat it as Quit: a daemon nobody can talk to must not sit there
            // holding keyboards.
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(live) = session.take() {
                    live.stop.store(true, Ordering::SeqCst);
                    reap(live, &state, out);
                }
                panel.set_emulating(false);
                set_run(&state, RunState::Quitting);
                let _ = out.flush();
                return;
            }
        }
        let _ = out.flush();
    }
}

/// Stop whatever is running, re-read the configuration, start again — the
/// tray's "Reload config", and the fallback every structural change takes.
///
/// One function so `Reload` and the bounce half of [`apply_bindings`] can never
/// drift into two different teardown orders.
fn restart(
    session: &mut Option<LiveSession>,
    factory: &mut dyn SessionFactory,
    state: &SharedState,
    panel: &mut dyn PanelKeyboard,
    out: &mut dyn Write,
) {
    if let Some(live) = session.take() {
        live.stop.store(true, Ordering::SeqCst);
        reap(live, state, out);
    }
    set_game(state, factory.game());
    panel.set_emulating(true);
    *session = start(factory, state, out);
    if session.is_none() {
        panel.set_emulating(false);
    }
}

/// The hot-swap decision, in one place.
///
/// The rules, stated once so the UI can quote them:
///
/// - **nothing running** → nothing to do; the next start reads the files.
/// - **binding-only change** (preset contents, or a slot pointing at a
///   different preset) → hand the rebuilt tables to the live engine. Pads stay
///   plugged, keyboards stay captured, Steam does not re-enumerate, and a game
///   in progress notices nothing except the new binding. Controls held across
///   the swap are released by the engine, so a rebind cannot strand a pressed
///   virtual button.
/// - **structural change** (slot count, slot numbering, persona, keyboard or
///   mouse assignment, blocking policy, capture backend) → bounce, and say so.
/// - **cannot tell** (the config no longer resolves, or the session has no
///   swap handle because it is still starting) → do NOT bounce on a config we
///   could not read: tearing a working session down to fail the restart is the
///   worst of both. Report and leave it running.
fn apply_bindings(
    generation: u64,
    session: &mut Option<LiveSession>,
    factory: &mut dyn SessionFactory,
    state: &SharedState,
    panel: &mut dyn PanelKeyboard,
    out: &mut dyn Write,
) -> ApplyReport {
    let report = |ok: bool, hot: bool, restarted: bool, message: String| ApplyReport {
        generation,
        ok,
        hot,
        restarted,
        message,
    };
    if session.is_none() {
        return report(
            true,
            false,
            false,
            "no session is running — the next start reads the new bindings".to_owned(),
        );
    }

    let plan = match factory.resolve_plan() {
        Ok(plan) => plan,
        Err(err) => {
            return report(
                false,
                false,
                false,
                format!(
                    "the session is still running on its old bindings: the configuration \
                     on disk does not resolve ({err})"
                ),
            );
        }
    };

    // A session becomes "running" the moment its thread is spawned, but its
    // engine — and therefore its swap door — appears a little later, after the
    // pads have plugged. Mapping in that window is exactly what a user does
    // right after pressing Start, so wait it out instead of punishing them
    // with a bounce. Blocking here is free (module docs); the wait is bounded,
    // and a runner that never publishes simply falls through to the restart.
    let handle = {
        let deadline = Instant::now() + SWAP_HANDLE_GRACE;
        loop {
            match session.as_ref().and_then(|live| live.swap.handle()) {
                Some(handle) => break Some(handle),
                None if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                None => break None,
            }
        }
    };
    let bounce = |reason: String,
                  session: &mut Option<LiveSession>,
                  factory: &mut dyn SessionFactory,
                  panel: &mut dyn PanelKeyboard,
                  out: &mut dyn Write| {
        restart(session, factory, state, panel, out);
        let started = session.is_some();
        report(
            started,
            false,
            started,
            if started {
                format!("session restarted — {reason} needs the pads replugged")
            } else {
                format!("session stopped — {reason} needs a restart, which then failed")
            },
        )
    };

    match handle {
        Some(handle) => match handle.apply(&plan) {
            Ok(crate::run::supervisor::SwapVerdict::Applied) => report(
                true,
                true,
                false,
                "bindings applied live — pads untouched".to_owned(),
            ),
            Ok(crate::run::supervisor::SwapVerdict::NeedsRestart(reason)) => {
                bounce(reason, session, factory, panel, out)
            }
            Err(err) => bounce(err.to_owned(), session, factory, panel, out),
        },
        // Starting: the engine does not exist yet, so there is nothing to swap
        // into. A bounce is the only correct answer and it is cheap here — the
        // pads are not up.
        None => bounce(
            "the session was still starting".to_owned(),
            session,
            factory,
            panel,
            out,
        ),
    }
}

struct LiveSession {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<anyhow::Result<SessionSummary>>>,
    started: Instant,
    /// Filled in by the session thread once its capture backend exists.
    health: HealthSlot,
    /// Filled in by the session thread once its engine thread exists — the
    /// door a binding edit walks through without waking the drivers.
    swap: crate::run::supervisor::HotSwapSlot,
    /// The last sample pushed into [`DaemonState::live`], so an unchanged
    /// reading costs no lock and a *newly* appeared problem is logged once
    /// rather than four times a second.
    reported: Option<LiveHealth>,
}

impl LiveSession {
    /// Sample the session's health into the shared state.
    fn refresh_health(&mut self, state: &SharedState) {
        let Some(fresh) = self.health.poll() else {
            return;
        };
        if self.reported == Some(fresh) {
            return;
        }
        // A problem that has just appeared also goes to the log — the tooltip
        // is 128 characters that somebody has to be looking at, and this is
        // precisely the event a cabinet's log file exists to have caught.
        let note = fresh.note();
        if let Some(note) = &note {
            if self.reported.and_then(|h| h.note()).as_ref() != Some(note) {
                tracing::warn!("capture health: {note}");
            }
        }
        self.reported = Some(fresh);
        if let Ok(mut s) = state.lock() {
            s.live = Some(fresh);
        }
    }
}

/// Last-resort teardown: if the control thread unwinds, this local is dropped
/// during the unwind and the session is stopped and joined here. Without it a
/// panicking control thread leaves the session thread alive with the keyboards
/// still captured and nobody left to send `Stop` — the one path where the
/// daemon would be weaker than plain `ksx run`. The escapes would still free
/// the keyboards (they live in the capture thread), but nothing should depend
/// on the user knowing that.
impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start(
    factory: &mut dyn SessionFactory,
    state: &SharedState,
    out: &mut dyn Write,
) -> Option<LiveSession> {
    set_run(state, RunState::Starting);
    let mut runner = match factory.make() {
        Ok(runner) => runner,
        Err(err) => {
            let message = err.to_string();
            let _ = writeln!(out, "[FAIL] cannot start: {message}");
            set_run(state, RunState::Failed { message });
            return None;
        }
    };
    let slots = runner.slots();
    // Grabbed before the runner moves onto its own thread; the runner publishes
    // into it as soon as its capture backend is up.
    let health = runner.health_slot();
    let swap = runner.hot_swap_slot();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = std::thread::Builder::new()
        .name("ksx-session".into())
        .spawn({
            let stop = stop.clone();
            move || {
                // The session's own output goes to stderr: stdout in headless
                // mode is the command channel's echo, and interleaving the two
                // makes both unreadable.
                let mut err = std::io::stderr();
                runner.run(stop, &mut err)
            }
        })
        .ok()?;
    set_run(state, RunState::Running { slots });
    let _ = writeln!(out, "started ({slots} slot(s))");
    Some(LiveSession {
        stop,
        handle: Some(handle),
        started: Instant::now(),
        health,
        swap,
        reported: None,
    })
}

fn reap(mut live: LiveSession, state: &SharedState, out: &mut dyn Write) {
    let elapsed = live.started.elapsed();
    // Take the handle so the Drop guard below has nothing left to join: reap()
    // is the ordinary path and reports the outcome; Drop is only the unwind net.
    let handle = live.handle.take().expect("reaped once");
    match handle.join() {
        Ok(Ok(summary)) => {
            let _ = writeln!(
                out,
                "session ended after {:.0}s: {} ({})",
                elapsed.as_secs_f64(),
                summary.message,
                summary.stop_code
            );
            let failed = summary.exit_code != 0;
            let message = summary.message.clone();
            if let Ok(mut s) = state.lock() {
                s.last = Some(LastSession {
                    stop_code: summary.stop_code,
                    message: summary.message,
                    reboot_required: summary.reboot_required,
                    watchdog_tripped: summary.watchdog_tripped,
                    dropped_events: summary.dropped_events,
                    exit_code: summary.exit_code,
                });
                s.run = if failed {
                    RunState::Failed { message }
                } else {
                    RunState::Stopped
                };
            }
        }
        Ok(Err(err)) => {
            let message = err.to_string();
            let _ = writeln!(out, "[FAIL] session error: {message}");
            set_run(state, RunState::Failed { message });
        }
        Err(_) => {
            // A panicked session thread still freed the keyboards: the capture
            // backend's drop guard resets the filters with no cleanup needed.
            let message = "the session thread panicked (keyboards were released)".to_owned();
            let _ = writeln!(out, "[FAIL] {message}");
            set_run(state, RunState::Failed { message });
        }
    }
    // Nothing is running any more, so there is no live health: the verdict this
    // reap just recorded in `last` is the truth from here on. Leaving a stale
    // live reading would pin the tooltip to a session that no longer exists.
    if let Ok(mut s) = state.lock() {
        s.live = None;
    }
}

fn set_run(state: &SharedState, run: RunState) {
    if let Ok(mut s) = state.lock() {
        s.run = run;
    }
}

fn set_game(state: &SharedState, game: Option<String>) {
    if let Ok(mut s) = state.lock() {
        s.game = game;
    }
}

/// CLI entry point for `ksx daemon`.
///
/// The tray runs on **this** thread (a Win32 message pump must own a thread
/// with a window on it) and the control loop moves to a worker; headless is the
/// other way round. Either way the control loop is the same code with the same
/// commands.
///
/// # The console
///
/// In tray mode this function gives up the console window it inherited (see
/// [`crate::console`]) — but only *after* the icon is on screen, and only after
/// every refusal path above has had somewhere to print to. Get that order wrong
/// and a tray that cannot be created leaves a daemon with no icon, no console
/// and no stdin: reachable only by `taskkill`.
pub fn run(
    game: Option<String>,
    no_launch: bool,
    headless: bool,
    console: bool,
    autostart: bool,
) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    // Fail fast on a broken configuration rather than showing a tray icon that
    // can only ever report errors.
    // `resolve_as`, not `resolve`: a "nothing to run" refusal here must suggest
    // `ksx daemon --game "…"`. Suggesting `ksx run` would hand a daemon user a
    // foreground session and let them conclude the daemon cannot do profiles.
    let plan = match crate::run::plan::resolve_as(&root, game.as_deref(), "ksx daemon") {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!("refusing to start the daemon:\n{err}");
            std::process::exit(crate::run::EXIT_CANNOT_START);
        }
    };

    // M6: the claim is made HERE, once, and released when this function
    // returns (or when the process dies, which needs no cleanup — the binding
    // outlives us either way). Everything after this point borrows it. A claim
    // failure is a refusal to start: a daemon that could not take the panel it
    // was configured for would silently be a daemon with a dead panel.
    #[cfg(windows)]
    let claimed = match crate::capture::claim_panel(&plan) {
        Ok(panel) => panel,
        Err(err) => {
            eprintln!("refusing to start the daemon: {err}");
            std::process::exit(crate::run::EXIT_CANNOT_START);
        }
    };
    #[cfg(not(windows))]
    let claimed: Option<Arc<panel::Panel>> = {
        let _ = &plan;
        None
    };

    // The daemon-lifetime live fan-out. Created here, before any session, and
    // outliving all of them: a surface subscribed to it watches sessions come
    // and go rather than losing its subscription with each one. With nobody
    // subscribed — the normal state of a cabinet — every publish is one
    // relaxed atomic load (`crate::feed`).
    let feed = crate::feed::LiveSink::new();

    let mut factory = live::LiveFactory {
        root,
        game,
        no_launch,
        panel: claimed.clone(),
        feed: feed.clone(),
    };

    let (tx, rx) = crossbeam_channel::unbounded::<DaemonCommand>();
    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
    if autostart {
        let _ = tx.send(DaemonCommand::Start { game: None });
    }

    // The external control channel (M10a): a named-pipe thread that can only
    // do what the tray can — enqueue a DaemonCommand, read the DaemonState
    // snapshot, and read games.toml from disk. It has no path to the factory,
    // the panel or any pipeline thread. Failure to create the pipe (a second
    // daemon already owns the name) is logged, not fatal: the tray and stdin
    // surfaces are untouched.
    #[cfg(windows)]
    {
        let profiles_root = factory.root.clone();
        let map_root = factory.root.clone();
        let (map_writer, macro_writer) = pipe::preset_writers(map_root.clone());
        pipe::server::spawn(
            pipe::PIPE_NAME.to_owned(),
            pipe::PipeDeps {
                tx: tx.clone(),
                state: state.clone(),
                profiles: Box::new(move || pipe::profile_rows(&profiles_root)),
                // The mapper verbs (M7 slice): `map` writes presets through
                // the same crate::mapping::apply the CLI verb uses (with the
                // once-per-lifetime session backup); `map-restore` pulls the
                // defaults / session-backup safety nets; learn-key observes
                // idle keyboards over Raw Input.
                map: map_writer,
                // `map-macro` writes a whole [macros.<name>] table through the
                // same crate::mapping writer the `ksx macro` CLI uses, behind
                // the session backup it shares with `map`.
                save_macro: macro_writer,
                restore: pipe::restore_fn(map_root.clone()),
                clear_all: pipe::clear_all_fn(map_root.clone()),
                backups: pipe::backups_fn(map_root.clone()),
                // `slot-assign`: which preset a slot uses. The one write here
                // that is not a preset edit, and the one whose `reload` is a
                // BOUNCE rather than a hot swap.
                slot_assign: pipe::slot_assign_fn(map_root),
                learn: learn::LearnService::with_rawinput(),
            },
        );
    }

    // The two surfaces a tray click can open. Built HERE because this is where
    // the channel, the snapshot, the config root and the fan-out all exist at
    // once — and handed to the control loop as a trait object, so the loop
    // itself still knows nothing about windows.
    let ui = ui_host(
        tx.clone(),
        state.clone(),
        factory.root.clone(),
        feed.clone(),
    );

    // M6 seam. Taken FROM THE FACTORY on purpose: the panel the control loop
    // mutes and the panel the sessions borrow have to be the same object, and
    // for the whole of M6 they were not — the loop got `panel_for(None)` while
    // each session claimed its own. Asking the factory makes that class of bug
    // unspellable. `None` inside means no claim, which is every configuration
    // whose devices are still keyboards to Windows.
    let mut panel = factory.panel_keyboard();

    #[cfg(windows)]
    if !headless {
        // The icon FIRST, before the control loop and before the console is
        // released: `tray::create` is the only thing that can tell us whether
        // this machine can show a tray at all, and the answer decides whether
        // the console is still the user's last way in.
        let tray = tray::create(tx.clone(), state.clone());
        let mode = crate::console::mode(headless, console);
        if tray.is_some() && mode.detaches() {
            // The notice names the log file, which is the whole answer to
            // "where did my daemon go" — and says so *here*, on the last
            // console output this process will ever produce.
            println!(
                "{}",
                crate::console::detach_notice(crate::logging::active_path())
            );
            crate::console::detach();
        }

        // Control loop on a worker; the tray (or the stdin fallback) owns this
        // thread. Spawned after the detach so its first writes already go to
        // the right place.
        let worker = {
            let state = state.clone();
            std::thread::Builder::new()
                .name("ksx-daemon".into())
                .spawn(move || {
                    let mut out = std::io::stdout();
                    control_loop_with(
                        rx,
                        state,
                        &mut factory,
                        panel.as_mut(),
                        ui.as_ref(),
                        &mut out,
                    );
                })?
        };
        if let Some(tray) = tray {
            tray.pump();
            // The tray exited (Quit, or the icon was destroyed): make sure the
            // control loop hears about it even if the click never arrived.
            let _ = tx.send(DaemonCommand::Quit);
            drop(tx);
            let _ = worker.join();
            release_claim(claimed.as_ref());
            return Ok(());
        }
        // The tray could not be created (Session 0, a locked-down desktop, no
        // shell). Fall through to headless rather than leaving a daemon nobody
        // can talk to — the control surface is identical, so nothing is lost
        // but the icon. The console was deliberately NOT released above: it is
        // now the only way in.
        eprintln!("[WARN] the tray icon could not be created; running headless.");
        eprintln!("{}", DaemonCommand::help());
        std::thread::spawn({
            let tx = tx.clone();
            move || stdin_commands(tx)
        });
        drop(tx);
        let _ = worker.join();
        release_claim(claimed.as_ref());
        return Ok(());
    }

    // Headless keeps its console unconditionally — stdin is the control surface.
    debug_assert!(!crate::console::mode(headless, console).detaches());
    let _ = (headless, console);
    println!("ksx daemon (headless). {}", DaemonCommand::help());
    std::thread::spawn({
        let tx = tx.clone();
        move || stdin_commands(tx)
    });
    drop(tx);
    let mut out = std::io::stdout();
    control_loop_with(
        rx,
        state,
        &mut factory,
        panel.as_mut(),
        ui.as_ref(),
        &mut out,
    );
    release_claim(claimed.as_ref());
    Ok(())
}

/// The [`UiHost`] this build can offer.
///
/// A build with neither UI feature gets [`NoUi`], whose defaults SAY they
/// cannot act and name the feature that would — which is the same obligation
/// every refusal on this control surface carries, applied to a tray menu item
/// that would otherwise be a silent no-op.
fn ui_host(
    tx: Sender<DaemonCommand>,
    state: SharedState,
    root: ksx_config::ConfigRoot,
    feed: crate::feed::LiveSink,
) -> Box<dyn UiHost> {
    let _ = (&tx, &state, &root, &feed);
    #[cfg(any(feature = "cabinet", feature = "studio"))]
    let host: Box<dyn UiHost> = Box::new(Surfaces {
        #[cfg(feature = "cabinet")]
        tx,
        #[cfg(feature = "cabinet")]
        state,
        #[cfg(feature = "cabinet")]
        root,
        #[cfg(feature = "cabinet")]
        feed,
    });
    #[cfg(not(any(feature = "cabinet", feature = "studio")))]
    let host: Box<dyn UiHost> = Box::new(NoUi);
    host
}

/// The real host: everything the windows this build HAS need, in one place.
///
/// The fields belong to the cabinet — the in-process window is the only
/// surface that needs the channel, the snapshot and the fan-out, because it is
/// the only one hosted here. Studio is a child process reached by URL and
/// needs none of them, which is why they are gated rather than carried
/// everywhere.
#[cfg(any(feature = "cabinet", feature = "studio"))]
struct Surfaces {
    #[cfg(feature = "cabinet")]
    tx: Sender<DaemonCommand>,
    #[cfg(feature = "cabinet")]
    state: SharedState,
    #[cfg(feature = "cabinet")]
    root: ksx_config::ConfigRoot,
    #[cfg(feature = "cabinet")]
    feed: crate::feed::LiveSink,
}

#[cfg(any(feature = "cabinet", feature = "studio"))]
impl UiHost for Surfaces {
    #[cfg(feature = "cabinet")]
    fn open_cabinet(&self, out: &mut dyn Write) {
        let _ = writeln!(out, "opening the cabinet panel…");
        crate::cabinet::spawn_in_daemon(
            self.tx.clone(),
            self.state.clone(),
            self.root.clone(),
            self.feed.clone(),
        );
    }

    #[cfg(feature = "studio")]
    fn open_studio(&self, out: &mut dyn Write) {
        crate::studio_launch::open(out);
    }
}

/// Release the daemon's claim on the way out.
///
/// [`panel::Panel`]'s `Drop` does this anyway — that is what covers a panicking
/// control thread — but the ordinary exit says so explicitly rather than relying
/// on the order in which three `Arc`s happen to drop.
fn release_claim(panel: Option<&Arc<panel::Panel>>) {
    if let Some(panel) = panel {
        panel.shutdown();
    }
}

/// Read commands from stdin and forward them. Runs on its own thread so the
/// control loop is never blocked on a console read.
pub fn stdin_commands(tx: Sender<DaemonCommand>) {
    use std::io::BufRead as _;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match DaemonCommand::parse(&line) {
            Some(command) => {
                let quitting = command == DaemonCommand::Quit;
                if tx.send(command).is_err() || quitting {
                    return;
                }
            }
            None => {
                eprintln!(
                    "unknown command '{}'. {}",
                    line.trim(),
                    DaemonCommand::help()
                );
            }
        }
    }
    // stdin closed (a service, a redirected pipe): ask the daemon to shut down
    // rather than leaving it running with nobody able to stop it.
    let _ = tx.send(DaemonCommand::Quit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    /// Shared ordered log, so panel transitions and session lifecycle events
    /// can be interleaved and asserted against each other.
    type Trace = Arc<Mutex<Vec<&'static str>>>;

    fn note(trace: &Trace, what: &'static str) {
        if let Ok(mut log) = trace.lock() {
            log.push(what);
        }
    }

    /// A [`PanelKeyboard`] that records the transitions it is told to make.
    struct RecordingPanel {
        trace: Trace,
    }

    impl PanelKeyboard for RecordingPanel {
        fn set_emulating(&mut self, emulating: bool) {
            note(
                &self.trace,
                if emulating {
                    "panel:muted"
                } else {
                    "panel:typing"
                },
            );
        }
    }

    /// A runner that blocks until told to stop, and reports whatever we script.
    struct FakeRunner {
        summary: SessionSummary,
        slots: usize,
        /// Set when the session actually ran.
        ran: Arc<AtomicBool>,
        /// End on its own after this long, ignoring `stop`.
        self_ends_after: Option<Duration>,
        trace: Option<Trace>,
        /// Stands in for the capture backend's health, so a test can trip a
        /// flag from outside while the session runs.
        health: Option<ksx_capture::HealthHandle>,
        slot: HealthSlot,
        /// The binding hot-swap door. `shape` is what the running session
        /// claims to be; `swaps` counts what came through it.
        swap: crate::run::supervisor::HotSwapSlot,
        shape: Option<crate::run::supervisor::SessionShape>,
        swaps: Option<Arc<std::sync::atomic::AtomicUsize>>,
    }

    impl SessionRunner for FakeRunner {
        fn health_slot(&self) -> HealthSlot {
            self.slot.clone()
        }

        fn run(
            &mut self,
            stop: Arc<AtomicBool>,
            _out: &mut dyn Write,
        ) -> anyhow::Result<SessionSummary> {
            if let Some(trace) = &self.trace {
                note(trace, "session:started");
            }
            // Same moment the real runner publishes: once the backend exists.
            if let Some(health) = &self.health {
                self.slot
                    .publish(ksx_capture::HealthView::new(health.clone()));
            }
            self.ran.store(true, Ordering::SeqCst);
            // Stand in for `supervise`: the engine exists, so a binding edit
            // can reach it. Real sessions publish the same thing here.
            if let Some(shape) = self.shape.clone() {
                let rx = self.swap.publish_test_handle(shape);
                if let Some(seen) = &self.swaps {
                    let seen = seen.clone();
                    std::thread::spawn(move || {
                        while rx.recv().is_ok() {
                            seen.fetch_add(1, Ordering::SeqCst);
                        }
                    });
                }
            }
            let deadline = self.self_ends_after.map(|d| Instant::now() + d);
            loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if let Some(trace) = &self.trace {
                note(trace, "session:ended");
            }
            Ok(self.summary.clone())
        }

        fn slots(&self) -> usize {
            self.slots
        }

        fn hot_swap_slot(&self) -> crate::run::supervisor::HotSwapSlot {
            self.swap.clone()
        }
    }

    struct FakeFactory {
        summary: SessionSummary,
        slots: usize,
        ran: Arc<AtomicBool>,
        self_ends_after: Option<Duration>,
        fail_with: Option<String>,
        makes: Arc<Mutex<u32>>,
        trace: Option<Trace>,
        health: Option<ksx_capture::HealthHandle>,
        game: Option<String>,
        /// What `resolve_plan()` answers. `None` = "I cannot tell", which is
        /// the default and makes every factory in these tests bounce.
        plan: Option<crate::run::plan::RunPlan>,
        /// The shape the RUNNING session publishes. Usually `SessionShape::of`
        /// the same plan, so a binding edit is hot; give it a different one to
        /// script a structural change.
        shape: Option<crate::run::supervisor::SessionShape>,
        swap: crate::run::supervisor::HotSwapSlot,
        swaps: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Default for FakeFactory {
        fn default() -> Self {
            Self {
                summary: SessionSummary {
                    stop_code: "ctrl-c".into(),
                    message: "stopped by Ctrl+C".into(),
                    ..SessionSummary::default()
                },
                slots: 4,
                ran: Arc::new(AtomicBool::new(false)),
                self_ends_after: None,
                fail_with: None,
                makes: Arc::new(Mutex::new(0)),
                trace: None,
                health: None,
                game: Some("Street Fighter".into()),
                plan: None,
                shape: None,
                swap: crate::run::supervisor::HotSwapSlot::default(),
                swaps: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    impl SessionFactory for FakeFactory {
        fn make(&mut self) -> anyhow::Result<Box<dyn SessionRunner>> {
            *self.makes.lock().unwrap() += 1;
            if let Some(message) = &self.fail_with {
                anyhow::bail!("{message}");
            }
            Ok(Box::new(FakeRunner {
                summary: self.summary.clone(),
                slots: self.slots,
                ran: self.ran.clone(),
                self_ends_after: self.self_ends_after,
                trace: self.trace.clone(),
                health: self.health.clone(),
                slot: HealthSlot::default(),
                swap: self.swap.clone(),
                shape: self.shape.clone(),
                swaps: Some(self.swaps.clone()),
            }))
        }

        fn config_dir(&self) -> std::path::PathBuf {
            std::path::PathBuf::from(r"C:\cfg\ksx")
        }

        fn game(&self) -> Option<String> {
            self.game.clone()
        }

        fn set_game(&mut self, game: Option<String>) {
            self.game = game;
        }

        fn resolve_plan(&self) -> anyhow::Result<crate::run::plan::RunPlan> {
            self.plan
                .clone()
                .ok_or_else(|| anyhow::anyhow!("this fake factory has no plan"))
        }
    }

    fn drive(factory: &mut FakeFactory, script: &[DaemonCommand]) -> (DaemonState, String) {
        let (tx, rx) = unbounded();
        for command in script {
            tx.send(command.clone()).unwrap();
        }
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(rx, state.clone(), factory, &mut NoPanel, &NoUi, &mut out);
        let final_state = state.lock().unwrap().clone();
        (final_state, String::from_utf8(out).unwrap())
    }

    #[test]
    fn start_then_stop_runs_exactly_one_session() {
        let mut factory = FakeFactory::default();
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start { game: None },
                DaemonCommand::Stop,
                DaemonCommand::Quit,
            ],
        );
        assert!(factory.ran.load(Ordering::SeqCst), "{text}");
        assert_eq!(*factory.makes.lock().unwrap(), 1);
        assert_eq!(state.run, RunState::Quitting);
        assert!(text.contains("started (4 slot(s))"), "{text}");
        assert!(text.contains("session ended"), "{text}");
        assert_eq!(
            state.last.as_ref().map(|l| l.stop_code.as_str()),
            Some("ctrl-c")
        );
    }

    /// Double-start must not plug a second set of pads: 8 virtual pads into 4
    /// XInput slots is the failure the playbook calls out by name.
    #[test]
    fn starting_twice_does_not_start_a_second_session() {
        let mut factory = FakeFactory::default();
        let (_, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start { game: None },
                DaemonCommand::Start { game: None },
                DaemonCommand::Quit,
            ],
        );
        assert_eq!(*factory.makes.lock().unwrap(), 1, "{text}");
        assert!(text.contains("already running"), "{text}");
    }

    /// Reload is a clean stop and a clean start — the configuration is re-read,
    /// never patched into a live pipeline.
    #[test]
    fn reload_stops_and_starts_a_fresh_session() {
        let mut factory = FakeFactory::default();
        let (_, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start { game: None },
                DaemonCommand::Reload,
                DaemonCommand::Quit,
            ],
        );
        assert_eq!(
            *factory.makes.lock().unwrap(),
            2,
            "reload must build a new session from disk: {text}"
        );
        assert!(text.contains("reloading configuration"), "{text}");
    }

    // -- FIX 3: ApplyBindings, the mapper's save path ----------------------

    /// A one-slot plan, so the tests can build shapes that match or don't.
    fn tiny_plan(slot: u8, persona: ksx_core::Persona) -> crate::run::plan::RunPlan {
        let preset = ksx_core::Preset::builtin_empty();
        crate::run::plan::RunPlan {
            source: crate::run::plan::PlanSource::Config,
            config_path: std::path::PathBuf::from("test"),
            slots: vec![ksx_core::ResolvedSlot {
                spec: ksx_core::SlotSpec::new(
                    slot,
                    Some(ksx_core::DeviceId::from("BOARD")),
                    None,
                    preset.name.clone(),
                )
                .expect("valid slot")
                .with_persona(persona),
                preset,
            }],
            block_keyboards: true,
            block_mice: false,
            captureable: vec![ksx_core::DeviceId::from("BOARD")],
            winusb: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The headline: a binding-only edit reaches the live session without the
    /// pads being rebuilt. `makes` staying at 1 IS the assertion — a second
    /// `make()` would mean a teardown and a fresh pipeline, which is exactly
    /// the disconnect/reconnect Victor was asking about.
    #[test]
    fn apply_bindings_hot_swaps_without_restarting_the_session() {
        let plan = tiny_plan(1, ksx_core::Persona::Xbox360);
        let mut factory = FakeFactory {
            shape: Some(crate::run::supervisor::SessionShape::of(&plan)),
            plan: Some(plan),
            ..FakeFactory::default()
        };
        let swaps = factory.swaps.clone();
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start { game: None },
                DaemonCommand::ApplyBindings,
                DaemonCommand::Quit,
            ],
        );
        assert_eq!(
            *factory.makes.lock().unwrap(),
            1,
            "the session must NOT have been rebuilt: {text}"
        );
        let report = state.apply.expect("a verdict was recorded");
        assert!(report.ok && report.hot && !report.restarted, "{report:?}");
        assert_eq!(report.generation, 1);
        assert_eq!(report.message, "bindings applied live — pads untouched");
        assert!(text.contains("pads untouched"), "{text}");
        // The tables really went down the channel to the engine.
        assert_eq!(swaps.load(Ordering::SeqCst), 1);
    }

    /// A MACRO BODY is a binding change, so it takes the in-place swap and
    /// never bounces the pads. Stated against `SessionShape` itself, because
    /// that is the thing that decides: a preset that grew (or lost) a timed
    /// sequence has the same slots, personas, devices, blocking policy and
    /// capture backends, so there is nothing for `bounce_reason` to name — and
    /// `map-macro` therefore reaches the live engine exactly the way `map`
    /// does, through `ApplyBindings`.
    #[test]
    fn a_macro_body_change_is_a_binding_change_not_a_bounce() {
        let mut with_macro = tiny_plan(1, ksx_core::Persona::Xbox360);
        with_macro.slots[0]
            .preset
            .macros
            .defs
            .push(ksx_core::Macro::new(
                "hadouken",
                vec![ksx_core::MacroStep::new(
                    vec![ksx_core::Binding::Dpad(ksx_core::DpadDirection::Down)],
                    50,
                )],
            ));
        with_macro.slots[0]
            .preset
            .macros
            .triggers
            .push(ksx_core::MacroTrigger::new(ksx_core::Key::P, 0));
        let plain = tiny_plan(1, ksx_core::Persona::Xbox360);

        let running = crate::run::supervisor::SessionShape::of(&plain);
        let edited = crate::run::supervisor::SessionShape::of(&with_macro);
        assert_eq!(
            running.bounce_reason(&edited),
            None,
            "adding a macro must not replug the pads"
        );
        assert_eq!(
            edited.bounce_reason(&running),
            None,
            "...and neither must deleting one"
        );
        // The sanity check on the other side of the line, so this test cannot
        // pass by SessionShape having stopped comparing anything at all.
        assert!(running
            .bounce_reason(&crate::run::supervisor::SessionShape::of(&tiny_plan(
                1,
                ksx_core::Persona::PlayStation
            )))
            .is_some());
    }

    /// A structural change takes the old road, and says so in the same field
    /// shape so the caller can tell them apart.
    #[test]
    fn apply_bindings_bounces_a_structural_change_and_names_it() {
        // The session is running slot 1 as an Xbox pad; the config on disk now
        // says PlayStation. That is a different device node: it has to replug.
        let running = tiny_plan(1, ksx_core::Persona::Xbox360);
        let ondisk = tiny_plan(1, ksx_core::Persona::PlayStation);
        let mut factory = FakeFactory {
            shape: Some(crate::run::supervisor::SessionShape::of(&running)),
            plan: Some(ondisk),
            ..FakeFactory::default()
        };
        let swaps = factory.swaps.clone();
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start { game: None },
                DaemonCommand::ApplyBindings,
                DaemonCommand::Quit,
            ],
        );
        assert_eq!(
            *factory.makes.lock().unwrap(),
            2,
            "a persona change must rebuild the session: {text}"
        );
        let report = state.apply.expect("a verdict was recorded");
        assert!(report.ok && !report.hot && report.restarted, "{report:?}");
        assert!(report.message.contains("session restarted"), "{report:?}");
        assert!(report.message.contains("persona"), "{report:?}");
        assert_eq!(swaps.load(Ordering::SeqCst), 0, "nothing was hot-swapped");
    }

    /// Nothing running: nothing to do, and the answer says why rather than
    /// starting a session nobody asked for.
    #[test]
    fn apply_bindings_with_no_session_changes_nothing() {
        let mut factory = FakeFactory::default();
        let (state, _) = drive(
            &mut factory,
            &[DaemonCommand::ApplyBindings, DaemonCommand::Quit],
        );
        assert_eq!(*factory.makes.lock().unwrap(), 0);
        let report = state.apply.expect("a verdict was recorded");
        assert!(report.ok && !report.hot && !report.restarted, "{report:?}");
        assert!(
            report.message.contains("no session is running"),
            "{report:?}"
        );
    }

    /// A config that no longer resolves must NOT cost the user their running
    /// session: tearing it down to fail the restart is the worst of both.
    #[test]
    fn apply_bindings_keeps_a_running_session_when_the_config_is_broken() {
        // `plan: None` = resolve_plan() errors.
        let mut factory = FakeFactory::default();
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start { game: None },
                DaemonCommand::ApplyBindings,
                DaemonCommand::Quit,
            ],
        );
        assert_eq!(
            *factory.makes.lock().unwrap(),
            1,
            "the running session must survive a broken config: {text}"
        );
        let report = state.apply.expect("a verdict was recorded");
        assert!(!report.ok, "{report:?}");
        assert!(
            report.message.contains("still running on its old bindings"),
            "{report:?}"
        );
    }

    /// A pipe `start --game X` repoints this and every later session at that
    /// profile — the same thing restarting the daemon with `--game X` does.
    #[test]
    fn start_with_a_profile_override_repoints_the_factory() {
        let mut factory = FakeFactory::default();
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start {
                    game: Some("Metal Slug".into()),
                },
                DaemonCommand::Stop,
                DaemonCommand::Quit,
            ],
        );
        assert_eq!(factory.game.as_deref(), Some("Metal Slug"), "{text}");
        assert_eq!(state.game.as_deref(), Some("Metal Slug"));
        assert!(factory.ran.load(Ordering::SeqCst), "{text}");
    }

    /// ...but an override whose start never started is rolled back: a typo'd
    /// title must not repoint every later tray Start at the broken profile.
    #[test]
    fn a_failed_override_start_restores_the_previous_profile() {
        let mut factory = FakeFactory {
            fail_with: Some("refusing to start: unknown game".into()),
            ..FakeFactory::default()
        };
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start {
                    game: Some("Typo Fighter".into()),
                },
                DaemonCommand::Quit,
            ],
        );
        assert!(text.contains("cannot start"), "{text}");
        assert_eq!(factory.game.as_deref(), Some("Street Fighter"), "{text}");
        assert_eq!(state.game.as_deref(), Some("Street Fighter"));
    }

    /// Quit while running must stop the session, not orphan it.
    #[test]
    fn quit_stops_a_running_session_first() {
        let mut factory = FakeFactory::default();
        let (state, text) = drive(
            &mut factory,
            &[DaemonCommand::Start { game: None }, DaemonCommand::Quit],
        );
        assert!(text.contains("stopping before exit"), "{text}");
        assert!(text.contains("bye"), "{text}");
        assert_eq!(state.run, RunState::Quitting);
    }

    /// A session that ends by itself (the game exited, an escape) is noticed
    /// without anyone pressing Stop.
    #[test]
    fn a_session_that_ends_on_its_own_is_reaped_and_reported() {
        let mut factory = FakeFactory {
            self_ends_after: Some(Duration::from_millis(20)),
            summary: SessionSummary {
                stop_code: "game-exited".into(),
                message: "the game exited".into(),
                ..SessionSummary::default()
            },
            ..FakeFactory::default()
        };
        let (tx, rx) = unbounded();
        tx.send(DaemonCommand::Start { game: None }).unwrap();
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let watcher = state.clone();
        std::thread::spawn(move || {
            // Give the session time to end on its own, then quit.
            std::thread::sleep(Duration::from_millis(300));
            let _ = tx.send(DaemonCommand::Quit);
        });
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(
            rx,
            state.clone(),
            &mut factory,
            &mut NoPanel,
            &NoUi,
            &mut out,
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("game-exited"), "{text}");
        let last = watcher.lock().unwrap().last.clone().expect("recorded");
        assert_eq!(last.stop_code, "game-exited");
    }

    /// A failure to even build the session must land in the state, not vanish
    /// into a log line the tray never sees.
    #[test]
    fn a_failed_start_is_visible_in_the_state_and_the_tooltip() {
        let mut factory = FakeFactory {
            fail_with: Some("refusing to start: 2 configuration problem(s)".into()),
            ..FakeFactory::default()
        };
        let (state, text) = drive(
            &mut factory,
            &[DaemonCommand::Start { game: None }, DaemonCommand::Quit],
        );
        assert!(text.contains("cannot start"), "{text}");
        assert!(
            matches!(state.run, RunState::Quitting),
            "quit still wins: {state:?}"
        );
        assert!(!factory.ran.load(Ordering::SeqCst));
    }

    /// Losing the command channel (the tray thread died) must shut the daemon
    /// down rather than leave it holding keyboards with no way to stop it.
    #[test]
    fn a_disconnected_command_channel_shuts_the_daemon_down() {
        let mut factory = FakeFactory::default();
        let (tx, rx) = unbounded();
        tx.send(DaemonCommand::Start { game: None }).unwrap();
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(
            rx,
            state.clone(),
            &mut factory,
            &mut NoPanel,
            &NoUi,
            &mut out,
        );
        assert_eq!(state.lock().unwrap().run, RunState::Quitting);
    }

    #[test]
    fn the_tooltip_surfaces_capture_health_and_stays_within_the_win32_limit() {
        let state = DaemonState {
            run: RunState::Running { slots: 4 },
            game: Some("Street Fighter".into()),
            last: Some(LastSession {
                reboot_required: true,
                ..LastSession::default()
            }),
            live: None,
            apply: None,
        };
        let tip = state.tooltip();
        assert!(tip.contains("running, 4 pad(s)"), "{tip}");
        assert!(tip.contains("Street Fighter"), "{tip}");
        assert!(tip.contains("REBOOT REQUIRED"), "{tip}");
        assert!(tip.encode_utf16().count() <= 127, "{tip}");

        let long = DaemonState {
            run: RunState::Failed {
                message: "x".repeat(400),
            },
            game: Some("y".repeat(200)),
            last: None,
            live: None,
            apply: None,
        };
        assert!(long.tooltip().encode_utf16().count() <= 127);
        assert!(long.tooltip().ends_with('…'));
    }

    // -----------------------------------------------------------------
    // Live health: what is wrong NOW, not what was wrong then
    // -----------------------------------------------------------------

    /// **The mid-session case, which is the whole point.** `last` is written by
    /// `reap()`, which by definition runs after a session is over — so a
    /// REBOOT REQUIRED that happens forty minutes into a two-hour game used to
    /// be visible nowhere at all until the player quit. Here nothing has ever
    /// been reaped (`last: None`) and the tooltip must still say it.
    #[test]
    fn the_tooltip_reports_the_running_sessions_health_with_nothing_reaped_yet() {
        let state = DaemonState {
            run: RunState::Running { slots: 4 },
            game: Some("Street Fighter".into()),
            last: None,
            live: Some(LiveHealth {
                reboot_required: true,
                ..LiveHealth::default()
            }),
            apply: None,
        };
        let tip = state.tooltip();
        assert!(tip.contains("running, 4 pad(s)"), "{tip}");
        assert!(
            tip.contains("REBOOT REQUIRED"),
            "a mid-session problem must be in the tooltip while the session runs: {tip}"
        );
        assert!(tip.encode_utf16().count() <= 127, "{tip}");

        // ...and the same for the other two, each phrased in the present tense.
        let watchdog = DaemonState {
            run: RunState::Running { slots: 2 },
            live: Some(LiveHealth {
                watchdog_tripped: true,
                ..LiveHealth::default()
            }),
            ..DaemonState::default()
        };
        let tip = watchdog.tooltip();
        assert!(tip.contains("watchdog TRIPPED"), "{tip}");
        assert!(
            !tip.contains("last session"),
            "this session is not over: {tip}"
        );

        let dropped = DaemonState {
            run: RunState::Running { slots: 2 },
            live: Some(LiveHealth {
                dropped_events: 12,
                ..LiveHealth::default()
            }),
            ..DaemonState::default()
        };
        assert!(dropped.tooltip().contains("12 event(s) dropped"));
    }

    /// A healthy running session must not erase the previous session's verdict
    /// — `reboot_required` in particular describes the *machine* and stays true
    /// until Windows restarts, so "the current session looks fine" is not a
    /// reason to stop saying it.
    #[test]
    fn a_healthy_live_session_falls_back_to_the_last_sessions_verdict() {
        let state = DaemonState {
            run: RunState::Running { slots: 4 },
            game: None,
            last: Some(LastSession {
                reboot_required: true,
                ..LastSession::default()
            }),
            live: Some(LiveHealth::default()),
            apply: None,
        };
        assert!(state.tooltip().contains("REBOOT REQUIRED"), "{state:?}");
    }

    /// ...but a live problem outranks a stale one: a 128-unit tooltip has room
    /// for exactly one line, and it has to be the actionable one.
    #[test]
    fn a_live_problem_outranks_the_last_sessions_note() {
        let state = DaemonState {
            run: RunState::Running { slots: 4 },
            game: None,
            last: Some(LastSession {
                dropped_events: 3,
                ..LastSession::default()
            }),
            live: Some(LiveHealth {
                watchdog_tripped: true,
                ..LiveHealth::default()
            }),
            apply: None,
        };
        let tip = state.tooltip();
        assert!(tip.contains("watchdog TRIPPED"), "{tip}");
        assert!(!tip.contains("dropped"), "one line, the live one: {tip}");
    }

    /// Nothing wrong must say nothing at all. A tooltip that always carries a
    /// `[!]` teaches people to ignore the `[!]`.
    #[test]
    fn a_healthy_daemon_has_no_health_line() {
        let state = DaemonState {
            run: RunState::Running { slots: 4 },
            live: Some(LiveHealth::default()),
            last: Some(LastSession {
                stop_code: "ctrl-c".into(),
                message: "stopped by Ctrl+C".into(),
                ..LastSession::default()
            }),
            game: None,
            apply: None,
        };
        assert!(!state.tooltip().contains("[!]"), "{}", state.tooltip());
    }

    /// End to end through the real control loop: a watchdog trip published by a
    /// running session reaches `DaemonState` **while it is still running**, with
    /// nothing reaped. This is gap B at the level it is caused — the tooltip
    /// test above proves the rendering, this proves the plumbing.
    #[test]
    fn a_mid_session_trip_reaches_the_state_before_anything_is_reaped() {
        let health = ksx_capture::HealthHandle::new();
        let mut factory = FakeFactory {
            health: Some(health.clone()),
            ..FakeFactory::default()
        };
        let (tx, rx) = unbounded();
        tx.send(DaemonCommand::Start { game: None }).unwrap();
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));

        // What the tray would have seen at the moment the trip became visible.
        let seen: Arc<Mutex<Option<DaemonState>>> = Arc::new(Mutex::new(None));
        std::thread::spawn({
            let watcher = state.clone();
            let seen = seen.clone();
            move || {
                // Let the session come up and publish its view.
                std::thread::sleep(Duration::from_millis(100));
                health.set_watchdog_tripped();
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    let snapshot = watcher.lock().unwrap().clone();
                    if snapshot.live.is_some_and(|h| h.watchdog_tripped) {
                        *seen.lock().unwrap() = Some(snapshot);
                        break;
                    }
                    if Instant::now() > deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let _ = tx.send(DaemonCommand::Quit);
            }
        });

        let mut out: Vec<u8> = Vec::new();
        control_loop_with(
            rx,
            state.clone(),
            &mut factory,
            &mut NoPanel,
            &NoUi,
            &mut out,
        );

        let seen = seen
            .lock()
            .unwrap()
            .clone()
            .expect("the trip never reached DaemonState — the tray would show nothing");
        assert!(
            matches!(seen.run, RunState::Running { .. }),
            "the session must still have been running when the tray could see it: {seen:?}"
        );
        assert!(
            seen.last.is_none(),
            "nothing has been reaped: this is exactly the window that used to be blind"
        );
        assert!(seen.tooltip().contains("watchdog TRIPPED"), "{seen:?}");

        // Once the session IS reaped, the live reading is dropped rather than
        // left pinned to a session that no longer exists.
        let after = state.lock().unwrap().clone();
        assert_eq!(after.live, None, "{after:?}");
    }

    /// A runner with no capture backend publishes nothing, and "nothing to
    /// report" must not be rendered as "nothing wrong".
    #[test]
    fn an_unpublished_slot_reports_nothing_at_all() {
        let slot = HealthSlot::default();
        assert_eq!(slot.poll(), None);
        let handle = ksx_capture::HealthHandle::new();
        handle.set_reboot_required();
        slot.publish(ksx_capture::HealthView::new(handle));
        assert_eq!(
            slot.poll(),
            Some(LiveHealth {
                reboot_required: true,
                watchdog_tripped: false,
                dropped_events: 0,
            })
        );
    }

    #[test]
    fn the_menu_disables_what_cannot_be_done_right_now() {
        // The two surfaces lead the menu (see the test below) and are always
        // available; the session verbs sit behind them.
        let start = 2;
        let stopped = DaemonState::default().menu();
        assert_eq!(
            stopped[start],
            (DaemonCommand::Start { game: None }, "Start emulation", true)
        );
        assert_eq!(
            stopped[start + 1],
            (DaemonCommand::Stop, "Stop emulation", false)
        );

        let running = DaemonState {
            run: RunState::Running { slots: 4 },
            ..DaemonState::default()
        }
        .menu();
        assert!(!running[start].2, "cannot start what is already running");
        assert!(running[start + 1].2, "stop must be available while running");
        // Everything else is always available — including both surfaces, which
        // is the point of opening a window: you look at ksx when it is running
        // AND when it will not.
        assert!(running[..start].iter().all(|(_, _, enabled)| *enabled));
        assert!(running[start + 2..].iter().all(|(_, _, enabled)| *enabled));
    }

    // -----------------------------------------------------------------
    // M6: the claimed-panel contract
    // -----------------------------------------------------------------

    fn drive_with_panel(script: &[DaemonCommand]) -> Vec<&'static str> {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let mut factory = FakeFactory {
            trace: Some(trace.clone()),
            ..FakeFactory::default()
        };
        let mut panel = RecordingPanel {
            trace: trace.clone(),
        };
        let (tx, rx) = unbounded();
        for command in script {
            tx.send(command.clone()).unwrap();
        }
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(rx, state, &mut factory, &mut panel, &NoUi, &mut out);
        let log = trace.lock().unwrap().clone();
        log
    }

    /// **The M6 daemon contract.** The panel must stop typing *before* the
    /// session exists and start again *after* it is gone.
    ///
    /// Get the first one wrong and a player's inputs are translated to pad
    /// state and typed onto the desktop behind the game at the same time. Get
    /// the second one wrong and they come back to a frontend they cannot
    /// navigate — which is the whole failure a WinUSB claim introduces and the
    /// whole reason this mechanism exists.
    #[test]
    fn the_panel_stops_typing_before_a_session_starts_and_resumes_after_it_ends() {
        let log = drive_with_panel(&[
            DaemonCommand::Start { game: None },
            DaemonCommand::Stop,
            DaemonCommand::Quit,
        ]);
        let mute = log.iter().position(|s| *s == "panel:muted").expect("muted");
        let started = log
            .iter()
            .position(|s| *s == "session:started")
            .expect("started");
        let ended = log
            .iter()
            .position(|s| *s == "session:ended")
            .expect("ended");
        let typing = log
            .iter()
            .position(|s| *s == "panel:typing")
            .expect("resumed");
        assert!(
            mute < started,
            "the panel must be muted before the session exists: {log:?}"
        );
        assert!(
            ended < typing,
            "the panel must resume only after the session is gone: {log:?}"
        );
    }

    /// A session that ends by itself (the game exited) hands the panel back
    /// too — otherwise quitting a game leaves a dead frontend.
    #[test]
    fn a_self_ending_session_hands_the_panel_back() {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let mut factory = FakeFactory {
            self_ends_after: Some(Duration::from_millis(20)),
            trace: Some(trace.clone()),
            ..FakeFactory::default()
        };
        let mut panel = RecordingPanel {
            trace: trace.clone(),
        };
        let (tx, rx) = unbounded();
        tx.send(DaemonCommand::Start { game: None }).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _ = tx.send(DaemonCommand::Quit);
        });
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(rx, state, &mut factory, &mut panel, &NoUi, &mut out);

        let log = trace.lock().unwrap().clone();
        let ended = log
            .iter()
            .position(|s| *s == "session:ended")
            .unwrap_or_else(|| panic!("the fake session must have ended: {log:?}"));
        assert!(
            log[ended + 1..].contains(&"panel:typing"),
            "the panel must be handed back after a game exits on its own: {log:?}"
        );
        // ...and it was muted first, or it was never ksx's to hand back.
        assert!(log[..ended].contains(&"panel:muted"), "{log:?}");
    }

    /// A start that never starts must not leave the panel muted: nothing owns
    /// it, so it has to keep typing.
    #[test]
    fn a_failed_start_gives_the_panel_straight_back() {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let mut factory = FakeFactory {
            fail_with: Some("refusing to start".into()),
            trace: Some(trace.clone()),
            ..FakeFactory::default()
        };
        let mut panel = RecordingPanel {
            trace: trace.clone(),
        };
        let (tx, rx) = unbounded();
        tx.send(DaemonCommand::Start { game: None }).unwrap();
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(rx, state, &mut factory, &mut panel, &NoUi, &mut out);

        let log = trace.lock().unwrap().clone();
        assert_eq!(log.first(), Some(&"panel:muted"), "{log:?}");
        assert_eq!(
            log.get(1),
            Some(&"panel:typing"),
            "a failed start must hand the panel straight back: {log:?}"
        );
        assert!(!log.contains(&"session:started"), "{log:?}");
    }

    /// Losing the command channel is a teardown path too.
    #[test]
    fn a_disconnected_channel_hands_the_panel_back_before_returning() {
        let log = drive_with_panel(&[DaemonCommand::Start { game: None }]);
        assert_eq!(
            log.last(),
            Some(&"panel:typing"),
            "the last thing a dying daemon does is give the panel back: {log:?}"
        );
    }

    /// The default is `NoPanel`: with a backend whose devices are still
    /// keyboards, ksx injecting as well would double every keystroke.
    #[test]
    fn the_default_panel_does_nothing() {
        let mut panel = NoPanel;
        panel.set_emulating(true);
        panel.set_emulating(false);
        // Compiles, does nothing, and `control_loop` uses it — which is the
        // assertion: existing behaviour is unchanged.
    }

    #[test]
    fn headless_commands_parse_the_documented_words() {
        for (line, want) in [
            ("start", DaemonCommand::Start { game: None }),
            ("  STOP ", DaemonCommand::Stop),
            ("reload", DaemonCommand::Reload),
            ("config", DaemonCommand::OpenConfigFolder),
            ("cabinet", DaemonCommand::OpenCabinet),
            ("ui", DaemonCommand::OpenCabinet),
            ("studio", DaemonCommand::OpenStudio),
            ("status", DaemonCommand::Status),
            ("quit", DaemonCommand::Quit),
            ("exit", DaemonCommand::Quit),
        ] {
            assert_eq!(DaemonCommand::parse(line), Some(want), "{line}");
        }
        assert_eq!(DaemonCommand::parse("launch nukes"), None);
        // Every command the tray offers must be reachable headlessly, or
        // "identical control surface" is a lie.
        for (command, _, _) in DaemonState::default().menu() {
            assert!(
                DaemonCommand::help().contains(match command {
                    DaemonCommand::Start { .. } => "start",
                    DaemonCommand::Stop => "stop",
                    DaemonCommand::Reload => "reload",
                    DaemonCommand::OpenConfigFolder => "config",
                    DaemonCommand::OpenCabinet => "cabinet",
                    DaemonCommand::OpenStudio => "studio",
                    DaemonCommand::Status => "status",
                    DaemonCommand::Quit => "quit",
                    // Not a menu item: the mapper's save path, reachable
                    // over the pipe only (there is nothing for a human to
                    // click that means "apply bindings and nothing else").
                    DaemonCommand::ApplyBindings => "reload",
                }),
                "{command:?} is in the tray menu but not reachable headlessly"
            );
        }
    }

    /// **The tray's primary action is the cabinet panel.** The M9 pass moved
    /// it there deliberately: the first item is the surface you can drive from
    /// the machine, and Studio — which needs a keyboard — is second.
    #[test]
    fn the_trays_first_two_items_are_the_two_surfaces_in_order() {
        let menu = DaemonState::default().menu();
        assert_eq!(menu[0].0, DaemonCommand::OpenCabinet);
        assert_eq!(menu[0].1, "Open cabinet UI");
        assert!(menu[0].2, "always available — it is how you look at ksx");
        assert_eq!(menu[1].0, DaemonCommand::OpenStudio);
        assert_eq!(menu[1].1, "Open Studio");
        // ...and Quit is still last, and still the tray's alone. Closing a
        // window must never do what this item does.
        assert_eq!(
            menu.last().map(|item| item.0.clone()),
            Some(DaemonCommand::Quit)
        );
    }

    /// Neither window may be able to end the daemon: the control loop's Quit
    /// arm is reachable from the tray and from stdin, and from nothing else.
    /// A cabinet whose emulation stopped because somebody shut a status panel
    /// would be a cabinet nobody trusts.
    #[test]
    fn opening_a_surface_never_stops_a_session() {
        struct Recorder(Arc<Mutex<Vec<&'static str>>>);
        impl UiHost for Recorder {
            fn open_cabinet(&self, _out: &mut dyn Write) {
                self.0.lock().unwrap().push("cabinet");
            }
            fn open_studio(&self, _out: &mut dyn Write) {
                self.0.lock().unwrap().push("studio");
            }
        }

        let opened = Arc::new(Mutex::new(Vec::new()));
        let mut factory = FakeFactory::default();
        let (tx, rx) = unbounded();
        for command in [
            DaemonCommand::Start { game: None },
            DaemonCommand::OpenCabinet,
            DaemonCommand::OpenStudio,
            DaemonCommand::Status,
            DaemonCommand::Quit,
        ] {
            tx.send(command).unwrap();
        }
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop_with(
            rx,
            state.clone(),
            &mut factory,
            &mut NoPanel,
            &Recorder(opened.clone()),
            &mut out,
        );
        let text = String::from_utf8(out).unwrap();
        assert_eq!(*opened.lock().unwrap(), ["cabinet", "studio"]);
        // The session started, survived both surfaces, and only ended at Quit.
        assert_eq!(*factory.makes.lock().unwrap(), 1, "{text}");
        assert!(factory.ran.load(Ordering::SeqCst), "{text}");
        assert!(
            !text.contains("stopping…"),
            "opening a window must not stop a session: {text}"
        );
    }

    /// A build with no UI features still answers the menu item — in words,
    /// naming the feature that would. Never a silent no-op.
    #[test]
    fn a_build_with_no_surfaces_says_so_instead_of_doing_nothing() {
        let mut out: Vec<u8> = Vec::new();
        NoUi.open_cabinet(&mut out);
        NoUi.open_studio(&mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("--features cabinet"), "{text}");
        assert!(text.contains("--features studio"), "{text}");
    }
}
