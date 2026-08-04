//! When is the game over? A pure state machine, with the clock injected.
//!
//! # The legacy rule, and why it is not enough on its own
//!
//! The C# app launched the game, waited, and only treated the exit as real if
//! the process had lived **more than 3 seconds** (`Models/Game.cs:216-330`,
//! step 7). Anything shorter is a launcher or a shim: `steam.exe` hands the
//! request to the already-running client and returns; a Battle.net stub spawns
//! the real binary; MAME frontends `exec` through a `.bat`. Treating those
//! exits as "the player quit" would tear emulation down two seconds into a
//! session.
//!
//! Legacy stopped there: a sub-3-second exit meant "never report an exit at
//! all", so emulation ran until somebody stopped it by hand. That is safe but
//! useless on a cabinet — the whole point of `ksx run --game` is that quitting
//! the game returns the machine to the frontend.
//!
//! # Why 3 seconds was the wrong number (measured, not guessed)
//!
//! On the cabinet, `C:\Program Files (x86)\Steam\steam.exe` took **5 seconds**
//! to hand off to the already-running client and exit. Under the 3 s rule ksx
//! classified that as the game itself quitting and stopped emulation *while the
//! game was still loading* — pads gone, panel back to typing, mid-launch. The
//! threshold is now [`DEFAULT_LAUNCHER_GRACE_MS`] (10 s) and is configurable per
//! profile (`launcher_grace_ms` in `games.toml`), because the right value is a
//! property of the launcher, not of ksx.
//!
//! **The trade, stated honestly.** This threshold answers one question — "did
//! that process live long enough to have *been* the game?" — and both wrong
//! answers cost something:
//!
//! - **too low**: a slow launcher's exit is read as the game's, and emulation
//!   stops during a launch that is still in progress. That is the observed bug,
//!   and it is the expensive one: the player is left with a dead panel and no
//!   pads, and only an emergency escape or a restart fixes it.
//! - **too high**: a genuinely short play session (start, quit after 8 s) is
//!   read as a hand-off. ksx then hunts for `process_name` — finds nothing,
//!   because the game really has gone — and waits out the 60 s hand-off grace
//!   before saying so. Emulation keeps running for up to a minute longer than it
//!   needed to, which is untidy but harms nothing: the pads work, the escapes
//!   work, and with a `process_name` set the hunt gives up on its own.
//!
//! One failure strands a player mid-game; the other delays a teardown nobody is
//! waiting on. 10 s buys twice the observed hand-off time against a cost that is
//! only ever measured in seconds of extra emulation, so the default errs high.
//! A cabinet with a launcher slower still (a cold Battle.net on a spinning disk)
//! raises `launcher_grace_ms`; a MAME-only cabinet where every exit is the game
//! can drop it back to legacy's 3 s.
//!
//! The 60 s hand-off hunt ([`DEFAULT_HANDOFF_GRACE_MS`]) is a **separate**
//! timer answering a different question — "how long do we look for the game the
//! launcher started?" — and is deliberately untouched by any of this.
//!
//! # What this adds: `LauncherHandoff`
//!
//! A short-lived launch is not the end of the session, it is a **hand-off**. So
//! the machine keeps the >3 s rule verbatim and, when it trips, spends a
//! configurable grace period (60 s by default) hunting for the profile's
//! `process_name` in the process list. Find it → track *that* process to its
//! exit. Don't find it in time → say so, plainly, and keep running: the session
//! is still perfectly playable and every emergency escape still ends it.
//!
//! `steam://`-style profiles start in `LauncherHandoff` directly, because there
//! was never a process to hold in the first place.
//!
//! # Why it is a state machine and not a thread that sleeps
//!
//! Every input arrives as an [`Observation`]: a monotonic timestamp, whether
//! the launched process is still alive, and the current process list. Nothing
//! in here reads a clock, spawns anything, or touches the OS — so the whole
//! decision table (including the 60-second grace and the exit-confirmation
//! debounce) is exercised in microseconds by CI, on any platform, with fakes.

use ksx_platform::process::ProcessEntry;

/// Legacy's threshold (`Game.cs`, "> 3 seconds"), kept as a named constant
/// because it is the number every legacy behaviour comparison is against — and
/// a reasonable value for a profile that launches the game directly.
///
/// **Not the default any more**: see [`DEFAULT_LAUNCHER_GRACE_MS`] and the
/// module docs for the 5-second Steam hand-off that retired it.
pub const LEGACY_LAUNCHER_THRESHOLD_MS: u64 = 3_000;

/// A process that lives this long or less handed off to something else.
///
/// 10 s: twice the 5 s hand-off measured from `steam.exe` on the cabinet, which
/// is the margin a cold launcher on a loaded machine needs. Overridable per
/// profile with `launcher_grace_ms` — the full trade-off is in the module docs,
/// and the short version is that being too low strands a player mid-launch while
/// being too high only delays a teardown.
pub const DEFAULT_LAUNCHER_GRACE_MS: u64 = 10_000;

/// How long to hunt for `process_name` after a hand-off before giving up.
///
/// 60 s because a cold Steam start on a spinning disk genuinely takes that
/// long: the client updates itself, shows a splash, decrypts, and only then
/// spawns the game. Too short and a legitimate slow start is misread as "gone".
///
/// Independent of [`DEFAULT_LAUNCHER_GRACE_MS`] and unchanged by its retuning:
/// that one asks "was that the game or a launcher?", this one asks "how long do
/// we look for what the launcher started?".
pub const DEFAULT_HANDOFF_GRACE_MS: u64 = 60_000;

/// Consecutive misses required before a tracked process is declared gone.
///
/// [`ksx_platform::process::snapshot`] returns an empty vector when the
/// snapshot could not be taken — a documented "nothing matched yet", not an
/// error. One empty result must therefore never end a session; three in a row
/// (≥150 ms at the supervisor's 50 ms poll) is a real exit.
pub const EXIT_CONFIRMATIONS: u8 = 3;

/// Tunables, so a cabinet with a slow launcher can be told so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackPolicy {
    /// Image name to hunt for after a hand-off. `None` means ksx has no way to
    /// find the game once the launcher lets go.
    pub process_name: Option<String>,
    pub launcher_threshold_ms: u64,
    pub handoff_grace_ms: u64,
    pub exit_confirmations: u8,
}

impl Default for TrackPolicy {
    fn default() -> Self {
        Self {
            process_name: None,
            launcher_threshold_ms: DEFAULT_LAUNCHER_GRACE_MS,
            handoff_grace_ms: DEFAULT_HANDOFF_GRACE_MS,
            exit_confirmations: EXIT_CONFIRMATIONS,
        }
    }
}

impl TrackPolicy {
    pub fn for_process(name: Option<String>) -> Self {
        Self {
            process_name: name,
            ..Self::default()
        }
    }

    /// The policy a `games.toml` profile asks for: its `process_name` and its
    /// optional `launcher_grace_ms`.
    ///
    /// `None` for the grace means the default — a profile that says nothing
    /// must keep tracking the default as it moves, rather than freezing whatever
    /// it happened to be when the file was written.
    pub fn for_profile(name: Option<String>, launcher_grace_ms: Option<u64>) -> Self {
        Self {
            process_name: name,
            launcher_threshold_ms: launcher_grace_ms.unwrap_or(DEFAULT_LAUNCHER_GRACE_MS),
            ..Self::default()
        }
    }
}

/// One poll's worth of facts.
#[derive(Clone, Copy, Debug)]
pub struct Observation<'a> {
    /// Monotonic milliseconds. Only differences are used, so the epoch is the
    /// caller's business.
    pub now_ms: u64,
    /// `Some(alive)` when ksx holds a process handle; `None` for a protocol URL
    /// where there is nothing to hold.
    pub launched_alive: Option<bool>,
    /// The process list as of this poll. May legitimately be empty when the
    /// snapshot failed — the confirmation counter is what absorbs that.
    pub processes: &'a [ProcessEntry],
}

/// What the machine is doing right now. Public so the tray/tooltip and
/// `--json` can report it without inventing their own vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackState {
    /// Nothing launched yet.
    Idle,
    /// ksx holds a handle and the process is running.
    Running,
    /// The launched thing is gone but lived ≤ the threshold; hunting for
    /// `process_name` until the grace period expires.
    LauncherHandoff { since_ms: u64 },
    /// A process matching `process_name` was found and is being followed.
    Tracking { pid: u32 },
    /// The game is over.
    Exited,
    /// ksx cannot tell when this session ends. Terminal, but **not** a stop:
    /// the session keeps running until an emergency escape.
    Unresolvable(Unresolvable),
}

/// Why exit detection gave up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unresolvable {
    /// A protocol URL, or a launcher hand-off, with no `process_name` to hunt.
    NoProcessName,
    /// `process_name` never appeared inside the grace period.
    HandoffTimedOut,
}

impl Unresolvable {
    pub fn code(&self) -> &'static str {
        match self {
            Unresolvable::NoProcessName => "no-process-name",
            Unresolvable::HandoffTimedOut => "handoff-timed-out",
        }
    }
}

/// What the supervisor should do about this poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackOutcome {
    /// Nothing to do; keep the session running.
    Watching,
    /// The game ended. Stop emulation, exit 0.
    GameExited,
    /// Detection gave up. Warn once (the caller has the wording) and keep
    /// running — the pads still work and the escapes still work.
    GaveUp(Unresolvable),
}

/// The machine.
#[derive(Clone, Debug)]
pub struct GameTracker {
    policy: TrackPolicy,
    state: TrackState,
    started_ms: u64,
    /// Consecutive polls in which the tracked process was not seen.
    misses: u8,
    /// True once `GaveUp` has been returned, so the caller warns exactly once.
    announced: bool,
}

impl GameTracker {
    pub fn new(policy: TrackPolicy) -> Self {
        Self {
            policy,
            state: TrackState::Idle,
            started_ms: 0,
            misses: 0,
            announced: false,
        }
    }

    pub fn state(&self) -> &TrackState {
        &self.state
    }

    pub fn policy(&self) -> &TrackPolicy {
        &self.policy
    }

    /// The game was started, and ksx holds a process handle for it.
    pub fn started_with_handle(&mut self, now_ms: u64) {
        self.started_ms = now_ms;
        self.misses = 0;
        self.state = TrackState::Running;
    }

    /// A protocol URL was activated: nothing to hold, so hunt immediately.
    pub fn started_detached(&mut self, now_ms: u64) -> TrackOutcome {
        self.started_ms = now_ms;
        self.misses = 0;
        match self.policy.process_name {
            Some(_) => {
                self.state = TrackState::LauncherHandoff { since_ms: now_ms };
                TrackOutcome::Watching
            }
            None => self.give_up(Unresolvable::NoProcessName),
        }
    }

    /// Advance one poll.
    pub fn observe(&mut self, obs: Observation<'_>) -> TrackOutcome {
        match self.state.clone() {
            TrackState::Idle => TrackOutcome::Watching,
            TrackState::Exited => TrackOutcome::GameExited,
            TrackState::Unresolvable(reason) => {
                // Already announced: report `Watching` so the caller does not
                // re-warn every 50 ms for the rest of the session.
                if self.announced {
                    TrackOutcome::Watching
                } else {
                    self.announced = true;
                    TrackOutcome::GaveUp(reason)
                }
            }
            TrackState::Running => self.observe_running(obs),
            TrackState::LauncherHandoff { since_ms } => self.observe_handoff(obs, since_ms),
            TrackState::Tracking { pid } => self.observe_tracking(obs, pid),
        }
    }

    fn observe_running(&mut self, obs: Observation<'_>) -> TrackOutcome {
        // `None` here means the caller lost the handle without telling us; the
        // safe reading is "still running", because declaring an exit we cannot
        // see would stop a live session.
        if obs.launched_alive.unwrap_or(true) {
            return TrackOutcome::Watching;
        }
        let lifetime = obs.now_ms.saturating_sub(self.started_ms);
        if lifetime > self.policy.launcher_threshold_ms {
            // Lived long enough to have been the game itself.
            self.state = TrackState::Exited;
            return TrackOutcome::GameExited;
        }
        // Legacy's launcher case. Hand off, or admit we cannot follow it.
        match self.policy.process_name {
            Some(_) => {
                self.state = TrackState::LauncherHandoff {
                    since_ms: obs.now_ms,
                };
                self.misses = 0;
                // Evaluate the hunt against *this* observation rather than
                // waiting for the next poll: a launcher that spawns the game
                // and exits has usually already been seen by the snapshot in
                // hand, and burning a poll on it is 50 ms of latency for free.
                self.observe_handoff(obs, obs.now_ms)
            }
            None => self.give_up(Unresolvable::NoProcessName),
        }
    }

    fn observe_handoff(&mut self, obs: Observation<'_>, since_ms: u64) -> TrackOutcome {
        if let Some(pid) = self.find_target(obs.processes) {
            self.state = TrackState::Tracking { pid };
            self.misses = 0;
            return TrackOutcome::Watching;
        }
        if obs.now_ms.saturating_sub(since_ms) >= self.policy.handoff_grace_ms {
            return self.give_up(Unresolvable::HandoffTimedOut);
        }
        TrackOutcome::Watching
    }

    fn observe_tracking(&mut self, obs: Observation<'_>, pid: u32) -> TrackOutcome {
        let name = self.policy.process_name.as_deref().unwrap_or_default();
        if obs.processes.iter().any(|p| p.pid == pid) {
            self.misses = 0;
            return TrackOutcome::Watching;
        }
        // The pid we were following is gone. A multi-stage launcher may have
        // replaced it with another process of the same name (a relauncher, a
        // 32→64-bit trampoline, a "restart to apply settings"). Re-latch rather
        // than call it a quit.
        if let Some(next) = obs
            .processes
            .iter()
            .find(|p| p.name_matches(name))
            .map(|p| p.pid)
        {
            tracing::info!(
                from = pid,
                to = next,
                name,
                "tracked game process was replaced by another of the same name; following it"
            );
            self.state = TrackState::Tracking { pid: next };
            self.misses = 0;
            return TrackOutcome::Watching;
        }
        // Nothing by that name. Could be a failed snapshot — confirm first.
        self.misses = self.misses.saturating_add(1);
        if self.misses >= self.policy.exit_confirmations {
            self.state = TrackState::Exited;
            return TrackOutcome::GameExited;
        }
        TrackOutcome::Watching
    }

    fn find_target(&self, processes: &[ProcessEntry]) -> Option<u32> {
        let name = self.policy.process_name.as_deref()?;
        processes
            .iter()
            .find(|p| p.name_matches(name))
            .map(|p| p.pid)
    }

    fn give_up(&mut self, reason: Unresolvable) -> TrackOutcome {
        self.state = TrackState::Unresolvable(reason);
        self.announced = true;
        TrackOutcome::GaveUp(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs(names: &[(u32, &str)]) -> Vec<ProcessEntry> {
        names
            .iter()
            .map(|(pid, name)| ProcessEntry {
                pid: *pid,
                parent_pid: 0,
                name: (*name).to_owned(),
            })
            .collect()
    }

    fn obs<'a>(now_ms: u64, alive: Option<bool>, processes: &'a [ProcessEntry]) -> Observation<'a> {
        Observation {
            now_ms,
            launched_alive: alive,
            processes,
        }
    }

    fn tracker(process_name: Option<&str>) -> GameTracker {
        GameTracker::new(TrackPolicy::for_process(process_name.map(str::to_owned)))
    }

    /// The rule, unchanged in shape: a process that outlives the threshold
    /// *was* the game, and its exit ends the session. Only the number moved.
    #[test]
    fn a_long_lived_process_exiting_ends_the_session() {
        let mut t = tracker(None);
        t.started_with_handle(0);
        assert_eq!(
            t.observe(obs(1_000, Some(true), &[])),
            TrackOutcome::Watching
        );
        assert_eq!(
            t.observe(obs(DEFAULT_LAUNCHER_GRACE_MS + 1, Some(false), &[])),
            TrackOutcome::GameExited
        );
        assert_eq!(t.state(), &TrackState::Exited);
        // ...and it stays exited.
        assert_eq!(
            t.observe(obs(999_999, Some(false), &[])),
            TrackOutcome::GameExited
        );
    }

    /// The comparison is `>`, not `>=`: exactly the threshold is still a
    /// launcher. An off-by-one here silently changes which games work.
    #[test]
    fn the_threshold_is_strictly_greater_than_the_configured_grace() {
        let mut t = tracker(Some("game.exe"));
        t.started_with_handle(0);
        assert_eq!(
            t.observe(obs(DEFAULT_LAUNCHER_GRACE_MS, Some(false), &[])),
            TrackOutcome::Watching,
            "a life of exactly the grace is a launcher, not the game"
        );
        assert!(matches!(t.state(), TrackState::LauncherHandoff { .. }));

        let mut t = tracker(Some("game.exe"));
        t.started_with_handle(0);
        assert_eq!(
            t.observe(obs(DEFAULT_LAUNCHER_GRACE_MS + 1, Some(false), &[])),
            TrackOutcome::GameExited
        );
    }

    /// **The cabinet regression.** `steam.exe` took 5 seconds to hand off to the
    /// already-running client and exit. Under legacy's 3 s rule that read as
    /// "the player quit" and emulation stopped mid-launch, with the game still
    /// loading. It must read as a hand-off, and the hunt must find the game.
    #[test]
    fn a_five_second_steam_handoff_is_not_the_game_quitting() {
        // The test is only meaningful while 5 s sits above legacy's threshold
        // and below the new default — i.e. exactly in the window the bug lived
        // in. Checked at compile time so moving either constant into 5 s fails
        // the build rather than making this test silently vacuous.
        const _: () = assert!(LEGACY_LAUNCHER_THRESHOLD_MS < 5_000);
        const _: () = assert!(DEFAULT_LAUNCHER_GRACE_MS > 5_000);
        let mut t = tracker(Some("portal2.exe"));
        t.started_with_handle(0);
        // Still up at 4 s.
        assert_eq!(
            t.observe(obs(4_000, Some(true), &[])),
            TrackOutcome::Watching
        );
        // Hands off and exits at 5 s — the measured number.
        assert_eq!(
            t.observe(obs(5_000, Some(false), &[])),
            TrackOutcome::Watching,
            "a 5 s launcher exit must NOT stop emulation"
        );
        assert_eq!(t.state(), &TrackState::LauncherHandoff { since_ms: 5_000 });
        // ...and the game the launcher started is picked up normally.
        t.observe(obs(25_000, None, &procs(&[(42, "portal2.exe")])));
        assert_eq!(t.state(), &TrackState::Tracking { pid: 42 });
    }

    /// A profile may still ask for legacy's number — a MAME cabinet where every
    /// launch is the game itself wants the faster stop.
    #[test]
    fn a_profile_can_ask_for_the_legacy_three_second_rule_back() {
        let mut t = GameTracker::new(TrackPolicy::for_profile(
            Some("mame.exe".into()),
            Some(LEGACY_LAUNCHER_THRESHOLD_MS),
        ));
        t.started_with_handle(0);
        assert_eq!(
            t.observe(obs(3_001, Some(false), &[])),
            TrackOutcome::GameExited
        );
        // ...and saying nothing means the default, not a frozen copy of it.
        let policy = TrackPolicy::for_profile(Some("x.exe".into()), None);
        assert_eq!(policy.launcher_threshold_ms, DEFAULT_LAUNCHER_GRACE_MS);
    }

    /// The whole hand-off arc: launcher returns fast, the game appears a while
    /// later, ksx follows it, and quitting *it* ends the session.
    #[test]
    fn a_launcher_handoff_finds_the_game_and_tracks_it_to_exit() {
        let mut t = tracker(Some("portal2.exe"));
        t.started_with_handle(0);
        // Steam returned after 800 ms.
        assert_eq!(
            t.observe(obs(800, Some(false), &[])),
            TrackOutcome::Watching
        );
        assert_eq!(t.state(), &TrackState::LauncherHandoff { since_ms: 800 });

        // Nothing yet — the client is still updating.
        let others = procs(&[(10, "steam.exe"), (11, "explorer.exe")]);
        assert_eq!(
            t.observe(obs(20_000, None, &others)),
            TrackOutcome::Watching
        );

        // There it is.
        let running = procs(&[(10, "steam.exe"), (42, "Portal2.exe")]);
        assert_eq!(
            t.observe(obs(30_000, None, &running)),
            TrackOutcome::Watching
        );
        assert_eq!(
            t.state(),
            &TrackState::Tracking { pid: 42 },
            "process names are matched case-insensitively, like Windows"
        );

        // Playing...
        assert_eq!(
            t.observe(obs(600_000, None, &running)),
            TrackOutcome::Watching
        );

        // Quit. Takes EXIT_CONFIRMATIONS polls to be believed.
        for i in 1..EXIT_CONFIRMATIONS {
            assert_eq!(
                t.observe(obs(600_000 + i as u64 * 50, None, &others)),
                TrackOutcome::Watching,
                "poll {i} must not be enough on its own"
            );
        }
        assert_eq!(
            t.observe(obs(700_000, None, &others)),
            TrackOutcome::GameExited
        );
    }

    /// A failed `snapshot()` comes back as an empty list. One of those must
    /// never look like "the player quit".
    #[test]
    fn a_transient_empty_snapshot_does_not_end_the_session() {
        let mut t = tracker(Some("mame.exe"));
        t.started_with_handle(0);
        t.observe(obs(500, Some(false), &[]));
        let running = procs(&[(7, "mame.exe")]);
        t.observe(obs(1_000, None, &running));
        assert_eq!(t.state(), &TrackState::Tracking { pid: 7 });

        // Two failed snapshots, then it comes back: no exit, and the miss
        // counter resets.
        assert_eq!(t.observe(obs(1_050, None, &[])), TrackOutcome::Watching);
        assert_eq!(t.observe(obs(1_100, None, &[])), TrackOutcome::Watching);
        assert_eq!(
            t.observe(obs(1_150, None, &running)),
            TrackOutcome::Watching
        );
        assert_eq!(t.observe(obs(1_200, None, &[])), TrackOutcome::Watching);
        assert_eq!(t.observe(obs(1_250, None, &[])), TrackOutcome::Watching);
        assert_eq!(
            t.observe(obs(1_300, None, &[])),
            TrackOutcome::GameExited,
            "three consecutive misses is a real exit"
        );
    }

    /// A relauncher (32→64-bit trampoline, "restart to apply") replaces the
    /// pid but keeps the name. Following the name is what keeps the session up.
    #[test]
    fn a_replaced_process_of_the_same_name_is_followed() {
        let mut t = tracker(Some("game.exe"));
        t.started_with_handle(0);
        t.observe(obs(100, Some(false), &procs(&[(5, "game.exe")])));
        assert_eq!(t.state(), &TrackState::Tracking { pid: 5 });
        assert_eq!(
            t.observe(obs(200, None, &procs(&[(6, "game.exe")]))),
            TrackOutcome::Watching
        );
        assert_eq!(t.state(), &TrackState::Tracking { pid: 6 });
    }

    /// The grace period is real: 59 s of nothing is patience, 60 s is giving up.
    #[test]
    fn the_handoff_grace_expires_and_says_so_exactly_once() {
        let mut t = tracker(Some("never.exe"));
        t.started_with_handle(0);
        t.observe(obs(1_000, Some(false), &[]));
        assert_eq!(
            t.observe(obs(1_000 + DEFAULT_HANDOFF_GRACE_MS - 1, None, &[])),
            TrackOutcome::Watching
        );
        assert_eq!(
            t.observe(obs(1_000 + DEFAULT_HANDOFF_GRACE_MS, None, &[])),
            TrackOutcome::GaveUp(Unresolvable::HandoffTimedOut)
        );
        // Warned once, then quiet — this is polled 20x a second.
        for step in 1..5 {
            assert_eq!(
                t.observe(obs(200_000 + step * 50, None, &[])),
                TrackOutcome::Watching
            );
        }
    }

    /// A `steam://` profile with no `process_name`: ksx cannot know when the
    /// game ends, says so immediately, and keeps running (never refuses).
    #[test]
    fn a_detached_launch_without_a_process_name_gives_up_immediately_but_keeps_running() {
        let mut t = tracker(None);
        assert_eq!(
            t.started_detached(0),
            TrackOutcome::GaveUp(Unresolvable::NoProcessName)
        );
        assert_eq!(
            t.state(),
            &TrackState::Unresolvable(Unresolvable::NoProcessName)
        );
        assert_eq!(t.observe(obs(50, None, &[])), TrackOutcome::Watching);
        assert_eq!(
            t.observe(obs(10_000_000, None, &[])),
            TrackOutcome::Watching,
            "the session must never be ended by a detection gap"
        );
    }

    /// A `steam://` profile *with* a `process_name` starts hunting at once —
    /// there was never a handle, so there is nothing to wait 3 seconds for.
    #[test]
    fn a_detached_launch_with_a_process_name_hunts_from_the_start() {
        let mut t = tracker(Some("portal2.exe"));
        assert_eq!(t.started_detached(0), TrackOutcome::Watching);
        assert_eq!(t.state(), &TrackState::LauncherHandoff { since_ms: 0 });
        t.observe(obs(5_000, None, &procs(&[(9, "portal2.exe")])));
        assert_eq!(t.state(), &TrackState::Tracking { pid: 9 });
    }

    /// A short-lived launch with nothing to hunt for: legacy kept running
    /// forever and never said why. ksx keeps running and says why.
    #[test]
    fn a_launcher_with_no_process_name_admits_it_cannot_follow() {
        let mut t = tracker(None);
        t.started_with_handle(0);
        assert_eq!(
            t.observe(obs(500, Some(false), &[])),
            TrackOutcome::GaveUp(Unresolvable::NoProcessName)
        );
        assert_eq!(
            t.observe(obs(600, Some(false), &[])),
            TrackOutcome::Watching,
            "the session survives — only the escapes end it"
        );
    }

    /// Losing the handle is not evidence of an exit.
    #[test]
    fn an_unknown_liveness_answer_keeps_the_session_running() {
        let mut t = tracker(None);
        t.started_with_handle(0);
        assert_eq!(t.observe(obs(50_000, None, &[])), TrackOutcome::Watching);
        assert_eq!(t.state(), &TrackState::Running);
    }

    /// A configurable grace really is configurable — a cabinet with a slow
    /// launcher can be told to wait longer without a rebuild of the logic.
    #[test]
    fn the_policy_is_tunable() {
        let mut t = GameTracker::new(TrackPolicy {
            process_name: Some("slow.exe".into()),
            launcher_threshold_ms: 1_000,
            handoff_grace_ms: 5_000,
            exit_confirmations: 1,
        });
        t.started_with_handle(0);
        assert_eq!(
            t.observe(obs(1_001, Some(false), &[])),
            TrackOutcome::GameExited
        );

        let mut t = GameTracker::new(TrackPolicy {
            process_name: Some("slow.exe".into()),
            launcher_threshold_ms: 1_000,
            handoff_grace_ms: 5_000,
            exit_confirmations: 1,
        });
        t.started_with_handle(0);
        t.observe(obs(900, Some(false), &[]));
        assert_eq!(
            t.observe(obs(5_900, None, &[])),
            TrackOutcome::GaveUp(Unresolvable::HandoffTimedOut)
        );
    }
}
