//! `ksx run --game <Title>`: the [`SessionHook`] that launches the profile's
//! game and ends the session when it exits.
//!
//! All the policy is in `ksx-games` (pure, clock-injected, fully tested there).
//! This file is the adapter: it turns a [`GameSession`] into the three
//! lifecycle callbacks the supervisor knows about, and owns the wording the
//! user sees.
//!
//! # Where the launch happens, and why it matters
//!
//! [`SessionHook::started`] is called by the supervisor *after* the pads are
//! plugged, their XInput user indices are resolved, capture is running and
//! blocking has been applied. That ordering is the entire reason this is a hook
//! rather than a call in `run()`: a game started before the pads exist
//! enumerates zero controllers and never asks again.
//!
//! # What it never does
//!
//! Kill the game. [`SessionHook::finished`] runs on every path out — clean
//! stop, `LeftCtrl ×5`, `Ctrl+Alt+Del`, watchdog, panic — and all it does is
//! stop *watching*. If you stop emulation while the game is up, the game keeps
//! running and your keyboard starts typing into it again.

use std::io::Write;
use std::path::PathBuf;

use ksx_games::session::{GameHost, GameSession};
use ksx_games::{LaunchSpec, TrackOutcome, TrackState};

use super::supervisor::{HookStop, SessionHook};

/// Launches and follows one game for the lifetime of a session.
pub struct GameHook<H: GameHost + Send> {
    session: GameSession<H>,
    games_toml: PathBuf,
    /// Set once the game is known to have exited, so `finished` can say so.
    ended: bool,
}

impl<H: GameHost + Send> GameHook<H> {
    pub fn new(spec: LaunchSpec, host: H, games_toml: PathBuf) -> Self {
        Self {
            session: GameSession::new(spec, host),
            games_toml,
            ended: false,
        }
    }
}

impl<H: GameHost + Send> SessionHook for GameHook<H> {
    fn started(&mut self, out: &mut dyn Write) -> Result<(), String> {
        let title = self.session.spec().title.clone();
        let what = self.session.spec().target.describe();
        let report = self
            .session
            .start(&self.games_toml)
            .map_err(|err| err.message)?;

        let _ = match report.pid {
            Some(pid) => writeln!(out, "launched '{title}': {what} (pid {pid})"),
            None => writeln!(out, "launched '{title}': {what}"),
        };
        for warning in &report.warnings {
            // Both channels on purpose: the console is what a person is looking
            // at, and tracing is what a headless cabinet's log file keeps.
            tracing::warn!(%title, "game profile cannot detect its own exit");
            let _ = writeln!(out, "{warning}");
        }
        let _ = out.flush();
        Ok(())
    }

    fn poll(&mut self, out: &mut dyn Write) -> Option<HookStop> {
        match self.session.poll() {
            TrackOutcome::Watching => None,
            TrackOutcome::GameExited => {
                self.ended = true;
                let _ = writeln!(
                    out,
                    "'{}' exited — stopping emulation",
                    self.session.spec().title
                );
                let _ = out.flush();
                Some(HookStop::GameExited)
            }
            TrackOutcome::GaveUp(reason) => {
                // NOT a stop. ksx has lost the thread of what the game is
                // doing, which is a reason to say so — never a reason to yank
                // the pads out from under someone who is still playing.
                tracing::warn!(
                    reason = reason.code(),
                    "cannot detect this game's exit; session runs until an escape"
                );
                let _ = writeln!(
                    out,
                    "{}",
                    self.session.gave_up_message(reason, &self.games_toml)
                );
                let _ = out.flush();
                None
            }
        }
    }

    fn finished(&mut self, out: &mut dyn Write) {
        if self.ended {
            return;
        }
        // Emulation is stopping for some other reason while the game is (as far
        // as we know) still up. Say what that means, because it is not obvious:
        // the game does not close, and the keyboard goes back to typing into it.
        let _ = writeln!(
            out,
            "note: '{}' was not closed by ksx — ksx never kills a game it \
             started. It keeps running, and your keyboard now types into it \
             normally.",
            self.session.spec().title
        );
        let _ = out.flush();
    }

    fn status_line(&self) -> Option<String> {
        let title = &self.session.spec().title;
        Some(match self.session.state() {
            TrackState::Idle => format!("'{title}': not started"),
            TrackState::Running => format!("'{title}': running"),
            TrackState::LauncherHandoff { .. } => {
                format!("'{title}': launcher handed off, looking for the game")
            }
            TrackState::Tracking { pid } => format!("'{title}': running (pid {pid})"),
            TrackState::Exited => format!("'{title}': exited"),
            TrackState::Unresolvable(reason) => {
                format!(
                    "'{title}': running, exit not detectable ({})",
                    reason.code()
                )
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::GameEntry;
    use ksx_games::session::FakeHost;

    fn entry(path: &str, process_name: Option<&str>) -> GameEntry {
        GameEntry {
            title: "Portal 2".into(),
            notes: String::new(),
            path: path.into(),
            arguments: String::new(),
            process_name: process_name.map(str::to_owned),
            launcher_grace_ms: None,
            block_keyboards: ksx_core::Blocking::Whole,
            block_mice: false,
            slots: Vec::new(),
        }
    }

    fn hook(path: &str, process_name: Option<&str>, host: FakeHost) -> GameHook<FakeHost> {
        GameHook::new(
            LaunchSpec::from_entry(&entry(path, process_name)),
            host,
            PathBuf::from(r"C:\cfg\ksx\games.toml"),
        )
    }

    #[test]
    fn a_normal_session_reports_the_launch_and_then_the_exit() {
        let mut out: Vec<u8> = Vec::new();
        let mut h = hook(r"C:\g\portal2.exe", None, FakeHost::running(4242));
        h.started(&mut out).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("launched 'Portal 2'"), "{text}");
        assert!(text.contains("pid 4242"), "{text}");

        assert_eq!(h.poll(&mut out), None);
        assert_eq!(h.status_line().unwrap(), "'Portal 2': running");

        h.session_host_mut().alive = Some(false);
        // Past the launcher grace: this exit is the game's own, not a hand-off.
        h.session_host_mut().now_ms = ksx_games::DEFAULT_LAUNCHER_GRACE_MS + 1;
        assert_eq!(h.poll(&mut out), Some(HookStop::GameExited));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("exited — stopping emulation"), "{text}");
    }

    /// The launch failed after the pads were up: the hook reports it, and the
    /// supervisor's contract turns that into exit 3.
    #[test]
    fn a_launch_failure_is_reported_as_an_error_string() {
        let mut out: Vec<u8> = Vec::new();
        let mut h = hook(
            r"C:\g\portal2.exe",
            None,
            FakeHost::failing("Access is denied. (os error 5)"),
        );
        let err = h.started(&mut out).unwrap_err();
        assert!(err.contains("Access is denied"), "{err}");
    }

    /// `finished` on a session the player is still in must explain what just
    /// happened to their keyboard.
    #[test]
    fn stopping_while_the_game_is_up_explains_that_the_game_survives() {
        let mut out: Vec<u8> = Vec::new();
        let mut h = hook(r"C:\g\portal2.exe", None, FakeHost::running(1));
        h.started(&mut out).unwrap();
        out.clear();
        h.finished(&mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("never kills a game"), "{text}");
        assert!(text.contains("types into it"), "{text}");
    }

    /// ...and after a clean game exit there is nothing to explain.
    #[test]
    fn finishing_after_the_game_exited_says_nothing_extra() {
        let mut out: Vec<u8> = Vec::new();
        let mut h = hook(r"C:\g\portal2.exe", None, FakeHost::running(1));
        h.started(&mut out).unwrap();
        h.session_host_mut().alive = Some(false);
        h.session_host_mut().now_ms = ksx_games::DEFAULT_LAUNCHER_GRACE_MS + 1;
        assert_eq!(h.poll(&mut out), Some(HookStop::GameExited));
        out.clear();
        h.finished(&mut out);
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
    }

    /// A `steam://` profile with no `process_name`: warn loudly at launch, warn
    /// once more when detection formally gives up, and never stop the session.
    #[test]
    fn a_steam_profile_without_a_process_name_warns_and_keeps_running() {
        let mut out: Vec<u8> = Vec::new();
        let mut h = hook("steam://rungameid/620", None, FakeHost::running(0));
        h.started(&mut out).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("process_name"), "{text}");
        assert!(text.contains(r"C:\cfg\ksx\games.toml"), "{text}");

        for step in 1..200u64 {
            h.session_host_mut().now_ms = step * 1_000;
            assert_eq!(h.poll(&mut out), None, "the session must never be stopped");
        }
        assert!(h.status_line().unwrap().contains("not detectable"));
    }

    /// The *other* give-up: `process_name` was set, ksx hunted for the whole
    /// grace period and never saw it. The advice must be about the name being
    /// wrong — telling somebody to add a `process_name` they already added
    /// sends them looking in the one place that is already correct.
    #[test]
    fn a_handoff_that_times_out_blames_the_name_not_a_missing_key() {
        let mut out: Vec<u8> = Vec::new();
        // An exe profile: steam.exe hands off, and "portla2.exe" is a typo that
        // will never appear in the process list.
        let mut h = hook(r"C:\g\steam.exe", Some("portla2.exe"), FakeHost::running(9));
        h.started(&mut out).unwrap();
        assert!(
            !String::from_utf8(out.clone()).unwrap().contains("[WARN]"),
            "a profile WITH a process_name must not be warned at launch"
        );
        out.clear();

        // The launcher exits well inside the launcher grace -> hand-off.
        h.session_host_mut().alive = Some(false);
        h.session_host_mut().now_ms = 800;
        assert_eq!(h.poll(&mut out), None);

        // Nothing ever appears; the grace period expires.
        h.session_host_mut().now_ms = 800 + ksx_games::DEFAULT_HANDOFF_GRACE_MS;
        assert_eq!(
            h.poll(&mut out),
            None,
            "giving up on detection must never stop the session"
        );

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("portla2.exe"),
            "the name ksx actually looked for must be quoted back: {text}"
        );
        assert!(
            !text.contains("starts a URL"),
            "an .exe profile does not start a URL: {text}"
        );
        assert!(
            !text.contains("# <- add this line"),
            "process_name is already set; do not tell the user to add it: {text}"
        );
        assert!(text.contains("LeftCtrl x5"), "the way out must be stated");
    }

    /// The exact ordering guarantee, asserted where it is cheap to assert:
    /// nothing is launched until `started` is called.
    #[test]
    fn nothing_launches_before_started() {
        let mut out: Vec<u8> = Vec::new();
        let mut h = hook(r"C:\g\portal2.exe", None, FakeHost::running(1));
        for _ in 0..10 {
            assert_eq!(h.poll(&mut out), None);
        }
        assert!(
            h.session_host().launched.is_empty(),
            "a poll must never start a game"
        );
        h.started(&mut out).unwrap();
        assert_eq!(h.session_host().launched.len(), 1);
    }

    impl GameHook<FakeHost> {
        fn session_host(&self) -> &FakeHost {
            self.session.host()
        }
        fn session_host_mut(&mut self) -> &mut FakeHost {
            self.session.host_mut()
        }
    }
}
