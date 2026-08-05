//! The data contract between a status provider (ksx-app) and the rendered
//! page. Deliberately tiny and presentation-shaped: the provider composes the
//! human-readable lines, this crate only places them into the page.

use serde::{Deserialize, Serialize};

/// Supplies a fresh [`StatusSnapshot`] per request. Implementations live with
/// the caller (ksx-app builds one from the existing collectors); this crate
/// never gathers machine state itself.
pub trait StatusSource: Send + Sync {
    fn snapshot(&self) -> StatusSnapshot;
}

/// Everything the cabinet status sections show. Point-in-time by design: a
/// snapshot is re-read on every request and never claims to be live session
/// data — that is the session panel's job, over [`crate::ControlSource`] and
/// the daemon pipe.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    /// When the snapshot was taken, already formatted (e.g. RFC-3339-ish UTC).
    pub generated_at: String,
    /// One line describing ViGEmBus (installed / service state / version).
    pub vigem: String,
    /// One line describing the Interception keyboard filter.
    pub interception: String,
    /// A ksx process other than the caller is alive right now.
    pub daemon_running: bool,
    /// The evidence behind [`daemon_running`](Self::daemon_running).
    pub daemon_detail: String,
    /// One line describing the logon-task registration.
    pub autostart: String,
    /// Pads the bus is exposing right now.
    pub pads: Vec<PadRow>,
    /// Profiles found in games.toml.
    pub profiles: Vec<ProfileRow>,
    /// The config root the profiles were read from.
    pub config_root: String,
}

impl StatusSnapshot {
    /// A snapshot whose every line carries `reason` — what renders when the
    /// provider itself failed (e.g. panicked). Honest by construction: no
    /// field keeps a stale or default-looking value.
    pub fn degraded(reason: &str) -> Self {
        Self {
            generated_at: "(unavailable)".to_owned(),
            vigem: reason.to_owned(),
            interception: reason.to_owned(),
            daemon_running: false,
            daemon_detail: reason.to_owned(),
            autostart: reason.to_owned(),
            pads: Vec::new(),
            profiles: Vec::new(),
            config_root: "(unavailable)".to_owned(),
        }
    }
}

/// One virtual pad currently exposed by ViGEmBus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadRow {
    /// Human persona label, e.g. "Xbox 360 pad".
    pub persona: String,
    /// PnP instance id of the bus child.
    pub instance: String,
}

/// One games.toml profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRow {
    pub title: String,
    /// Composed detail line (path, slot count, …).
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "2026-08-04 12:00:00 UTC".into(),
            vigem: "installed — service running — driver v1.21.442.0".into(),
            interception: "installed — keyboard filter active".into(),
            daemon_running: true,
            daemon_detail: "ksx.exe alive (pid 4242)".into(),
            autostart: "registered — ksx daemon".into(),
            pads: vec![PadRow {
                persona: "Xbox 360 pad".into(),
                instance: "USB\\VID_045E&PID_028E\\2&AA&0&01".into(),
            }],
            profiles: vec![ProfileRow {
                title: "Street Fighter".into(),
                detail: "C:\\games\\sf.exe — 2 slots".into(),
            }],
            config_root: "C:\\Users\\arcade\\AppData\\Roaming\\ksx".into(),
        }
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let snap = sample();
        let json = serde_json::to_string(&snap).unwrap();
        let back: StatusSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }

    #[test]
    fn snapshot_serializes_to_stable_field_names() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(
            v.pointer("/vigem"),
            Some(&serde_json::json!(
                "installed — service running — driver v1.21.442.0"
            ))
        );
        assert_eq!(v.pointer("/daemon_running"), Some(&serde_json::json!(true)));
        assert_eq!(
            v.pointer("/pads/0/persona"),
            Some(&serde_json::json!("Xbox 360 pad"))
        );
        assert_eq!(
            v.pointer("/profiles/0/title"),
            Some(&serde_json::json!("Street Fighter"))
        );
    }

    #[test]
    fn degraded_snapshot_carries_the_reason_everywhere_visible() {
        let snap = StatusSnapshot::degraded("collector panicked");
        for line in [
            &snap.vigem,
            &snap.interception,
            &snap.daemon_detail,
            &snap.autostart,
        ] {
            assert_eq!(line, "collector panicked");
        }
        assert!(!snap.daemon_running);
        assert!(snap.pads.is_empty() && snap.profiles.is_empty());
    }
}
