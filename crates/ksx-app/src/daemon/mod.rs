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

pub mod live;
#[cfg(windows)]
pub mod tray;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

/// Everything the tray (or stdin) can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonCommand {
    /// Start emulation if it is not already running.
    Start,
    /// Stop the current session. The game, if any, keeps running.
    Stop,
    /// Stop, re-read the configuration, start again.
    Reload,
    /// Open the config folder in Explorer.
    OpenConfigFolder,
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
            "start" | "s" => Some(Self::Start),
            "stop" | "x" => Some(Self::Stop),
            "reload" | "r" => Some(Self::Reload),
            "config" | "c" => Some(Self::OpenConfigFolder),
            "status" | "?" => Some(Self::Status),
            "quit" | "q" | "exit" => Some(Self::Quit),
            _ => None,
        }
    }

    /// The one-line help shown at startup and on an unrecognised line.
    pub fn help() -> &'static str {
        "commands: start | stop | reload | config | status | quit"
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

/// The state the tray polls. Small, cloneable, no borrows of anything live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DaemonState {
    pub run: RunState,
    pub game: Option<String>,
    pub last: Option<LastSession>,
}

impl DaemonState {
    /// The tray tooltip. Windows truncates it at 128 UTF-16 units, so the most
    /// important thing goes first and the health notes are appended only if
    /// there is something to say.
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
        if let Some(last) = &self.last {
            if last.reboot_required {
                text.push_str("\n[!] REBOOT REQUIRED (Interception slot exhaustion)");
            } else if last.watchdog_tripped {
                text.push_str("\n[!] capture watchdog tripped last session");
            } else if last.dropped_events > 0 {
                text.push_str(&format!("\n[!] {} event(s) dropped", last.dropped_events));
            }
        }
        truncate_utf16(&text, 127)
    }

    /// Menu item labels + whether each is enabled right now.
    pub fn menu(&self) -> Vec<(DaemonCommand, &'static str, bool)> {
        let running = matches!(self.run, RunState::Running { .. } | RunState::Starting);
        vec![
            (DaemonCommand::Start, "Start emulation", !running),
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
}

/// Makes a fresh runner per session, re-reading configuration each time. That
/// is what "Reload config" means: not a hot-patch of a live pipeline, but a
/// clean stop and a clean start from whatever is on disk now.
pub trait SessionFactory: Send {
    fn make(&mut self) -> anyhow::Result<Box<dyn SessionRunner>>;
    fn config_dir(&self) -> std::path::PathBuf;
    fn game(&self) -> Option<String>;
}

/// The control loop. Returns when [`DaemonCommand::Quit`] is handled or the
/// command channel closes.
///
/// Blocking here is free: nothing in the input path waits on this thread.
pub fn control_loop(
    commands: Receiver<DaemonCommand>,
    state: SharedState,
    factory: &mut dyn SessionFactory,
    out: &mut dyn Write,
) {
    let mut session: Option<LiveSession> = None;
    set_game(&state, factory.game());

    loop {
        // Reap a session that ended on its own (the game exited, an escape, a
        // driver failure) so the tray stops claiming it is running.
        if let Some(live) = &session {
            if live.handle.as_ref().is_some_and(|h| h.is_finished()) {
                let finished = session.take().expect("checked");
                reap(finished, &state, out);
            }
        }

        match commands.recv_timeout(Duration::from_millis(200)) {
            Ok(DaemonCommand::Start) => {
                if session.is_some() {
                    let _ = writeln!(out, "already running");
                    continue;
                }
                session = start(factory, &state, out);
            }
            Ok(DaemonCommand::Stop) => match session.take() {
                Some(live) => {
                    let _ = writeln!(out, "stopping…");
                    live.stop.store(true, Ordering::SeqCst);
                    reap(live, &state, out);
                }
                None => {
                    let _ = writeln!(out, "not running");
                }
            },
            Ok(DaemonCommand::Reload) => {
                if let Some(live) = session.take() {
                    live.stop.store(true, Ordering::SeqCst);
                    reap(live, &state, out);
                }
                let _ = writeln!(out, "reloading configuration…");
                set_game(&state, factory.game());
                session = start(factory, &state, out);
            }
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
                set_run(&state, RunState::Quitting);
                let _ = out.flush();
                return;
            }
        }
        let _ = out.flush();
    }
}

struct LiveSession {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<anyhow::Result<SessionSummary>>>,
    started: Instant,
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
pub fn run(
    game: Option<String>,
    no_launch: bool,
    headless: bool,
    autostart: bool,
) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    // Fail fast on a broken configuration rather than showing a tray icon that
    // can only ever report errors.
    if let Err(err) = crate::run::plan::resolve(&root, game.as_deref()) {
        eprintln!("refusing to start the daemon:\n{err}");
        std::process::exit(crate::run::EXIT_CANNOT_START);
    }
    let mut factory = live::LiveFactory {
        root,
        game,
        no_launch,
    };

    let (tx, rx) = crossbeam_channel::unbounded::<DaemonCommand>();
    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
    if autostart {
        let _ = tx.send(DaemonCommand::Start);
    }

    #[cfg(windows)]
    if !headless {
        // Control loop on a worker; tray owns this thread.
        let worker = {
            let state = state.clone();
            std::thread::Builder::new()
                .name("ksx-daemon".into())
                .spawn(move || {
                    let mut out = std::io::stdout();
                    control_loop(rx, state, &mut factory, &mut out);
                })?
        };
        if tray::run(tx.clone(), state.clone()) {
            // The tray exited (Quit, or the icon was destroyed): make sure the
            // control loop hears about it even if the click never arrived.
            let _ = tx.send(DaemonCommand::Quit);
            drop(tx);
            let _ = worker.join();
            return Ok(());
        }
        // The tray could not be created (Session 0, a locked-down desktop, no
        // shell). Fall through to headless rather than leaving a daemon nobody
        // can talk to — the control surface is identical, so nothing is lost
        // but the icon.
        eprintln!("[WARN] the tray icon could not be created; running headless.");
        eprintln!("{}", DaemonCommand::help());
        std::thread::spawn({
            let tx = tx.clone();
            move || stdin_commands(tx)
        });
        drop(tx);
        let _ = worker.join();
        return Ok(());
    }

    let _ = headless;
    println!("ksx daemon (headless). {}", DaemonCommand::help());
    std::thread::spawn({
        let tx = tx.clone();
        move || stdin_commands(tx)
    });
    drop(tx);
    let mut out = std::io::stdout();
    control_loop(rx, state, &mut factory, &mut out);
    Ok(())
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

    /// A runner that blocks until told to stop, and reports whatever we script.
    struct FakeRunner {
        summary: SessionSummary,
        slots: usize,
        /// Set when the session actually ran.
        ran: Arc<AtomicBool>,
        /// End on its own after this long, ignoring `stop`.
        self_ends_after: Option<Duration>,
    }

    impl SessionRunner for FakeRunner {
        fn run(
            &mut self,
            stop: Arc<AtomicBool>,
            _out: &mut dyn Write,
        ) -> anyhow::Result<SessionSummary> {
            self.ran.store(true, Ordering::SeqCst);
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
            Ok(self.summary.clone())
        }

        fn slots(&self) -> usize {
            self.slots
        }
    }

    struct FakeFactory {
        summary: SessionSummary,
        slots: usize,
        ran: Arc<AtomicBool>,
        self_ends_after: Option<Duration>,
        fail_with: Option<String>,
        makes: Arc<Mutex<u32>>,
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
            }))
        }

        fn config_dir(&self) -> std::path::PathBuf {
            std::path::PathBuf::from(r"C:\cfg\ksx")
        }

        fn game(&self) -> Option<String> {
            Some("Street Fighter".into())
        }
    }

    fn drive(factory: &mut FakeFactory, script: &[DaemonCommand]) -> (DaemonState, String) {
        let (tx, rx) = unbounded();
        for command in script {
            tx.send(*command).unwrap();
        }
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop(rx, state.clone(), factory, &mut out);
        let final_state = state.lock().unwrap().clone();
        (final_state, String::from_utf8(out).unwrap())
    }

    #[test]
    fn start_then_stop_runs_exactly_one_session() {
        let mut factory = FakeFactory::default();
        let (state, text) = drive(
            &mut factory,
            &[
                DaemonCommand::Start,
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
                DaemonCommand::Start,
                DaemonCommand::Start,
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
                DaemonCommand::Start,
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

    /// Quit while running must stop the session, not orphan it.
    #[test]
    fn quit_stops_a_running_session_first() {
        let mut factory = FakeFactory::default();
        let (state, text) = drive(&mut factory, &[DaemonCommand::Start, DaemonCommand::Quit]);
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
        tx.send(DaemonCommand::Start).unwrap();
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let watcher = state.clone();
        std::thread::spawn(move || {
            // Give the session time to end on its own, then quit.
            std::thread::sleep(Duration::from_millis(300));
            let _ = tx.send(DaemonCommand::Quit);
        });
        let mut out: Vec<u8> = Vec::new();
        control_loop(rx, state.clone(), &mut factory, &mut out);
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
        let (state, text) = drive(&mut factory, &[DaemonCommand::Start, DaemonCommand::Quit]);
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
        tx.send(DaemonCommand::Start).unwrap();
        drop(tx);
        let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
        let mut out: Vec<u8> = Vec::new();
        control_loop(rx, state.clone(), &mut factory, &mut out);
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
        };
        assert!(long.tooltip().encode_utf16().count() <= 127);
        assert!(long.tooltip().ends_with('…'));
    }

    #[test]
    fn the_menu_disables_what_cannot_be_done_right_now() {
        let stopped = DaemonState::default().menu();
        assert_eq!(stopped[0], (DaemonCommand::Start, "Start emulation", true));
        assert_eq!(stopped[1], (DaemonCommand::Stop, "Stop emulation", false));

        let running = DaemonState {
            run: RunState::Running { slots: 4 },
            ..DaemonState::default()
        }
        .menu();
        assert!(!running[0].2, "cannot start what is already running");
        assert!(running[1].2, "stop must be available while running");
        // The other three are always available.
        assert!(running[2..].iter().all(|(_, _, enabled)| *enabled));
    }

    #[test]
    fn headless_commands_parse_the_documented_words() {
        for (line, want) in [
            ("start", DaemonCommand::Start),
            ("  STOP ", DaemonCommand::Stop),
            ("reload", DaemonCommand::Reload),
            ("config", DaemonCommand::OpenConfigFolder),
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
                [
                    DaemonCommand::Start,
                    DaemonCommand::Stop,
                    DaemonCommand::Reload,
                    DaemonCommand::OpenConfigFolder,
                    DaemonCommand::Quit
                ]
                .contains(&command)
                    && DaemonCommand::help().contains(match command {
                        DaemonCommand::Start => "start",
                        DaemonCommand::Stop => "stop",
                        DaemonCommand::Reload => "reload",
                        DaemonCommand::OpenConfigFolder => "config",
                        DaemonCommand::Status => "status",
                        DaemonCommand::Quit => "quit",
                    }),
                "{command:?} is in the tray menu but not reachable headlessly"
            );
        }
    }
}
