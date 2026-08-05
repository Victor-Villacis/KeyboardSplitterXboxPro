//! The control contract between the session panel and whoever can actually
//! reach a daemon. Same shape as [`crate::snapshot::StatusSource`]: this
//! crate renders and routes, the caller (ksx-app) talks to the daemon pipe.
//! Nothing here can touch capture, output, or a live session — an
//! implementation is a client of the daemon's control channel, never a
//! second control loop.

/// Performs the session verbs. Implementations live with the caller; ksx-app
/// wraps each method around one `\\.\pipe\ksx-daemon` request, which enqueues
/// the same `DaemonCommand` the tray menu produces (docs/CONTROL-SURFACE.md).
pub trait ControlSource: Send + Sync {
    /// Where the daemon stands right now. Called per page load, alongside the
    /// status snapshot.
    fn session(&self) -> SessionView;
    /// Start emulation, optionally under a games.toml profile title. `Ok` is
    /// a human-readable confirmation, `Err` the daemon's (or the pipe's)
    /// refusal — both render as the post-redirect flash line.
    fn start(&self, profile: Option<&str>) -> Result<String, String>;
    fn stop(&self) -> Result<String, String>;
    fn reload(&self) -> Result<String, String>;
}

/// What the session panel needs to know, presentation-shaped like
/// [`crate::snapshot::StatusSnapshot`]: the provider composes the line, this
/// crate only places it and picks which controls to render. Serialized as
/// part of the `/api/status` payload and the island props (snapshot.rs
/// [`StatusPayload`](crate::snapshot::StatusPayload)) — field names are
/// client contract.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionView {
    /// A daemon control channel answered. `false` renders every control
    /// disabled with the reason — never hidden, never silently inert.
    pub reachable: bool,
    /// A session is running (or starting): Stop/Reload render instead of
    /// Start.
    pub running: bool,
    /// The one state line: "running — Street Fighter — 4 pad(s)", "idle", or
    /// why the daemon cannot be reached.
    pub line: String,
}

impl SessionView {
    /// The no-channel view, with `reason` as the state line.
    pub fn unreachable(reason: impl Into<String>) -> Self {
        Self {
            reachable: false,
            running: false,
            line: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_carries_the_reason_and_disables_everything() {
        let view = SessionView::unreachable("no pipe");
        assert!(!view.reachable);
        assert!(!view.running);
        assert_eq!(view.line, "no pipe");
    }
}
