//! ksx-games — game/profile launching and exit detection.
//!
//! Successor to the legacy games list + `game=` CLI autostart. `games.toml`
//! itself lives in `ksx-config`; this crate is the part with behaviour:
//!
//! - [`profile`] — a `[[game]]` entry resolved into something launchable
//!   (an executable ksx can hold a handle to, or a `steam://`-style URL it
//!   cannot), plus the checks that can be made *before* a pad is plugged.
//! - [`tracker`] — the pure exit-detection state machine: legacy's >3-second
//!   launcher rule, plus a `LauncherHandoff` state that follows a launcher to
//!   the process it started. Clock injected, no I/O, fully CI-testable.
//! - [`session`] — the thin layer that feeds the tracker real facts, behind a
//!   [`session::GameHost`] trait with a scriptable [`session::FakeHost`].
//!
//! # Two rules that are not negotiable
//!
//! **Launch last.** The game starts only after the pads are plugged and capture
//! is armed. Started earlier, it enumerates zero controllers and never looks
//! again.
//!
//! **Never kill the game.** ksx starts it as a convenience; it does not own it.
//! There is no kill primitive anywhere in this crate or in
//! `ksx_platform::process` — see that module's no-kill policy.

pub mod profile;
pub mod session;
pub mod tracker;

pub use profile::{preflight, LaunchSpec, LaunchTarget, PreflightError};
pub use session::{GameHost, GameSession, LaunchFailure, RealHost, StartReport};
pub use tracker::{
    GameTracker, TrackOutcome, TrackPolicy, TrackState, Unresolvable, DEFAULT_HANDOFF_GRACE_MS,
};
