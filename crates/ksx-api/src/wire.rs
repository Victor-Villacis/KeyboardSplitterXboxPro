//! **The one place the JSON wire shapes are derived from.**
//!
//! Every ksx front end and the daemon itself read and write the daemon control
//! protocol through the types in this module. That is the whole point of the
//! layer: before it, a request was hand-built with `serde_json::json!` at the
//! caller and hand-parsed with `request.get("field")` at the daemon, and the
//! two descriptions of the same message lived 3,000 lines apart.
//!
//! On 2026-08-06 that cost a real bug: the `map-macro` body reader carried an
//! ALLOWLIST of body fields, `repeat` (and its `turbo_hz`/`gap_ms` rate) was
//! not on it, and a macro card that set `while-held` saved `once` — with a
//! cheerful "saved" toast, because nothing on either side could tell that a
//! field had been dropped on the way in. [`MapMacroRequest`] is the fix in
//! type form: its body half is `ksx_config::MacroFile` itself, serialized and
//! deserialized as a unit, so the wire's macro fields ARE the file's macro
//! fields and a field added to the table is on the wire the moment it
//! compiles. The list that remains is the ENVELOPE's (`verb`, `preset`,
//! `name`, `delete`, `reload`) — a closed set this module owns — and getting
//! that one wrong is loud (`MacroFile` is `deny_unknown_fields`, so a stray
//! envelope key is a refusal in words) instead of silent.
//!
//! The protocol itself is unchanged and stays deliberately dumb: one JSON
//! request line in, one JSON response line out, per connection
//! (docs/CONTROL-SURFACE.md).

use serde::{Deserialize, Serialize};

use crate::refusal::{codes, Refusal};
use crate::status::ProfileRow;

/// The one well-known control-channel name. Tests use throwaway names;
/// everything else uses this.
pub const PIPE_NAME: &str = r"\\.\pipe\ksx-daemon";

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// One control-channel request. Internally tagged by `verb`, which is exactly
/// how the protocol has always spelled it — `{"verb":"status"}` is what this
/// serializes to, byte for byte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum Request {
    Status,
    Start {
        /// A games.toml profile title. Absent = whatever the daemon is already
        /// configured with.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
    Stop,
    Reload,
    Map(MapRequest),
    MapMacro(MapMacroRequest),
    MapRestore(RestoreRequest),
    MapClearAll(ClearAllRequest),
    MapBackups(BackupsRequest),
    SlotAssign(SlotAssignRequest),
    LearnKey,
    LearnPoll,
    LearnCancel,
}

impl Request {
    /// The verb word this request carries on the wire.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Start { .. } => "start",
            Self::Stop => "stop",
            Self::Reload => "reload",
            Self::Map(_) => "map",
            Self::MapMacro(_) => "map-macro",
            Self::MapRestore(_) => "map-restore",
            Self::MapClearAll(_) => "map-clear-all",
            Self::MapBackups(_) => "map-backups",
            Self::SlotAssign(_) => "slot-assign",
            Self::LearnKey => "learn-key",
            Self::LearnPoll => "learn-poll",
            Self::LearnCancel => "learn-cancel",
        }
    }

    /// The request as the line that goes on the pipe (no trailing newline —
    /// the transport adds it).
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|err| {
            // Nothing in these types can fail to serialize (no maps with
            // non-string keys, no NaN); saying so beats an unwrap with no
            // sentence attached.
            unreachable!("a control request is always serializable: {err}")
        })
    }
}

/// The `map` verb: one control's binding, with the whole key list.
///
/// Field-for-field the shape docs/CONTROL-SURFACE.md documents. `key` is the
/// one-key spelling of `keys` — exactly one of the two, never both — and
/// `clear` is the third alternative.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapRequest {
    pub preset: String,
    pub function: String,
    /// The single key. `None` when `keys` or `clear` carries the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// MANY KEYS → ONE CONTROL: the whole list this control should hold, in
    /// ONE atomic write (docs/INPUT-TRANSFORMS.md §1a).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Unbind, leaving the inert `"None"` placeholder.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear: bool,
    /// Write despite a CROSS-SLOT conflict. Removes no binding, anywhere.
    #[serde(default, skip_serializing_if = "is_false")]
    pub force: bool,
    /// Take this key off THAT function — the explicit hand-over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_from: Option<String>,
    /// CHORD guards (docs/INPUT-TRANSFORMS.md §1b).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unless: Vec<String>,
    /// AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3). ABSENT means "not asked
    /// about", which leaves an existing rate alone; `0` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turbo_hz: Option<u32>,
    /// Apply it to a running session (a binding change hot-swaps).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reload: bool,
}

impl MapRequest {
    /// The daemon's reader: one request object → this type, or the sentence to
    /// refuse it with.
    ///
    /// The validation lives HERE rather than in the daemon so that a client can
    /// be refused before a round trip and hear the identical words — and so
    /// that "which combinations of key/keys/clear are legal" is answered once.
    pub fn from_json(request: &serde_json::Value) -> Result<Self, Refusal> {
        let Some(preset) = field(request, "preset") else {
            return Err(bad_request(r#"map needs a "preset""#));
        };
        let Some(function) = field(request, "function") else {
            return Err(bad_request(r#"map needs a "function""#));
        };
        let clear = flag(request, "clear");
        let keys = list(request, "keys");
        let key = field(request, "key");
        if key.is_some() && !keys.is_empty() {
            return Err(bad_request(r#"map takes either "key" or "keys", not both"#));
        }
        if key.is_none() && keys.is_empty() && !clear {
            return Err(bad_request(
                r#"map needs a "key" (or "keys", or "clear": true)"#,
            ));
        }
        if (key.is_some() || !keys.is_empty()) && clear {
            return Err(bad_request(
                r#"map takes either "key"/"keys" or "clear", not both"#,
            ));
        }
        Ok(Self {
            preset,
            function,
            key,
            keys,
            clear,
            force: flag(request, "force"),
            move_from: field(request, "move_from"),
            when: list(request, "when"),
            unless: list(request, "unless"),
            turbo_hz: request
                .get("turbo_hz")
                .and_then(serde_json::Value::as_u64)
                .and_then(|hz| u32::try_from(hz).ok()),
            reload: flag(request, "reload"),
        })
    }

    /// The key list this write means, whatever spelling it arrived in: empty
    /// for a clear, one entry for `key`, the list for `keys`.
    pub fn key_list(&self) -> Vec<String> {
        match (&self.key, self.keys.as_slice()) {
            (Some(key), []) => vec![key.clone()],
            (_, many) => many.to_vec(),
        }
    }
}

/// The `map-macro` verb: one preset's WHOLE `[macros.<name>]` table.
///
/// Serde is written by hand for exactly one reason, and it is the reason this
/// crate exists: the body half is [`ksx_config::MacroFile`] itself, converted
/// as a unit, so no list of body field names exists anywhere to fall out of
/// date. See the module docs for the bug that taught us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapMacroRequest {
    pub preset: String,
    /// The `[macros.<name>]` table. Matched case-insensitively on disk.
    pub name: String,
    /// Which of the three things this write is.
    pub write: MacroWriteKind,
    /// Apply it to a running session — a macro body is a BINDING change, so
    /// the pads stay plugged.
    pub reload: bool,
}

/// The three things a `map-macro` request can mean. Spelling them as a sum
/// type is not decoration: on the wire they are distinguished by which fields
/// are ABSENT (a toggle is `enabled` with no `steps`), and that is precisely
/// the kind of distinction a struct with eight optional fields loses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroWriteKind {
    /// A whole-table write. Never partial: the editor holds the entire grid,
    /// so it always sends all of it.
    Body(Box<ksx_config::MacroFile>),
    /// Switch an existing table on or off, touching nothing else — the whole
    /// promise of disabling instead of deleting.
    Toggle(bool),
    /// Remove the table, and the `macro.<name>` trigger rows that would
    /// otherwise dangle. An explicit word, never an empty step list.
    Delete,
}

/// The envelope's own field names — the closed set that is NOT part of the
/// macro table. Everything else in a `map-macro` object is body, which is what
/// keeps [`ksx_config::MacroFile`] the single description of a macro.
const MACRO_ENVELOPE_FIELDS: &[&str] = &["verb", "preset", "name", "delete", "reload"];

impl MapMacroRequest {
    /// The daemon's reader: one request object → this type, or the sentence to
    /// refuse it with. Mirrors the pipe's documented order of checks.
    pub fn from_json(request: &serde_json::Value) -> Result<Self, Refusal> {
        let Some(preset) = field(request, "preset") else {
            return Err(bad_request(r#"map-macro needs a "preset""#));
        };
        let Some(name) = field(request, "name") else {
            return Err(bad_request(
                r#"map-macro needs a "name" (the [macros.<name>] table)"#,
            ));
        };
        let delete = flag(request, "delete");
        let steps = request.get("steps");
        let enabled = request.get("enabled");
        // A bare `"enabled"` with no body is the TOGGLE spelling; with a body
        // it is just another field of the table being written.
        let toggle = match (delete, steps, enabled) {
            (false, None, Some(value)) => match value.as_bool() {
                Some(enabled) => Some(enabled),
                None => return Err(bad_request(r#"map-macro "enabled" is true or false"#)),
            },
            _ => None,
        };
        if !delete && toggle.is_none() && steps.is_none() {
            return Err(bad_request(
                "map-macro needs \"steps\" (or \"delete\": true, or \"enabled\": true/false to \
                 switch an existing macro on or off without touching it) — an absent step list \
                 is a misspelled field far more often than an intended deletion, and deleting a \
                 macro is its own word",
            ));
        }
        let write = match (delete, toggle) {
            (true, _) => MacroWriteKind::Delete,
            (false, Some(enabled)) => MacroWriteKind::Toggle(enabled),
            (false, None) => MacroWriteKind::Body(Box::new(macro_body(request)?)),
        };
        Ok(Self {
            preset,
            name,
            write,
            reload: flag(request, "reload"),
        })
    }

    /// The request object, envelope and body merged — the inverse of
    /// [`Self::from_json`].
    pub fn to_json(&self) -> serde_json::Value {
        let mut object = serde_json::Map::new();
        object.insert("preset".to_owned(), self.preset.clone().into());
        object.insert("name".to_owned(), self.name.clone().into());
        match &self.write {
            MacroWriteKind::Delete => {
                object.insert("delete".to_owned(), true.into());
            }
            MacroWriteKind::Toggle(enabled) => {
                object.insert("enabled".to_owned(), (*enabled).into());
            }
            MacroWriteKind::Body(body) => {
                if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(body.as_ref()) {
                    for (key, value) in fields {
                        object.insert(key, value);
                    }
                }
                // `MacroFile` omits an empty `steps`, but a BODY write must
                // always declare one: an absent step list is what the daemon
                // reads as "toggle or typo", and an editor that saved an empty
                // grid deserves the validator's refusal, not that one.
                object
                    .entry("steps")
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            }
        }
        if self.reload {
            object.insert("reload".to_owned(), true.into());
        }
        serde_json::Value::Object(object)
    }

    /// The macro table this write carries, for a caller that needs one whether
    /// or not the request has a body (a delete and a toggle both leave the
    /// table on disk alone, so the defaults are never read).
    pub fn body(&self) -> ksx_config::MacroFile {
        match &self.write {
            MacroWriteKind::Body(body) => body.as_ref().clone(),
            _ => ksx_config::MacroFile::default(),
        }
    }

    /// `Some(enabled)` when this request is the TOGGLE, `None` otherwise —
    /// the `set_enabled` half of the writer's spec.
    pub fn set_enabled(&self) -> Option<bool> {
        match &self.write {
            MacroWriteKind::Toggle(enabled) => Some(*enabled),
            _ => None,
        }
    }

    pub fn is_delete(&self) -> bool {
        matches!(self.write, MacroWriteKind::Delete)
    }
}

impl Serialize for MapMacroRequest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MapMacroRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json(&value).map_err(|refusal| serde::de::Error::custom(refusal.message))
    }
}

/// The `[macros.<name>]` table a request carries, in the preset file's own
/// vocabulary — **everything that is not an envelope field**.
///
/// A DENYLIST, deliberately, where an allowlist used to be. The envelope is a
/// closed set this module owns; the body is `MacroFile`, which grows. With the
/// list pointed the other way, adding a field to the table put it on the wire
/// only if somebody remembered — and forgetting was silent. Now forgetting is
/// impossible in that direction, and the other direction (a new envelope field
/// that is not listed here) is loud: `MacroFile` is `deny_unknown_fields`, so
/// it comes back as a refusal that names the field.
pub fn macro_body(request: &serde_json::Value) -> Result<ksx_config::MacroFile, Refusal> {
    let mut object = serde_json::Map::new();
    if let Some(fields) = request.as_object() {
        for (key, value) in fields {
            if !MACRO_ENVELOPE_FIELDS.contains(&key.as_str()) {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    serde_json::from_value(object.into()).map_err(|err| {
        Refusal::new(
            codes::MACRO_INVALID,
            format!(
                "map-macro could not read the macro body: {err} — steps are \
                 [{{\"hold\":[\"dpad.down\"],\"ms\":50}}] (exactly one of \"ms\"/\"frames\", \
                 optional \"allow_short\"), the policies are \"finish\"|\"abort\", \
                 \"ignore\"|\"restart\", \"none\"|\"any-input\"|\"opposing\", and the repeat \
                 policy is \"once\"|\"while-held\"|\"turbo\" (a turbo gives exactly one of \
                 \"turbo_hz\"/\"gap_ms\")"
            ),
        )
    })
}

/// The three restore destinations. An enum rather than a string because
/// "restore" is the one word every surface must NOT use on its own: `defaults`
/// writes the generic keyboard layout over an arcade panel, and a typo that
/// reached the daemon as a string would be a refusal one round trip later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreMode {
    /// `ksx_core::Preset::builtin_default()` — the GENERIC KEYBOARD layout
    /// (S=A, D=B, A=X, W=Y…), NOT "this preset as it shipped".
    Defaults,
    /// The preset as it was before this daemon lifetime's first `map` write.
    SessionBackup,
    /// The newest `<preset>.toml.bak-YYYYMMDD-HHMMSS`.
    LatestBackup,
}

impl RestoreMode {
    /// The wire word — the same three strings `ksx map --restore` takes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Defaults => "defaults",
            Self::SessionBackup => "session-backup",
            Self::LatestBackup => "latest-backup",
        }
    }

    pub fn parse(mode: &str) -> Option<Self> {
        match mode {
            "defaults" => Some(Self::Defaults),
            "session-backup" => Some(Self::SessionBackup),
            "latest-backup" => Some(Self::LatestBackup),
            _ => None,
        }
    }

    /// All three, in the order a UI offers them.
    pub const ALL: [Self; 3] = [Self::Defaults, Self::SessionBackup, Self::LatestBackup];
}

/// The three wire words, for a surface that validates a string at its edge
/// (the no-JavaScript form post) before it has a [`RestoreMode`].
pub const RESTORE_MODES: [&str; 3] = ["defaults", "session-backup", "latest-backup"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub preset: String,
    pub mode: RestoreMode,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reload: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearAllRequest {
    pub preset: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reload: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupsRequest {
    pub preset: String,
}

/// The `slot-assign` verb: **which preset a slot uses** (docs/CONTROL-SURFACE.md
/// honest gaps 1 and 5 — the one E5 verb the mapper never covered, because the
/// mapper edits bindings INSIDE a preset and this says which preset a slot
/// points at).
///
/// One slot, one preset, one file. Deliberately narrow: `ksx config
/// export|import` can already rewrite the whole document, and that is precisely
/// its problem — a whole-file rewrite loses the comments the annotated config
/// is for. This writes one field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotAssignRequest {
    /// 1..=`ksx_core::MAX_SLOTS`.
    pub slot: u8,
    /// The preset's `name` (its file's `name` field), e.g. `"IPAC P1"`.
    pub preset: String,
    /// A games.toml profile title. ABSENT means config.toml's `[[slot]]` list —
    /// the same either/or `ksx setup` asks about, spelled the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Apply it to a RUNNING session.
    ///
    /// Unlike every other `reload` on this protocol this one is a **bounce**,
    /// not a hot swap: see [`SlotAssignResponse::restarted`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub reload: bool,
}

impl SlotAssignRequest {
    /// The daemon's reader: one request object → this type, or the sentence to
    /// refuse it with. Same rule as [`MapRequest::from_json`] — the validation
    /// lives in the crate both sides link, so a client hears the identical
    /// words with no round trip.
    pub fn from_json(request: &serde_json::Value) -> Result<Self, Refusal> {
        // The bound is formatted from ksx-core's constant, never spelled out:
        // these sentences are what a client SEES, so a stale literal here is a
        // daemon telling a 16-player cabinet that slot 9 does not exist.
        let max = ksx_core::MAX_SLOTS;
        let Some(slot) = request.get("slot").and_then(serde_json::Value::as_u64) else {
            return Err(bad_request(format!(
                r#"slot-assign needs a "slot" (the slot NUMBER, 1..={max})"#
            )));
        };
        let Ok(slot) = u8::try_from(slot) else {
            return Err(Refusal::new(
                codes::BAD_SLOT,
                format!("slot {slot} is not a slot number (1..={max})"),
            ));
        };
        let Some(preset) = field(request, "preset") else {
            return Err(bad_request(
                r#"slot-assign needs a "preset" (the preset this slot should use)"#,
            ));
        };
        Ok(Self {
            slot,
            preset,
            profile: field(request, "profile"),
            reload: flag(request, "reload"),
        })
    }

    /// Where this write lands, as a sentence — `config.toml` or the profile.
    pub fn destination(&self) -> String {
        match &self.profile {
            Some(profile) => format!("profile \"{profile}\" (games.toml)"),
            None => "config.toml".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// One control-channel answer, typed per verb.
///
/// Deserialization is deliberately TOLERANT (every field defaults) and
/// serialization deliberately FAITHFUL (nothing is skipped): a daemon older
/// than this binary is missing fields rather than lying, and the drift test in
/// ksx-app re-serializes a real daemon answer through these types and checks
/// that nothing the daemon said was dropped on the way in.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Status(StatusResponse),
    Action(ActionResponse),
    Map(MapResponse),
    Macro(MacroResponse),
    Restore(RestoreResponse),
    Backups(BackupsResponse),
    SlotAssign(SlotAssignResponse),
    Learn(LearnResponse),
}

impl Response {
    /// Read the answer to `request`. The verb decides the shape — the protocol
    /// has no discriminator of its own, and inventing one would be a protocol
    /// change this layer is not allowed to make (docs/M9-DECISION.md §7).
    ///
    /// `Err` means the answer could not be READ (a torn line, a daemon that
    /// said something else entirely). A daemon that answered `ok: false` is not
    /// an error here: it is a refusal, it is typed, and each surface knows how
    /// to show its own.
    pub fn parse(request: &Request, value: serde_json::Value) -> Result<Self, Refusal> {
        let verb = request.verb();
        let read = |what: &'static str| {
            move |err: serde_json::Error| {
                Refusal::new(
                    codes::PIPE_ERROR,
                    format!("the daemon's answer to `{what}` could not be read: {err}"),
                )
            }
        };
        Ok(match request {
            Request::Status => Self::Status(serde_json::from_value(value).map_err(read(verb))?),
            Request::Start { .. } | Request::Stop | Request::Reload => {
                Self::Action(serde_json::from_value(value).map_err(read(verb))?)
            }
            Request::Map(_) => Self::Map(serde_json::from_value(value).map_err(read(verb))?),
            Request::MapMacro(_) => Self::Macro(serde_json::from_value(value).map_err(read(verb))?),
            Request::MapRestore(_) | Request::MapClearAll(_) => {
                Self::Restore(serde_json::from_value(value).map_err(read(verb))?)
            }
            Request::MapBackups(_) => {
                Self::Backups(serde_json::from_value(value).map_err(read(verb))?)
            }
            Request::SlotAssign(_) => {
                Self::SlotAssign(serde_json::from_value(value).map_err(read(verb))?)
            }
            Request::LearnKey | Request::LearnPoll | Request::LearnCancel => {
                Self::Learn(serde_json::from_value(value).map_err(read(verb))?)
            }
        })
    }

    /// The answer as the daemon would have written it. Used by the drift test
    /// and by an in-process host that wants to log what it answered.
    pub fn to_json(&self) -> serde_json::Value {
        let value = match self {
            Self::Status(r) => serde_json::to_value(r),
            Self::Action(r) => serde_json::to_value(r),
            Self::Map(r) => serde_json::to_value(r),
            Self::Macro(r) => serde_json::to_value(r),
            Self::Restore(r) => serde_json::to_value(r),
            Self::Backups(r) => serde_json::to_value(r),
            Self::SlotAssign(r) => serde_json::to_value(r),
            Self::Learn(r) => serde_json::to_value(r),
        };
        value.unwrap_or(serde_json::Value::Null)
    }
}

/// Answers that are just "it worked, here is the sentence" / "it did not".
macro_rules! refusal_of {
    ($ty:ty) => {
        impl $ty {
            /// The refusal this answer carries, or `None` when it succeeded.
            pub fn refusal(&self) -> Option<Refusal> {
                if self.ok {
                    return None;
                }
                Some(Refusal::from_wire(
                    self.code.as_deref(),
                    self.error
                        .clone()
                        .unwrap_or_else(|| "the daemon refused".to_owned()),
                ))
            }
        }
    };
}

/// `status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub ok: bool,
    /// `stopped` | `starting` | `running` | `failed` | `quitting`.
    #[serde(default)]
    pub run: String,
    #[serde(default)]
    pub slots: Option<u32>,
    /// Why the last session failed, when `run` is `failed`.
    #[serde(default)]
    pub message: Option<String>,
    /// The games.toml profile the daemon is pointed at.
    #[serde(default)]
    pub game: Option<String>,
    /// The daemon's own one-line self-description — the tray tooltip.
    #[serde(default)]
    pub tooltip: Option<String>,
    #[serde(default)]
    pub profiles: Vec<ProfileRow>,
    #[serde(default)]
    pub last: Option<LastSessionView>,
    #[serde(default)]
    pub live: Option<HealthView>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

refusal_of!(StatusResponse);

/// The health of a session that is OVER.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSessionView {
    #[serde(default)]
    pub stop_code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub reboot_required: bool,
    #[serde(default)]
    pub watchdog_tripped: bool,
    #[serde(default)]
    pub dropped_events: u64,
}

/// The health of the session running RIGHT NOW.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthView {
    #[serde(default)]
    pub reboot_required: bool,
    #[serde(default)]
    pub watchdog_tripped: bool,
    #[serde(default)]
    pub dropped_events: u64,
}

/// `start` / `stop` / `reload`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResponse {
    pub ok: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

refusal_of!(ActionResponse);

/// `map`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapResponse {
    pub ok: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub function: Option<String>,
    /// The FIRST key (`null` for a clear) — the one-key spelling every
    /// pre-list reader still sees.
    #[serde(default)]
    pub key: Option<String>,
    /// The control's whole list as the file now holds it.
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub when: Vec<String>,
    #[serde(default)]
    pub unless: Vec<String>,
    /// MULTI-BIND: the other controls of this preset the key drives now.
    #[serde(default)]
    pub also_drives: Vec<String>,
    #[serde(default)]
    pub moved_from: Option<MovedFromView>,
    #[serde(default)]
    pub conflicts: Vec<ConflictView>,
    #[serde(default)]
    pub flash: Vec<FlashView>,
    #[serde(default)]
    pub turbo_hz: Option<u32>,
    /// The rate the game will actually SEE (~15 Hz ceiling).
    #[serde(default)]
    pub turbo_effective_hz: Option<u32>,
    #[serde(default)]
    pub reloaded: bool,
    /// The live session took it with the pads left plugged.
    #[serde(default)]
    pub hot_swap: bool,
}

refusal_of!(MapResponse);

/// What `move_from` unbound.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedFromView {
    pub function: String,
    #[serde(default)]
    pub remaining: Vec<String>,
    #[serde(default)]
    pub unbound: bool,
}

/// One cross-slot conflict.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictView {
    /// Always `"profile"` from a current daemon.
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub function: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub slot: Option<u8>,
}

/// One chord constituent that is ALSO bound on its own — the honest "you will
/// see this fire for a moment first" caveat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashView {
    pub key: String,
    pub bound_to: String,
}

/// `map-macro`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroResponse {
    pub ok: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    /// The refusals one per row, so a page can list them.
    #[serde(default)]
    pub problems: Vec<String>,
    /// ADVISORIES from a write that SUCCEEDED. Never silent.
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub steps: Option<u32>,
    #[serde(default)]
    pub total_ms: Option<u64>,
    #[serde(default)]
    pub deleted: bool,
    /// Does the table RUN now?
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// This write was ONLY the `enabled` flag.
    #[serde(default)]
    pub toggled: bool,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub backup: Option<BackupView>,
    #[serde(default)]
    pub reloaded: bool,
    #[serde(default)]
    pub hot_swap: bool,
}

refusal_of!(MacroResponse);

/// `map-restore` and `map-clear-all` — one shape, because they are one write
/// with two destinations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResponse {
    pub ok: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    /// The destination word, echoed.
    #[serde(default)]
    pub mode: Option<String>,
    /// What the caller's confirm dialog promised, echoed back.
    #[serde(default)]
    pub wrote: Option<String>,
    #[serde(default)]
    pub backup: Option<BackupView>,
    #[serde(default)]
    pub reloaded: bool,
    #[serde(default)]
    pub hot_swap: bool,
}

refusal_of!(RestoreResponse);

/// `map-backups` — read-only, so it never reports a reload.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupsResponse {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub backups: Vec<BackupView>,
}

refusal_of!(BackupsResponse);

/// `slot-assign`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotAssignResponse {
    pub ok: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    /// The file that was written.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub slot: Option<u8>,
    #[serde(default)]
    pub preset: Option<String>,
    /// What the slot pointed at BEFORE. `None` when the slot was created — and
    /// that difference is the whole reason this field exists: a surface must be
    /// able to say "slot 3: IPAC P3 → Player 3" rather than only the new half.
    #[serde(default)]
    pub previous_preset: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    /// The slot did not exist in that file and was ADDED.
    #[serde(default)]
    pub created: bool,
    /// Nothing changed: the slot already used that preset. `ok` all the same —
    /// re-asserting a state is not an error — but a surface says "already" so a
    /// user does not go looking for the pad bounce that never came.
    #[serde(default)]
    pub unchanged: bool,
    /// The timestamped copy taken before the write.
    #[serde(default)]
    pub backup: Option<BackupView>,
    /// A running session was BOUNCED onto the new wiring: pads replugged.
    ///
    /// The one write on this protocol that never claims a hot swap. See
    /// [`crate::control::SlotOutcome::restarted`] for why.
    #[serde(default)]
    pub restarted: bool,
    /// `reload` was asked for and the daemon acted on it.
    #[serde(default)]
    pub reloaded: bool,
}

refusal_of!(SlotAssignResponse);

/// One timestamped restore point.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupView {
    /// `YYYYMMDD-HHMMSS`, sortable.
    pub stamp: String,
    /// The same instant as a human reads it: `2026-08-05 14:32:07 UTC`.
    pub label: String,
    pub path: String,
}

/// `learn-key` / `learn-poll` / `learn-cancel`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnResponse {
    pub ok: bool,
    /// `listening` | `hit` | `timeout` | `cancelled` | `failed` | `idle`.
    #[serde(default)]
    pub state: String,
    /// Bumped per observation, so a stale poll cannot be mistaken for this one.
    #[serde(default)]
    pub generation: Option<u64>,
    /// The visible countdown — `Some` only while listening.
    #[serde(default)]
    pub remaining_ms: Option<u64>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

refusal_of!(LearnResponse);

// ---------------------------------------------------------------------------
// Small shared readers — the daemon's field conventions, in one place
// ---------------------------------------------------------------------------

/// A string field, treating blank as absent (the daemon's own rule: a
/// whitespace `"profile"` means "no profile", not a profile called `" "`).
fn field(request: &serde_json::Value, name: &str) -> Option<String> {
    request
        .get(name)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
}

fn flag(request: &serde_json::Value, name: &str) -> bool {
    request
        .get(name)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// A string-array field, blanks dropped — same rule as [`field`], applied per
/// element.
fn list(request: &serde_json::Value, name: &str) -> Vec<String> {
    request
        .get(name)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn bad_request(message: impl Into<String>) -> Refusal {
    Refusal::new(codes::BAD_REQUEST, message)
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature
fn is_false(value: &bool) -> bool {
    !*value
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line each verb puts on the pipe is the line docs/CONTROL-SURFACE.md
    /// documents — byte for byte for the ones a human reads in that document.
    #[test]
    fn every_verb_serializes_to_the_documented_request_line() {
        assert_eq!(Request::Status.to_line(), r#"{"verb":"status"}"#);
        assert_eq!(Request::Stop.to_line(), r#"{"verb":"stop"}"#);
        assert_eq!(Request::Reload.to_line(), r#"{"verb":"reload"}"#);
        assert_eq!(Request::LearnKey.to_line(), r#"{"verb":"learn-key"}"#);
        assert_eq!(Request::LearnPoll.to_line(), r#"{"verb":"learn-poll"}"#);
        assert_eq!(Request::LearnCancel.to_line(), r#"{"verb":"learn-cancel"}"#);
        assert_eq!(
            Request::Start { profile: None }.to_line(),
            r#"{"verb":"start"}"#
        );
        assert_eq!(
            serde_json::to_value(Request::Start {
                profile: Some("Street Fighter".into())
            })
            .unwrap(),
            serde_json::json!({ "verb": "start", "profile": "Street Fighter" })
        );
    }

    /// Every request round-trips: what a client writes is what the daemon
    /// reads. This is the property that makes one type on both sides worth
    /// having.
    #[test]
    fn every_request_round_trips_through_json() {
        let requests = [
            Request::Status,
            Request::Start {
                profile: Some("Steam".into()),
            },
            Request::Stop,
            Request::Reload,
            Request::Map(MapRequest {
                preset: "IPAC P1".into(),
                function: "A".into(),
                keys: vec!["S".into(), "Enter".into()],
                force: true,
                reload: true,
                turbo_hz: Some(12),
                when: vec!["F".into()],
                ..MapRequest::default()
            }),
            Request::MapMacro(MapMacroRequest {
                preset: "SF P1".into(),
                name: "hadouken".into(),
                write: MacroWriteKind::Body(Box::new(
                    toml::from_str(
                        r#"
repeat = "turbo"
turbo_hz = 10
steps = [{ hold = ["A"], frames = 2, allow_short = true }]
"#,
                    )
                    .unwrap(),
                )),
                reload: true,
            }),
            Request::MapMacro(MapMacroRequest {
                preset: "SF P1".into(),
                name: "hadouken".into(),
                write: MacroWriteKind::Toggle(false),
                reload: false,
            }),
            Request::MapMacro(MapMacroRequest {
                preset: "SF P1".into(),
                name: "hadouken".into(),
                write: MacroWriteKind::Delete,
                reload: true,
            }),
            Request::MapRestore(RestoreRequest {
                preset: "IPAC P1".into(),
                mode: RestoreMode::LatestBackup,
                reload: true,
            }),
            Request::MapClearAll(ClearAllRequest {
                preset: "IPAC P1".into(),
                reload: true,
            }),
            Request::MapBackups(BackupsRequest {
                preset: "IPAC P1".into(),
            }),
            Request::LearnKey,
        ];
        for request in requests {
            let line = request.to_line();
            let back: Request = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("{line} did not read back: {err}"));
            assert_eq!(back, request, "{line}");
        }
    }

    /// THE REGRESSION, in type form. A macro body written with a `repeat`
    /// policy and its rate must arrive with them — and it must do so because
    /// the body IS `MacroFile`, not because somebody remembered to add two
    /// strings to a list.
    #[test]
    fn the_macro_body_is_the_files_own_type_so_no_field_can_be_dropped() {
        let file: ksx_config::MacroFile = toml::from_str(
            r#"
on_release = "abort"
retrigger = "restart"
interrupt = "opposing"
repeat = "turbo"
turbo_hz = 10
enabled = false
steps = [{ hold = ["dpad.down"], ms = 50 }, { hold = ["A"], frames = 3, allow_short = true }]
"#,
        )
        .unwrap();
        let request = MapMacroRequest {
            preset: "SF P1".into(),
            name: "hadouken".into(),
            write: MacroWriteKind::Body(Box::new(file.clone())),
            reload: true,
        };
        let wire = request.to_json();
        // Every field of the table is on the wire, by construction...
        assert_eq!(wire["repeat"], "turbo");
        assert_eq!(wire["turbo_hz"], 10);
        assert_eq!(wire["on_release"], "abort");
        assert_eq!(wire["retrigger"], "restart");
        assert_eq!(wire["interrupt"], "opposing");
        assert_eq!(wire["enabled"], false);
        assert_eq!(wire["steps"][1]["frames"], 3);
        assert_eq!(wire["steps"][1]["allow_short"], true);
        // ...and the daemon's reader gets the identical table back.
        assert_eq!(macro_body(&wire).unwrap(), file);

        // The envelope never leaks into the body, whatever it grows.
        for envelope in MACRO_ENVELOPE_FIELDS {
            let mut probe = wire.clone();
            probe[*envelope] = serde_json::json!("nonsense");
            assert!(
                macro_body(&probe).is_ok(),
                "`{envelope}` is envelope, not body"
            );
        }
    }

    /// The three meanings of a `map-macro` request survive the wire, and they
    /// are told apart by exactly what the protocol tells them apart by: an
    /// absent `steps`.
    #[test]
    fn the_three_macro_write_kinds_are_distinguishable_on_the_wire() {
        let toggle = MapMacroRequest {
            preset: "P".into(),
            name: "m".into(),
            write: MacroWriteKind::Toggle(false),
            reload: false,
        };
        let wire = toggle.to_json();
        assert_eq!(wire["enabled"], false);
        assert!(
            wire.get("steps").is_none(),
            "a toggle sends no body: {wire}"
        );
        assert_eq!(MapMacroRequest::from_json(&wire).unwrap(), toggle);

        let delete = MapMacroRequest {
            write: MacroWriteKind::Delete,
            ..toggle.clone()
        };
        let wire = delete.to_json();
        assert_eq!(wire["delete"], true);
        assert!(wire.get("steps").is_none(), "{wire}");
        assert_eq!(MapMacroRequest::from_json(&wire).unwrap(), delete);

        // A BODY write always declares its steps, even an empty grid — which
        // then earns the validator's refusal rather than being read as a
        // toggle or a typo.
        let empty = MapMacroRequest {
            write: MacroWriteKind::Body(Box::default()),
            ..toggle.clone()
        };
        let wire = empty.to_json();
        assert_eq!(wire["steps"], serde_json::json!([]));
        assert_eq!(MapMacroRequest::from_json(&wire).unwrap(), empty);
    }

    /// The refusals a malformed request earns, in the words the daemon has
    /// always used — the whole reason validation moved here rather than being
    /// copied here.
    #[test]
    fn malformed_requests_are_refused_in_the_daemons_own_words() {
        let err =
            MapRequest::from_json(&serde_json::json!({"function":"A","key":"G"})).unwrap_err();
        assert!(err.message.contains(r#"map needs a "preset""#), "{err}");
        assert_eq!(err.code, codes::BAD_REQUEST);

        let err = MapRequest::from_json(
            &serde_json::json!({"preset":"P","function":"A","key":"G","keys":["S"]}),
        )
        .unwrap_err();
        assert!(err.message.contains("not both"), "{err}");

        let err =
            MapRequest::from_json(&serde_json::json!({"preset":"P","function":"A"})).unwrap_err();
        assert!(err.message.contains(r#"map needs a "key""#), "{err}");

        let err =
            MapMacroRequest::from_json(&serde_json::json!({"preset":"P","name":"m"})).unwrap_err();
        assert!(err.message.contains(r#"map-macro needs "steps""#), "{err}");

        let err = MapMacroRequest::from_json(
            &serde_json::json!({"preset":"P","name":"m","enabled":"yes"}),
        )
        .unwrap_err();
        assert!(err.message.contains("true or false"), "{err}");

        let err = macro_body(&serde_json::json!({"steps":[{"hold":["A"],"ms":50}],
                                                 "repeat":"sometimes"}))
        .unwrap_err();
        assert!(err.message.contains("while-held"), "{err}");
        assert_eq!(err.code, codes::MACRO_INVALID);
    }

    /// A key list arrives however it was spelled, and `key_list` is the one
    /// answer to "what does this control hold now".
    #[test]
    fn a_map_request_reads_both_spellings_of_its_key_list() {
        let one = MapRequest::from_json(&serde_json::json!({
            "preset":"P","function":"A","key":"G"
        }))
        .unwrap();
        assert_eq!(one.key_list(), ["G"]);

        let many = MapRequest::from_json(&serde_json::json!({
            "preset":"P","function":"A","keys":["S","Enter",""," "]
        }))
        .unwrap();
        assert_eq!(many.key_list(), ["S", "Enter"], "blanks are not keys");

        let cleared =
            MapRequest::from_json(&serde_json::json!({"preset":"P","function":"A","clear":true}))
                .unwrap();
        assert!(cleared.key_list().is_empty());
        assert!(cleared.clear);
    }

    /// The restore destinations are the three the CLI takes, spelled once.
    #[test]
    fn restore_modes_agree_with_their_wire_words() {
        for (mode, word) in RestoreMode::ALL.into_iter().zip(RESTORE_MODES) {
            assert_eq!(mode.as_str(), word);
            assert_eq!(RestoreMode::parse(word), Some(mode));
            assert_eq!(serde_json::to_value(mode).unwrap(), serde_json::json!(word));
        }
        assert_eq!(RestoreMode::parse("factory-reset"), None);
    }

    /// An answer with `ok: false` is a typed refusal, and an answer from a
    /// daemon that gave no code is not guessed at.
    #[test]
    fn a_refused_answer_carries_its_code_and_sentence() {
        let refused: MapResponse = serde_json::from_value(serde_json::json!({
            "ok": false, "code": "conflict", "error": "refusing to bind G",
            "conflicts": [{"scope":"profile","preset":"IPAC P2","function":"A",
                           "profile":"Steam","slot":2}],
        }))
        .unwrap();
        let refusal = refused.refusal().expect("ok:false is a refusal");
        assert_eq!(refusal.code, codes::CONFLICT);
        assert_eq!(refusal.message, "refusing to bind G");
        assert_eq!(refused.conflicts[0].slot, Some(2));

        let plain: ActionResponse =
            serde_json::from_value(serde_json::json!({"ok": false, "error": "not running"}))
                .unwrap();
        assert_eq!(plain.refusal().unwrap().code, codes::REFUSED);

        let fine: ActionResponse =
            serde_json::from_value(serde_json::json!({"ok": true, "message": "stopped"})).unwrap();
        assert!(fine.refusal().is_none());
    }

    /// `slot-assign`'s two slot refusals quote the LIVE bound.
    ///
    /// These sentences are the only thing a client sees, and this crate links
    /// ksx-core, so there is no excuse for a literal: one hardcoded `1..=8`
    /// here is a daemon telling a 16-player cabinet that slot 9 is not a slot
    /// number. Asserted against the constant so a raise cannot go stale.
    #[test]
    fn the_slot_assign_refusals_quote_max_slots_not_a_literal() {
        let bound = format!("1..={}", ksx_core::MAX_SLOTS);

        let missing = SlotAssignRequest::from_json(&serde_json::json!({"preset": "P"}))
            .expect_err("no slot is a refusal");
        assert_eq!(missing.code, codes::BAD_REQUEST);
        assert!(missing.message.contains(&bound), "{}", missing.message);

        // Past `u8` entirely — the only slot value this reader itself rejects;
        // 1..=MAX_SLOTS proper is checked where the config is (ksx-app).
        let huge = SlotAssignRequest::from_json(&serde_json::json!({"slot": 999, "preset": "P"}))
            .expect_err("999 is not a slot");
        assert_eq!(huge.code, codes::BAD_SLOT);
        assert!(huge.message.contains(&bound), "{}", huge.message);

        // A slot past the old eight-player ceiling is an ordinary request to
        // this reader, whose only slot rule is "fits in u8" — pinned so nobody
        // adds a second, literal ceiling on the wire beside ksx-app's.
        let ninth = SlotAssignRequest::from_json(&serde_json::json!({"slot": 9, "preset": "P"}))
            .expect("slot 9 is a slot");
        assert_eq!(ninth.slot, 9);
    }

    /// **A surface decision that is currently enforced by a missing field.**
    ///
    /// `docs/SURFACES.md` §10 settles the slot persona menu by observing that
    /// `SlotAssignRequest` carries no persona, "so no surface can re-persona a
    /// slot until the wire type changes". That is a real guarantee — this is the
    /// whole config-writing wire, `slot-assign` is its only verb — but it is
    /// guaranteed by *absence*, which is the kind of invariant that disappears
    /// the moment someone adds a convenient field.
    ///
    /// So the field set is pinned. Adding `persona` here fails this test, and
    /// whoever adds it has to come and delete the assertion — which is exactly
    /// the moment the "which surface owns persona?" decision should be re-taken,
    /// rather than settled by whoever needed the field.
    ///
    /// Breaks against: any new field on the request, in either direction.
    #[test]
    fn slot_assign_carries_no_persona_and_the_doc_depends_on_that() {
        let full = SlotAssignRequest {
            slot: 1,
            preset: "P1".into(),
            profile: Some("MAME 4P".into()),
            reload: true,
        };
        let mut fields: Vec<String> = serde_json::to_value(&full)
            .unwrap()
            .as_object()
            .expect("a request is an object")
            .keys()
            .cloned()
            .collect();
        fields.sort();
        assert_eq!(
            fields,
            vec!["preset", "profile", "reload", "slot"],
            "the config-writing wire's field set — see docs/SURFACES.md §10"
        );

        // ...and the two optional halves stay optional, so the minimal request
        // a CLI or a form sends is still slot + preset and nothing else.
        let minimal = SlotAssignRequest {
            slot: 1,
            preset: "P1".into(),
            ..Default::default()
        };
        let json = serde_json::to_value(&minimal).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 2, "{json}");
    }
}
