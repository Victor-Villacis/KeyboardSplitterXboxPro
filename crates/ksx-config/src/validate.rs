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
use crate::preset::{BindingEntry, GuardedEntry, PresetFile};
use ksx_core::socd::{opposing_pairs, shadowing_chord};
use ksx_core::{Binding, Key, Persona, Preset, Socd, MAX_SLOTS, MAX_XINPUT_SLOTS, MIN_STEP_MS};

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
            Issue::DuplicateMacroName { preset, name } => write!(
                f,
                "preset '{preset}': more than one [macros] table is called '{name}' (macro names \
                 are matched ignoring case, so one would shadow the other)"
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
            socd_issues(&cores, slot.number, &slot.preset, slot.socd, &mut issues);
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
}

/// Everything that can only go wrong once a preset carries a timed sequence
/// (docs/INPUT-TRANSFORMS.md §1c).
///
/// The sampling rule (§0.2) is checked here and nowhere else: `MacroStep`
/// decides what the engine *runs*, and this decides what the user is *told*.
fn validate_macros(preset: &PresetFile, pairs: &[(String, Flat<'_>)], issues: &mut Vec<Issue>) {
    let name = || preset.name.clone();

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
        for (i, step) in def.steps.iter().enumerate() {
            for function in &step.hold {
                if parse_function(function).is_err() {
                    issues.push(Issue::UnknownMacroHold {
                        preset: name(),
                        name: macro_name.clone(),
                        step: i,
                        function: function.clone(),
                    });
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
                },
                SlotEntry {
                    number: 1,
                    keyboard: None,
                    mouse: None,
                    preset: "default".into(),
                    persona: Persona::default(),
                    socd: Socd::default(),
                },
                SlotEntry {
                    number: MAX_SLOTS + 1,
                    keyboard: None,
                    mouse: None,
                    preset: "empty".into(),
                    persona: Persona::default(),
                    socd: Socd::default(),
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

        // The supported 8-player shape: 4 Xbox + 4 PlayStation. No issues.
        let cfg = ConfigFile {
            slots: (1..=8)
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
}
