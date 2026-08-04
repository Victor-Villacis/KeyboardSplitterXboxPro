//! Starting a game, and following it to its end.
//!
//! [`tracker`](crate::tracker) decides *when* a session is over; this module is
//! the thin layer that gives it facts. Every OS interaction goes through
//! [`GameHost`], so the whole thing — launch failure, launcher hand-off, exit,
//! the give-up warning — is driven in CI by [`FakeHost`] without starting a
//! single process.
//!
//! # Ordering (the bug this shape exists to prevent)
//!
//! A game must be started **after** the pads are plugged and capture is armed.
//! A game that starts first enumerates zero controllers, caches that, and shows
//! "no gamepad detected" for the rest of the session — with four perfectly good
//! virtual pads sitting behind it. Nothing here enforces that on its own; it is
//! enforced by *where* the supervisor calls [`GameSession::start`], and pinned
//! by a test over the supervisor's step trace.

use std::path::Path;

use ksx_platform::process::{GameProcess, ProcessEntry};

use crate::profile::{LaunchSpec, LaunchTarget};
use crate::tracker::{
    GameTracker, Observation, TrackOutcome, TrackPolicy, TrackState, Unresolvable,
};

/// Everything a session needs from the outside world.
pub trait GameHost {
    /// Monotonic milliseconds. Only differences are used.
    fn now_ms(&mut self) -> u64;
    /// Start an executable and keep its handle.
    fn spawn(
        &mut self,
        exe: &Path,
        args: &[String],
        working_dir: Option<&Path>,
    ) -> Result<u32, String>;
    /// Hand a protocol URL to the shell. No handle comes back — by nature.
    fn activate(&mut self, url: &str) -> Result<(), String>;
    /// Is the spawned process still running? `None` when there is no handle.
    fn launched_alive(&mut self) -> Option<bool>;
    /// The current process list (empty on snapshot failure — see the tracker).
    fn processes(&mut self) -> Vec<ProcessEntry>;
}

/// How the launch went, for the caller to print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartReport {
    /// Pid, when ksx holds a handle.
    pub pid: Option<u32>,
    /// True when the target was a protocol URL.
    pub detached: bool,
    /// Lines to print loudly — today: "this profile cannot detect its own
    /// exit". Never fatal.
    pub warnings: Vec<String>,
}

/// The launch itself failed. Emulation is already up when this happens, so the
/// caller tears down and exits 3 — see `ksx-app`'s exit-code table.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("could not start '{title}': {message}")]
pub struct LaunchFailure {
    pub title: String,
    pub message: String,
}

/// One game, from launch to exit.
pub struct GameSession<H: GameHost> {
    host: H,
    spec: LaunchSpec,
    tracker: GameTracker,
    started: bool,
}

impl<H: GameHost> GameSession<H> {
    pub fn new(spec: LaunchSpec, host: H) -> Self {
        Self::with_policy(
            spec.clone(),
            host,
            // Both tunables come from the profile: `process_name` (what to hunt
            // for) and `launcher_grace_ms` (how long a launch may live and still
            // be a launcher). The second one is why a 5-second Steam hand-off no
            // longer reads as the player quitting.
            TrackPolicy::for_profile(spec.process_name.clone(), spec.launcher_grace_ms),
        )
    }

    pub fn with_policy(spec: LaunchSpec, host: H, policy: TrackPolicy) -> Self {
        Self {
            host,
            spec,
            tracker: GameTracker::new(policy),
            started: false,
        }
    }

    pub fn spec(&self) -> &LaunchSpec {
        &self.spec
    }

    /// The host, for tests that need to script it after construction.
    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn state(&self) -> &TrackState {
        self.tracker.state()
    }

    /// Launch. Call this only once the pads are up and capture is armed.
    pub fn start(&mut self, games_toml: &Path) -> Result<StartReport, LaunchFailure> {
        let now = self.host.now_ms();
        self.started = true;
        let mut warnings = Vec::new();

        match self.spec.target.clone() {
            LaunchTarget::Executable {
                exe,
                args,
                working_dir,
            } => {
                let pid = self
                    .host
                    .spawn(&exe, &args, working_dir.as_deref())
                    .map_err(|message| LaunchFailure {
                        title: self.spec.title.clone(),
                        message,
                    })?;
                self.tracker.started_with_handle(now);
                Ok(StartReport {
                    pid: Some(pid),
                    detached: false,
                    warnings,
                })
            }
            LaunchTarget::Protocol { url, .. } => {
                self.host.activate(&url).map_err(|message| LaunchFailure {
                    title: self.spec.title.clone(),
                    message,
                })?;
                if let TrackOutcome::GaveUp(_) = self.tracker.started_detached(now) {
                    warnings.push(crate::profile::missing_process_name_warning(
                        &self.spec, games_toml,
                    ));
                }
                Ok(StartReport {
                    pid: None,
                    detached: true,
                    warnings,
                })
            }
        }
    }

    /// One supervisor poll. Cheap: a `try_wait` and, only while hunting or
    /// tracking, one process snapshot.
    pub fn poll(&mut self) -> TrackOutcome {
        if !self.started {
            return TrackOutcome::Watching;
        }
        let now = self.host.now_ms();
        let launched_alive = self.host.launched_alive();
        // The snapshot is the expensive part (a full Toolhelp pass), so it is
        // only taken in the states that can consume it — plus the one poll on
        // which `Running` turns into a hand-off, so the hunt can start against
        // the same observation instead of waiting another 50 ms.
        let processes = match self.tracker.state() {
            TrackState::LauncherHandoff { .. } | TrackState::Tracking { .. } => {
                self.host.processes()
            }
            TrackState::Running if launched_alive == Some(false) => self.host.processes(),
            _ => Vec::new(),
        };
        self.tracker.observe(Observation {
            now_ms: now,
            launched_alive,
            processes: &processes,
        })
    }

    /// The wording for a `GaveUp` outcome the caller should print once.
    ///
    /// The two reasons need genuinely different advice: one is "you never told
    /// ksx what to look for", the other is "ksx looked for what you told it and
    /// never found it". Printing the first for the second tells the user to add
    /// a key that is already in their file.
    pub fn gave_up_message(&self, reason: Unresolvable, games_toml: &Path) -> String {
        match reason {
            Unresolvable::NoProcessName => {
                crate::profile::missing_process_name_warning(&self.spec, games_toml)
            }
            Unresolvable::HandoffTimedOut => crate::profile::handoff_timed_out_warning(
                &self.spec,
                games_toml,
                self.tracker.policy().handoff_grace_ms,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The real host
// ---------------------------------------------------------------------------

/// [`GameHost`] backed by the OS.
pub struct RealHost {
    epoch: std::time::Instant,
    child: Option<GameProcess>,
}

impl Default for RealHost {
    fn default() -> Self {
        Self::new()
    }
}

impl RealHost {
    pub fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),
            child: None,
        }
    }
}

impl GameHost for RealHost {
    fn now_ms(&mut self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn spawn(
        &mut self,
        exe: &Path,
        args: &[String],
        working_dir: Option<&Path>,
    ) -> Result<u32, String> {
        let child =
            ksx_platform::process::launch(exe, args, working_dir).map_err(|err| err.to_string())?;
        let pid = child.pid();
        self.child = Some(child);
        Ok(pid)
    }

    fn activate(&mut self, url: &str) -> Result<(), String> {
        ksx_platform::process::shell_open(url).map_err(|err| err.to_string())
    }

    fn launched_alive(&mut self) -> Option<bool> {
        self.child.as_mut().map(GameProcess::is_alive)
    }

    fn processes(&mut self) -> Vec<ProcessEntry> {
        ksx_platform::process::snapshot()
    }
}

// ---------------------------------------------------------------------------
// The fake host
// ---------------------------------------------------------------------------

/// Scriptable [`GameHost`] for tests. Not `#[cfg(test)]`: `ksx-app`'s pipeline
/// tests drive the real supervisor through it, and they live in another crate.
#[derive(Default)]
pub struct FakeHost {
    pub now_ms: u64,
    pub spawn_result: Option<Result<u32, String>>,
    pub activate_result: Option<Result<(), String>>,
    pub alive: Option<bool>,
    pub processes: Vec<ProcessEntry>,
    /// Everything that was launched, in order — so ordering assertions can see
    /// that nothing started before it should have.
    pub launched: Vec<String>,
}

impl FakeHost {
    pub fn running(pid: u32) -> Self {
        Self {
            spawn_result: Some(Ok(pid)),
            activate_result: Some(Ok(())),
            alive: Some(true),
            ..Self::default()
        }
    }

    pub fn failing(message: &str) -> Self {
        Self {
            spawn_result: Some(Err(message.to_owned())),
            activate_result: Some(Err(message.to_owned())),
            ..Self::default()
        }
    }

    pub fn advance(&mut self, ms: u64) {
        self.now_ms += ms;
    }

    pub fn set_processes(&mut self, entries: &[(u32, &str)]) {
        self.processes = entries
            .iter()
            .map(|(pid, name)| ProcessEntry {
                pid: *pid,
                parent_pid: 0,
                name: (*name).to_owned(),
            })
            .collect();
    }
}

impl GameHost for FakeHost {
    fn now_ms(&mut self) -> u64 {
        self.now_ms
    }

    fn spawn(
        &mut self,
        exe: &Path,
        _args: &[String],
        _working_dir: Option<&Path>,
    ) -> Result<u32, String> {
        self.launched.push(exe.display().to_string());
        self.spawn_result
            .clone()
            .unwrap_or(Err("no spawn scripted".to_owned()))
    }

    fn activate(&mut self, url: &str) -> Result<(), String> {
        self.launched.push(url.to_owned());
        self.activate_result
            .clone()
            .unwrap_or(Err("no activation scripted".to_owned()))
    }

    fn launched_alive(&mut self) -> Option<bool> {
        self.alive
    }

    fn processes(&mut self) -> Vec<ProcessEntry> {
        self.processes.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::GameEntry;

    fn entry(path: &str, process_name: Option<&str>) -> GameEntry {
        GameEntry {
            title: "Portal 2".into(),
            notes: String::new(),
            path: path.into(),
            arguments: String::new(),
            process_name: process_name.map(str::to_owned),
            launcher_grace_ms: None,
            block_keyboards: true,
            block_mice: false,
            slots: Vec::new(),
        }
    }

    fn games_toml() -> &'static Path {
        Path::new(r"C:\cfg\ksx\games.toml")
    }

    #[test]
    fn an_executable_session_runs_until_the_process_exits() {
        let spec = LaunchSpec::from_entry(&entry(r"C:\g\portal2.exe", None));
        let mut session = GameSession::new(spec, FakeHost::running(1234));
        let report = session.start(games_toml()).unwrap();
        assert_eq!(report.pid, Some(1234));
        assert!(!report.detached);
        assert!(report.warnings.is_empty());
        assert_eq!(session.host.launched, vec![r"C:\g\portal2.exe"]);

        // Past the launcher grace, so this exit is the game's own.
        session
            .host
            .advance(crate::tracker::DEFAULT_LAUNCHER_GRACE_MS + 1);
        assert_eq!(session.poll(), TrackOutcome::Watching);
        session.host.alive = Some(false);
        assert_eq!(session.poll(), TrackOutcome::GameExited);
    }

    /// **The cabinet regression, at the session level.** A profile that points
    /// straight at `steam.exe`: the program exits after 5 seconds having handed
    /// off, and that must not be reported as the game ending. Under the old 3 s
    /// rule this returned `GameExited` and emulation stopped mid-launch.
    #[test]
    fn a_five_second_launcher_exit_does_not_end_the_session() {
        let spec = LaunchSpec::from_entry(&entry(
            r"C:\Program Files (x86)\Steam\steam.exe",
            Some("portal2.exe"),
        ));
        let mut session = GameSession::new(spec, FakeHost::running(1234));
        session.start(games_toml()).unwrap();

        session.host.advance(5_000);
        session.host.alive = Some(false);
        assert_eq!(
            session.poll(),
            TrackOutcome::Watching,
            "a 5 s hand-off must not stop emulation"
        );
        assert!(matches!(
            session.state(),
            TrackState::LauncherHandoff { .. }
        ));

        // The game the launcher started is then followed to its real exit.
        session.host.set_processes(&[(77, "portal2.exe")]);
        session.host.advance(20_000);
        assert_eq!(session.poll(), TrackOutcome::Watching);
        assert_eq!(session.state(), &TrackState::Tracking { pid: 77 });
    }

    /// A profile may override the grace, and the override reaches the tracker
    /// through `GameSession::new` — not only through `with_policy`, which is
    /// what the production path uses.
    #[test]
    fn a_profile_grace_override_reaches_the_tracker() {
        let mut spec = LaunchSpec::from_entry(&entry(r"C:\g\x.exe", Some("x.exe")));
        spec.launcher_grace_ms = Some(1_000);
        let mut session = GameSession::new(spec, FakeHost::running(7));
        session.start(games_toml()).unwrap();
        session.host.advance(1_001);
        session.host.alive = Some(false);
        assert_eq!(
            session.poll(),
            TrackOutcome::GameExited,
            "with a 1 s grace, a 1001 ms life is the game itself"
        );
    }

    #[test]
    fn a_failed_spawn_is_a_launch_failure_not_a_panic() {
        let spec = LaunchSpec::from_entry(&entry(r"C:\g\portal2.exe", None));
        let mut session =
            GameSession::new(spec, FakeHost::failing("Access is denied. (os error 5)"));
        let err = session.start(games_toml()).unwrap_err();
        assert_eq!(err.title, "Portal 2");
        assert!(err.to_string().contains("Access is denied"), "{err}");
    }

    /// The approved judgement call: warn loudly, name the file and the key,
    /// and run anyway.
    #[test]
    fn a_steam_profile_without_a_process_name_warns_but_still_runs() {
        let spec = LaunchSpec::from_entry(&entry("steam://rungameid/620", None));
        let mut session = GameSession::new(spec, FakeHost::running(0));
        let report = session.start(games_toml()).unwrap();
        assert!(report.detached);
        assert_eq!(report.pid, None);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("process_name"), "{report:?}");
        assert!(report.warnings[0].contains(r"C:\cfg\ksx\games.toml"));
        assert_eq!(session.host.launched, vec!["steam://rungameid/620"]);

        // ...and it never ends the session by itself.
        for step in 0..50 {
            session.host.advance(1_000 * step);
            assert_eq!(session.poll(), TrackOutcome::Watching);
        }
        assert_eq!(
            session.state(),
            &TrackState::Unresolvable(Unresolvable::NoProcessName)
        );
    }

    #[test]
    fn a_steam_profile_with_a_process_name_tracks_the_game() {
        let spec = LaunchSpec::from_entry(&entry("steam://rungameid/620", Some("portal2.exe")));
        let mut session = GameSession::new(spec, FakeHost::running(0));
        let report = session.start(games_toml()).unwrap();
        assert!(report.warnings.is_empty(), "{report:?}");

        session.host.advance(10_000);
        assert_eq!(session.poll(), TrackOutcome::Watching);

        session.host.set_processes(&[(77, "Portal2.exe")]);
        session.host.advance(5_000);
        assert_eq!(session.poll(), TrackOutcome::Watching);
        assert_eq!(session.state(), &TrackState::Tracking { pid: 77 });

        session.host.set_processes(&[]);
        for _ in 0..2 {
            session.host.advance(50);
            assert_eq!(session.poll(), TrackOutcome::Watching);
        }
        session.host.advance(50);
        assert_eq!(session.poll(), TrackOutcome::GameExited);
    }

    /// Polling before `start` must be inert — the supervisor calls `poll` on
    /// every tick, including the ones before the hook has launched anything.
    #[test]
    fn polling_before_start_does_nothing() {
        let spec = LaunchSpec::from_entry(&entry(r"C:\g\x.exe", None));
        let mut session = GameSession::new(spec, FakeHost::running(1));
        assert_eq!(session.poll(), TrackOutcome::Watching);
        assert!(
            session.host.launched.is_empty(),
            "nothing may launch on a poll"
        );
    }

    /// The expensive Toolhelp snapshot is only taken in the two states that can
    /// use it — this runs 20 times a second for a whole play session.
    #[test]
    fn a_plain_running_session_never_takes_a_process_snapshot() {
        struct CountingHost {
            inner: FakeHost,
            snapshots: std::cell::Cell<u32>,
        }
        impl GameHost for CountingHost {
            fn now_ms(&mut self) -> u64 {
                self.inner.now_ms()
            }
            fn spawn(
                &mut self,
                exe: &Path,
                args: &[String],
                wd: Option<&Path>,
            ) -> Result<u32, String> {
                self.inner.spawn(exe, args, wd)
            }
            fn activate(&mut self, url: &str) -> Result<(), String> {
                self.inner.activate(url)
            }
            fn launched_alive(&mut self) -> Option<bool> {
                self.inner.launched_alive()
            }
            fn processes(&mut self) -> Vec<ProcessEntry> {
                self.snapshots.set(self.snapshots.get() + 1);
                self.inner.processes()
            }
        }

        let spec = LaunchSpec::from_entry(&entry(r"C:\g\x.exe", Some("x.exe")));
        let host = CountingHost {
            inner: FakeHost::running(3),
            snapshots: std::cell::Cell::new(0),
        };
        let mut session = GameSession::new(spec, host);
        session.start(games_toml()).unwrap();
        for _ in 0..100 {
            session.host.inner.advance(50);
            assert_eq!(session.poll(), TrackOutcome::Watching);
        }
        assert_eq!(
            session.host.snapshots.get(),
            0,
            "a healthy running game needs no process enumeration at all"
        );
    }
}
