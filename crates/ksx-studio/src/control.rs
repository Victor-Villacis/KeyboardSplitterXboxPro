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
    /// Write one binding (pipe `map`). `request.key == None` clears it.
    fn bind(&self, _request: &BindRequest) -> BindOutcome {
        BindOutcome::failed("this control source cannot write bindings")
    }

    /// Write the control's WHOLE key list — MANY KEYS → ONE CONTROL, the
    /// MAME-style OR-chain the engine has always executed
    /// (docs/INPUT-TRANSFORMS.md §1a: `A = ["S", "Enter"]`, press either).
    ///
    /// This is the seam the mapper's "Add another key" and per-key ✕ compute
    /// against: both are read-modify-write on a SET (current keys ∪ {k},
    /// current keys ∖ {k}), and the whole set is what gets written, so a
    /// caller never has to know how the writer spells the edit.
    ///
    /// **ksx-app overrides this**: the daemon's `map` verb takes a `"keys":
    /// [...]` list, so the whole set lands in ONE atomic write —
    /// replace-per-function with the list the caller computed, no
    /// read-modify-write and no half-applied edit when a write is refused.
    ///
    /// The DEFAULT implementation is the fallback for a control source that
    /// has only [`ControlSource::bind`] (one `"key"` per call): an empty set
    /// is a clear, a one-key set is an ordinary bind, and a two-key set has no
    /// wire shape at all there. Rather than write the first key and silently
    /// drop the rest — the Synapse-4 sin MAPPER-UX commandment 7 bans — it
    /// refuses in words that name the missing field.
    /// `turbo_hz` is the AUTO-FIRE rate (docs/INPUT-TRANSFORMS.md §3), and it
    /// has the same three states the `map` verb gives it: `None` leaves the
    /// control's existing rate alone (so "Add another key" cannot silently
    /// switch an auto-fire off), `Some(0)` clears it, `Some(n)` sets it. The
    /// default implementation below has nowhere to put it and says so rather
    /// than dropping it.
    fn bind_keys(
        &self,
        preset: &str,
        function: &str,
        keys: &[String],
        force: bool,
        reload: bool,
        turbo_hz: Option<u32>,
    ) -> BindOutcome {
        if turbo_hz.is_some() {
            return BindOutcome::failed(
                "this control source cannot set a turbo rate (the `map` verb's `turbo_hz` \
                 field is what carries it)",
            );
        }
        let one = |key: Option<String>| BindRequest {
            preset: preset.to_owned(),
            function: function.to_owned(),
            key,
            force,
            reload,
        };
        match keys {
            [] => self.bind(&one(None)),
            [only] => self.bind(&one(Some(only.clone()))),
            _ => BindOutcome::failed(multi_key_refusal(function, keys)),
        }
    }

    /// Restore a whole preset (pipe `map-restore`): `mode` is one of
    /// [`RESTORE_MODES`]. `Ok` is the daemon's confirmation line — which
    /// already names what was written and what was backed up first.
    fn restore(&self, _preset: &str, _mode: &str) -> Result<String, String> {
        Err("this control source cannot restore presets".to_owned())
    }
    /// Unbind every function of a preset, keeping the file structurally valid
    /// (pipe `map-clear-all`). A timestamped backup is taken first, like any
    /// other whole-preset write.
    fn clear_all(&self, _preset: &str) -> Result<String, String> {
        Err("this control source cannot clear presets".to_owned())
    }

    /// Write a preset's WHOLE `[macros.<name>]` table (pipe `map-macro`) — the
    /// macro editor's save.
    ///
    /// WHOLE-MACRO, not per-step, and for the same reason
    /// [`ControlSource::bind_keys`] is whole-list: the editor already holds
    /// the entire grid, so it can always send all of it, and a partial-step
    /// protocol would have to carry indices computed against a file that may
    /// have moved — one dropped message and the preset holds a sequence
    /// nobody authored. One table in, one answer out.
    ///
    /// Defaulted, like the mapper verbs, so a pre-macro control source keeps
    /// compiling and says so on screen instead of pretending to save.
    fn save_macro(&self, _request: &MacroWrite) -> MacroOutcome {
        MacroOutcome::failed("this control source cannot write macros")
    }
}

/// One whole-macro write, straight onto the pipe `map-macro` verb's fields.
///
/// `steps` reuses [`crate::MacroStepView`] — the SAME type the read side
/// serves — so what the editor was shown and what it saves are one shape, and
/// a duration authored in `frames` survives the round trip as frames.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MacroWrite {
    pub preset: String,
    /// The `[macros.<name>]` table. Matched case-insensitively against what is
    /// on disk; a table that already exists keeps its own spelling.
    pub name: String,
    #[serde(default)]
    pub steps: Vec<crate::snapshot::MacroStepView>,
    /// `"finish"` | `"abort"` — blank means the default, as the file's own
    /// omitted-field rule does.
    #[serde(default)]
    pub on_release: String,
    /// `"ignore"` | `"restart"`.
    #[serde(default)]
    pub retrigger: String,
    /// `"none"` | `"any-input"` | `"opposing"`.
    #[serde(default)]
    pub interrupt: String,
    /// `"once"` | `"while-held"` | `"turbo"` — blank means the default
    /// (`once`), as the file's own omitted-field rule does.
    #[serde(default)]
    pub repeat: String,
    /// The turbo rate, in the unit the author chose. Exactly one of these two,
    /// never both: they are two spellings of one number, and the daemon
    /// refuses a table that gives both rather than picking a winner.
    #[serde(default)]
    pub turbo_hz: Option<u32>,
    #[serde(default)]
    pub gap_ms: Option<u32>,
    /// DELETE the table (and the `macro.<name>` trigger rows that would
    /// otherwise dangle) instead of writing it. An explicit flag on purpose:
    /// an empty `steps` list is a REFUSAL, so an editor that lost its grid
    /// cannot delete the user's macro by omission.
    #[serde(default)]
    pub delete: bool,
    /// Does this macro RUN? Two meanings, decided by whether a BODY came with
    /// it — exactly as on the pipe verb this maps onto:
    ///
    /// - with `steps`: a field of the table being written, so an editor that
    ///   saves a disabled macro does not silently switch it back on;
    /// - `Some(_)` with NO `steps` and no `delete`: a TOGGLE. The table on disk
    ///   keeps every step and every policy and only the flag moves, which is
    ///   the whole promise of disabling instead of deleting.
    ///
    /// `None` on a write means "the default", i.e. enabled, like every other
    /// omitted field here.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Apply it to a running session. A macro body is a BINDING change, so the
    /// daemon swaps it into the live engine with the pads left plugged.
    #[serde(default)]
    pub reload: bool,
}

/// The pipe `map-macro` response, typed.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MacroOutcome {
    pub ok: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    /// Stable refusal word (`macro-invalid`, `unknown-preset`, `unknown-macro`…).
    pub code: Option<String>,
    /// The refusals one per row, so a page can list them instead of taking a
    /// sentence apart.
    #[serde(default)]
    pub problems: Vec<String>,
    /// ADVISORIES from a write that SUCCEEDED — a step below the sampling
    /// floor was raised, or runs as written and may be missed. Never silent.
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub deleted: bool,
    /// Does the table RUN now? (`true` for an ordinary write and for a delete.)
    #[serde(default)]
    pub enabled: bool,
    /// This write was ONLY `enabled` — the table's steps and policies were not
    /// touched, and the toast says so instead of claiming a rewrite.
    #[serde(default)]
    pub toggled: bool,
    /// Human label of the timestamped backup taken before the write — the undo
    /// this edit leaves behind. The mapper's existing "Restore backup from …"
    /// (mode `latest-backup`) is what walks it back; this field is what lets
    /// the toast say the road home exists without waiting for the next poll.
    pub backup: Option<String>,
    pub reloaded: bool,
}

impl MacroOutcome {
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// Why a multi-key write cannot land on a daemon whose only shape is one key
/// per call, in one sentence a page can flash verbatim. A current ksx daemon
/// never says this — it overrides [`ControlSource::bind_keys`] and writes the
/// list — so it is the honest answer for a control source that predates the
/// `"keys"` field, kept in one place rather than formatted at three.
pub fn multi_key_refusal(function: &str, keys: &[String]) -> String {
    format!(
        "{function} would have to hold {} keys at once ({}), and this daemon's map verb writes \
         ONE key per control — it replaces the whole binding, so the other keys would be lost. \
         Nothing was changed. (A \"keys\" list on the map verb would make this one atomic write; \
         until then, more than one key per control is a preset-file edit.)",
        keys.len(),
        keys.join(" · ")
    )
}

/// `keys` with `key` appended — the "Add another key" edit. Already there
/// (case-insensitively: the vocabulary is canonical, a hand-made POST is not)
/// = unchanged, so adding twice is not an error and never a duplicate row.
pub fn with_key(keys: &[String], key: &str) -> Vec<String> {
    let mut next: Vec<String> = keys.to_vec();
    if !next.iter().any(|k| k.eq_ignore_ascii_case(key)) {
        next.push(key.to_owned());
    }
    next
}

/// `keys` without `key` — the per-key ✕. Removing the last one leaves an
/// empty set, which [`ControlSource::bind_keys`] writes as an honest clear.
pub fn without_key(keys: &[String], key: &str) -> Vec<String> {
    keys.iter()
        .filter(|k| !k.eq_ignore_ascii_case(key))
        .cloned()
        .collect()
}

/// The three restore destinations, as the wire spells them. Validated at the
/// HTTP edge so a typo is a flashed error, not a daemon round-trip; the same
/// three strings are `ksx map --restore`'s values (mapping.rs `RestoreKind`).
pub const RESTORE_MODES: [&str; 3] = ["defaults", "session-backup", "latest-backup"];

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
/// write (informational: cross-slot bindings that still exist).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BindOutcome {
    pub ok: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub code: Option<String>,
    pub conflicts: Vec<BindConflict>,
    /// MULTI-BIND: the other controls of the SAME preset this key drives now
    /// that the write is done. Not a conflict and never was a refusal — the
    /// engine fires all of them (docs/INPUT-TRANSFORMS.md §1a). The mapper
    /// shows the same fact as the legend's "also A · B" badges, which
    /// `render_map::shared_labels` re-derives from disk; this is the write's
    /// own answer, so a caller can say it without waiting for the next poll.
    #[serde(default)]
    pub also_drives: Vec<String>,
    /// AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3): the rate this control now
    /// holds, as authored, and the rate it will actually DELIVER. The second
    /// is the one worth showing — a press AND a release must each survive a
    /// 60 Hz poll, so a request above ~15 Hz cannot be met however it is
    /// spelled, and the mapper says so rather than echoing the number back.
    #[serde(default)]
    pub turbo_hz: Option<u32>,
    #[serde(default)]
    pub turbo_effective_hz: Option<u32>,
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
    /// Always `"profile"` from a current daemon: another slot's preset, never
    /// auto-edited. (`"preset"` was the same-preset case, which is now a
    /// multi-bind reported as [`BindOutcome::also_drives`] instead of a
    /// conflict; the field stays because the wire word is contract.)
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
    /// The games.toml profile the daemon is pointed at, if any.
    ///
    /// Two jobs, both about not losing the user's place: the mapper's
    /// "Pause emulation & map" remembers it so "Resume emulation" starts the
    /// SAME thing back up, and the no-daemon banner prints the exact command
    /// (`ksx daemon --game "Steam"`) rather than a generic one that would
    /// start the wrong profile.
    #[serde(default)]
    pub profile: Option<String>,
}

impl SessionView {
    /// The no-channel view, with `reason` as the state line.
    pub fn unreachable(reason: impl Into<String>) -> Self {
        Self {
            reachable: false,
            running: false,
            line: reason.into(),
            profile: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The set arithmetic behind "Add another key" and the per-key ✕. Both
    /// are read-modify-write on the control's key LIST, so the order the file
    /// holds is preserved and an add that changes nothing is a no-op, not a
    /// second row.
    #[test]
    fn the_key_set_helpers_add_and_remove_exactly_one_key() {
        let keys = vec!["S".to_owned(), "Enter".to_owned()];
        assert_eq!(with_key(&keys, "G"), ["S", "Enter", "G"]);
        assert_eq!(with_key(&keys, "S"), ["S", "Enter"], "already there");
        assert_eq!(with_key(&keys, "enter"), ["S", "Enter"], "case-insensitive");
        assert_eq!(without_key(&keys, "S"), ["Enter"]);
        assert_eq!(without_key(&keys, "enter"), ["S"]);
        assert!(without_key(&["S".to_owned()], "S").is_empty());
        assert_eq!(without_key(&keys, "G"), ["S", "Enter"], "not there");
    }

    /// The default [`ControlSource::bind_keys`] composition: a set of nothing
    /// is a clear, a set of one is an ordinary bind — and a set of two is
    /// REFUSED in words, never written as its first key with the rest
    /// silently dropped (MAPPER-UX commandment 7).
    #[test]
    fn bind_keys_composes_what_the_map_verb_can_express_and_refuses_the_rest() {
        #[derive(Default)]
        struct Recorder(std::sync::Mutex<Vec<Option<String>>>);
        impl ControlSource for Recorder {
            fn session(&self) -> SessionView {
                SessionView::unreachable("test")
            }
            fn start(&self, _profile: Option<&str>) -> Result<String, String> {
                Err("test".into())
            }
            fn stop(&self) -> Result<String, String> {
                Err("test".into())
            }
            fn reload(&self) -> Result<String, String> {
                Err("test".into())
            }
            fn bind(&self, request: &BindRequest) -> BindOutcome {
                self.0.lock().unwrap().push(request.key.clone());
                BindOutcome {
                    ok: true,
                    ..BindOutcome::default()
                }
            }
        }

        let control = Recorder::default();
        assert!(control.bind_keys("P1", "A", &[], false, true, None).ok);
        assert!(
            control
                .bind_keys("P1", "A", &["G".to_owned()], false, true, None)
                .ok
        );
        let two = vec!["S".to_owned(), "Enter".to_owned()];
        let refused = control.bind_keys("P1", "A", &two, false, true, None);
        assert!(!refused.ok);
        let error = refused.error.unwrap();
        assert!(error.contains("S · Enter"), "{error}");
        assert!(error.contains("Nothing was changed"), "{error}");
        // The refusal must not have written anything at all.
        assert_eq!(*control.0.lock().unwrap(), [None, Some("G".to_owned())]);
    }

    #[test]
    fn unreachable_carries_the_reason_and_disables_everything() {
        let view = SessionView::unreachable("no pipe");
        assert!(!view.reachable);
        assert!(!view.running);
        assert_eq!(view.line, "no pipe");
    }
}
