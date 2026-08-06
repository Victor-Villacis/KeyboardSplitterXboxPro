//! The READ side: everything a surface can show with **no daemon running**.
//!
//! Kept apart from [`crate::ControlSource`] on purpose, and the split is
//! load-bearing (docs/M9-DECISION.md §6): ksx-app's collectors read the config
//! store and the platform directly, which is why `ksx studio` renders a
//! read-only mapper behind the "No daemon" banner instead of an error page.
//! Merging the two traits would delete that path — a surface that needed a
//! write channel to render a preset would have nothing to render when the
//! channel is what broke.
//!
//! These types are presentation-shaped by design: the provider composes the
//! human-readable lines, a surface only places them. That is what lets one
//! contract serve a web page, a native window and an MCP tool without any of
//! them re-deriving a fourth wording for "installed — service running".

use serde::{Deserialize, Serialize};

/// Supplies a fresh [`StatusSnapshot`] per request. Implementations live with
/// the caller (ksx-app builds one from the existing collectors); this crate
/// never gathers machine state itself.
pub trait StatusSource: Send + Sync {
    fn snapshot(&self) -> StatusSnapshot;

    /// The mapper page's data: slots with their presets and bindings. The
    /// default is an honest "no data" so existing sources keep compiling;
    /// ksx-app overrides it with the config-store reader.
    fn mapper(&self) -> MapperSnapshot {
        MapperSnapshot::unavailable("this status source supplies no mapper data")
    }

    /// One preset's `[macros]` tables — the macro editor's whole read side
    /// (docs/INPUT-TRANSFORMS.md §1c).
    ///
    /// A separate method rather than a field on [`MapperSlot`] for one
    /// deliberate reason: [`MapperSlot`] is built by the CALLER (ksx-app's
    /// `collect_mapper`), so a new required field would be a compile break in
    /// a crate this seam is not allowed to reach into. A defaulted trait
    /// method is the same shape [`StatusSource::mapper`] itself used to grow
    /// in, and it lets the page say — in words, on screen — exactly which
    /// provider call is still missing instead of rendering an empty grid that
    /// looks like "this preset has no macros".
    fn macros(&self, _preset: &str) -> MacroSnapshot {
        MacroSnapshot::unavailable("this status source supplies no macro data")
    }
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

/// What the mapper page maps: slots (from `config.toml` `[[slot]]` entries or
/// a games.toml profile — the provider says which in [`source`](Self::source))
/// with their presets' current bindings. Point-in-time like
/// [`StatusSnapshot`]; re-read per request / per poll, which is exactly how a
/// fresh `ksx map` write becomes a fresh zone tag.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapperSnapshot {
    pub generated_at: String,
    /// Where the slots came from, e.g. `slots of profile "Steam" (games.toml)`
    /// — or, with an empty `slots`, why there is nothing to map.
    pub source: String,
    pub config_root: String,
    pub slots: Vec<MapperSlot>,
}

impl MapperSnapshot {
    /// No mappable slots; `reason` renders where the slot strip would be.
    pub fn unavailable(reason: &str) -> Self {
        Self {
            generated_at: "(unavailable)".to_owned(),
            source: reason.to_owned(),
            config_root: "(unavailable)".to_owned(),
            slots: Vec::new(),
        }
    }
}

/// One mappable slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapperSlot {
    pub number: u8,
    /// Machine persona id: `"xbox360"` or `"playstation"` (picks the art).
    pub persona: String,
    /// Human persona label, e.g. `"Xbox 360"`.
    pub persona_label: String,
    /// Preset name the slot binds (`preset` in the slot entry).
    pub preset: String,
    /// Keyboard alias or hardware id, `"(any)"` when unassigned.
    pub keyboard: String,
    /// Canonical function name → bound key names (the inert `"None"` filtered
    /// out — an unbound function is an EMPTY list here, so the page renders
    /// an honest "unbound" tag instead of the placeholder's name).
    pub bindings: std::collections::BTreeMap<String, Vec<String>>,
    /// Human label of the NEWEST `<preset>.toml.bak-YYYYMMDD-HHMMSS` on disk,
    /// e.g. `2026-08-05 14:32:07 UTC`. `None` when this preset has never been
    /// restored, in which case the mapper hides the "Restore backup from …"
    /// button instead of offering a road home that does not exist.
    ///
    /// Read straight from the config store, not from the daemon: the label is
    /// still true (and still worth showing) when nothing answers the pipe.
    #[serde(default)]
    pub backup: Option<String>,
    /// Canonical function name → its AUTO-FIRE rate in hertz, as authored
    /// (docs/INPUT-TRANSFORMS.md §3). Absent = the control does not auto-fire,
    /// which is every control of every preset written before turbo existed.
    ///
    /// Keyed by FUNCTION, not by key, because that is what turbo is a property
    /// of: several keys on one control share one clock, so there is exactly
    /// one rate per row of the legend.
    #[serde(default)]
    pub turbo: std::collections::BTreeMap<String, u32>,
    /// This slot says `macros = "off"` (config.toml or the games.toml profile)
    /// — the TOURNAMENT SWITCH. Every macro of its preset is silenced whatever
    /// each one's own `enabled` says; nothing is deleted.
    ///
    /// The negative again, and for the same reason as
    /// [`MacroView::disabled`]: `false` is the ordinary case, so every payload
    /// that predates the switch still means "macros run".
    #[serde(default)]
    pub macros_off: bool,
}

/// The macro editor's read side: every `[macros.<name>]` table of ONE preset,
/// in the shape the file spells them.
///
/// Deliberately the FILE's shape (`ms` / `frames` kept apart, `allow_short` as
/// written) rather than a resolved one: the page's whole job here is to show
/// the author what the file says and to emit a TOML block they can paste back,
/// so a duration authored in frames has to survive the round trip as frames
/// (`ksx_config::MacroStepFile`, and `ksx_core::StepDuration::Frames`'s "an
/// ergonomic unit, and only that").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroSnapshot {
    /// The provider actually read a preset. `false` means "nobody has told
    /// this page anything about macros" — which the editor says out loud,
    /// because it is NOT the same fact as "this preset defines none".
    pub available: bool,
    /// Why not, when [`available`](Self::available) is false. Rendered.
    pub reason: String,
    /// The preset these came from — the `--preset` of every CLI line the page
    /// prints.
    pub preset: String,
    pub macros: Vec<MacroView>,
}

impl MacroSnapshot {
    /// No macro data at all; `reason` renders where the macro list would be.
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            reason: reason.to_owned(),
            preset: String::new(),
            macros: Vec::new(),
        }
    }

    /// A preset that was read successfully — `macros` may still be empty, and
    /// that emptiness is now a FACT rather than an absence of information.
    pub fn read(preset: &str, macros: Vec<MacroView>) -> Self {
        Self {
            available: true,
            reason: String::new(),
            preset: preset.to_owned(),
            macros,
        }
    }
}

/// One `[macros.<name>]` table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroView {
    /// The table name, which is also half of the `macro.<name>` function.
    pub name: String,
    pub steps: Vec<MacroStepView>,
    /// `"finish"` | `"abort"` — as `ksx_core::OnRelease::as_str` spells it.
    pub on_release: String,
    /// `"ignore"` | `"restart"`.
    pub retrigger: String,
    /// `"none"` | `"any-input"` | `"opposing"`.
    pub interrupt: String,
    /// `"once"` | `"while-held"` | `"turbo"` — what the END of a run does
    /// while the trigger is still held (docs/INPUT-TRANSFORMS.md §1c).
    #[serde(default)]
    pub repeat: String,
    /// The turbo rate as AUTHORED, in full cycles a second. Exactly one of
    /// this and `gap_ms` is set in a valid file; the editor shows whichever
    /// the file used rather than converting behind the author's back.
    #[serde(default)]
    pub turbo_hz: Option<u32>,
    /// The same number said the other way: the neutral window BETWEEN runs.
    #[serde(default)]
    pub gap_ms: Option<u32>,
    /// Key names that START this macro (the `macro.<name>` rows in
    /// `[bindings]`). Many keys → one macro is native, like any binding.
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Does this macro RUN? Spelled as the negative so that the default
    /// (`false`) is the ordinary case, which keeps every existing payload and
    /// every older client reading exactly what it read before.
    ///
    /// A disabled macro keeps its steps and its trigger row and never runs, so
    /// the card must say so loudly: it is the one state where the whole card
    /// describes something that will not happen.
    #[serde(default)]
    pub disabled: bool,
}

/// One step: what it HOLDS, and for how long, in the unit the file used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroStepView {
    /// Canonical function names (`"dpad.down"`, `"A"`). Empty is legal and
    /// means a deliberate neutral gap.
    #[serde(default)]
    pub hold: Vec<String>,
    /// Exactly one of these two is `Some` in a valid file; both, or neither,
    /// is a fault the editor flags rather than resolves.
    #[serde(default)]
    pub ms: Option<u32>,
    #[serde(default)]
    pub frames: Option<u32>,
    /// "I know this is shorter than a 60 Hz poller can see."
    #[serde(default)]
    pub allow_short: bool,
}

impl MacroStepView {
    /// The step as the preset FILE spells it — the same serde type the store
    /// reads and writes.
    ///
    /// One conversion, in one place, in both directions ([`Self::from_file`]):
    /// this is the hop where a hand-written struct literal used to be able to
    /// forget a member, and where `allow_short` would then quietly become
    /// `false` for an author who had asked for it.
    pub fn to_file(&self) -> ksx_config::MacroStepFile {
        ksx_config::MacroStepFile {
            hold: self.hold.clone(),
            ms: self.ms,
            frames: self.frames,
            allow_short: self.allow_short,
        }
    }

    /// The inverse of [`Self::to_file`].
    pub fn from_file(step: &ksx_config::MacroStepFile) -> Self {
        Self {
            hold: step.hold.clone(),
            ms: step.ms,
            frames: step.frames,
            allow_short: step.allow_short,
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

    /// A step's two spellings are ONE conversion each way, and every field
    /// survives both — including the two that a struct literal has historically
    /// been able to drop (`frames`, `allow_short`).
    #[test]
    fn a_macro_step_round_trips_through_the_files_own_type() {
        for step in [
            MacroStepView {
                hold: vec!["dpad.down".into(), "dpad.right".into()],
                ms: Some(50),
                frames: None,
                allow_short: false,
            },
            MacroStepView {
                hold: vec!["A".into()],
                ms: None,
                frames: Some(3),
                allow_short: true,
            },
            MacroStepView::default(),
        ] {
            assert_eq!(MacroStepView::from_file(&step.to_file()), step);
        }
    }
}
