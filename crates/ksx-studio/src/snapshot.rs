//! The status contract — **now `ksx-api`'s** — plus the two PAGE payloads,
//! which stay here because they are this page's shape and nobody else's.
//!
//! `StatusSource` and its snapshots moved to `ksx-api` for the reason
//! docs/M9-DECISION.md §6 gives: the read side must be satisfiable with NO
//! daemon running (ksx-app's collectors read the config store and the platform
//! directly), and it is consumed by surfaces that do not link this crate. What
//! remains below is the part that genuinely belongs to a web page: the
//! envelope the islands protocol serializes into the document and the poller
//! reads back.

pub use ksx_api::status::*;

use serde::{Deserialize, Serialize};

/// The one live-data shape: what `GET /api/status` serves AND what the page
/// embeds (render.rs serializes it into the `__ksx-payload` script block).
/// One struct, one serializer — the client seeds its signals from the block
/// and then overwrites the SAME signals from `/api/status` every 2 s, so the
/// two must never drift. `render.rs` has the parity test;
/// `studio-ui/src/StatusIsland.ts` mirrors the field names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusPayload {
    pub snapshot: StatusSnapshot,
    pub session: crate::control::SessionView,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/status` — a poll is not an action — and `Some` only in the
    /// page-render props, where the client shows it once and clears it.
    pub flash: Option<String>,
}

/// What `GET /api/map` serves AND what the mapper island's props carry — the
/// same one-struct-one-serializer rule as [`StatusPayload`], parity pinned in
/// `render_map.rs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapPayload {
    pub mapper: MapperSnapshot,
    pub session: crate::control::SessionView,
    /// Where the daemon's learner stands (also tells the page whether
    /// learning is possible at all).
    pub learn: crate::control::LearnView,
    /// Slot number selected for the SSR paint (`/map?slot=N`, defaulting to
    /// the first slot). The client keeps its own selection afterwards.
    pub selected: u8,
    /// The selected slot's macros, read per request like everything else.
    #[serde(default)]
    pub macros: MacroSnapshot,
    /// Macro name selected for the SSR paint (`/map?macro=NAME`), empty for
    /// "the first one". Same contract as [`selected`](Self::selected): it
    /// drives the server paint, the client keeps its own choice afterwards —
    /// and because the macro tabs are anchors, a page with no JavaScript can
    /// still walk through every macro the preset defines.
    #[serde(default)]
    pub macro_selected: String,
}

/// What `GET /api/devices` serves AND what the `/devices` island's props
/// carry — the same one-struct-one-serializer rule as [`StatusPayload`],
/// parity pinned in `render_devices.rs`.
///
/// The scan and the reason it is missing are SEPARATE fields on purpose. An
/// empty `DeviceScanView` is a real answer on a machine with nothing plugged
/// in; it is also what a refusal would degrade to if the two were collapsed,
/// and "no boards found" on a machine with four boards is the worst possible
/// lie for this page to tell. [`Self::unavailable`] non-empty means the view
/// below is not a reading of anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesPayload {
    pub scan: ksx_api::DeviceScanView,
    /// Session state, for the header pill and for the one caution this page
    /// owes a running cabinet: a `[[device]]` edit lands in `config.toml`, and
    /// the session already running keeps the devices it opened until it is
    /// restarted.
    pub session: crate::control::SessionView,
    /// Empty when the scan answered. Otherwise the refusal, verbatim.
    #[serde(default)]
    pub unavailable: String,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/devices` — a poll is not an action.
    pub flash: Option<String>,
}

/// What `GET /api/profiles` serves AND what the Profiles island's props carry
/// — the same one-struct-one-serializer rule as [`StatusPayload`], parity
/// pinned in `render_profiles.rs`.
///
/// Two machine views side by side rather than one flattened shape, because
/// they are two backend reads with two failure modes: games.toml can be
/// unreadable while the presets folder is fine, and the page has to be able to
/// say which. [`notes`](Self::notes) is where either read's complaint lands —
/// a refusal renders as a note beside an empty list, never as an empty list on
/// its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesPayload {
    pub profiles: ksx_api::ProfilesView,
    pub presets: ksx_api::PresetsView,
    pub session: crate::control::SessionView,
    /// Anything either read had to say out loud, including a whole read that
    /// refused. Rendered; never swallowed.
    #[serde(default)]
    pub notes: Vec<String>,
    /// One-shot action feedback (the `?flash=` query). Always `None` from
    /// `/api/profiles` — a poll is not an action.
    pub flash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload's field names are client contract (StatusIsland.ts reads
    /// them); pin the envelope on top of the snapshot's own pinned names
    /// (`ksx-api`'s `status` module keeps those).
    #[test]
    fn payload_serializes_to_stable_envelope_field_names() {
        let payload = StatusPayload {
            snapshot: StatusSnapshot {
                generated_at: "2026-08-04 12:00:00 UTC".into(),
                ..StatusSnapshot::default()
            },
            session: crate::control::SessionView {
                reachable: true,
                running: false,
                line: "idle — daemon reachable".into(),
                profile: None,
            },
            flash: None,
        };
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            v.pointer("/snapshot/generated_at"),
            Some(&serde_json::json!("2026-08-04 12:00:00 UTC"))
        );
        assert_eq!(
            v.pointer("/session/reachable"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/session/running"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            v.pointer("/session/line"),
            Some(&serde_json::json!("idle — daemon reachable"))
        );
        // `flash` is always present (null when absent) — the client types it
        // `string | null`, not optional.
        assert_eq!(v.pointer("/flash"), Some(&serde_json::json!(null)));
    }
}
