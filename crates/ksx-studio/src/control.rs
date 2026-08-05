//! The control contract between the session panel and whoever can actually
//! reach a daemon. Same shape as [`crate::snapshot::StatusSource`]: this
//! crate renders and routes, the caller (ksx-app) talks to the daemon pipe.
//! Nothing here can touch capture, output, or a live session — an
//! implementation is a client of the daemon's control channel, never a
//! second control loop.

/// Performs the session verbs. Implementations live with the caller; ksx-app
/// wraps each method around one `\\.\pipe\ksx-daemon` request, which enqueues
/// the same `DaemonCommand` the tray menu produces (docs/CONTROL-SURFACE.md).
///
/// The mapper verbs (`learn_*`, `bind`) default to an honest "unavailable" so
/// pre-mapper implementations keep compiling; ksx-app overrides them with the
/// pipe's `learn-key`/`learn-poll`/`learn-cancel`/`map` requests. They are
/// pipe verbs first, GUI affordances second — the standing no-GUI-only-paths
/// rule.
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

    /// Ask the daemon to listen for the next panel key (pipe `learn-key`).
    fn learn_start(&self) -> LearnView {
        LearnView::unavailable("this control source has no learner")
    }
    /// Where the learner stands (pipe `learn-poll`).
    fn learn_poll(&self) -> LearnView {
        LearnView::unavailable("this control source has no learner")
    }
    /// Stop listening (pipe `learn-cancel`).
    fn learn_cancel(&self) -> LearnView {
        LearnView::unavailable("this control source has no learner")
    }
    /// Write one binding (pipe `map`).
    fn bind(&self, _request: &BindRequest) -> BindOutcome {
        BindOutcome::failed("this control source cannot write bindings")
    }
    /// Restore a whole preset (pipe `map-restore`): `mode` is `"defaults"`
    /// or `"session-backup"`. `Ok` is the daemon's confirmation line.
    fn restore(&self, _preset: &str, _mode: &str) -> Result<String, String> {
        Err("this control source cannot restore presets".to_owned())
    }
}

/// The daemon learner's state, presentation-shaped from the pipe's
/// `learn-key`/`learn-poll` response. `state` is the pipe's word verbatim
/// (`listening`, `hit`, `timeout`, `cancelled`, `failed`, `idle`) plus this
/// crate's `"unavailable"` for "no daemon / daemon predates the verb".
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LearnView {
    pub ok: bool,
    pub state: String,
    /// Countdown for the mapper's visible timer (the PadForge gap the design
    /// closes) — `Some` only while listening.
    pub remaining_ms: Option<u64>,
    /// Device instance path of the hit.
    pub device: Option<String>,
    /// Learned key name (a spelling `ksx map --key` accepts).
    pub key: Option<String>,
    pub error: Option<String>,
}

impl LearnView {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            state: "unavailable".to_owned(),
            remaining_ms: None,
            device: None,
            key: None,
            error: Some(reason.into()),
        }
    }
}

/// One mapper write, straight onto the pipe `map` verb's fields.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BindRequest {
    pub preset: String,
    pub function: String,
    /// `None` = clear.
    pub key: Option<String>,
    #[serde(default)]
    pub force: bool,
    /// Bounce a running session onto the new binding (a clean daemon Reload).
    #[serde(default)]
    pub reload: bool,
}

/// The pipe `map` response, typed. `conflicts` is filled both on refusal
/// (`ok: false`, `code: "conflict"` — the caller decides) and on a forced
/// write (informational: cross-profile bindings that still exist).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BindOutcome {
    pub ok: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub code: Option<String>,
    pub conflicts: Vec<BindConflict>,
    pub reloaded: bool,
}

impl BindOutcome {
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// One conflicting binding, as the pipe reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BindConflict {
    /// `"preset"` (same preset — a force steals it) or `"profile"` (another
    /// slot's preset — never auto-edited).
    pub scope: String,
    pub preset: String,
    pub function: String,
    pub profile: Option<String>,
    pub slot: Option<u8>,
}

impl BindConflict {
    /// The dialog line: `G is "IPAC P2"'s A (slot 2 of "Steam")`.
    pub fn describe(&self, key: &str) -> String {
        if self.scope == "preset" {
            format!("{key} is already this preset's {}", self.function)
        } else {
            let where_ = match (&self.profile, self.slot) {
                (Some(profile), Some(slot)) => format!(" (slot {slot} of \"{profile}\")"),
                (Some(profile), None) => format!(" (\"{profile}\")"),
                _ => String::new(),
            };
            format!("{key} is \"{}\"'s {}{where_}", self.preset, self.function)
        }
    }
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
