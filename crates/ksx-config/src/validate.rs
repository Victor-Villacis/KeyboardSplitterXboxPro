//! Cross-file validation: structured issues, never panics, never fails.
//!
//! Loading is lenient (see [`crate::store`]); validation is where problems
//! become actionable. Every issue carries enough context for the CLI to print
//! a root-cause message (and serializes cleanly for `--json` output).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;

use crate::config::ConfigFile;
use crate::error::ConfigError;
use crate::function::{macro_name, parse_function, CONSUME, MACRO_PREFIX};
use crate::games::GamesFile;
use crate::preset::{BindingEntry, GuardedEntry, MacroFile, PresetFile};
use ksx_core::socd::{opposing_pairs, shadowing_chord};
use ksx_core::{
    Binding, Key, MacroSwitch, Persona, Preset, Repeat, Socd, TurboBinding, MAX_SLOTS,
    MAX_XINPUT_SLOTS, MIN_STEP_MS, TURBO_MAX_HZ,
};

/// One validation finding. All findings are non-fatal: the caller decides
/// whether to refuse to start emulation (unknown preset ref) or just warn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Issue {
    /// `settings.mouse_move_deadzone` outside 0..=12.
    MouseMoveDeadzoneOutOfRange { value: u8 },
    /// `settings.starting_user_index` outside 1..=4.
    StartingUserIndexOutOfRange { value: u8 },
    /// Two `[[device]]` entries share an alias (references become ambiguous).
    DuplicateDeviceAlias { alias: String },
    /// Two `[[slot]]` entries share a number.
    DuplicateSlotNumber { number: u8 },
    /// `[[slot]]` number outside 1..=[`MAX_SLOTS`].
    SlotNumberOutOfRange { number: u8 },
    /// More slots ask for an XInput persona than Windows has XInput slots.
    ///
    /// Not a ksx limit and not fixable by any virtual bus: Windows exposes
    /// exactly four. The extra pads would plug and then be invisible to every
    /// game, which is a failure that looks like success — so it is reported
    /// here, where the fix (a HID persona) can be named.
    TooManyXinputSlots { count: usize },
    /// A `[[slot]]` names a persona this BUILD of ksx cannot create.
    ///
    /// Not a typo and not an unknown name — the persona is real, it parses, and
    /// there is simply no code behind it yet ([`ksx_core::Persona::can_plug`]).
    /// Reported here for the same reason [`Issue::TooManyXinputSlots`] is: the
    /// alternative is a session that starts, plugs three pads, and dies on the
    /// fourth with a driver-flavoured error naming a driver that would not have
    /// helped.
    ///
    /// Deliberately **not** derived from a driver probe. A probe flips to
    /// "installed" the moment a user installs HIDMaestro, and this would then
    /// go quiet while the plug still failed.
    PersonaNotImplemented {
        slot: u8,
        /// Canonical name, as written in the file.
        persona: String,
        /// What is missing, verbatim from [`ksx_core::PadBackend::gap`] —
        /// carried rather than re-derived so `--json` consumers read the same
        /// sentence the text output prints.
        reason: String,
        /// The nearest persona that does work, so the fix is in the message.
        instead: String,
    },
    /// See [`Issue::PersonaNotImplemented`], for one game's slot list.
    GamePersonaNotImplemented {
        game: String,
        slot: u8,
        persona: String,
        reason: String,
        instead: String,
    },
    /// Slot references a preset that is neither a preset file nor a built-in.
    UnknownPresetRef { slot: u8, preset: String },
    /// Slot keyboard/mouse is neither a `[[device]]` alias nor an instance
    /// path (paths are recognized by containing `\`).
    UnknownDeviceRef { slot: u8, reference: String },
    /// Preset binding key that is not a function name.
    UnknownFunction { preset: String, function: String },
    /// Axis function with a value that is not `min`/`max`/i16.
    InvalidAxisValue { preset: String, function: String },
    /// Preset binding value that fails `Key::from_name`.
    UnknownKeyName {
        preset: String,
        function: String,
        key: String,
    },
    /// A chord's `when`/`unless` names a key that does not exist.
    UnknownGuardKey {
        preset: String,
        function: String,
        key: String,
    },
    /// A chord's guard lists its own trigger key. The trigger is already
    /// required to be down, so this is always a mistake (and it would make the
    /// chord look more specific than it is).
    GuardIncludesTriggerKey {
        preset: String,
        function: String,
        key: String,
    },
    /// A chord lists the same key in `when` and in `unless`: it can never fire.
    ContradictoryGuard {
        preset: String,
        function: String,
        key: String,
    },
    /// Two chords on the same trigger key, with guards of the SAME size, that
    /// can be satisfied at the same moment. Which one wins would be a build
    /// order accident, so it is refused instead of resolved.
    AmbiguousChords {
        preset: String,
        key: String,
        function: String,
        other: String,
    },
    /// Advisory: a chord constituent is ALSO bound on its own. With no
    /// deferral (ksx v1 has none, deliberately — see
    /// docs/INPUT-TRANSFORMS.md §1b), the game briefly sees that individual
    /// output between the first and second keypress.
    ChordConstituentAlsoBound {
        preset: String,
        function: String,
        key: String,
        bound_to: String,
    },
    /// Advisory: a `consume` row with no guard. Consumption is what a CHORD
    /// does; an unguarded row consumes nothing and drives nothing, so it is
    /// simply inert (docs/INPUT-TRANSFORMS.md §2.6).
    ConsumeWithoutGuard { preset: String, key: String },
    /// A macro's `[macros.<name>]` table has no steps, so nothing can happen.
    EmptyMacro { preset: String, name: String },
    /// A step gives both `ms` and `frames`, or neither.
    MacroStepBadDuration {
        preset: String,
        name: String,
        step: usize,
        reason: String,
    },
    /// A step's `hold` names something that is not a pad function.
    UnknownMacroHold {
        preset: String,
        name: String,
        step: usize,
        function: String,
    },
    /// A `macro.<name>` binding row names a macro this preset does not define.
    UnknownMacroRef {
        preset: String,
        function: String,
        name: String,
    },
    /// A `macro.<name>` row carries a when/unless guard. A chord that starts a
    /// sequence is not implemented; the guard would be silently ignored.
    GuardedMacroTrigger { preset: String, name: String },
    /// Advisory: ONE key starts several macros. Legal and deliberate (multi-bind
    /// is native to triggers too), and the reason it is said out loud is that
    /// what the game reads is then the SUPERPOSITION of every one of them —
    /// which contains states no single macro's step list has, and repeats for
    /// as long as the loudest `repeat` policy among them says.
    ///
    /// This is the shape behind both 2026-08-06 cabinet reports ("a phantom
    /// direction between my steps", "`once` still repeats while I hold it"), and
    /// neither is visible by reading the macro that was blamed.
    SharedMacroTrigger {
        preset: String,
        key: String,
        /// Every macro this key starts, in file order.
        macros: Vec<String>,
    },
    /// Advisory: a macro says `enabled = false`. It keeps its steps and its
    /// trigger row and never runs — deliberate, and exactly the kind of thing
    /// that is invisible from a cabinet, so it is said out loud every run.
    MacroDisabled { preset: String, name: String },
    /// Advisory: a slot says `macros = "off"` while its preset defines some.
    /// The other half of [`Issue::MacroDisabled`]: "my macro does nothing" has
    /// two causes and this is the one you cannot see by reading the preset.
    SlotMacrosOff {
        slot: u8,
        preset: String,
        /// How many macros the preset defines — all of them silenced.
        macros: usize,
    },
    /// Two `[macros]` tables whose names differ only in case. Macro names are
    /// matched case-insensitively (function names are), so one would shadow the
    /// other and a trigger would silently start the wrong sequence.
    DuplicateMacroName { preset: String, name: String },
    /// Advisory: a step asked for less than [`ksx_core::MIN_STEP_MS`] and was
    /// RAISED to it. The macro still works; it takes longer than the file says
    /// (docs/INPUT-TRANSFORMS.md §0.2, the sampling rule).
    MacroStepRaised {
        preset: String,
        name: String,
        step: usize,
        ms: u32,
    },
    /// Advisory: a step is shorter than [`ksx_core::MIN_STEP_MS`] and said
    /// `allow_short`, so ksx runs it as written — and a 60 Hz poller may never
    /// see it. This is the opt-out being honored, and said out loud.
    MacroStepMayBeMissed {
        preset: String,
        name: String,
        step: usize,
        ms: u32,
    },
    /// Advisory, and the one that explains "the diagonal never comes out": a
    /// macro step holds a DIRECTION on a mechanism this preset does not drive —
    /// dpad bits in a preset whose stick is the left analog axes, or the
    /// reverse. The engine publishes exactly what the step says; a game that
    /// reads the other mechanism sees that step as neutral, so a ↓ / ↓→ / →
    /// motion loses whichever step was written on the wrong one.
    MacroHoldsOtherMechanism {
        preset: String,
        name: String,
        step: usize,
        function: String,
        /// What the step drives, in words ("the dpad", "the left stick").
        holds: String,
        /// What this preset's own direction keys drive.
        preset_drives: String,
    },
    /// A macro gives both `turbo_hz` and `gap_ms`, or is `repeat = "turbo"`
    /// with neither.
    MacroTurboBadRate {
        preset: String,
        name: String,
        reason: String,
    },
    /// Advisory: the requested `turbo_hz` is not deliverable — either above the
    /// [`ksx_core::TURBO_MAX_HZ`] sampling ceiling, or faster than the macro's
    /// own run plus a gap a 60 Hz poller can see. The effective rate is stated.
    TurboRateClamped {
        preset: String,
        name: String,
        asked_hz: u32,
        effective_hz: u32,
    },
    /// Advisory: an explicit `gap_ms` below the sampling floor was RAISED. A
    /// gap the game never samples is not a gap — it reads one long hold.
    TurboGapRaised {
        preset: String,
        name: String,
        ms: u32,
        raised_to: u32,
    },
    /// Advisory: `turbo_hz`/`gap_ms` on a macro that does not repeat. The
    /// number is kept (flipping `repeat` back and forth must not lose it) and
    /// does nothing until `repeat = "turbo"`.
    TurboRateWithoutTurbo {
        preset: String,
        name: String,
        repeat: String,
    },
    /// Advisory: a per-binding `turbo_hz` (docs/INPUT-TRANSFORMS.md §3) that is
    /// not deliverable as written. Same arithmetic as the macro one and stated
    /// the same way — both numbers, never a silent substitution.
    BindingTurboClamped {
        preset: String,
        function: String,
        asked_hz: u32,
        effective_hz: u32,
    },
    /// Two rows on ONE function give different `turbo_hz`. Turbo belongs to the
    /// output, so there is exactly one rate per function; which of two won
    /// would be a file-order accident, so it is refused instead of resolved.
    ConflictingTurboRates {
        preset: String,
        function: String,
        first_hz: u32,
        other_hz: u32,
    },
    /// Advisory: `turbo_hz` on a `consume` row. A consume-only chord drives no
    /// endpoint at all, so there is nothing for a rate to auto-fire.
    TurboOnConsume { preset: String, function: String },
    /// `macro.<name> = { key = …, turbo_hz = … }`. A macro repeats by saying so
    /// in its own table; a second spelling for the same thing would make "which
    /// one runs" something a reader has to remember.
    GuardedMacroTurbo { preset: String, name: String },
    /// Advisory: a slot's `socd` policy would have generated a rule for a key
    /// pair the preset already chords by hand. The hand-written one wins —
    /// a deliberate statement beats a default — and this says so out loud.
    SocdShadowedByChord {
        slot: u8,
        preset: String,
        socd: String,
        keys: String,
        function: String,
    },
    /// Game slot number outside 1..=[`MAX_SLOTS`].
    GameSlotNumberOutOfRange { game: String, number: u8 },
    /// See [`Issue::TooManyXinputSlots`], for one game's slot list.
    GameTooManyXinputSlots { game: String, count: usize },
    /// Two slots of one game share a number.
    GameDuplicateSlotNumber { game: String, number: u8 },
    /// Game slot references an unknown preset.
    GameUnknownPresetRef {
        game: String,
        slot: u8,
        preset: String,
    },
    /// Advisory `user_index` outside 1..=4.
    GameUserIndexOutOfRange { game: String, slot: u8, value: u8 },
}

impl Issue {
    /// Is this a piece of ADVICE rather than a fault?
    ///
    /// Everything validation reports is worth saying, but not everything is
    /// worth refusing to start over. An advisory describes a configuration
    /// that works exactly as written and merely has a cost, or a consequence,
    /// the user should know about: the chord flash
    /// ([`Issue::ChordConstituentAlsoBound`] — binding a chord over keys that
    /// already do something is a legitimate, deliberate choice), an inert
    /// `consume` row, and a hand-written chord winning over a generated SOCD
    /// rule. All three print as warnings and the session starts.
    /// A short macro step is advisory on both sides on purpose: raising it
    /// keeps the macro *correct* (it just runs longer), and honoring
    /// `allow_short` is the author having been asked and having answered. What
    /// is never allowed is silence — both print, every run.
    pub fn is_advisory(&self) -> bool {
        matches!(
            self,
            Issue::ChordConstituentAlsoBound { .. }
                | Issue::ConsumeWithoutGuard { .. }
                | Issue::SocdShadowedByChord { .. }
                | Issue::MacroStepRaised { .. }
                | Issue::MacroStepMayBeMissed { .. }
                // Advisory because the file is not WRONG — the engine does
                // exactly what it says. It is the game that will not be
                // looking there, and only the user knows what the game reads.
                | Issue::MacroHoldsOtherMechanism { .. }
                | Issue::TurboRateClamped { .. }
                | Issue::TurboGapRaised { .. }
                | Issue::TurboRateWithoutTurbo { .. }
                | Issue::BindingTurboClamped { .. }
                | Issue::TurboOnConsume { .. }
                // Advisory because several macros on one key is a real thing to
                // want; what it is not is something to discover from a cabinet.
                // Kept advisory rather than promoted to a fault even though the
                // WRITERS now refuse to create one: files that already have the
                // shape (hand-edited, or written before the rule) must keep
                // loading and keep warning, not break.
                | Issue::SharedMacroTrigger { .. }
                // Advisory on both sides for the same reason: switching a macro
                // off is a deliberate act, and the only failure mode is
                // forgetting you did it.
                | Issue::MacroDisabled { .. }
                | Issue::SlotMacrosOff { .. }
        )
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::MouseMoveDeadzoneOutOfRange { value } => {
                write!(
                    f,
                    "settings.mouse_move_deadzone is {value}, allowed range is 0..=12"
                )
            }
            Issue::StartingUserIndexOutOfRange { value } => {
                write!(
                    f,
                    "settings.starting_user_index is {value}, allowed range is 1..=4"
                )
            }
            Issue::DuplicateDeviceAlias { alias } => {
                write!(f, "more than one [[device]] uses alias '{alias}'")
            }
            Issue::DuplicateSlotNumber { number } => {
                write!(f, "more than one [[slot]] uses number {number}")
            }
            Issue::SlotNumberOutOfRange { number } => {
                write!(f, "[[slot]] number {number} is outside 1..={MAX_SLOTS}")
            }
            Issue::TooManyXinputSlots { count } => {
                write!(
                    f,
                    "{count} slots use persona '{}', but Windows has only {MAX_XINPUT_SLOTS} XInput slots; \
                     give the extra slots persona '{}' (HID/DirectInput — read by MAME, RetroArch, Steam Input)",
                    Persona::Xbox360,
                    Persona::PlayStation
                )
            }
            Issue::PersonaNotImplemented {
                slot,
                persona,
                reason,
                instead,
            } => write!(
                f,
                "slot {slot}: persona '{persona}' cannot be plugged by this build of ksx — \
                 {reason}. The name is valid and the file is fine; there is just nothing \
                 behind it yet. Use persona '{instead}' on that [[slot]]"
            ),
            Issue::GamePersonaNotImplemented {
                game,
                slot,
                persona,
                reason,
                instead,
            } => write!(
                f,
                "game '{game}': slot {slot} persona '{persona}' cannot be plugged by this \
                 build of ksx — {reason}. Use persona '{instead}' on that slot"
            ),
            Issue::UnknownPresetRef { slot, preset } => {
                write!(f, "slot {slot} references preset '{preset}', which is neither a preset file nor a built-in")
            }
            Issue::UnknownDeviceRef { slot, reference } => {
                write!(f, "slot {slot} references device '{reference}', which is neither a [[device]] alias nor an instance path")
            }
            Issue::UnknownFunction { preset, function } => {
                write!(f, "preset '{preset}': '{function}' is not a function name")
            }
            Issue::InvalidAxisValue { preset, function } => {
                write!(
                    f,
                    "preset '{preset}': '{function}' needs min, max or a signed 16-bit value"
                )
            }
            Issue::UnknownKeyName {
                preset,
                function,
                key,
            } => {
                write!(
                    f,
                    "preset '{preset}': '{function}' is bound to unknown key '{key}'"
                )
            }
            Issue::UnknownGuardKey {
                preset,
                function,
                key,
            } => write!(
                f,
                "preset '{preset}': '{function}' is guarded by unknown key '{key}' \
                 (`ksx monitor` shows the name for any key you press)"
            ),
            Issue::GuardIncludesTriggerKey {
                preset,
                function,
                key,
            } => write!(
                f,
                "preset '{preset}': '{function}' is triggered by '{key}' and also guards on \
                 '{key}' — drop it from when/unless; the trigger is always required"
            ),
            Issue::ContradictoryGuard {
                preset,
                function,
                key,
            } => write!(
                f,
                "preset '{preset}': '{function}' requires '{key}' in `when` and forbids it in \
                 `unless`, so it can never fire"
            ),
            Issue::AmbiguousChords {
                preset,
                key,
                function,
                other,
            } => write!(
                f,
                "preset '{preset}': '{function}' and '{other}' are both triggered by '{key}' \
                 with guards of the same size and can be satisfied together — make one of them \
                 more specific (a bigger guard wins; equal size is a coin flip, so it is \
                 refused)"
            ),
            Issue::ChordConstituentAlsoBound {
                preset,
                function,
                key,
                bound_to,
            } => write!(
                f,
                "preset '{preset}': chord '{function}' uses '{key}', which is also bound on its \
                 own to '{bound_to}' — ksx does not defer input, so pressing '{key}' first makes \
                 the game see '{bound_to}' for a moment before the chord takes over. Prefer a \
                 chord key with no individual binding (then consumption is free and instant)"
            ),
            Issue::ConsumeWithoutGuard { preset, key } => write!(
                f,
                "preset '{preset}': '{CONSUME}' is bound to '{key}' with no when/unless guard, \
                 so it does nothing — consumption is what a chord does, and a chord needs a \
                 guard (try `{CONSUME} = {{ key = \"{key}\", when = [\"<other key>\"] }}`)"
            ),
            Issue::EmptyMacro { preset, name } => write!(
                f,
                "preset '{preset}': macro '{name}' has no steps, so triggering it does nothing \
                 (give [macros.{name}] a `steps` list)"
            ),
            Issue::MacroStepBadDuration {
                preset,
                name,
                step,
                reason,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' step {step} {reason} — `frames` is 60 Hz and                  is a readable unit, not a promise that the game samples exactly that many                  frames (docs/INPUT-TRANSFORMS.md §1c)"
            ),
            Issue::UnknownMacroHold {
                preset,
                name,
                step,
                function,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' step {step} holds '{function}', which is not \
                 a function name (try A, lt, dpad.down, lx.min)"
            ),
            Issue::MacroHoldsOtherMechanism {
                preset,
                name,
                step,
                function,
                holds,
                preset_drives,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' step {step} holds '{function}', which drives \
                 {holds} — but this preset's own direction keys drive {preset_drives}. ksx \
                 publishes the step exactly as written; a game reading {preset_drives} sees that \
                 step as NEUTRAL, which is what makes a down/down-forward/forward motion come out \
                 missing whichever step was written on the other one. Write the step's holds with \
                 the same functions the [bindings] use (docs/INPUT-TRANSFORMS.md §1c)"
            ),
            Issue::MacroTurboBadRate {
                preset,
                name,
                reason,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' {reason} — a turbo's rate is `turbo_hz = <n>` \
                 or `gap_ms = <n>`, exactly one of them"
            ),
            Issue::TurboRateClamped {
                preset,
                name,
                asked_hz,
                effective_hz,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' asks for turbo_hz = {asked_hz} and will run at \
                 about {effective_hz} Hz. A 60 Hz poller resolves at most {TURBO_MAX_HZ} press/\
                 release pairs a second (§0.2), and each run plus its gap must survive being \
                 sampled — so the rate is capped by the macro's own length, not refused"
            ),
            Issue::TurboGapRaised {
                preset,
                name,
                ms,
                raised_to,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' asks for a {ms} ms turbo gap and gets \
                 {raised_to} ms — a released window a 60 Hz poller never samples is not a gap, \
                 the game reads one long hold and the turbo does nothing (§0.2)"
            ),
            Issue::TurboRateWithoutTurbo {
                preset,
                name,
                repeat,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' sets a turbo rate but is \
                 `repeat = \"{repeat}\"`, so the rate does nothing until it says \
                 `repeat = \"turbo\"` (the number is kept)"
            ),
            Issue::BindingTurboClamped {
                preset,
                function,
                asked_hz,
                effective_hz,
            } => write!(
                f,
                "preset '{preset}': '{function}' asks for turbo_hz = {asked_hz} and will \
                 auto-fire at about {effective_hz} Hz. One cycle is a press AND a release, a \
                 60 Hz poller resolves at most {TURBO_MAX_HZ} of those a second, and each half \
                 must survive being sampled ({MIN_STEP_MS} ms) — so the rate is capped rather \
                 than refused (docs/INPUT-TRANSFORMS.md §3)"
            ),
            Issue::ConflictingTurboRates {
                preset,
                function,
                first_hz,
                other_hz,
            } => write!(
                f,
                "preset '{preset}': '{function}' gives two turbo rates ({first_hz} Hz and \
                 {other_hz} Hz). Turbo belongs to the OUTPUT, not to the key — several keys on \
                 one function share one clock — so give the rate once, on one row"
            ),
            Issue::GuardedMacroTurbo { preset, name } => write!(
                f,
                "preset '{preset}': '{MACRO_PREFIX}{name}' sets turbo_hz, but a macro repeats by \
                 saying `repeat = \"turbo\"` in its own [macros.{name}] table — per-binding turbo \
                 is for the plain bindings a macro is the alternative to (§3)"
            ),
            Issue::TurboOnConsume { preset, function } => write!(
                f,
                "preset '{preset}': '{function}' sets turbo_hz, but `consume` drives no endpoint \
                 at all — its whole effect is suppressing its constituents, and there is nothing \
                 there to auto-fire"
            ),
            Issue::UnknownMacroRef {
                preset,
                function,
                name,
            } => write!(
                f,
                "preset '{preset}': '{function}' triggers macro '{name}', which this preset does \
                 not define — add a [macros.{name}] table with a `steps` list"
            ),
            Issue::GuardedMacroTrigger { preset, name } => write!(
                f,
                "preset '{preset}': '{MACRO_PREFIX}{name}' carries a when/unless guard, but a \
                 macro is started by a key — a chord that starts a sequence is not implemented"
            ),
            Issue::MacroDisabled { preset, name } => write!(
                f,
                "preset '{preset}': macro '{name}' is `enabled = false`, so pressing its trigger \
                 key does nothing — the steps and the `{MACRO_PREFIX}{name}` row are still there, \
                 waiting for `enabled = true` (or `ksx macro --preset \"{preset}\" --name {name} \
                 --enable`)"
            ),
            Issue::SlotMacrosOff {
                slot,
                preset,
                macros,
            } => write!(
                f,
                "slot {slot}: macros = \"off\", so none of the {macros} macro(s) preset '{preset}' \
                 defines will run on it — nothing is deleted and each macro's own `enabled` is \
                 untouched; set `macros = \"on\"` on that [[slot]] to bring them back"
            ),
            Issue::DuplicateMacroName { preset, name } => write!(
                f,
                "preset '{preset}': more than one [macros] table is called '{name}' (macro names \
                 are matched ignoring case, so one would shadow the other)"
            ),
            Issue::SharedMacroTrigger {
                preset,
                key,
                macros,
            } => write!(
                f,
                "preset '{preset}': key '{key}' starts {} macros ({}) — they all run at once, so \
                 the game reads their SUPERPOSITION, which can hold directions no single one of \
                 them has, and it keeps repeating if ANY of them says `repeat = \"while-held\"` \
                 or `\"turbo\"`. If only one of them is wanted, delete the other \
                 `{MACRO_PREFIX}<name> = \"{key}\"` rows or move them to their own keys",
                macros.len(),
                macros.join(", "),
            ),
            Issue::MacroStepRaised {
                preset,
                name,
                step,
                ms,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' step {step} asks for {ms} ms and was raised to \
                 {MIN_STEP_MS} ms — a game polling at 60 Hz samples every ~17 ms, so anything \
                 shorter is not unreliable, it is invisible. Set `allow_short = true` on that \
                 step if you really mean it"
            ),
            Issue::MacroStepMayBeMissed {
                preset,
                name,
                step,
                ms,
            } => write!(
                f,
                "preset '{preset}': macro '{name}' step {step} is {ms} ms with `allow_short`, so \
                 ksx runs it as written — a game polling at 60 Hz may never see it"
            ),
            Issue::SocdShadowedByChord {
                slot,
                preset,
                socd,
                keys,
                function,
            } => write!(
                f,
                "slot {slot}: socd = '{socd}' would clean '{keys}' in preset '{preset}', but a \
                 chord on those keys is already written by hand ('{function}') — the hand-written \
                 one wins and nothing is generated for that pair"
            ),
            Issue::GameSlotNumberOutOfRange { game, number } => {
                write!(
                    f,
                    "game '{game}': slot number {number} is outside 1..={MAX_SLOTS}"
                )
            }
            Issue::GameTooManyXinputSlots { game, count } => {
                write!(
                    f,
                    "game '{game}': {count} slots use persona '{}', but Windows has only \
                     {MAX_XINPUT_SLOTS} XInput slots; give the extra slots persona '{}'",
                    Persona::Xbox360,
                    Persona::PlayStation
                )
            }
            Issue::GameDuplicateSlotNumber { game, number } => {
                write!(f, "game '{game}': more than one slot uses number {number}")
            }
            Issue::GameUnknownPresetRef { game, slot, preset } => {
                write!(
                    f,
                    "game '{game}': slot {slot} references unknown preset '{preset}'"
                )
            }
            Issue::GameUserIndexOutOfRange { game, slot, value } => {
                write!(
                    f,
                    "game '{game}': slot {slot} user_index is {value}, allowed range is 1..=4"
                )
            }
        }
    }
}

/// Validate the main config plus the loaded preset files. Built-in presets
/// (`default`, `empty`) always count as known.
pub fn validate(config: &ConfigFile, presets: &[PresetFile]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let known_presets = known_preset_names(presets);

    let settings = &config.settings;
    if settings.mouse_move_deadzone > 12 {
        issues.push(Issue::MouseMoveDeadzoneOutOfRange {
            value: settings.mouse_move_deadzone,
        });
    }
    if !(1..=4).contains(&settings.starting_user_index) {
        issues.push(Issue::StartingUserIndexOutOfRange {
            value: settings.starting_user_index,
        });
    }

    let mut alias_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for device in &config.devices {
        *alias_counts.entry(device.alias.as_str()).or_default() += 1;
    }
    for (alias, count) in &alias_counts {
        if *count > 1 {
            issues.push(Issue::DuplicateDeviceAlias {
                alias: (*alias).to_owned(),
            });
        }
    }

    let mut number_counts: BTreeMap<u8, usize> = BTreeMap::new();
    for slot in &config.slots {
        *number_counts.entry(slot.number).or_default() += 1;
    }
    for (number, count) in &number_counts {
        if *count > 1 {
            issues.push(Issue::DuplicateSlotNumber { number: *number });
        }
    }

    for slot in &config.slots {
        if slot.number == 0 || slot.number > MAX_SLOTS {
            issues.push(Issue::SlotNumberOutOfRange {
                number: slot.number,
            });
        }
        if !known_presets.contains(slot.preset.as_str()) {
            issues.push(Issue::UnknownPresetRef {
                slot: slot.number,
                preset: slot.preset.clone(),
            });
        }
        if let Some((reason, instead)) = persona_gap(slot.persona) {
            issues.push(Issue::PersonaNotImplemented {
                slot: slot.number,
                persona: slot.persona.to_string(),
                reason,
                instead,
            });
        }
        for reference in [&slot.keyboard, &slot.mouse].into_iter().flatten() {
            let is_path = reference.contains('\\');
            let is_alias = alias_counts.contains_key(reference.as_str());
            if !is_path && !is_alias {
                issues.push(Issue::UnknownDeviceRef {
                    slot: slot.number,
                    reference: reference.clone(),
                });
            }
        }
    }

    let xinput_slots = config
        .slots
        .iter()
        .filter(|s| s.persona.is_xinput())
        .count();
    if xinput_slots > usize::from(MAX_XINPUT_SLOTS) {
        issues.push(Issue::TooManyXinputSlots {
            count: xinput_slots,
        });
    }

    for preset in presets {
        validate_preset(preset, &mut issues);
    }

    let cores = core_presets(presets);
    for slot in &config.slots {
        socd_issues(&cores, slot.number, &slot.preset, slot.socd, &mut issues);
        macros_off_issue(&cores, slot.number, &slot.preset, slot.macros, &mut issues);
    }
    issues
}

/// Validate `games.toml` against the loaded preset files.
pub fn validate_games(games: &GamesFile, presets: &[PresetFile]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let known_presets = known_preset_names(presets);
    let cores = core_presets(presets);
    for game in &games.games {
        let mut number_counts: BTreeMap<u8, usize> = BTreeMap::new();
        for slot in &game.slots {
            *number_counts.entry(slot.number).or_default() += 1;
        }
        for (number, count) in &number_counts {
            if *count > 1 {
                issues.push(Issue::GameDuplicateSlotNumber {
                    game: game.title.clone(),
                    number: *number,
                });
            }
        }
        for slot in &game.slots {
            if slot.number == 0 || slot.number > MAX_SLOTS {
                issues.push(Issue::GameSlotNumberOutOfRange {
                    game: game.title.clone(),
                    number: slot.number,
                });
            }
            if !known_presets.contains(slot.preset.as_str()) {
                issues.push(Issue::GameUnknownPresetRef {
                    game: game.title.clone(),
                    slot: slot.number,
                    preset: slot.preset.clone(),
                });
            }
            if let Some((reason, instead)) = persona_gap(slot.persona) {
                issues.push(Issue::GamePersonaNotImplemented {
                    game: game.title.clone(),
                    slot: slot.number,
                    persona: slot.persona.to_string(),
                    reason,
                    instead,
                });
            }
            socd_issues(&cores, slot.number, &slot.preset, slot.socd, &mut issues);
            macros_off_issue(&cores, slot.number, &slot.preset, slot.macros, &mut issues);
            if let Some(value) = slot.user_index {
                if !(1..=4).contains(&value) {
                    issues.push(Issue::GameUserIndexOutOfRange {
                        game: game.title.clone(),
                        slot: slot.number,
                        value,
                    });
                }
            }
        }
        let xinput_slots = game.slots.iter().filter(|s| s.persona.is_xinput()).count();
        if xinput_slots > usize::from(MAX_XINPUT_SLOTS) {
            issues.push(Issue::GameTooManyXinputSlots {
                game: game.title.clone(),
                count: xinput_slots,
            });
        }
    }
    issues
}

/// The gap between a persona a file may name and one ksx can build, or `None`
/// when there is none. Returns `(what is missing, what to write instead)`.
///
/// Reads [`Persona::can_plug`] and nothing else — never a driver probe. A probe
/// answers "is HIDMaestro on this machine", which is a different question with
/// a different answer: install it and the probe says yes while the plug still
/// fails, so a probe-based check would go quiet exactly when it was needed.
fn persona_gap(persona: Persona) -> Option<(String, String)> {
    if persona.can_plug() {
        return None;
    }
    Some((
        persona
            .backend()
            .gap()
            .unwrap_or("no reason recorded")
            .to_owned(),
        persona.nearest_pluggable().to_string(),
    ))
}

/// Every preset that can be resolved to the core model, by name — the
/// built-ins included, because a slot may reference one. Files that fail to
/// convert are skipped: their own issues already name the reason.
fn core_presets(presets: &[PresetFile]) -> BTreeMap<String, Preset> {
    let mut out: BTreeMap<String, Preset> = Preset::builtins()
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();
    for file in presets {
        if let Ok(core) = file.to_core() {
            out.insert(file.name.clone(), core);
        }
    }
    out
}

/// The one thing a `socd` setting can be wrong about: it collides with a chord
/// the user wrote by hand. Generation skips those pairs (see
/// [`ksx_core::socd::generate`]); this is where that silence gets a voice.
fn socd_issues(
    cores: &BTreeMap<String, Preset>,
    slot: u8,
    preset_name: &str,
    policy: Socd,
    issues: &mut Vec<Issue>,
) {
    if !policy.is_active() {
        return;
    }
    let Some(core) = cores.get(preset_name) else {
        return; // an unknown preset ref is already reported
    };
    for pair in opposing_pairs(core) {
        let Some(chord) = shadowing_chord(core, &pair) else {
            continue;
        };
        issues.push(Issue::SocdShadowedByChord {
            slot,
            preset: preset_name.to_owned(),
            socd: policy.to_string(),
            keys: format!("{}+{}", pair.keys.0.name(), pair.keys.1.name()),
            function: crate::function_name(&chord.binding),
        });
    }
}

/// The master switch, said out loud — but only when it actually silences
/// something. A slot with `macros = "off"` and a preset that defines none is a
/// setting with no consequence, and validation does not narrate those.
fn macros_off_issue(
    cores: &BTreeMap<String, Preset>,
    slot: u8,
    preset_name: &str,
    switch: MacroSwitch,
    issues: &mut Vec<Issue>,
) {
    if switch.is_on() {
        return;
    }
    let Some(core) = cores.get(preset_name) else {
        return; // an unknown preset ref is already reported
    };
    if core.macros.defs.is_empty() {
        return;
    }
    issues.push(Issue::SlotMacrosOff {
        slot,
        preset: preset_name.to_owned(),
        macros: core.macros.defs.len(),
    });
}

fn known_preset_names(presets: &[PresetFile]) -> BTreeSet<&str> {
    let mut names: BTreeSet<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    names.insert(Preset::DEFAULT_NAME);
    names.insert(Preset::EMPTY_NAME);
    names
}

fn validate_preset(preset: &PresetFile, issues: &mut Vec<Issue>) {
    let mut pairs = Vec::new();
    for (function, entry) in &preset.bindings {
        flatten_bindings(function, entry, &mut pairs);
    }

    validate_macros(preset, &pairs, issues);

    let mut checked_functions = BTreeSet::new();
    for (function, flat) in &pairs {
        // Macro rows are their own grammar (`macro.<name>`), checked above and
        // deliberately never fed to the pad-function vocabulary — the key name
        // below is still checked, because a trigger is still a key.
        let is_macro = macro_name(function).is_some();
        if !is_macro && checked_functions.insert(function.clone()) {
            match parse_function(function) {
                Ok(_) => {}
                Err(ConfigError::InvalidAxisValue(_)) => issues.push(Issue::InvalidAxisValue {
                    preset: preset.name.clone(),
                    function: function.clone(),
                }),
                Err(_) => issues.push(Issue::UnknownFunction {
                    preset: preset.name.clone(),
                    function: function.clone(),
                }),
            }
        }
        let key = match flat {
            Flat::Plain(key) => key,
            Flat::Guard(guard) => guard.key.as_str(),
        };
        if Key::from_name(key).is_none() {
            issues.push(Issue::UnknownKeyName {
                preset: preset.name.clone(),
                function: function.clone(),
                key: key.to_owned(),
            });
        }
        // `consume` only means something with a guard: it is a chord whose
        // whole output is the suppression of its constituents.
        let unguarded = match flat {
            Flat::Plain(_) => true,
            Flat::Guard(guard) => guard.when.is_empty() && guard.unless.is_empty(),
        };
        if unguarded && matches!(parse_function(function), Ok(Binding::Consume)) {
            issues.push(Issue::ConsumeWithoutGuard {
                preset: preset.name.clone(),
                key: key.to_owned(),
            });
        }
    }

    validate_chords(preset, &pairs, issues);
    validate_binding_turbo(preset, &pairs, issues);
}

/// Per-binding auto-fire (docs/INPUT-TRANSFORMS.md §3).
///
/// Three things can go wrong and exactly one of them is fatal. The rate being
/// undeliverable is not: the engine runs the closest thing a 60 Hz sampler can
/// see, and saying BOTH numbers is the whole point of resolving the clamp in
/// one place. Two rows on one function disagreeing IS fatal, because turbo
/// belongs to the output and picking a winner by file order is the kind of
/// silent decision this project refuses to make.
fn validate_binding_turbo(
    preset: &PresetFile,
    pairs: &[(String, Flat<'_>)],
    issues: &mut Vec<Issue>,
) {
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for (function, flat) in pairs {
        let Flat::Guard(guard) = flat else { continue };
        let Some(hz) = guard.turbo_hz else { continue };
        // `macro.<name>` with a rate is refused at load time
        // (`ConfigError::TurboOnMacroTrigger`); named here too so `ksx check`
        // reports it rather than only failing to parse.
        if let Some(name) = macro_name(function) {
            issues.push(Issue::GuardedMacroTurbo {
                preset: preset.name.clone(),
                name: name.to_owned(),
            });
            continue;
        }
        match seen.get(function.as_str()) {
            Some(&first) if first != hz => issues.push(Issue::ConflictingTurboRates {
                preset: preset.name.clone(),
                function: function.clone(),
                first_hz: first,
                other_hz: hz,
            }),
            Some(_) => {}
            None => {
                seen.insert(function.as_str(), hz);
                let Ok(binding) = parse_function(function) else {
                    continue; // an unknown function is already reported above
                };
                if binding == Binding::Consume {
                    issues.push(Issue::TurboOnConsume {
                        preset: preset.name.clone(),
                        function: function.clone(),
                    });
                    continue;
                }
                if let Some((asked_hz, effective_hz)) = TurboBinding::new(binding, hz).clamped() {
                    issues.push(Issue::BindingTurboClamped {
                        preset: preset.name.clone(),
                        function: function.clone(),
                        asked_hz,
                        effective_hz,
                    });
                }
            }
        }
    }
}

/// Everything that can only go wrong once a preset carries a timed sequence
/// (docs/INPUT-TRANSFORMS.md §1c).
///
/// The sampling rule (§0.2) is checked here and nowhere else: `MacroStep`
/// decides what the engine *runs*, and this decides what the user is *told*.
/// Which CONTROL a directional binding drives.
///
/// A pad has three ways to say "right", and a game reads whichever one it was
/// written for. Buttons and triggers have no mechanism — they are read the same
/// way whatever the preset does — so they never produce a mismatch.
///
/// ONE DEFINITION, re-exported: this used to be a private copy that decided
/// "which control" separately from `socd::pointing`, which decides "which
/// direction". They can no longer drift — a centred axis points nowhere, so it
/// has no mechanism, so it is never half of a diagonal
/// (`ksx_core::diagonal::fold`) and never an opposing pair either.
/// `describe()`'s wording is contractual: it is inside
/// [`Issue::MacroHoldsOtherMechanism`]'s message.
use ksx_core::DirMechanism as Mechanism;

fn describe_all(mechanisms: &[Mechanism]) -> String {
    let names: Vec<&str> = mechanisms.iter().map(|m| m.describe()).collect();
    names.join(" and ")
}

/// Every mechanism this preset's own BOUND direction keys drive.
///
/// Inert rows (`Key::None`) do not count: a placeholder is a function the
/// preset lists, not a direction the player can produce.
fn driven_mechanisms(pairs: &[(String, Flat<'_>)]) -> Vec<Mechanism> {
    let mut out: Vec<Mechanism> = Vec::new();
    for (function, flat) in pairs {
        if macro_name(function).is_some() {
            continue;
        }
        let key = match flat {
            Flat::Plain(key) => *key,
            Flat::Guard(guard) => guard.key.as_str(),
        };
        if key.eq_ignore_ascii_case("None") {
            continue;
        }
        let Ok(binding) = parse_function(function) else {
            continue;
        };
        if let Some(mechanism) = Mechanism::of(binding) {
            if !out.contains(&mechanism) {
                out.push(mechanism);
            }
        }
    }
    out
}

fn validate_macros(preset: &PresetFile, pairs: &[(String, Flat<'_>)], issues: &mut Vec<Issue>) {
    let name = || preset.name.clone();
    // What the PLAYER's own direction keys drive on this preset — the best
    // available evidence for what the game on this slot actually reads.
    let driven = driven_mechanisms(pairs);

    // Names are matched case-insensitively (function names are), so two tables
    // differing only in case would have one silently shadow the other.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for macro_name in preset.macros.keys() {
        if !seen.insert(macro_name.to_ascii_lowercase()) {
            issues.push(Issue::DuplicateMacroName {
                preset: name(),
                name: macro_name.clone(),
            });
        }
    }

    for (macro_name, def) in &preset.macros {
        if def.steps.is_empty() {
            issues.push(Issue::EmptyMacro {
                preset: name(),
                name: macro_name.clone(),
            });
        }
        // Said out loud every run, because "this macro does nothing" is the
        // report, and a flag in a file is not somewhere anyone looks first.
        if !def.enabled {
            issues.push(Issue::MacroDisabled {
                preset: name(),
                name: macro_name.clone(),
            });
        }
        validate_turbo(preset, macro_name, def, issues);
        for (i, step) in def.steps.iter().enumerate() {
            for function in &step.hold {
                let Ok(binding) = parse_function(function) else {
                    issues.push(Issue::UnknownMacroHold {
                        preset: name(),
                        name: macro_name.clone(),
                        step: i,
                        function: function.clone(),
                    });
                    continue;
                };
                // The "my diagonal never comes out" trap. A step that holds a
                // direction on a mechanism this preset does not otherwise drive
                // is published faithfully and read by nobody — and because it
                // is usually ONE step of a motion that was written that way
                // (the diagonal, copied from an example), the symptom is a
                // motion with a hole in it rather than a macro that does
                // nothing. Only reported when the preset gives us something to
                // compare against.
                if let Some(mechanism) = Mechanism::of(binding) {
                    if !driven.is_empty() && !driven.contains(&mechanism) {
                        issues.push(Issue::MacroHoldsOtherMechanism {
                            preset: name(),
                            name: macro_name.clone(),
                            step: i,
                            function: function.clone(),
                            holds: mechanism.describe().to_owned(),
                            preset_drives: describe_all(&driven),
                        });
                    }
                }
            }
            // Exactly one unit, always. Neither is not "instant": it is a file
            // that forgot to say, and a zero-length step is an invisible input.
            let ms = match step.duration() {
                Ok(duration) => duration.ms(),
                Err(reason) => {
                    issues.push(Issue::MacroStepBadDuration {
                        preset: name(),
                        name: macro_name.clone(),
                        step: i,
                        reason: reason.to_owned(),
                    });
                    continue;
                }
            };
            if ms >= MIN_STEP_MS {
                continue;
            }
            issues.push(if step.allow_short {
                Issue::MacroStepMayBeMissed {
                    preset: name(),
                    name: macro_name.clone(),
                    step: i,
                    ms,
                }
            } else {
                Issue::MacroStepRaised {
                    preset: name(),
                    name: macro_name.clone(),
                    step: i,
                    ms,
                }
            });
        }
    }

    // One key, several macros. Native (a trigger is multi-bind like any other
    // row) and never what a reader assumes, because the evidence for it is
    // spread across as many `macro.<name>` rows as there are macros — the one
    // shape you cannot see by reading the macro you are debugging. Grouped by
    // key so it is reported once per key rather than once per row.
    let mut by_key: Vec<(String, Vec<String>)> = Vec::new();
    for (function, flat) in pairs {
        let Some(target) = macro_name(function) else {
            continue;
        };
        let key = match flat {
            Flat::Plain(key) => *key,
            Flat::Guard(guard) => guard.key.as_str(),
        };
        // An inert placeholder row starts nothing and shares nothing.
        if key.eq_ignore_ascii_case("None") {
            continue;
        }
        match by_key.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some((_, names)) => {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(target)) {
                    names.push(target.to_owned());
                }
            }
            None => by_key.push((key.to_owned(), vec![target.to_owned()])),
        }
    }
    for (key, names) in by_key {
        if names.len() > 1 {
            issues.push(Issue::SharedMacroTrigger {
                preset: name(),
                key,
                macros: names,
            });
        }
    }

    // Trigger rows: the macro has to exist, and it cannot be a chord.
    for (function, flat) in pairs {
        let Some(target) = macro_name(function) else {
            continue;
        };
        if !preset.macros.keys().any(|k| k.eq_ignore_ascii_case(target)) {
            issues.push(Issue::UnknownMacroRef {
                preset: name(),
                function: function.clone(),
                name: target.to_owned(),
            });
        }
        if let Flat::Guard(guard) = flat {
            if !guard.when.is_empty() || !guard.unless.is_empty() {
                issues.push(Issue::GuardedMacroTrigger {
                    preset: name(),
                    name: target.to_owned(),
                });
            }
        }
    }
}

/// The repeat/turbo settings (docs/INPUT-TRANSFORMS.md §1c, "repeating").
///
/// The rate is never silently invented and never silently unachievable: a
/// `turbo` with no rate is refused, a rate on a macro that does not repeat is
/// named as inert, and a rate the sampling ceiling will not deliver is reported
/// with the number the preset actually gets.
fn validate_turbo(preset: &PresetFile, macro_name: &str, def: &MacroFile, issues: &mut Vec<Issue>) {
    match def.turbo_rate() {
        Err(reason) => {
            issues.push(Issue::MacroTurboBadRate {
                preset: preset.name.clone(),
                name: macro_name.to_owned(),
                reason: reason.to_owned(),
            });
            return;
        }
        Ok(None) => return,
        Ok(Some(_)) if def.repeat != Repeat::Turbo => {
            issues.push(Issue::TurboRateWithoutTurbo {
                preset: preset.name.clone(),
                name: macro_name.to_owned(),
                repeat: def.repeat.to_string(),
            });
            return;
        }
        Ok(Some(_)) => {}
    }
    // The arithmetic lives in ksx-core and is done ONCE, so what validation
    // reports and what the engine runs cannot drift apart. A body whose steps
    // do not convert is already reported step by step; there is nothing to add.
    let Ok(core) = def.to_core(macro_name) else {
        return;
    };
    if let Some((asked_hz, effective_hz)) = core.turbo_rate_clamped() {
        issues.push(Issue::TurboRateClamped {
            preset: preset.name.clone(),
            name: macro_name.to_owned(),
            asked_hz,
            effective_hz,
        });
    }
    if let Some((ms, raised_to)) = core.turbo_gap_raised() {
        issues.push(Issue::TurboGapRaised {
            preset: preset.name.clone(),
            name: macro_name.to_owned(),
            ms,
            raised_to,
        });
    }
}

/// Everything that can only go wrong once a binding carries a guard
/// (docs/INPUT-TRANSFORMS.md §1b).
fn validate_chords(preset: &PresetFile, pairs: &[(String, Flat<'_>)], issues: &mut Vec<Issue>) {
    let name = || preset.name.clone();
    // Keys bound on their own, and to what — the flash advisory reads this.
    let mut plain: BTreeMap<&str, String> = BTreeMap::new();
    for (function, flat) in pairs {
        if let Flat::Plain(key) = flat {
            plain.entry(key).or_insert_with(|| function.clone());
        }
    }

    let chords: Vec<(&String, &GuardedEntry)> = pairs
        .iter()
        .filter_map(|(function, flat)| match flat {
            Flat::Guard(guard) => Some((function, *guard)),
            Flat::Plain(_) => None,
        })
        // An empty guard is a plain binding, not a chord (preset.rs
        // normalizes it), so it carries none of these rules. Nor is a macro
        // trigger a chord — `validate_macros` reports a guard on one.
        .filter(|(function, guard)| {
            !(guard.when.is_empty() && guard.unless.is_empty()) && macro_name(function).is_none()
        })
        .collect();

    for (function, guard) in &chords {
        for key in guard.when.iter().chain(guard.unless.iter()) {
            if Key::from_name(key).is_none() {
                issues.push(Issue::UnknownGuardKey {
                    preset: name(),
                    function: (*function).clone(),
                    key: key.clone(),
                });
            }
            if *key == guard.key {
                issues.push(Issue::GuardIncludesTriggerKey {
                    preset: name(),
                    function: (*function).clone(),
                    key: key.clone(),
                });
            }
        }
        for key in &guard.when {
            if guard.unless.contains(key) {
                issues.push(Issue::ContradictoryGuard {
                    preset: name(),
                    function: (*function).clone(),
                    key: key.clone(),
                });
            }
        }
        // The honest caveat, said out loud: a constituent that is bound on its
        // own flashes that binding before the chord completes.
        for key in std::iter::once(&guard.key).chain(guard.when.iter()) {
            if let Some(bound_to) = plain.get(key.as_str()) {
                issues.push(Issue::ChordConstituentAlsoBound {
                    preset: name(),
                    function: (*function).clone(),
                    key: key.clone(),
                    bound_to: bound_to.clone(),
                });
            }
        }
    }

    // Equal specificity on the same trigger, both satisfiable at once: the
    // engine would activate both in build order, so the file has to say which
    // one it means.
    for (i, (function, guard)) in chords.iter().enumerate() {
        for (other_function, other) in chords.iter().skip(i + 1) {
            let same_guard = guard.when == other.when && guard.unless == other.unless;
            if guard.key != other.key
                || guard.when.len() + guard.unless.len()
                    != other.when.len() + other.unless.len()
                // Identical guards are a MULTI-BIND (one chord, several
                // functions) — native, not ambiguous.
                || same_guard
            {
                continue;
            }
            let exclusive = guard.when.iter().any(|k| other.unless.contains(k))
                || other.when.iter().any(|k| guard.unless.contains(k));
            if exclusive {
                continue; // they can never be satisfied together
            }
            issues.push(Issue::AmbiguousChords {
                preset: name(),
                key: guard.key.clone(),
                function: (*function).clone(),
                other: (*other_function).clone(),
            });
        }
    }
}

/// One flattened binding value: a plain key name or a guarded entry.
enum Flat<'a> {
    Plain(&'a str),
    Guard(&'a GuardedEntry),
}

/// Flatten nested dotted-key groups into `(function, value)` pairs, the same
/// shape [`PresetFile::to_core`] consumes.
fn flatten_bindings<'a>(
    function: &str,
    entry: &'a BindingEntry,
    out: &mut Vec<(String, Flat<'a>)>,
) {
    match entry {
        BindingEntry::Key(key) => out.push((function.to_owned(), Flat::Plain(key.as_str()))),
        BindingEntry::Keys(keys) => {
            out.extend(
                keys.iter()
                    .map(|k| (function.to_owned(), Flat::Plain(k.as_str()))),
            );
        }
        BindingEntry::Guarded(guard) => out.push((function.to_owned(), Flat::Guard(guard))),
        BindingEntry::Many(entries) => {
            for entry in entries {
                flatten_bindings(function, entry, out);
            }
        }
        BindingEntry::Group(group) => {
            for (sub, entry) in group {
                flatten_bindings(&format!("{function}.{sub}"), entry, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DeviceEntry, SlotEntry};

    fn preset(name: &str, toml_bindings: &str) -> PresetFile {
        toml::from_str(&format!("name = \"{name}\"\n[bindings]\n{toml_bindings}")).unwrap()
    }

    fn config(toml_str: &str) -> ConfigFile {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn clean_config_has_no_issues() {
        let cfg = config(
            r#"
schema_version = 1

[[device]]
id = "HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"
alias = "P1 I-PAC"

[[slot]]
number = 1
keyboard = "P1 I-PAC"
preset = "sf2"

[[slot]]
number = 2
keyboard = 'HID\VID_D209&PID_0430&MI_00\9&1&0'
preset = "default"
"#,
        );
        let presets = vec![preset(
            "sf2",
            "A = \"S\"\ndpad.up = \"I\"\n\"lx.min\" = \"Left\"",
        )];
        assert_eq!(validate(&cfg, &presets), Vec::new());
    }

    #[test]
    fn settings_ranges_are_checked() {
        let mut cfg = ConfigFile::default();
        cfg.settings.mouse_move_deadzone = 13;
        cfg.settings.starting_user_index = 0;
        let issues = validate(&cfg, &[]);
        assert!(issues.contains(&Issue::MouseMoveDeadzoneOutOfRange { value: 13 }));
        assert!(issues.contains(&Issue::StartingUserIndexOutOfRange { value: 0 }));
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn slot_and_device_references_are_checked() {
        let cfg = ConfigFile {
            slots: vec![
                SlotEntry {
                    number: 1,
                    keyboard: Some("Ghost Alias".into()),
                    mouse: None,
                    preset: "missing".into(),
                    persona: Persona::default(),
                    socd: Socd::default(),
                    macros: Default::default(),
                },
                SlotEntry {
                    number: 1,
                    keyboard: None,
                    mouse: None,
                    preset: "default".into(),
                    persona: Persona::default(),
                    socd: Socd::default(),
                    macros: Default::default(),
                },
                SlotEntry {
                    number: MAX_SLOTS + 1,
                    keyboard: None,
                    mouse: None,
                    preset: "empty".into(),
                    persona: Persona::default(),
                    socd: Socd::default(),
                    macros: Default::default(),
                },
            ],
            ..ConfigFile::default()
        };
        let issues = validate(&cfg, &[]);
        assert!(issues.contains(&Issue::DuplicateSlotNumber { number: 1 }));
        assert!(issues.contains(&Issue::SlotNumberOutOfRange {
            number: MAX_SLOTS + 1
        }));
        assert!(issues.contains(&Issue::UnknownPresetRef {
            slot: 1,
            preset: "missing".into()
        }));
        assert!(issues.contains(&Issue::UnknownDeviceRef {
            slot: 1,
            reference: "Ghost Alias".into()
        }));
        assert_eq!(issues.len(), 4);
    }

    #[test]
    fn a_fifth_xinput_slot_is_reported_with_the_fix_named() {
        let slot = |number: u8, persona: Persona| SlotEntry {
            number,
            keyboard: None,
            mouse: None,
            preset: "default".into(),
            persona,
            socd: Socd::default(),
            macros: Default::default(),
        };
        // 5 Xbox slots: one more than Windows can ever show to a game.
        let cfg = ConfigFile {
            slots: (1..=5).map(|n| slot(n, Persona::Xbox360)).collect(),
            ..ConfigFile::default()
        };
        let issues = validate(&cfg, &[]);
        assert_eq!(issues, vec![Issue::TooManyXinputSlots { count: 5 }]);
        // The message must point at the actual fix, not just complain.
        let msg = issues[0].to_string();
        assert!(msg.contains("playstation"), "{msg}");

        // The supported full-house shape: 4 Xbox and the rest PlayStation, all
        // the way to the ceiling. Written as `1..=MAX_SLOTS` so the raise from
        // 8 to 16 actually widened what is under test instead of leaving this
        // exercising the first half of the range.
        let cfg = ConfigFile {
            slots: (1..=MAX_SLOTS)
                .map(|n| {
                    slot(
                        n,
                        if n <= 4 {
                            Persona::Xbox360
                        } else {
                            Persona::PlayStation
                        },
                    )
                })
                .collect(),
            ..ConfigFile::default()
        };
        assert_eq!(validate(&cfg, &[]), vec![]);
    }

    #[test]
    fn a_games_fifth_xinput_slot_is_reported_per_game() {
        use crate::games::GameSlotEntry;
        let slot = |number: u8| GameSlotEntry {
            number,
            user_index: None,
            keyboard: None,
            mouse: None,
            preset: "default".into(),
            persona: Persona::Xbox360,
            socd: Socd::default(),
            macros: Default::default(),
        };
        let mut games: GamesFile =
            toml::from_str("[[game]]\ntitle = \"MAME 8P\"\npath = \"C:\\\\mame\\\\mame.exe\"\n")
                .unwrap();
        games.games[0].slots = (1..=6).map(slot).collect();
        let issues = validate_games(&games, &[]);
        assert_eq!(
            issues,
            vec![Issue::GameTooManyXinputSlots {
                game: "MAME 8P".into(),
                count: 6
            }]
        );
    }

    /// A slot naming a persona nothing can build must be caught by `ksx check`
    /// and `ksx run --dry-run`, not by a plug three steps later.
    #[test]
    fn a_persona_this_build_cannot_plug_is_refused_at_validate_time() {
        let slot = |number: u8, persona: Persona| SlotEntry {
            number,
            keyboard: None,
            mouse: None,
            preset: "default".into(),
            persona,
            socd: Socd::default(),
            macros: Default::default(),
        };
        for persona in [Persona::DualSense, Persona::SwitchPro, Persona::XboxSeries] {
            let cfg = ConfigFile {
                slots: vec![slot(1, persona)],
                ..ConfigFile::default()
            };
            let issues = validate(&cfg, &[]);
            let found = issues
                .iter()
                .find(|i| matches!(i, Issue::PersonaNotImplemented { .. }))
                .unwrap_or_else(|| panic!("{persona} produced no issue: {issues:?}"));
            let msg = found.to_string();
            assert!(msg.contains(persona.as_str()), "{msg}");
            // A refusal that stops at "no" makes the user guess. It has to name
            // what is missing AND what to write instead.
            assert!(msg.contains("cannot be plugged by this build"), "{msg}");
            assert!(
                msg.contains(persona.nearest_pluggable().as_str()),
                "{msg} must name the persona to use instead"
            );
            // It is a fault, not advice: the slot does not work as written.
            assert!(!found.is_advisory(), "{msg}");
        }
    }

    #[test]
    fn the_personas_that_do_work_produce_no_persona_issue() {
        // The other half of the gate: a cabinet on xbox360/playstation must
        // stay silent, or the warning becomes noise nobody reads.
        let slot = |number: u8, persona: Persona| SlotEntry {
            number,
            keyboard: None,
            mouse: None,
            preset: "default".into(),
            persona,
            socd: Socd::default(),
            macros: Default::default(),
        };
        let cfg = ConfigFile {
            slots: vec![
                slot(1, Persona::Xbox360),
                slot(2, Persona::PlayStation),
                slot(3, Persona::Xbox360),
            ],
            ..ConfigFile::default()
        };
        assert_eq!(validate(&cfg, &[]), vec![]);
    }

    #[test]
    fn a_games_unbuildable_persona_is_reported_per_game() {
        use crate::games::GameSlotEntry;
        let mut games: GamesFile =
            toml::from_str("[[game]]\ntitle = \"Bloodborne\"\npath = \"C:\\\\ps.exe\"\n").unwrap();
        games.games[0].slots = vec![GameSlotEntry {
            number: 1,
            user_index: None,
            keyboard: None,
            mouse: None,
            preset: "default".into(),
            persona: Persona::DualSense,
            socd: Socd::default(),
            macros: Default::default(),
        }];
        let issues = validate_games(&games, &[]);
        assert_eq!(
            issues,
            vec![Issue::GamePersonaNotImplemented {
                game: "Bloodborne".into(),
                slot: 1,
                persona: "dualsense".into(),
                reason: ksx_core::PadBackend::HidMaestro.gap().unwrap().to_owned(),
                instead: "playstation".into(),
            }]
        );
        let msg = issues[0].to_string();
        assert!(msg.contains("Bloodborne"), "{msg}");
        assert!(msg.contains("playstation"), "{msg}");
    }

    /// The trap, stated as a test: this check may never consult the machine.
    ///
    /// A driver probe flips to "installed" the moment someone installs
    /// HIDMaestro — at which point a probe-based check would go quiet while the
    /// plug still failed, and the user would have installed a driver for
    /// nothing. So the reason printed here is the BUILD's, verbatim, and it
    /// says the install does not help.
    #[test]
    fn the_persona_gap_is_a_build_fact_and_says_installing_will_not_help() {
        let (reason, instead) =
            persona_gap(Persona::SwitchPro).expect("switchpro cannot be plugged by this build");
        assert_eq!(
            reason,
            ksx_core::PadBackend::HidMaestro.gap().unwrap(),
            "the sentence must come from the capability, not a second copy"
        );
        assert!(reason.contains("installing HIDMaestro does not change it"));
        assert_eq!(instead, "xbox360");
        // And no gap at all for what the cabinet actually runs.
        assert_eq!(persona_gap(Persona::Xbox360), None);
        assert_eq!(persona_gap(Persona::PlayStation), None);
    }

    #[test]
    fn duplicate_device_aliases_are_reported_once() {
        let device = |alias: &str| DeviceEntry {
            id: r"HID\X".into(),
            alias: alias.into(),
            backend: crate::config::Backend::Interception,
        };
        let cfg = ConfigFile {
            devices: vec![device("P1"), device("P1"), device("P1"), device("P2")],
            ..ConfigFile::default()
        };
        let issues = validate(&cfg, &[]);
        assert_eq!(
            issues,
            vec![Issue::DuplicateDeviceAlias { alias: "P1".into() }]
        );
    }

    #[test]
    fn preset_bindings_are_checked() {
        let presets = vec![preset(
            "bad",
            "warp = \"S\"\n\"lx.zillion\" = \"A\"\nA = [\"S\", \"NotAKey\"]\ndpad.up = \"AlsoFake\"",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(issues.contains(&Issue::UnknownFunction {
            preset: "bad".into(),
            function: "warp".into()
        }));
        assert!(issues.contains(&Issue::InvalidAxisValue {
            preset: "bad".into(),
            function: "lx.zillion".into()
        }));
        assert!(issues.contains(&Issue::UnknownKeyName {
            preset: "bad".into(),
            function: "A".into(),
            key: "NotAKey".into()
        }));
        assert!(issues.contains(&Issue::UnknownKeyName {
            preset: "bad".into(),
            function: "dpad.up".into(),
            key: "AlsoFake".into()
        }));
        assert_eq!(issues.len(), 4);
    }

    // ---- chords (docs/INPUT-TRANSFORMS.md §1b) ----------------------------

    /// The recommended shape — chord keys with no individual binding — must
    /// be completely clean, warning included.
    #[test]
    fn a_chord_on_dedicated_keys_is_clean() {
        let presets = vec![preset(
            "cab",
            "A = \"G\"\nrt = { key = \"D\", when = [\"F\"] }\n\
             lb = { key = \"D\", when = [\"F\", \"C\"], unless = [\"LeftShift\"] }",
        )];
        assert_eq!(validate(&ConfigFile::default(), &presets), Vec::new());
    }

    /// Every guard mistake the file can hold, named separately.
    #[test]
    fn guard_mistakes_are_reported() {
        let presets = vec![preset(
            "bad",
            "rt = { key = \"A\", when = [\"Nope\"] }\n\
             lb = { key = \"A\", when = [\"A\"] }\n\
             rb = { key = \"A\", when = [\"B\"], unless = [\"B\"] }",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(
            issues.contains(&Issue::UnknownGuardKey {
                preset: "bad".into(),
                function: "rt".into(),
                key: "Nope".into()
            }),
            "{issues:?}"
        );
        assert!(
            issues.contains(&Issue::GuardIncludesTriggerKey {
                preset: "bad".into(),
                function: "lb".into(),
                key: "A".into()
            }),
            "{issues:?}"
        );
        assert!(
            issues.contains(&Issue::ContradictoryGuard {
                preset: "bad".into(),
                function: "rb".into(),
                key: "B".into()
            }),
            "{issues:?}"
        );
    }

    /// The flash advisory: a constituent that is ALSO bound on its own. This
    /// is the one honest caveat of a zero-deferral design, so the message has
    /// to say what the player will see, not just that something is odd.
    #[test]
    fn an_individually_bound_constituent_warns_about_the_flash() {
        let presets = vec![preset(
            "flash",
            "X = \"A\"\nY = \"B\"\nrt = { key = \"A\", when = [\"B\"] }",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(
            issues.contains(&Issue::ChordConstituentAlsoBound {
                preset: "flash".into(),
                function: "rt".into(),
                key: "A".into(),
                bound_to: "X".into()
            }),
            "{issues:?}"
        );
        assert!(
            issues.contains(&Issue::ChordConstituentAlsoBound {
                preset: "flash".into(),
                function: "rt".into(),
                key: "B".into(),
                bound_to: "Y".into()
            }),
            "{issues:?}"
        );
        let message = issues
            .iter()
            .find(|i| matches!(i, Issue::ChordConstituentAlsoBound { .. }))
            .unwrap()
            .to_string();
        assert!(message.contains("does not defer"), "{message}");
        assert!(message.contains("for a moment"), "{message}");
    }

    /// Equal specificity on the same trigger, both satisfiable: refused, not
    /// raced. Identical guards are a multi-bind and stay legal.
    #[test]
    fn ambiguous_equal_specificity_chords_are_refused_but_multi_bind_is_not() {
        let ambiguous = vec![preset(
            "ambiguous",
            "rt = { key = \"A\", when = [\"B\"] }\nlb = { key = \"A\", when = [\"C\"] }",
        )];
        let issues = validate(&ConfigFile::default(), &ambiguous);
        assert_eq!(
            issues,
            vec![Issue::AmbiguousChords {
                preset: "ambiguous".into(),
                key: "A".into(),
                function: "lb".into(),
                other: "rt".into(),
            }],
            "{issues:?}"
        );
        assert!(issues[0].to_string().contains("more specific"));

        // Same trigger, same guard, two outputs: a multi-bind, native in ksx.
        let multibind = vec![preset(
            "multibind",
            "rt = { key = \"A\", when = [\"B\"] }\nlb = { key = \"A\", when = [\"B\"] }",
        )];
        assert_eq!(validate(&ConfigFile::default(), &multibind), Vec::new());

        // Different sizes: specificity decides, no issue.
        let nested = vec![preset(
            "nested",
            "rt = { key = \"A\", when = [\"B\"] }\nlb = { key = \"A\", when = [\"B\", \"C\"] }",
        )];
        assert_eq!(validate(&ConfigFile::default(), &nested), Vec::new());

        // Mutually exclusive guards can never both be satisfied.
        let exclusive = vec![preset(
            "exclusive",
            "rt = { key = \"A\", when = [\"B\"] }\nlb = { key = \"A\", unless = [\"B\"] }",
        )];
        assert_eq!(validate(&ConfigFile::default(), &exclusive), Vec::new());
    }

    // ---- macros (docs/INPUT-TRANSFORMS.md §1c) ----------------------------

    fn macro_preset(body: &str) -> PresetFile {
        toml::from_str(&format!("name = \"m\"\n{body}")).unwrap()
    }

    /// A well-formed macro at honest durations is completely clean.
    #[test]
    fn a_well_formed_macro_reports_nothing() {
        let presets = vec![macro_preset(
            "[bindings]\n\"dpad.down\" = \"Down\"\nmacro.hadouken = \"P\"\n\
             [macros.hadouken]\n\
             steps = [{ hold = [\"dpad.down\"], ms = 50 }, { hold = [\"A\"], frames = 3 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &presets), Vec::new());
    }

    /// The sampling rule (§0.2), both ways round. Neither is a refusal — one
    /// keeps the macro correct, the other is the author having been asked and
    /// having answered — but neither is ever silent.
    #[test]
    fn short_steps_are_reported_whichever_way_they_go() {
        let presets = vec![macro_preset(
            "[macros.m]\n\
             steps = [{ hold = [\"A\"], ms = 5 }, { hold = [\"B\"], ms = 5, allow_short = true }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert_eq!(
            issues,
            vec![
                Issue::MacroStepRaised {
                    preset: "m".into(),
                    name: "m".into(),
                    step: 0,
                    ms: 5
                },
                Issue::MacroStepMayBeMissed {
                    preset: "m".into(),
                    name: "m".into(),
                    step: 1,
                    ms: 5
                },
            ],
            "{issues:?}"
        );
        assert!(issues.iter().all(Issue::is_advisory));
        assert!(issues[0].to_string().contains("invisible"), "{}", issues[0]);
        assert!(issues[1].to_string().contains("may never see it"));

        // One frame is below the floor too, and is reported in ms.
        let framed = vec![macro_preset(
            "[macros.m]\nsteps = [{ hold = [\"A\"], frames = 1 }]\n",
        )];
        assert!(matches!(
            validate(&ConfigFile::default(), &framed).as_slice(),
            [Issue::MacroStepRaised { ms: 17, .. }]
        ));
    }

    /// Every structural mistake a macro can hold, named separately.
    #[test]
    fn macro_mistakes_are_reported() {
        let presets = vec![macro_preset(
            "[bindings]\nmacro.ghost = \"P\"\nmacro.m = { key = \"Q\", when = [\"R\"] }\n\
             [macros.m]\n\
             steps = [{ hold = [\"warp\"], ms = 50 }, { hold = [\"A\"], ms = 50, frames = 3 }]\n\
             [macros.empty]\nsteps = []\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        for expected in [
            Issue::EmptyMacro {
                preset: "m".into(),
                name: "empty".into(),
            },
            Issue::UnknownMacroHold {
                preset: "m".into(),
                name: "m".into(),
                step: 0,
                function: "warp".into(),
            },
            Issue::UnknownMacroRef {
                preset: "m".into(),
                function: "macro.ghost".into(),
                name: "ghost".into(),
            },
            Issue::GuardedMacroTrigger {
                preset: "m".into(),
                name: "m".into(),
            },
        ] {
            assert!(
                issues.contains(&expected),
                "missing {expected:?} in {issues:?}"
            );
        }
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, Issue::MacroStepBadDuration { step: 1, .. })),
            "{issues:?}"
        );
        // None of these is advice: a macro that cannot run must not start.
        assert!(!Issue::EmptyMacro {
            preset: String::new(),
            name: String::new()
        }
        .is_advisory());
    }

    /// The 2026-08-06 cabinet shape: THREE macros on one key. Legal, and
    /// invisible from any one of them, so it is said out loud — it is what makes
    /// a motion grow steps it does not have and a `repeat = "once"` macro look
    /// like it never stops.
    #[test]
    fn several_macros_on_one_key_are_reported_once_per_key() {
        let presets = vec![macro_preset(
            "[bindings]\nmacro.op = \"Q\"\nmacro.ppp = \"Q\"\nmacro.test = \"Q\"\n\
             macro.solo = \"R\"\nmacro.inert = \"None\"\n\
             [macros.op]\nrepeat = \"turbo\"\nturbo_hz = 10\n\
             steps = [{ hold = [\"ly.min\"], ms = 50 }]\n\
             [macros.ppp]\nsteps = [{ hold = [\"ly.min\"], ms = 50 }]\n\
             [macros.test]\nsteps = [{ hold = [\"ly.min\"], ms = 50 }]\n\
             [macros.solo]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n\
             [macros.inert]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        let shared: Vec<&Issue> = issues
            .iter()
            .filter(|i| matches!(i, Issue::SharedMacroTrigger { .. }))
            .collect();
        // ONE issue, for the one key that is shared — not one per row, and
        // nothing at all for the key that starts a single macro or for the
        // inert `"None"` placeholder.
        assert_eq!(shared.len(), 1, "{issues:?}");
        assert_eq!(
            *shared[0],
            Issue::SharedMacroTrigger {
                preset: "m".into(),
                key: "Q".into(),
                macros: vec!["op".into(), "ppp".into(), "test".into()],
            }
        );
        // Advice, not an error: several macros on one key is a real thing to
        // want. The text has to name all three, and say what it does.
        assert!(shared[0].is_advisory());
        let text = shared[0].to_string();
        for part in ["Q", "op", "ppp", "test", "SUPERPOSITION", "repeat"] {
            assert!(text.contains(part), "{text}");
        }
    }

    /// The advisory has to survive the WRITERS refusing to create the shape.
    /// Hand-edited files with a shared trigger already exist (Victor's did),
    /// and the rule for them is unchanged: they load, and they warn.
    #[test]
    fn a_pre_existing_shared_trigger_still_only_warns() {
        let presets = vec![macro_preset(
            "[bindings]\nmacro.a = \"Q\"\nmacro.b = \"Q\"\n\
             [macros.a]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n\
             [macros.b]\nsteps = [{ hold = [\"B\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(
            issues.iter().all(Issue::is_advisory),
            "an existing collision must never become a load failure: {issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, Issue::SharedMacroTrigger { .. })),
            "{issues:?}"
        );
    }

    /// A disabled macro is advice, said every run, because "this macro does
    /// nothing" is the report and a flag in a file is not where anyone looks.
    #[test]
    fn a_disabled_macro_is_named_and_advisory() {
        let presets = vec![macro_preset(
            "[bindings]\nmacro.hadouken = \"P\"\n\
             [macros.hadouken]\nenabled = false\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert_eq!(
            issues,
            vec![Issue::MacroDisabled {
                preset: "m".into(),
                name: "hadouken".into()
            }],
            "{issues:?}"
        );
        assert!(issues[0].is_advisory());
        let text = issues[0].to_string();
        for part in ["hadouken", "enabled = false", "--enable"] {
            assert!(text.contains(part), "{text}");
        }
    }

    /// The other half: the slot switch, reported only when it silences
    /// something. A setting with no consequence is not narrated.
    #[test]
    fn a_slot_with_macros_off_says_what_it_silenced() {
        let presets = vec![macro_preset(
            "[bindings]\nmacro.hadouken = \"P\"\n\
             [macros.hadouken]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        let slot = |preset: &str, macros: MacroSwitch| SlotEntry {
            number: 1,
            keyboard: None,
            mouse: None,
            preset: preset.into(),
            persona: Persona::default(),
            socd: Socd::default(),
            macros,
        };

        let cfg = ConfigFile {
            slots: vec![slot("m", MacroSwitch::Off)],
            ..ConfigFile::default()
        };
        let issues = validate(&cfg, &presets);
        assert!(
            issues.contains(&Issue::SlotMacrosOff {
                slot: 1,
                preset: "m".into(),
                macros: 1,
            }),
            "{issues:?}"
        );
        assert!(issues.iter().all(Issue::is_advisory), "{issues:?}");

        // `on` says nothing, and neither does `off` on a preset with no macros
        // to silence.
        let quiet = ConfigFile {
            slots: vec![
                slot("m", MacroSwitch::On),
                slot("default", MacroSwitch::Off),
            ],
            ..ConfigFile::default()
        };
        assert!(
            !validate(&quiet, &presets)
                .iter()
                .any(|i| matches!(i, Issue::SlotMacrosOff { .. })),
            "a switch with no consequence must not be narrated"
        );
    }

    /// Macro names are matched ignoring case, so two tables differing only in
    /// case would silently shadow one another.
    #[test]
    fn macro_names_that_differ_only_in_case_collide() {
        let presets = vec![macro_preset(
            "[macros.hadouken]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n\
             [macros.Hadouken]\nsteps = [{ hold = [\"B\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert_eq!(
            issues,
            vec![Issue::DuplicateMacroName {
                preset: "m".into(),
                name: "hadouken".into()
            }],
            "{issues:?}"
        );
    }

    /// A `macro.<name>` row must never be mistaken for a pad function or a
    /// chord: it has its own grammar and its own checks.
    #[test]
    fn a_macro_row_is_never_checked_as_a_pad_function() {
        let presets = vec![macro_preset(
            "[bindings]\nmacro.m = \"NotAKey\"\n\
             [macros.m]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        // The KEY is still checked — a trigger is still a key.
        assert_eq!(
            issues,
            vec![Issue::UnknownKeyName {
                preset: "m".into(),
                function: "macro.m".into(),
                key: "NotAKey".into()
            }],
            "{issues:?}"
        );
    }

    // ---- socd (docs/INPUT-TRANSFORMS.md §2.6) -----------------------------

    fn socd_only(issues: Vec<Issue>) -> Vec<Issue> {
        issues
            .into_iter()
            .filter(|i| matches!(i, Issue::SocdShadowedByChord { .. }))
            .collect()
    }

    fn socd_config(policy: &str) -> ConfigFile {
        config(&format!(
            "schema_version = 1\n\n[[slot]]\nnumber = 1\npreset = \"stick\"\nsocd = \"{policy}\"\n"
        ))
    }

    fn stick_preset() -> PresetFile {
        preset(
            "stick",
            "\"lx.min\" = \"Left\"\n\"lx.max\" = \"Right\"\n\
             \"ly.max\" = \"Up\"\n\"ly.min\" = \"Down\"",
        )
    }

    /// A plain SOCD slot is completely clean: the policy generates chords, and
    /// generated chords are not something the user can get wrong.
    #[test]
    fn an_ordinary_socd_slot_reports_nothing() {
        for policy in ["off", "neutral", "up-priority"] {
            assert_eq!(
                validate(&socd_config(policy), &[stick_preset()]),
                Vec::new(),
                "socd = {policy}"
            );
        }
    }

    /// The user wrote the pair by hand. Their chord wins — and the shadowing
    /// is said out loud instead of being silently skipped.
    #[test]
    fn a_hand_written_chord_over_an_socd_pair_is_reported_as_shadowing() {
        let mut file = stick_preset();
        file.bindings.insert(
            "rt".into(),
            // Written the other way round on purpose: a pair is a SET.
            BindingEntry::Guarded(GuardedEntry {
                key: "Right".into(),
                when: vec!["Left".into()],
                unless: Vec::new(),
                turbo_hz: None,
            }),
        );
        // (The flash advisory fires too — a direction key is by definition
        // bound on its own — so filter to the finding under test.)
        let issues = socd_only(validate(&socd_config("neutral"), &[file]));
        assert_eq!(
            issues,
            vec![Issue::SocdShadowedByChord {
                slot: 1,
                preset: "stick".into(),
                socd: "neutral".into(),
                keys: "Right+Left".into(),
                function: "rt".into(),
            }],
            "{issues:?}"
        );
        // Advisory: the config works exactly as written, so the session starts.
        assert!(issues[0].is_advisory());
        let msg = issues[0].to_string();
        assert!(msg.contains("by hand"), "{msg}");
    }

    /// `socd = "off"` generates nothing, so it can shadow nothing.
    #[test]
    fn socd_off_never_reports_shadowing() {
        let mut file = stick_preset();
        file.bindings.insert(
            "rt".into(),
            BindingEntry::Guarded(GuardedEntry {
                key: "Right".into(),
                when: vec!["Left".into()],
                unless: Vec::new(),
                turbo_hz: None,
            }),
        );
        assert_eq!(
            socd_only(validate(&socd_config("off"), &[file])),
            Vec::new()
        );
    }

    /// Game slots carry the policy too, and are checked the same way.
    #[test]
    fn game_slots_are_socd_checked() {
        let mut file = stick_preset();
        file.bindings.insert(
            "rt".into(),
            BindingEntry::Guarded(GuardedEntry {
                key: "Down".into(),
                when: vec!["Up".into()],
                unless: Vec::new(),
                turbo_hz: None,
            }),
        );
        let games: GamesFile = toml::from_str(
            "[[game]]\ntitle = \"SF\"\npath = 'C:\\sf.exe'\n\n\
             [[game.slot]]\nnumber = 2\npreset = \"stick\"\nsocd = \"up-priority\"\n",
        )
        .unwrap();
        let issues = socd_only(validate_games(&games, &[file]));
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(matches!(
            issues[0],
            Issue::SocdShadowedByChord { slot: 2, .. }
        ));
    }

    /// `consume` without a guard consumes nothing — inert, and worth saying.
    #[test]
    fn an_unguarded_consume_row_is_reported() {
        let presets = vec![preset("inert", "consume = \"Left\"")];
        let issues = validate(&ConfigFile::default(), &presets);
        assert_eq!(
            issues,
            vec![Issue::ConsumeWithoutGuard {
                preset: "inert".into(),
                key: "Left".into()
            }]
        );
        assert!(issues[0].is_advisory());
        assert!(issues[0].to_string().contains("does nothing"));

        // With a guard it is the SOCD primitive, and perfectly clean.
        let good = vec![preset(
            "neutral",
            "consume = { key = \"Left\", when = [\"Right\"] }",
        )];
        assert_eq!(validate(&ConfigFile::default(), &good), Vec::new());
    }

    #[test]
    fn builtin_presets_round_as_valid() {
        let files: Vec<PresetFile> = ksx_core::Preset::builtins()
            .iter()
            .map(PresetFile::from_core)
            .collect();
        assert_eq!(validate(&ConfigFile::default(), &files), Vec::new());
    }

    #[test]
    fn games_are_checked() {
        let games: GamesFile = toml::from_str(
            r#"
[[game]]
title = "Steam"
path = 'C:\steam.exe'

[[game.slot]]
number = 1
user_index = 5
preset = "missing"

[[game.slot]]
number = 1
preset = "default"

[[game.slot]]
number = 0
preset = "empty"
"#,
        )
        .unwrap();
        let issues = validate_games(&games, &[]);
        assert!(issues.contains(&Issue::GameDuplicateSlotNumber {
            game: "Steam".into(),
            number: 1
        }));
        assert!(issues.contains(&Issue::GameSlotNumberOutOfRange {
            game: "Steam".into(),
            number: 0
        }));
        assert!(issues.contains(&Issue::GameUnknownPresetRef {
            game: "Steam".into(),
            slot: 1,
            preset: "missing".into()
        }));
        assert!(issues.contains(&Issue::GameUserIndexOutOfRange {
            game: "Steam".into(),
            slot: 1,
            value: 5
        }));
        assert_eq!(issues.len(), 4);
    }

    #[test]
    fn issues_display_and_serialize() {
        let issue = Issue::UnknownPresetRef {
            slot: 2,
            preset: "sf2".into(),
        };
        assert_eq!(
            issue.to_string(),
            "slot 2 references preset 'sf2', which is neither a preset file nor a built-in"
        );
        let json = serde_json_shape(&issue);
        assert!(json.contains("unknown_preset_ref"));
    }

    // toml is the only serializer in-tree; shape-check via toml, which uses
    // the same serde field names a future --json path will see.
    fn serde_json_shape(issue: &Issue) -> String {
        #[derive(Serialize)]
        struct Wrap<'a> {
            issue: &'a Issue,
        }
        toml::to_string(&Wrap { issue }).unwrap()
    }

    /// **The "my diagonal never comes out" advisory.** Victor's preset drives
    /// its directions on the LEFT STICK with the dpad unbound; a macro whose
    /// diagonal step was written with `dpad.*` publishes dpad bits his game
    /// never reads, so the motion arrives with exactly that step missing. The
    /// engine is right and the file is the trap — so validation names it.
    #[test]
    fn a_macro_holding_the_other_mechanism_is_reported() {
        let presets = vec![macro_preset(
            "[bindings]\n\"ly.min\" = \"Down\"\n\"lx.max\" = \"Right\"\nA = \"S\"\n\
             macro.hadouken = \"P\"\n\
             [macros.hadouken]\n\
             steps = [{ hold = [\"ly.min\"], ms = 50 }, \
                      { hold = [\"dpad.down\",\"dpad.right\"], ms = 50 }, \
                      { hold = [\"lx.max\"], ms = 50 }, \
                      { hold = [\"A\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        // One per offending hold, naming the step — the diagonal, step 1.
        assert_eq!(issues.len(), 2, "{issues:?}");
        for (i, issue) in issues.iter().enumerate() {
            assert_eq!(
                *issue,
                Issue::MacroHoldsOtherMechanism {
                    preset: "m".into(),
                    name: "hadouken".into(),
                    step: 1,
                    function: if i == 0 { "dpad.down" } else { "dpad.right" }.into(),
                    holds: "the dpad".into(),
                    preset_drives: "the left stick (lx/ly)".into(),
                },
                "{issues:?}"
            );
            assert!(issue.is_advisory());
        }
        let text = issues[0].to_string();
        assert!(text.contains("NEUTRAL"), "{text}");
        assert!(text.contains("the left stick (lx/ly)"), "{text}");
    }

    /// ...and the same trap the other way round: a dpad preset with a macro
    /// written on the stick.
    #[test]
    fn the_mechanism_advisory_works_in_both_directions() {
        let presets = vec![macro_preset(
            "[bindings]\n\"dpad.down\" = \"Down\"\n\"dpad.right\" = \"Right\"\n\
             [macros.m]\nsteps = [{ hold = [\"lx.max\"], ms = 50 }]\n",
        )];
        assert!(matches!(
            validate(&ConfigFile::default(), &presets).as_slice(),
            [Issue::MacroHoldsOtherMechanism { step: 0, .. }]
        ));
    }

    /// It must not cry wolf. A preset that drives BOTH mechanisms reads both,
    /// buttons have no mechanism at all, and a preset with no direction keys
    /// gives nothing to compare against.
    #[test]
    fn the_mechanism_advisory_stays_quiet_when_it_has_nothing_to_say() {
        // Both driven: either spelling is fine.
        let both = vec![macro_preset(
            "[bindings]\n\"ly.min\" = \"Down\"\n\"dpad.down\" = \"Down\"\n\
             [macros.m]\nsteps = [{ hold = [\"dpad.down\",\"ly.min\"], ms = 50 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &both), Vec::new());

        // Buttons and triggers are read the same way whatever the preset does.
        let buttons = vec![macro_preset(
            "[bindings]\n\"ly.min\" = \"Down\"\n\
             [macros.m]\nsteps = [{ hold = [\"A\",\"rt\"], ms = 50 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &buttons), Vec::new());

        // No direction keys at all: nothing to compare against, so nothing said.
        let silent = vec![macro_preset(
            "[bindings]\nA = \"S\"\n[macros.m]\nsteps = [{ hold = [\"dpad.up\"], ms = 50 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &silent), Vec::new());

        // An UNBOUND direction row is a placeholder, not something the player
        // can produce — it must not count as "this preset drives the stick".
        let placeholder = vec![macro_preset(
            "[bindings]\n\"ly.min\" = \"None\"\n\"dpad.down\" = \"Down\"\n\
             [macros.m]\nsteps = [{ hold = [\"dpad.down\"], ms = 50 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &placeholder), Vec::new());
    }

    /// The turbo rate: refused when it cannot be read, reported when it cannot
    /// be delivered, and named as inert when nothing repeats.
    #[test]
    fn turbo_rates_are_reported_rather_than_silently_adjusted() {
        // Above the ceiling: clamped, with both numbers stated.
        let fast = vec![macro_preset(
            "[macros.m]\nrepeat = \"turbo\"\nturbo_hz = 120\n\
             steps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        assert_eq!(
            validate(&ConfigFile::default(), &fast),
            vec![Issue::TurboRateClamped {
                preset: "m".into(),
                name: "m".into(),
                asked_hz: 120,
                effective_hz: 12,
            }]
        );

        // A gap below the sampling floor is raised, loudly.
        let tight = vec![macro_preset(
            "[macros.m]\nrepeat = \"turbo\"\ngap_ms = 5\n\
             steps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &tight);
        assert_eq!(
            issues,
            vec![Issue::TurboGapRaised {
                preset: "m".into(),
                name: "m".into(),
                ms: 5,
                raised_to: MIN_STEP_MS,
            }]
        );
        assert!(issues[0].to_string().contains("one long hold"));

        // A rate on a macro that does not repeat does nothing, and says so.
        let inert = vec![macro_preset(
            "[macros.m]\nturbo_hz = 10\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        assert_eq!(
            validate(&ConfigFile::default(), &inert),
            vec![Issue::TurboRateWithoutTurbo {
                preset: "m".into(),
                name: "m".into(),
                repeat: "once".into(),
            }]
        );

        // Both units, and a turbo with none: refused, not guessed.
        for body in [
            "repeat = \"turbo\"\nturbo_hz = 10\ngap_ms = 50\n",
            "repeat = \"turbo\"\n",
        ] {
            let bad = vec![macro_preset(&format!(
                "[macros.m]\n{body}steps = [{{ hold = [\"A\"], ms = 50 }}]\n"
            ))];
            let issues = validate(&ConfigFile::default(), &bad);
            assert!(
                matches!(issues.as_slice(), [Issue::MacroTurboBadRate { .. }]),
                "{body:?} gave {issues:?}"
            );
            assert!(!issues[0].is_advisory());
        }
    }

    /// A deliverable turbo is completely clean — no advisory for a rate that
    /// arrives exactly as asked.
    #[test]
    fn a_deliverable_turbo_reports_nothing() {
        let presets = vec![macro_preset(
            "[bindings]\nA = \"S\"\nmacro.fire = \"P\"\n\
             [macros.fire]\nrepeat = \"turbo\"\nturbo_hz = 10\n\
             steps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &presets), Vec::new());
    }

    // ---- per-binding turbo (docs/INPUT-TRANSFORMS.md §3) ------------------

    /// A deliverable per-binding rate is completely clean.
    #[test]
    fn a_deliverable_binding_turbo_reports_nothing() {
        let presets = vec![preset("t", "A = { key = \"G\", turbo_hz = 12 }\n")];
        assert_eq!(validate(&ConfigFile::default(), &presets), Vec::new());
    }

    /// An undeliverable one is stated, not silently substituted — and it is
    /// ADVISORY, because the engine still does the closest honest thing.
    #[test]
    fn a_binding_turbo_above_the_ceiling_states_both_numbers() {
        let presets = vec![preset("t", "A = { key = \"G\", turbo_hz = 30 }\n")];
        let issues = validate(&ConfigFile::default(), &presets);
        assert_eq!(
            issues,
            vec![Issue::BindingTurboClamped {
                preset: "t".into(),
                function: "A".into(),
                asked_hz: 30,
                effective_hz: 15,
            }]
        );
        assert!(issues[0].is_advisory());
        let text = issues[0].to_string();
        assert!(text.contains("30"), "{text}");
        assert!(text.contains("15"), "{text}");
    }

    /// Two rows on one function that disagree is REFUSED, not resolved: which
    /// one wins would be a file-order accident.
    #[test]
    fn two_rates_on_one_function_are_refused() {
        let presets = vec![preset(
            "t",
            "A = [{ key = \"G\", turbo_hz = 12 }, { key = \"H\", turbo_hz = 6 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert_eq!(
            issues,
            vec![Issue::ConflictingTurboRates {
                preset: "t".into(),
                function: "A".into(),
                first_hz: 12,
                other_hz: 6,
            }]
        );
        assert!(!issues[0].is_advisory());
    }

    /// The same rate twice is a redundancy, not a contradiction: nothing to
    /// report, because there is only one answer.
    #[test]
    fn the_same_rate_twice_on_one_function_is_fine() {
        let presets = vec![preset(
            "t",
            "A = [{ key = \"G\", turbo_hz = 12 }, { key = \"H\", turbo_hz = 12 }]\n",
        )];
        assert_eq!(validate(&ConfigFile::default(), &presets), Vec::new());
    }

    /// A rate on `consume` auto-fires nothing: a consume-only chord drives no
    /// endpoint at all.
    #[test]
    fn a_rate_on_consume_is_reported() {
        let presets = vec![preset(
            "t",
            "\"lx.min\" = \"Left\"\n\"lx.max\" = \"Right\"\n\
             consume = { key = \"Left\", when = [\"Right\"], turbo_hz = 10 }\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(
            issues.contains(&Issue::TurboOnConsume {
                preset: "t".into(),
                function: "consume".into(),
            }),
            "{issues:?}"
        );
    }

    /// A macro repeats by saying so in its own table. A rate on its trigger row
    /// is named here as well as refused at load time, so `ksx check` explains
    /// it rather than the file merely failing to parse.
    #[test]
    fn a_rate_on_a_macro_trigger_is_named() {
        let presets = vec![macro_preset(
            "[bindings]\nA = \"S\"\nmacro.fire = { key = \"P\", turbo_hz = 10 }\n\
             [macros.fire]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )];
        let issues = validate(&ConfigFile::default(), &presets);
        assert!(
            issues.contains(&Issue::GuardedMacroTurbo {
                preset: "m".into(),
                name: "fire".into(),
            }),
            "{issues:?}"
        );
    }
}
