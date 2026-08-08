//! The WRITE side: the session and mapper verbs, with **exactly the tray's
//! reach and no more** (docs/CONTROL-SURFACE.md invariant 1).
//!
//! Every method here is one `DaemonCommand` enqueued on the daemon's control
//! loop, or one call into the single mapping writer — and nothing else. There
//! is no path from this trait to the capture thread, the engine thread or the
//! output thread, and an implementation is a CLIENT of the daemon's control
//! channel, never a second control loop. That is what makes the same trait
//! safe to satisfy over a pipe (Studio, `ksx session`, the E5 MCP shim) and
//! in-process (a shell that hosts the supervisor): the reach is identical
//! either way, so the choice of transport cannot widen it.
//!
//! Two shapes of answer, and the split is deliberate:
//!
//! - `Result<String, Refusal>` — a verb whose failure is a sentence.
//! - `BindOutcome` / `MacroOutcome` / `LearnView` — a verb whose failure
//!   carries STRUCTURE a sentence cannot hold (the conflicting slot, the
//!   validator's problems one per row, the learner's countdown). Each one
//!   still hands out a [`Refusal`] on request, so the "say so, per click"
//!   invariant reads the same everywhere.

use serde::{Deserialize, Serialize};

use crate::refusal::{codes, Refusal};
use crate::stage::{StageEdit, StageOutcome, StagedSetupView};
use crate::status::MacroStepView;
use crate::wire::{
    BackupView, MacroWriteKind, MapMacroRequest, MapRequest, MapResponse, RestoreMode,
    SlotAssignRequest,
};

/// Performs the session and mapper verbs against a daemon.
///
/// The mapper verbs default to an honest, WORDED "unavailable" rather than a
/// silent no-op: a partial implementation says which provider call is missing,
/// on screen, instead of rendering an empty grid that looks like real data.
pub trait ControlSource: Send + Sync {
    /// Where the daemon stands right now. Called per page load, alongside the
    /// status snapshot.
    fn session(&self) -> SessionView;

    /// Start emulation, optionally under a games.toml profile title. `Ok` is a
    /// human-readable confirmation, `Err` the daemon's (or the transport's)
    /// refusal — both render as the post-redirect flash line.
    fn start(&self, profile: Option<&str>) -> Result<String, Refusal>;
    fn stop(&self) -> Result<String, Refusal>;
    fn reload(&self) -> Result<String, Refusal>;

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
    /// caller never has to know how the writer spells the edit — and the whole
    /// set lands in ONE atomic write, with no half-applied list when a write
    /// is refused.
    ///
    /// The DEFAULT implementation is the fallback for a control source that
    /// has only [`ControlSource::bind`] (one `"key"` per call): an empty set is
    /// a clear, a one-key set is an ordinary bind, and a two-key set has no
    /// wire shape at all there. Rather than write the first key and silently
    /// drop the rest — the Synapse-4 sin MAPPER-UX commandment 7 bans — it
    /// refuses in words that name the missing field.
    ///
    /// `turbo_hz` has the same three states the `map` verb gives it: `None`
    /// leaves the control's existing rate alone (so "Add another key" cannot
    /// silently switch an auto-fire off), `Some(0)` clears it, `Some(n)` sets
    /// it. The default implementation below has nowhere to put it and says so
    /// rather than dropping it.
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

    /// Restore a whole preset (pipe `map-restore`). `Ok` is the daemon's
    /// confirmation line — which already names what was written and what was
    /// backed up first.
    fn restore(&self, _preset: &str, _mode: RestoreMode) -> Result<String, Refusal> {
        Err(Refusal::new(
            codes::NOT_HERE,
            "this control source cannot restore presets",
        ))
    }

    /// Unbind every function of a preset, keeping the file structurally valid
    /// (pipe `map-clear-all`). A timestamped backup is taken first, like any
    /// other whole-preset write.
    fn clear_all(&self, _preset: &str) -> Result<String, Refusal> {
        Err(Refusal::new(
            codes::NOT_HERE,
            "this control source cannot clear presets",
        ))
    }

    /// The timestamped restore points on disk, newest first (pipe
    /// `map-backups`). Read-only — the one mapper verb that never writes.
    fn backups(&self, _preset: &str) -> Result<Vec<BackupView>, Refusal> {
        Err(Refusal::new(
            codes::NOT_HERE,
            "this control source cannot list backups",
        ))
    }

    /// Write a preset's WHOLE `[macros.<name>]` table (pipe `map-macro`) — the
    /// macro editor's save.
    ///
    /// WHOLE-MACRO, not per-step, and for the same reason
    /// [`ControlSource::bind_keys`] is whole-list: the editor already holds the
    /// entire grid, so it can always send all of it, and a partial-step
    /// protocol would have to carry indices computed against a file that may
    /// have moved — one dropped message and the preset holds a sequence nobody
    /// authored. One table in, one answer out.
    fn save_macro(&self, _request: &MacroWrite) -> MacroOutcome {
        MacroOutcome::failed("this control source cannot write macros")
    }

    /// Point a slot at a preset (pipe `slot-assign`) — the E5 verb
    /// docs/CONTROL-SURFACE.md's honest gaps 1 and 5 name.
    ///
    /// The mapper edits bindings INSIDE a preset; this says which preset a
    /// slot uses, in `config.toml` or in one games.toml profile. It is the
    /// only write on this trait that is not a preset edit, and the only one
    /// whose `reload` is a BOUNCE — see [`SlotOutcome::restarted`].
    fn assign_slot(&self, _request: &SlotAssignRequest) -> SlotOutcome {
        SlotOutcome::failed(
            "this control source cannot re-wire slots",
            "run `ksx slot assign --slot N --preset NAME`",
        )
    }

    // ── The staged setup (docs/FIRST-RUN.md §2) ──────────────────────────
    //
    // The setup a visit is still deciding on: which keyboard, which persona
    // per slot, the bindings so far, and the split-or-freeze answer. It lives
    // in the daemon for the length of the visit and NEVER touches disk, which
    // is what makes "pick PS4, look at it, change to Xbox 360" free instead of
    // three file writes and two backups.
    //
    // Reach, stated: [`Self::staged`] and [`Self::stage_edit`] touch one value
    // in the daemon's own state — no file, no driver, no session, so they are
    // strictly *less* than this trait's other verbs. [`Self::stage_commit`] is
    // one config write, exactly like [`Self::assign_slot`]. [`Self::stage_play`]
    // is one `DaemonCommand::Start`, exactly like [`Self::start`] — the
    // difference being the plan it starts was built in memory rather than read
    // off disk. Nothing new is reachable from here.

    /// Where the staged setup stands. Called per page load, like
    /// [`Self::session`].
    fn staged(&self) -> StagedSetupView {
        StagedSetupView::unreachable("this control source has no staged setup")
    }

    /// One edit — choose a device, add or remove a controller, change a
    /// persona, set bindings, answer split-or-freeze, or start over.
    ///
    /// **Nothing here writes.** A refused edit leaves the setup exactly as it
    /// was and the outcome carries it back unchanged, so the screen the user is
    /// looking at stays true.
    fn stage_edit(&self, _edit: &StageEdit) -> StageOutcome {
        StageOutcome::unavailable(
            "this control source has no staged setup to edit — a daemon holds it \
             (`ksx daemon`)",
        )
    }

    /// **Save.** Turn the staged setup into `config.toml` + preset files,
    /// through the one translation the play path also uses, behind one
    /// timestamped backup.
    ///
    /// Saving does NOT start anything and does not claim anything — moment 7's
    /// "saving and playing are separate acts".
    fn stage_commit(&self) -> StageOutcome {
        StageOutcome::unavailable(
            "this control source cannot save a staged setup — a daemon holds it (`ksx daemon`)",
        )
    }

    /// **Play without saving.** Start a session from the staged setup, with no
    /// file written at all.
    ///
    /// The counterpart of [`Self::stage_commit`], and deliberately not a flag
    /// on it: §2 requires that a user may leave without saving and lose only
    /// what they typed, which is only true if playing is reachable without
    /// writing.
    fn stage_play(&self) -> StageOutcome {
        StageOutcome::unavailable(
            "this control source cannot start a staged setup — a daemon holds it (`ksx daemon`)",
        )
    }
}

/// The `slot-assign` answer, as a surface reads it.
///
/// Structured rather than `Result<String, Refusal>` for the same reason
/// [`BindOutcome`] is: the interesting half of a successful assignment is the
/// pair of presets (what it was, what it is) and whether the pads bounced, and
/// a sentence cannot be interrogated for those.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotOutcome {
    pub ok: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub code: Option<String>,
    pub slot: Option<u8>,
    pub preset: Option<String>,
    /// What the slot pointed at before; `None` when the slot was created.
    pub previous_preset: Option<String>,
    /// The persona the slot presents itself as NOW — canonical spelling,
    /// whether or not this request asked for a change.
    #[serde(default)]
    pub persona: Option<String>,
    /// What it was before, and `None` when this request left the persona
    /// alone. The pair, not just the new half — same rule as
    /// [`Self::previous_preset`].
    #[serde(default)]
    pub previous_persona: Option<String>,
    /// The games.toml profile this landed in, or `None` for `config.toml`.
    pub profile: Option<String>,
    /// The slot was ADDED to that file rather than repointed.
    #[serde(default)]
    pub created: bool,
    /// The slot already used that preset. Success, and nothing happened.
    #[serde(default)]
    pub unchanged: bool,
    /// **The pads replugged.**
    ///
    /// Every other write on this trait edits a key→function table, which the
    /// live engine takes in place (docs/CONTROL-SURFACE.md "the binding hot
    /// swap"): pads stay plugged, Windows plays no chime, a game in progress
    /// notices nothing. A slot assignment is not that. It changes what the
    /// slot IS, so it takes the blunt `Reload` — stop, re-read from disk,
    /// start — and every surface has to SAY so before it is used, because on
    /// a cabinet the cost is visible: four controllers vanish and come back,
    /// and anything mid-game sees them go.
    ///
    /// Worth writing down because a reader of `SessionShape` will notice the
    /// tension: which preset a slot NAMES is deliberately outside the shape
    /// (`run/supervisor.rs`), so a preset-only re-point would in fact hot-swap
    /// if this verb enqueued `ApplyBindings`. It does not, on purpose — this
    /// verb writes the slot ENTRY, whose other fields (keyboard, persona) are
    /// genuinely structural, and a verb whose pad behaviour depended on which
    /// field you happened to change is a verb nobody can predict. One answer,
    /// stated up front, beats a cheaper one that is sometimes true.
    #[serde(default)]
    pub restarted: bool,
    /// `reload` was asked for and the daemon acted on it.
    #[serde(default)]
    pub reloaded: bool,
    /// The file that was written.
    pub path: Option<String>,
}

impl SlotOutcome {
    /// A refusal with the way out attached — the shape every default on this
    /// trait owes (docs/CONTROL-SURFACE.md "a surface that cannot act must SAY
    /// so, per click").
    pub fn failed(reason: impl Into<String>, remedy: impl Into<String>) -> Self {
        let remedy = remedy.into();
        Self {
            ok: false,
            error: Some(format!("{} — {remedy}", reason.into())),
            code: Some(codes::NOT_HERE.to_owned()),
            ..Self::default()
        }
    }

    /// The refusal this outcome carries, or `None` when it succeeded.
    pub fn refusal(&self) -> Option<Refusal> {
        if self.ok {
            return None;
        }
        Some(Refusal::from_wire(
            self.code.as_deref(),
            self.error
                .clone()
                .unwrap_or_else(|| "the slot was not changed".to_owned()),
        ))
    }

    /// The one line a 10-foot surface prints after the write — including the
    /// pad bounce, which is the part a user must not have to infer.
    pub fn headline(&self) -> String {
        if let Some(refusal) = self.refusal() {
            return refusal.message;
        }
        // **The daemon's own sentence wins when there is one.** It is built
        // from what actually happened; the reconstruction below is built from
        // flags, and flags cannot express "a restart was asked for, a session
        // WAS running, and the restart then failed". Synthesising over the top
        // of that produced the one lie this surface could least afford — a
        // cabinet whose four pads had just vanished being told "nothing was
        // running, so nothing had to restart".
        //
        // The reconstruction stays for outcomes built in-process (a fake, a
        // future non-pipe sink) that carry the facts but no prose.
        if let Some(message) = self
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            return message.to_owned();
        }
        let slot = self.slot.map_or_else(|| "?".to_owned(), |n| n.to_string());
        let preset = self.preset.clone().unwrap_or_default();
        if self.unchanged {
            return format!("slot {slot} already uses \"{preset}\" — nothing changed");
        }
        let change = match &self.previous_preset {
            Some(previous) => format!("slot {slot}: \"{previous}\" → \"{preset}\""),
            None => format!("slot {slot} added, using \"{preset}\""),
        };
        let tail = if self.restarted {
            " — the session restarted and the pads replugged"
        } else if self.reloaded {
            " — nothing was running, so nothing had to restart"
        } else {
            " — the next start reads it"
        };
        format!("{change}{tail}")
    }
}

impl From<crate::wire::SlotAssignResponse> for SlotOutcome {
    fn from(response: crate::wire::SlotAssignResponse) -> Self {
        Self {
            ok: response.ok,
            message: response.message,
            error: response.error,
            code: response.code,
            slot: response.slot,
            preset: response.preset,
            previous_preset: response.previous_preset,
            persona: response.persona,
            previous_persona: response.previous_persona,
            profile: response.profile,
            created: response.created,
            unchanged: response.unchanged,
            restarted: response.restarted,
            reloaded: response.reloaded,
            path: response.path,
        }
    }
}

/// One whole-macro write as a SURFACE spells it: policies as the words a
/// `<select>` carries, blank meaning "the file's own default".
///
/// Kept apart from [`crate::wire::MapMacroRequest`] on purpose. This is what
/// arrives from a form or a JSON body — strings a human or a browser produced,
/// which may be blank and may be wrong. [`MacroWrite::to_request`] is the one
/// place that turns it into the typed request, and a bad policy is refused
/// THERE, in the same words the daemon would have used, without a round trip.
///
/// `steps` reuses [`MacroStepView`] — the SAME type the read side serves — so
/// what the editor was shown and what it saves are one shape, and a duration
/// authored in `frames` survives the round trip as frames.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroWrite {
    pub preset: String,
    /// The `[macros.<name>]` table. Matched case-insensitively against what is
    /// on disk; a table that already exists keeps its own spelling.
    pub name: String,
    #[serde(default)]
    pub steps: Vec<MacroStepView>,
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

impl MacroWrite {
    /// The typed request this write means, or the refusal that says why it
    /// cannot mean one.
    ///
    /// The three policies are normalized HERE rather than at the daemon: a
    /// blank select is the FILE's own "field omitted" case, which means the
    /// default, and spelling that out at the edge keeps the daemon's parser
    /// strict — a genuine typo is still refused, in words that name the
    /// options.
    pub fn to_request(&self) -> Result<MapMacroRequest, Refusal> {
        let write = if self.delete {
            MacroWriteKind::Delete
        } else if let (Some(enabled), true) = (self.enabled, self.steps.is_empty()) {
            // A pure TOGGLE. Sending an empty grid alongside it would be a
            // whole-table write with no steps, which is exactly the refusal
            // that protection exists for.
            MacroWriteKind::Toggle(enabled)
        } else {
            MacroWriteKind::Body(Box::new(self.body()?))
        };
        Ok(MapMacroRequest {
            preset: self.preset.clone(),
            name: self.name.clone(),
            write,
            reload: self.reload,
        })
    }

    /// The `[macros.<name>]` table this write carries, parsed through the
    /// preset file's OWN serde types — so a duration authored in `frames`
    /// stays frames, and an unknown policy is refused by the same parser that
    /// would have refused it on disk.
    fn body(&self) -> Result<ksx_config::MacroFile, Refusal> {
        let mut object = serde_json::Map::new();
        object.insert(
            "steps".to_owned(),
            serde_json::to_value(
                self.steps
                    .iter()
                    .map(MacroStepView::to_file)
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
        );
        let mut policy = |name: &str, value: &str| {
            let value = value.trim();
            if !value.is_empty() {
                object.insert(name.to_owned(), value.into());
            }
        };
        policy("on_release", &self.on_release);
        policy("retrigger", &self.retrigger);
        policy("interrupt", &self.interrupt);
        policy("repeat", &self.repeat);
        // The RATE is only carried when the author gave one. Two spellings of
        // one number, so exactly one goes on the wire; the daemon refuses both,
        // and sending a stale companion field would turn an editor slip into
        // that refusal.
        if let Some(hz) = self.turbo_hz {
            object.insert("turbo_hz".to_owned(), hz.into());
        } else if let Some(ms) = self.gap_ms {
            object.insert("gap_ms".to_owned(), ms.into());
        }
        if let Some(enabled) = self.enabled {
            object.insert("enabled".to_owned(), enabled.into());
        }
        crate::wire::macro_body(&serde_json::Value::Object(object))
    }
}

/// The `map-macro` answer, as a surface reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// this edit leaves behind.
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

    /// The refusal this outcome carries, or `None` when it succeeded.
    pub fn refusal(&self) -> Option<Refusal> {
        if self.ok {
            return None;
        }
        Some(Refusal::from_wire(
            self.code.as_deref(),
            self.error
                .clone()
                .unwrap_or_else(|| "the macro was not saved".to_owned()),
        ))
    }
}

/// Why a multi-key write cannot land on a daemon whose only shape is one key
/// per call, in one sentence a page can flash verbatim. A current ksx daemon
/// never says this — it writes the list — so it is the honest answer for a
/// control source that predates the `"keys"` field, kept in one place rather
/// than formatted at three.
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

/// The daemon learner's state, presentation-shaped from the pipe's
/// `learn-key`/`learn-poll` response. `state` is the pipe's word verbatim
/// (`listening`, `hit`, `timeout`, `cancelled`, `failed`, `idle`) plus this
/// crate's `"unavailable"` for "no daemon / daemon predates the verb".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

    /// The refusal a learner in the `unavailable` state carries — the reason
    /// the mapper prints beside the dead "press a key" dialog.
    pub fn refusal(&self) -> Option<Refusal> {
        if self.ok {
            return None;
        }
        Some(Refusal::new(
            codes::REFUSED,
            self.error
                .clone()
                .unwrap_or_else(|| "the learner is unavailable".to_owned()),
        ))
    }
}

/// One mapper write, straight onto the pipe `map` verb's fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BindRequest {
    pub preset: String,
    pub function: String,
    /// `None` = clear.
    pub key: Option<String>,
    #[serde(default)]
    pub force: bool,
    /// Apply it to a running session (a binding-only edit hot-swaps).
    #[serde(default)]
    pub reload: bool,
}

impl BindRequest {
    /// The typed wire request this bind means.
    pub fn to_request(&self) -> MapRequest {
        let keys: Vec<String> = self.key.clone().into_iter().collect();
        map_request(
            &self.preset,
            &self.function,
            &keys,
            self.force,
            self.reload,
            // `bind` edits keys, never the turbo rate: None leaves whatever
            // rate the binding already has alone.
            None,
        )
    }
}

/// The `map` request for a control's WHOLE key list. An EMPTY list is a clear
/// — the honest wire shape for "this control now holds nothing". One key sends
/// `"key"`, so a single-key write is byte-for-byte the request it always was;
/// two or more send `"keys"`.
pub fn map_request(
    preset: &str,
    function: &str,
    keys: &[String],
    force: bool,
    reload: bool,
    turbo_hz: Option<u32>,
) -> MapRequest {
    let mut request = MapRequest {
        preset: preset.to_owned(),
        function: function.to_owned(),
        force,
        reload,
        turbo_hz,
        ..MapRequest::default()
    };
    match keys {
        [] => request.clear = true,
        [only] => request.key = Some(only.clone()),
        many => request.keys = many.to_vec(),
    }
    request
}

/// The pipe `map` response, as a surface reads it. `conflicts` is filled both
/// on refusal (`ok: false`, `code: "conflict"` — the caller decides) and on a
/// forced write (informational: cross-slot bindings that still exist).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindOutcome {
    pub ok: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub code: Option<String>,
    pub conflicts: Vec<BindConflict>,
    /// MULTI-BIND: the other controls of the SAME preset this key drives now
    /// that the write is done. Not a conflict and never was a refusal — the
    /// engine fires all of them (docs/INPUT-TRANSFORMS.md §1a).
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

    /// The refusal this outcome carries, or `None` when it succeeded. The
    /// structured half (`conflicts`) stays on the outcome — a refusal is one
    /// sentence, and a cross-slot conflict is a table.
    pub fn refusal(&self) -> Option<Refusal> {
        if self.ok {
            return None;
        }
        Some(Refusal::from_wire(
            self.code.as_deref(),
            self.error
                .clone()
                .unwrap_or_else(|| "nothing was written".to_owned()),
        ))
    }
}

impl From<MapResponse> for BindOutcome {
    fn from(response: MapResponse) -> Self {
        Self {
            ok: response.ok,
            message: response.message,
            error: response.error,
            code: response.code,
            conflicts: response
                .conflicts
                .into_iter()
                .map(BindConflict::from)
                .collect(),
            also_drives: response.also_drives,
            turbo_hz: response.turbo_hz,
            turbo_effective_hz: response.turbo_effective_hz,
            reloaded: response.reloaded,
        }
    }
}

/// One conflicting binding, as the pipe reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindConflict {
    /// `"profile"` (another slot's preset inside a games.toml profile) or
    /// `"config"` (inside config.toml's `[[slot]]` table) from a current
    /// daemon. Neither is ever auto-edited. (`"preset"` was the same-preset
    /// case, which is now a multi-bind reported as
    /// [`BindOutcome::also_drives`] instead of a conflict; the field stays
    /// because the wire word is contract.)
    pub scope: String,
    pub preset: String,
    pub function: String,
    /// The file to go and edit. Empty from an old daemon, which sent none —
    /// and an empty file name is left OUT of the line rather than printed as a
    /// blank address.
    #[serde(default)]
    pub file: String,
    pub profile: Option<String>,
    pub slot: Option<u8>,
}

impl From<crate::wire::ConflictView> for BindConflict {
    fn from(view: crate::wire::ConflictView) -> Self {
        Self {
            scope: view.scope,
            preset: view.preset,
            function: view.function,
            file: view.file,
            profile: view.profile,
            slot: view.slot,
        }
    }
}

impl BindConflict {
    /// The dialog line: `G is "IPAC P2"'s A (slot 2 of "Steam" in
    /// games.toml)`, or `G is "IPAC P2"'s A (slot 2 in config.toml)`.
    pub fn describe(&self, key: &str) -> String {
        if self.scope == "preset" {
            return format!("{key} is already this preset's {}", self.function);
        }
        // The file is what makes this an address; an old daemon sends none, so
        // the line degrades to what it always said rather than to "( in )".
        let in_file = if self.file.is_empty() {
            String::new()
        } else {
            format!(" in {}", self.file)
        };
        let where_ = match (&self.profile, self.slot) {
            (Some(profile), Some(slot)) => format!(" (slot {slot} of \"{profile}\"{in_file})"),
            (Some(profile), None) => format!(" (\"{profile}\"{in_file})"),
            (None, Some(slot)) => format!(" (slot {slot}{in_file})"),
            (None, None) if in_file.is_empty() => String::new(),
            (None, None) => format!(" ({})", in_file.trim_start()),
        };
        format!("{key} is \"{}\"'s {}{where_}", self.preset, self.function)
    }
}

/// What the session panel needs to know, presentation-shaped like
/// [`crate::StatusSnapshot`]: the provider composes the line, a surface only
/// places it and picks which controls to render.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

impl From<crate::wire::StatusResponse> for SessionView {
    /// One presentation line from the daemon's status answer. The ONE place
    /// this wording lives, so a native shell and a web page cannot describe
    /// the same daemon differently.
    fn from(status: crate::wire::StatusResponse) -> Self {
        let running = matches!(status.run.as_str(), "running" | "starting");
        let game = status.game.as_deref();
        let line = match status.run.as_str() {
            "running" => {
                let slots = status.slots.unwrap_or(0);
                match game {
                    Some(game) => format!("running — {game} — {slots} pad(s)"),
                    None => format!("running — {slots} pad(s)"),
                }
            }
            "starting" => "starting…".to_owned(),
            "stopped" => match game {
                Some(game) => format!("idle — profile: {game}"),
                None => "idle".to_owned(),
            },
            "failed" => format!(
                "stopped: {}",
                status.message.as_deref().unwrap_or("last session failed")
            ),
            "quitting" => "daemon shutting down…".to_owned(),
            other => format!("daemon state: {other}"),
        };
        Self {
            reachable: true,
            running,
            line,
            profile: status.game,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A conflict line has to be an ADDRESS.**
    ///
    /// Fails against the version this replaced, whose location clause was
    /// built from `profile`/`slot` alone: a config.toml `[[slot]]` row carries
    /// no profile, so the whole clause collapsed to nothing and the dialog
    /// said "G is \"IPAC P2\"'s A" — true, unactionable, and identical to what
    /// it would say about a profile it had failed to name.
    #[test]
    fn a_conflict_line_names_the_file_and_the_slot_that_holds_it() {
        let profile = BindConflict {
            scope: "profile".into(),
            preset: "IPAC P2".into(),
            function: "A".into(),
            file: "games.toml".into(),
            profile: Some("Steam".into()),
            slot: Some(2),
        };
        assert_eq!(
            profile.describe("G"),
            "G is \"IPAC P2\"'s A (slot 2 of \"Steam\" in games.toml)"
        );

        let config = BindConflict {
            scope: "config".into(),
            profile: None,
            file: "config.toml".into(),
            ..profile.clone()
        };
        assert_eq!(
            config.describe("G"),
            "G is \"IPAC P2\"'s A (slot 2 in config.toml)"
        );

        // An OLD daemon sends no file. The line degrades to what it always
        // said rather than to a blank address like "( in )".
        let old = BindConflict {
            file: String::new(),
            ..profile.clone()
        };
        assert_eq!(
            old.describe("G"),
            "G is \"IPAC P2\"'s A (slot 2 of \"Steam\")"
        );
        let bare = BindConflict {
            file: String::new(),
            profile: None,
            slot: None,
            ..profile.clone()
        };
        assert_eq!(bare.describe("G"), "G is \"IPAC P2\"'s A");

        // The same-preset case an old daemon can still send is untouched.
        let same = BindConflict {
            scope: "preset".into(),
            ..profile
        };
        assert_eq!(same.describe("G"), "G is already this preset's A");
    }

    /// The set arithmetic behind "Add another key" and the per-key ✕. Both are
    /// read-modify-write on the control's key LIST, so the order the file holds
    /// is preserved and an add that changes nothing is a no-op, not a second
    /// row.
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
    /// REFUSED in words, never written as its first key with the rest silently
    /// dropped (MAPPER-UX commandment 7).
    #[test]
    fn bind_keys_composes_what_the_map_verb_can_express_and_refuses_the_rest() {
        #[derive(Default)]
        struct Recorder(std::sync::Mutex<Vec<Option<String>>>);
        impl ControlSource for Recorder {
            fn session(&self) -> SessionView {
                SessionView::unreachable("test")
            }
            fn start(&self, _profile: Option<&str>) -> Result<String, Refusal> {
                Err(Refusal::new(codes::REFUSED, "test"))
            }
            fn stop(&self) -> Result<String, Refusal> {
                Err(Refusal::new(codes::REFUSED, "test"))
            }
            fn reload(&self) -> Result<String, Refusal> {
                Err(Refusal::new(codes::REFUSED, "test"))
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
        let error = refused.error.clone().unwrap();
        assert!(error.contains("S · Enter"), "{error}");
        assert!(error.contains("Nothing was changed"), "{error}");
        // ...and it is a REFUSAL, with a code, not just a sentence.
        assert!(refused.refusal().is_some());
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

    /// The state line, composed from the daemon's own answer — one wording,
    /// wherever the surface is.
    #[test]
    fn the_session_line_is_composed_from_the_status_answer() {
        let running = SessionView::from(crate::wire::StatusResponse {
            ok: true,
            run: "running".into(),
            slots: Some(4),
            game: Some("Street Fighter".into()),
            ..Default::default()
        });
        assert!(running.reachable && running.running);
        assert_eq!(running.line, "running — Street Fighter — 4 pad(s)");
        assert_eq!(running.profile.as_deref(), Some("Street Fighter"));

        let idle = SessionView::from(crate::wire::StatusResponse {
            ok: true,
            run: "stopped".into(),
            ..Default::default()
        });
        assert!(idle.reachable && !idle.running);
        assert_eq!(idle.line, "idle");

        let failed = SessionView::from(crate::wire::StatusResponse {
            ok: true,
            run: "failed".into(),
            message: Some("refusing to start: bad config".into()),
            ..Default::default()
        });
        assert!(!failed.running);
        assert!(failed.line.contains("bad config"), "{}", failed.line);

        let starting = SessionView::from(crate::wire::StatusResponse {
            ok: true,
            run: "starting".into(),
            ..Default::default()
        });
        assert!(starting.running, "starting must offer Stop, not Start");
    }

    /// The editor's draft → the typed request, with the file's own defaults
    /// filled in for every blank select — and the three kinds told apart the
    /// way the protocol tells them apart.
    #[test]
    fn a_macro_write_becomes_the_typed_request_with_the_files_defaults() {
        let write = MacroWrite {
            preset: "SF P1".into(),
            name: "hadouken".into(),
            steps: vec![
                MacroStepView {
                    hold: vec!["dpad.down".into()],
                    ms: Some(50),
                    ..MacroStepView::default()
                },
                MacroStepView {
                    hold: vec!["A".into()],
                    frames: Some(3),
                    ..MacroStepView::default()
                },
            ],
            on_release: "abort".into(),
            // Blank is the file's own "field omitted" case: the default.
            retrigger: String::new(),
            interrupt: "  ".into(),
            repeat: "while-held".into(),
            reload: true,
            ..MacroWrite::default()
        };
        let request = write.to_request().expect("a valid draft");
        assert_eq!(request.preset, "SF P1");
        assert!(request.reload);
        let MacroWriteKind::Body(body) = &request.write else {
            panic!("steps present = a body write");
        };
        assert_eq!(body.on_release, ksx_core::OnRelease::Abort);
        assert_eq!(body.retrigger, ksx_core::Retrigger::default());
        assert_eq!(body.interrupt, ksx_core::Interrupt::default());
        assert_eq!(body.repeat, ksx_core::Repeat::WhileHeld);
        // A duration authored in frames survives as frames.
        assert_eq!(body.steps[1].frames, Some(3));
        assert_eq!(body.steps[1].ms, None);

        // A toggle: `enabled` and no steps.
        let toggle = MacroWrite {
            preset: "SF P1".into(),
            name: "hadouken".into(),
            enabled: Some(false),
            ..MacroWrite::default()
        }
        .to_request()
        .unwrap();
        assert_eq!(toggle.write, MacroWriteKind::Toggle(false));

        // A delete, whatever else it carries.
        let delete = MacroWrite {
            preset: "SF P1".into(),
            name: "hadouken".into(),
            delete: true,
            ..MacroWrite::default()
        }
        .to_request()
        .unwrap();
        assert!(delete.is_delete());

        // A typo in a policy is refused HERE, in the daemon's own words, with
        // no round trip — and nothing is written anywhere.
        let refused = MacroWrite {
            preset: "SF P1".into(),
            name: "hadouken".into(),
            steps: vec![MacroStepView {
                hold: vec!["A".into()],
                ms: Some(50),
                ..MacroStepView::default()
            }],
            repeat: "sometimes".into(),
            ..MacroWrite::default()
        }
        .to_request()
        .unwrap_err();
        assert!(refused.message.contains("while-held"), "{refused}");
    }

    /// The one-key/many-key/clear spellings of a `map` request — the shape
    /// every surface's edit funnels into.
    #[test]
    fn the_map_request_spells_a_key_list_the_way_the_verb_reads_it() {
        let two = vec!["S".to_owned(), "Enter".to_owned()];
        let many = map_request("IPAC P1", "A", &two, false, true, None);
        assert_eq!(many.keys, two);
        assert!(many.key.is_none() && !many.clear);

        let one = map_request("IPAC P1", "A", &["G".to_owned()], true, false, None);
        assert_eq!(one.key.as_deref(), Some("G"));
        assert!(one.keys.is_empty());
        assert!(one.force);

        let cleared = map_request("IPAC P1", "A", &[], false, true, None);
        assert!(cleared.clear);
        assert!(cleared.key.is_none() && cleared.keys.is_empty());

        // `bind` composes to exactly the same request, so the two entry points
        // cannot drift.
        assert_eq!(
            BindRequest {
                preset: "IPAC P1".into(),
                function: "A".into(),
                key: Some("G".into()),
                force: true,
                reload: false,
            }
            .to_request(),
            one
        );
    }

    /// **The daemon's sentence wins.** A slot assignment whose restart was
    /// asked for and then FAILED carries `restarted: false` and
    /// `reloaded: false`, and the flag-driven reconstruction has no way to say
    /// that — it used to print "nothing was running, so nothing had to
    /// restart" at somebody whose four pads had just vanished.
    #[test]
    fn a_slot_outcome_prints_what_the_daemon_said_rather_than_re_deriving_it() {
        let failed = SlotOutcome {
            ok: true,
            slot: Some(1),
            preset: Some("IPAC P2".to_owned()),
            previous_preset: Some("IPAC P1".to_owned()),
            message: Some(
                "slot 1: \"IPAC P1\" -> \"IPAC P2\" — a slot change needs the pads replugged                  — [FAIL] cannot start: no usable slot"
                    .to_owned(),
            ),
            restarted: false,
            reloaded: false,
            ..SlotOutcome::default()
        };
        let headline = failed.headline();
        assert!(headline.contains("cannot start"), "{headline}");
        assert!(
            !headline.contains("nothing was running"),
            "a failed restart must never read as an idle daemon: {headline}"
        );

        // With no prose — an outcome built in process by a fake — the
        // reconstruction is still there and still names the bounce.
        let synthesised = SlotOutcome {
            ok: true,
            slot: Some(2),
            preset: Some("IPAC P2".to_owned()),
            previous_preset: Some("IPAC P1".to_owned()),
            restarted: true,
            reloaded: true,
            ..SlotOutcome::default()
        };
        assert!(
            synthesised.headline().contains("pads replugged"),
            "{}",
            synthesised.headline()
        );
        // A blank message is not a message.
        let blank = SlotOutcome {
            message: Some("   ".to_owned()),
            ..synthesised
        };
        assert!(blank.headline().contains("pads replugged"));
    }
}
