//! Preset file schema: `%APPDATA%\ksx\presets\<name>.toml` — one file per
//! preset (diffable, shareable).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use ksx_core::{
    Interrupt, Macro, MacroStep, MacroTrigger, Macros, OnRelease, Repeat, Retrigger, StepDuration,
    TurboRate,
};

use crate::error::ConfigError;
use crate::function::{function_name, parse_function};
use crate::macro_serde::is_false;

/// One preset file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFile {
    pub name: String,
    /// Keys are function names (see [`crate::function`]); values are legacy
    /// key-name strings or arrays of them.
    #[serde(default)]
    pub bindings: BTreeMap<String, BindingEntry>,
    /// Timed sequences, keyed by macro name (docs/INPUT-TRANSFORMS.md §1c).
    /// A preset with none serializes to exactly the bytes it always did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub macros: BTreeMap<String, MacroFile>,
}

/// One `[macros.<name>]` table.
///
/// ```toml
/// [macros.hadouken]
/// steps = [
///   { hold = ["dpad.down"],              ms = 50 },
///   { hold = ["dpad.down","dpad.right"], ms = 50 },
///   { hold = ["dpad.right"],             ms = 50 },
///   { hold = ["A"],                      ms = 50 },
/// ]
///
/// [bindings]
/// macro.hadouken = "P"
/// ```
///
/// `deny_unknown_fields` because a typo in `on_release` must not silently mean
/// "the default" on a setting whose whole job is to decide what happens when
/// the player lets go.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroFile {
    /// Ordered steps. An empty list is accepted by the parser and reported by
    /// validation — loading is lenient, validation is where it becomes
    /// actionable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<MacroStepFile>,
    /// What a release of the trigger key does: `finish` (default) or `abort`.
    #[serde(
        default,
        with = "crate::macro_serde::on_release",
        skip_serializing_if = "crate::macro_serde::on_release::is_default"
    )]
    pub on_release: OnRelease,
    /// What a second press does while the macro runs: `ignore` (default) or
    /// `restart`.
    #[serde(
        default,
        with = "crate::macro_serde::retrigger",
        skip_serializing_if = "crate::macro_serde::retrigger::is_default"
    )]
    pub retrigger: Retrigger,
    /// What OTHER input does to a run in flight: `none` (default),
    /// `any-input` or `opposing`.
    #[serde(
        default,
        with = "crate::macro_serde::interrupt",
        skip_serializing_if = "crate::macro_serde::interrupt::is_default"
    )]
    pub interrupt: Interrupt,
    /// What the END of a run does while the trigger is still held: `once`
    /// (default), `while-held` or `turbo`.
    ///
    /// ```toml
    /// [macros.autofire]
    /// repeat = "turbo"
    /// turbo_hz = 10          # or: gap_ms = 50
    /// steps = [{ hold = ["A"], frames = 2 }]
    /// ```
    #[serde(
        default,
        with = "crate::macro_serde::repeat",
        skip_serializing_if = "crate::macro_serde::repeat::is_default"
    )]
    pub repeat: Repeat,
    /// Turbo rate in full cycles per second, clamped to
    /// [`ksx_core::TURBO_MAX_HZ`]. Mutually exclusive with `gap_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turbo_hz: Option<u32>,
    /// The neutral window BETWEEN turbo runs, in milliseconds — the other way
    /// to say the same number. Raised to [`ksx_core::MIN_STEP_MS`] for the same
    /// reason a step is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_ms: Option<u32>,
}

impl MacroFile {
    /// The authored turbo rate, or why it cannot be read.
    ///
    /// Both units at once is an error for the same reason `ms`+`frames` is. A
    /// `turbo` with NEITHER is an error too: an auto-fire whose rate ksx picked
    /// is an auto-fire nobody asked for. Outside `repeat = "turbo"` a rate is
    /// simply carried (so flipping the policy back and forth does not lose the
    /// number) and validation says it is doing nothing.
    pub fn turbo_rate(&self) -> Result<Option<TurboRate>, &'static str> {
        match (self.turbo_hz, self.gap_ms) {
            (Some(_), Some(_)) => Err("says both `turbo_hz` and `gap_ms`; use exactly one"),
            (Some(hz), None) => Ok(Some(TurboRate::Hz(hz))),
            (None, Some(ms)) => Ok(Some(TurboRate::GapMs(ms))),
            (None, None) if self.repeat == Repeat::Turbo => {
                Err("is `repeat = \"turbo\"` but gives no rate")
            }
            (None, None) => Ok(None),
        }
    }
}

/// One step: the function names to hold, and for how long.
///
/// The duration is given as EITHER `ms` or `frames`, never both — two units for
/// one number would make "which wins" a thing a reader has to remember.
/// `frames` is 60 Hz and is an ergonomic unit only; see
/// [`ksx_core::StepDuration::Frames`] for exactly how weak that promise is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroStepFile {
    /// Function names ([`crate::function`]), held together for the whole step.
    /// Empty is legal and means a deliberate neutral gap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hold: Vec<String>,
    /// Requested hold in milliseconds. Below [`ksx_core::MIN_STEP_MS`] it is
    /// RAISED unless `allow_short` says otherwise (§0.2, the sampling rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
    /// The same, in 60 Hz frames — the unit a fighting-game player thinks in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u32>,
    /// "I know this is shorter than a 60 Hz poller can see." Validation warns
    /// every time this is set.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_short: bool,
}

impl MacroStepFile {
    /// The authored duration, or why it cannot be read.
    ///
    /// Both units at once and neither unit at all are BOTH errors: a step with
    /// no duration is not "instant", it is a file that forgot to say something,
    /// and guessing a default here would put an invisible input on the wire.
    pub fn duration(&self) -> Result<StepDuration, &'static str> {
        match (self.ms, self.frames) {
            (Some(_), Some(_)) => Err("says both `ms` and `frames`; use exactly one"),
            (Some(ms), None) => Ok(StepDuration::Ms(ms)),
            (None, Some(frames)) => Ok(StepDuration::Frames(frames)),
            (None, None) => Err("has no duration; give it `ms` or `frames`"),
        }
    }
}

impl MacroFile {
    pub(crate) fn to_core(&self, name: &str) -> Result<Macro, ConfigError> {
        let mut steps = Vec::with_capacity(self.steps.len());
        for (i, step) in self.steps.iter().enumerate() {
            let duration = step.duration().map_err(|reason| {
                ConfigError::MacroStepDuration(format!("macro '{name}' step {i} {reason}"))
            })?;
            steps.push(MacroStep {
                hold: step
                    .hold
                    .iter()
                    .map(|f| parse_function(f))
                    .collect::<Result<Vec<_>, _>>()?,
                duration,
                allow_short: step.allow_short,
            });
        }
        let turbo = self
            .turbo_rate()
            .map_err(|reason| ConfigError::MacroTurboRate(format!("macro '{name}' {reason}")))?;
        Ok(Macro {
            name: name.to_owned(),
            steps,
            on_release: self.on_release,
            retrigger: self.retrigger,
            interrupt: self.interrupt,
            repeat: self.repeat,
            turbo,
        })
    }

    fn from_core(mac: &Macro) -> Self {
        Self {
            steps: mac
                .steps
                .iter()
                .map(|step| {
                    // Emitted in the unit it was authored in: a sequence written
                    // in frames must still read in frames after a round trip.
                    let (ms, frames) = match step.duration {
                        StepDuration::Ms(ms) => (Some(ms), None),
                        StepDuration::Frames(frames) => (None, Some(frames)),
                    };
                    MacroStepFile {
                        hold: step.hold.iter().map(function_name).collect(),
                        ms,
                        frames,
                        allow_short: step.allow_short,
                    }
                })
                .collect(),
            on_release: mac.on_release,
            retrigger: mac.retrigger,
            interrupt: mac.interrupt,
            repeat: mac.repeat,
            // Emitted in the unit it was authored in, like a step's duration:
            // a turbo written as a rate must still read as a rate.
            turbo_hz: match mac.turbo {
                Some(TurboRate::Hz(hz)) => Some(hz),
                _ => None,
            },
            gap_ms: match mac.turbo {
                Some(TurboRate::GapMs(ms)) => Some(ms),
                _ => None,
            },
        }
    }
}

/// Value side of a `[bindings]` entry: one key, several keys, a GUARDED key
/// (a chord), a mixed list of those, or a nested group produced by TOML dotted
/// keys (`dpad.up = "I"` parses as a `dpad` table containing `up`). A quoted
/// literal key with a dot (`"lx.min"`) and a nested group are equivalent;
/// conversion flattens groups with `.`.
///
/// ```toml
/// [bindings]
/// A  = "G"                                  # plain, exactly as before
/// rt = { key = "A", when = ["B"] }           # chord: A while B is held
/// lb = ["Q", { key = "A", when = ["C"] }]    # both, on one function
/// ```
///
/// Variant order is load-bearing: `serde(untagged)` tries them top to bottom,
/// so a plain string still parses as [`BindingEntry::Key`] and a table with a
/// `key` field is a guard rather than a dotted group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindingEntry {
    Key(String),
    Keys(Vec<String>),
    Guarded(GuardedEntry),
    /// Mixed list: plain keys and guarded keys on the same function.
    Many(Vec<BindingEntry>),
    Group(BTreeMap<String, BindingEntry>),
}

/// A guarded key — the file spelling of [`ksx_core::Chord`].
///
/// `key` is the trigger; `when` keys must all be held, `unless` keys must not
/// be. `deny_unknown_fields` is what keeps a dotted group (`lx = { min = … }`)
/// from ever being mistaken for a guard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardedEntry {
    /// Trigger key name (legacy spelling), or `"None"` for an inert row.
    pub key: String,
    /// All of these must be held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<String>,
    /// None of these may be held (MAME's `NOT`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unless: Vec<String>,
}

impl PresetFile {
    /// Convert to the core model. Key names use exact legacy spelling
    /// (`Key::from_name`); `"None"` entries are preserved as inert
    /// placeholders. `protected` is always `false`: built-ins live in code,
    /// files are user presets.
    /// Macro definitions are converted FIRST, because a `macro.<name>` binding
    /// row is resolved to an index into them; an unknown name is a hard error
    /// here and a named [`crate::Issue`] in validation.
    pub fn to_core(&self) -> Result<ksx_core::Preset, ConfigError> {
        let mut macros = Macros::default();
        for (name, def) in &self.macros {
            macros.defs.push(def.to_core(name)?);
        }
        let mut entries = Vec::new();
        let mut chords = Vec::new();
        for (function, entry) in &self.bindings {
            collect_entries(function, entry, &mut entries, &mut chords, &mut macros)?;
        }
        Ok(ksx_core::Preset {
            name: self.name.clone(),
            entries,
            chords,
            macros,
            protected: false,
        })
    }

    /// Convert from the core model. Entries are grouped by function name
    /// (multiple keys become an array); emission uses flat literal keys
    /// (`"dpad.up"`), which parse back identically to the nested form.
    /// `protected` does not survive the trip.
    ///
    /// Chords are emitted after the function's plain keys, so a preset with no
    /// chords serializes to exactly the bytes it always did.
    pub fn from_core(preset: &ksx_core::Preset) -> Self {
        let mut grouped: BTreeMap<String, (Vec<String>, Vec<GuardedEntry>)> = BTreeMap::new();
        for (key, binding) in &preset.entries {
            grouped
                .entry(function_name(binding))
                .or_default()
                .0
                .push(key.name().to_owned());
        }
        for chord in &preset.chords {
            grouped
                .entry(function_name(&chord.binding))
                .or_default()
                .1
                .push(GuardedEntry {
                    key: chord.key.name().to_owned(),
                    when: chord.when.iter().map(|k| k.name().to_owned()).collect(),
                    unless: chord.unless.iter().map(|k| k.name().to_owned()).collect(),
                });
        }
        // Macro triggers are ordinary `[bindings]` rows under a `macro.<name>`
        // function, so several keys on one macro emit as an array exactly like
        // several keys on one button.
        for trigger in &preset.macros.triggers {
            let Some(mac) = preset.macros.get(trigger.index) else {
                continue; // dangling index: validation names it, we drop it
            };
            grouped
                .entry(crate::function::macro_function_name(&mac.name))
                .or_default()
                .0
                .push(trigger.key.name().to_owned());
        }
        let bindings = grouped
            .into_iter()
            .map(|(function, (mut keys, mut guards))| {
                let entry = match (keys.len(), guards.len()) {
                    (1, 0) => BindingEntry::Key(keys.remove(0)),
                    (0, 1) => BindingEntry::Guarded(guards.remove(0)),
                    (_, 0) => BindingEntry::Keys(keys),
                    _ => BindingEntry::Many(
                        keys.into_iter()
                            .map(BindingEntry::Key)
                            .chain(guards.into_iter().map(BindingEntry::Guarded))
                            .collect(),
                    ),
                };
                (function, entry)
            })
            .collect();
        Self {
            name: preset.name.clone(),
            bindings,
            macros: preset
                .macros
                .defs
                .iter()
                .map(|mac| (mac.name.clone(), MacroFile::from_core(mac)))
                .collect(),
        }
    }
}

fn collect_entries(
    function: &str,
    entry: &BindingEntry,
    out: &mut Vec<(ksx_core::Key, ksx_core::Binding)>,
    chords: &mut Vec<ksx_core::Chord>,
    macros: &mut Macros,
) -> Result<(), ConfigError> {
    match entry {
        BindingEntry::Key(key) => push_entry(function, key, out, macros),
        BindingEntry::Keys(keys) => {
            for key in keys {
                push_entry(function, key, out, macros)?;
            }
            Ok(())
        }
        BindingEntry::Guarded(guarded) if guarded.when.is_empty() && guarded.unless.is_empty() => {
            push_entry(function, &guarded.key, out, macros)
        }
        BindingEntry::Guarded(guarded) => push_chord(function, guarded, chords),
        BindingEntry::Many(entries) => {
            for entry in entries {
                collect_entries(function, entry, out, chords, macros)?;
            }
            Ok(())
        }
        BindingEntry::Group(group) => {
            for (sub, entry) in group {
                collect_entries(&format!("{function}.{sub}"), entry, out, chords, macros)?;
            }
            Ok(())
        }
    }
}

fn push_entry(
    function: &str,
    key_name: &str,
    out: &mut Vec<(ksx_core::Key, ksx_core::Binding)>,
    macros: &mut Macros,
) -> Result<(), ConfigError> {
    // A `macro.<name>` row starts a sequence rather than driving an endpoint,
    // so it never reaches the pad-function vocabulary at all.
    if let Some(name) = crate::function::macro_name(function) {
        let index = macros
            .index_of(name)
            .ok_or_else(|| ConfigError::UnknownMacro(name.to_owned()))?;
        macros
            .triggers
            .push(MacroTrigger::new(key_named(key_name)?, index));
        return Ok(());
    }
    let binding = parse_function(function)?;
    let key = key_named(key_name)?;
    out.push((key, binding));
    Ok(())
}

/// A guarded entry with a non-empty guard. An EMPTY guard is normalized to a
/// plain entry by the caller: `{ key = "G" }` means exactly `"G"`, and must
/// not consume anything (a zero-key "chord" that suppressed its own trigger
/// would silently disable that key's other bindings).
fn push_chord(
    function: &str,
    guarded: &GuardedEntry,
    chords: &mut Vec<ksx_core::Chord>,
) -> Result<(), ConfigError> {
    if let Some(name) = crate::function::macro_name(function) {
        return Err(ConfigError::GuardedMacroTrigger(name.to_owned()));
    }
    let binding = parse_function(function)?;
    let key = key_named(&guarded.key)?;
    let when = guarded
        .when
        .iter()
        .map(|k| key_named(k))
        .collect::<Result<Vec<_>, _>>()?;
    let unless = guarded
        .unless
        .iter()
        .map(|k| key_named(k))
        .collect::<Result<Vec<_>, _>>()?;
    chords.push(ksx_core::Chord {
        key,
        binding,
        when,
        unless,
    });
    Ok(())
}

fn key_named(name: &str) -> Result<ksx_core::Key, ConfigError> {
    ksx_core::Key::from_name(name).ok_or_else(|| ConfigError::UnknownKey(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_core::{Axis, Binding, DpadDirection, Key, Preset, Trigger, XButton, AXIS_MIN};

    // docs/research/design-architecture.md §4.1, verbatim.
    const DOC_EXAMPLE: &str = r#"
name = "street-fighter-p1"
[bindings]
A = "S"
B = "D"
lt = "Q"
"lx.min" = "Left"
"lx.-16384" = "None"
dpad.up = "I"
"#;

    fn sorted_entries(preset: &Preset) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = preset
            .entries
            .iter()
            .map(|(k, b)| (function_name(b), k.name().to_owned()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn doc_example_parses_and_converts() {
        let file: PresetFile = toml::from_str(DOC_EXAMPLE).unwrap();
        assert_eq!(file.name, "street-fighter-p1");

        let core = file.to_core().unwrap();
        assert_eq!(core.name, "street-fighter-p1");
        assert!(!core.protected);
        assert_eq!(core.entries.len(), 6);
        assert!(core
            .entries
            .contains(&(Key::S, Binding::Button(XButton::A))));
        assert!(core
            .entries
            .contains(&(Key::D, Binding::Button(XButton::B))));
        assert!(core
            .entries
            .contains(&(Key::Q, Binding::Trigger(Trigger::Left))));
        assert!(core.entries.contains(&(
            Key::Left,
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN
            }
        )));
        // Custom axis value with a placeholder key survives.
        assert!(core.entries.contains(&(
            Key::None,
            Binding::Axis {
                axis: Axis::X,
                value: -16384
            }
        )));
        assert!(core
            .entries
            .contains(&(Key::I, Binding::Dpad(DpadDirection::Up))));
    }

    #[test]
    fn file_round_trips_through_toml() {
        let file: PresetFile = toml::from_str(DOC_EXAMPLE).unwrap();
        let serialized = toml::to_string(&file).unwrap();
        let reparsed: PresetFile = toml::from_str(&serialized).unwrap();
        assert_eq!(file, reparsed);
    }

    #[test]
    fn arrays_mean_many_to_one() {
        let file: PresetFile = toml::from_str(
            r#"
name = "p"
[bindings]
A = ["S", "Enter"]
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        assert_eq!(
            core.entries,
            vec![
                (Key::S, Binding::Button(XButton::A)),
                (Key::Enter, Binding::Button(XButton::A)),
            ]
        );
    }

    #[test]
    fn nested_and_literal_dotted_keys_are_equivalent() {
        let nested: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\ndpad.up = \"I\"\n").unwrap();
        let literal: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\n\"dpad.up\" = \"I\"\n").unwrap();
        // Different in-memory shapes...
        assert_ne!(nested, literal);
        // ...identical core meaning.
        assert_eq!(
            nested.to_core().unwrap().entries,
            literal.to_core().unwrap().entries
        );
    }

    #[test]
    fn builtin_default_survives_core_round_trip() {
        let original = Preset::builtin_default();
        let file = PresetFile::from_core(&original);
        // S is listed before Enter for button A (legacy entry order).
        assert_eq!(
            file.bindings.get("A"),
            Some(&BindingEntry::Keys(vec!["S".into(), "Enter".into()]))
        );
        let back = file.to_core().unwrap();
        assert_eq!(back.name, original.name);
        assert_eq!(sorted_entries(&back), sorted_entries(&original));
        // protected is a code-level attribute and does not survive.
        assert!(!back.protected);
    }

    #[test]
    fn builtin_empty_survives_core_round_trip() {
        let original = Preset::builtin_empty();
        let file = PresetFile::from_core(&original);
        assert_eq!(file.bindings.len(), 25);
        assert!(file
            .bindings
            .values()
            .all(|e| *e == BindingEntry::Key("None".into())));
        let back = file.to_core().unwrap();
        assert_eq!(sorted_entries(&back), sorted_entries(&original));
    }

    #[test]
    fn from_core_emission_reparses_via_toml() {
        let file = PresetFile::from_core(&Preset::builtin_default());
        let serialized = toml::to_string(&file).unwrap();
        let reparsed: PresetFile = toml::from_str(&serialized).unwrap();
        assert_eq!(file, reparsed);
    }

    // ---- chords (docs/INPUT-TRANSFORMS.md §1b) ----------------------------

    /// The documented file shape, and the promise that adding one does not
    /// disturb the plain rows around it.
    #[test]
    fn a_guarded_entry_parses_as_a_chord() {
        let file: PresetFile = toml::from_str(
            r#"
name = "chords"
[bindings]
A = "G"
rt = { key = "A", when = ["B"] }
lb = { key = "A", when = ["B", "C"], unless = ["LeftShift"] }
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        // Plain rows stay plain rows.
        assert_eq!(core.entries, vec![(Key::G, Binding::Button(XButton::A))]);
        assert_eq!(core.chords.len(), 2);
        assert!(core.chords.contains(&ksx_core::Chord {
            key: Key::A,
            binding: Binding::Trigger(Trigger::Right),
            when: vec![Key::B],
            unless: vec![],
        }));
        assert!(core.chords.contains(&ksx_core::Chord {
            key: Key::A,
            binding: Binding::Button(XButton::LeftBumper),
            when: vec![Key::B, Key::C],
            unless: vec![Key::LeftShift],
        }));
    }

    /// One function can carry plain keys AND chords; the list form round-trips.
    #[test]
    fn a_function_can_hold_plain_keys_and_chords_together() {
        let original = Preset {
            name: "mixed".into(),
            entries: vec![
                (Key::Q, Binding::Trigger(Trigger::Right)),
                (Key::E, Binding::Trigger(Trigger::Right)),
            ],
            chords: vec![ksx_core::Chord::new(
                Key::A,
                Binding::Trigger(Trigger::Right),
                vec![Key::B],
            )],
            macros: Default::default(),
            protected: false,
        };
        let file = PresetFile::from_core(&original);
        let text = toml::to_string(&file).unwrap();
        let reparsed: PresetFile = toml::from_str(&text).unwrap();
        assert_eq!(file, reparsed, "{text}");
        let back = reparsed.to_core().unwrap();
        assert_eq!(back.entries, original.entries);
        assert_eq!(back.chords, original.chords);
    }

    /// Chords survive core → file → TOML → file → core unchanged, including
    /// dotted function names.
    #[test]
    fn chords_round_trip_through_toml() {
        let original = Preset {
            name: "rt".into(),
            entries: vec![(Key::G, Binding::Button(XButton::A))],
            chords: vec![
                ksx_core::Chord::new(Key::D, Binding::Dpad(DpadDirection::Up), vec![Key::F]),
                ksx_core::Chord {
                    key: Key::D,
                    binding: Binding::Axis {
                        axis: Axis::X,
                        value: AXIS_MIN,
                    },
                    when: vec![Key::F, Key::G],
                    unless: vec![Key::LeftShift],
                },
            ],
            macros: Default::default(),
            protected: false,
        };
        let text = toml::to_string(&PresetFile::from_core(&original)).unwrap();
        let back: PresetFile = toml::from_str(&text).unwrap();
        let core = back.to_core().unwrap();
        assert_eq!(core.entries, original.entries, "{text}");
        assert_eq!(core.chords, original.chords, "{text}");
    }

    /// A guard with nothing in it is not a chord: it must not consume its own
    /// trigger key (which would silently disable that key's other bindings).
    #[test]
    fn an_empty_guard_is_a_plain_binding() {
        let file: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nA = { key = \"G\" }\n").unwrap();
        let core = file.to_core().unwrap();
        assert!(core.chords.is_empty());
        assert_eq!(core.entries, vec![(Key::G, Binding::Button(XButton::A))]);
    }

    /// The regression guarantee at the file layer: a preset without chords
    /// emits exactly the bytes it always did — no guard syntax anywhere.
    #[test]
    fn a_chordless_preset_emits_no_guard_syntax() {
        for preset in Preset::builtins() {
            let text = toml::to_string(&PresetFile::from_core(&preset)).unwrap();
            assert!(!text.contains("when"), "{text}");
            assert!(!text.contains("unless"), "{text}");
            assert!(!text.contains("key ="), "{text}");
        }
    }

    // ---- consume-only chords (docs/INPUT-TRANSFORMS.md §2.6) --------------

    /// The SOCD primitive as a file: a guarded row under the `consume`
    /// function, which drives nothing and exists to suppress its keys.
    #[test]
    fn a_consume_only_chord_parses_and_round_trips() {
        let file: PresetFile = toml::from_str(
            r#"
name = "neutral"
[bindings]
"lx.min" = "Left"
"lx.max" = "Right"
consume = { key = "Left", when = ["Right"] }
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        assert_eq!(
            core.chords,
            vec![ksx_core::Chord::consuming(Key::Left, vec![Key::Right])]
        );
        assert!(!core.chords[0].emits());

        let text = toml::to_string(&PresetFile::from_core(&core)).unwrap();
        assert!(text.contains("consume"), "{text}");
        let back: PresetFile = toml::from_str(&text).unwrap();
        let again = back.to_core().unwrap();
        assert_eq!(again.entries, core.entries, "{text}");
        assert_eq!(again.chords, core.chords, "{text}");
    }

    /// Several consume-only chords share one function name, so they emit as a
    /// list and must come back as separate chords.
    #[test]
    fn several_consume_chords_round_trip_as_a_list() {
        let original = Preset {
            name: "socd".into(),
            entries: Vec::new(),
            chords: vec![
                ksx_core::Chord::consuming(Key::Left, vec![Key::Right]),
                ksx_core::Chord::consuming(Key::Up, vec![Key::Down]),
            ],
            macros: Default::default(),
            protected: false,
        };
        let text = toml::to_string(&PresetFile::from_core(&original)).unwrap();
        let back: PresetFile = toml::from_str(&text).unwrap();
        assert_eq!(back.to_core().unwrap().chords, original.chords, "{text}");
    }

    // ---- macros (docs/INPUT-TRANSFORMS.md §1c) ----------------------------

    /// The documented file shape, exactly as the doc writes it.
    const HADOUKEN: &str = r#"
name = "sf-p1"

[bindings]
"dpad.down" = "Down"
"dpad.right" = "Right"
A = "S"
macro.hadouken = "P"

[macros.hadouken]
steps = [
  { hold = ["dpad.down"],                ms = 50 },
  { hold = ["dpad.down","dpad.right"],   ms = 50 },
  { hold = ["dpad.right"],               ms = 50 },
  { hold = ["A"],                        ms = 50 },
]
"#;

    #[test]
    fn the_documented_macro_shape_parses() {
        let file: PresetFile = toml::from_str(HADOUKEN).unwrap();
        let core = file.to_core().unwrap();

        assert_eq!(core.macros.defs.len(), 1);
        let mac = &core.macros.defs[0];
        assert_eq!(mac.name, "hadouken");
        assert_eq!(mac.steps.len(), 4);
        assert_eq!(mac.total_ms(), 200);
        // Defaults are the fighting-game behavior and are never written.
        assert_eq!(mac.on_release, ksx_core::OnRelease::Finish);
        assert_eq!(mac.retrigger, ksx_core::Retrigger::Ignore);
        assert_eq!(mac.interrupt, ksx_core::Interrupt::None);
        // ↘ is one step holding two bindings, not two steps.
        assert_eq!(
            mac.steps[1].hold,
            vec![
                Binding::Dpad(DpadDirection::Down),
                Binding::Dpad(DpadDirection::Right)
            ]
        );
        // The trigger row is a MACRO row, never a pad function.
        assert_eq!(
            core.macros.triggers,
            vec![ksx_core::MacroTrigger::new(Key::P, 0)]
        );
        // ...and the ordinary bindings beside it are untouched.
        assert_eq!(core.entries.len(), 3);
    }

    #[test]
    fn macros_round_trip_through_toml() {
        let file: PresetFile = toml::from_str(HADOUKEN).unwrap();
        let core = file.to_core().unwrap();
        let text = toml::to_string(&PresetFile::from_core(&core)).unwrap();
        let back: PresetFile = toml::from_str(&text).unwrap();
        let again = back.to_core().unwrap();
        assert_eq!(again.macros, core.macros, "{text}");
        assert_eq!(again.entries, core.entries, "{text}");
    }

    /// Policies and the frame unit survive the trip, and a sequence written in
    /// frames still reads in frames afterwards.
    #[test]
    fn policies_and_the_frame_unit_survive_a_round_trip() {
        let file: PresetFile = toml::from_str(
            r#"
name = "p"
[bindings]
macro.dp = "P"

[macros.dp]
on_release = "abort"
retrigger = "restart"
interrupt = "opposing"
steps = [
  { hold = ["dpad.right"], frames = 3 },
  { hold = ["A"], ms = 20, allow_short = true },
]
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        let mac = &core.macros.defs[0];
        assert_eq!(mac.on_release, ksx_core::OnRelease::Abort);
        assert_eq!(mac.retrigger, ksx_core::Retrigger::Restart);
        assert_eq!(mac.interrupt, ksx_core::Interrupt::Opposing);
        assert_eq!(mac.steps[0].duration, ksx_core::StepDuration::Frames(3));
        assert_eq!(mac.steps[0].requested_ms(), 50);
        assert!(mac.steps[1].allow_short);

        let text = toml::to_string(&PresetFile::from_core(&core)).unwrap();
        assert!(text.contains("frames = 3"), "{text}");
        assert!(text.contains("allow_short = true"), "{text}");
        assert_eq!(
            toml::from_str::<PresetFile>(&text)
                .unwrap()
                .to_core()
                .unwrap()
                .macros,
            core.macros,
            "{text}"
        );
    }

    /// A trigger with no `[macros]` table behind it is a hard error, not a
    /// silently inert row.
    #[test]
    fn a_trigger_for_an_undefined_macro_is_an_error() {
        let file: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nmacro.nope = \"P\"\n").unwrap();
        assert!(matches!(file.to_core(), Err(ConfigError::UnknownMacro(_))));
    }

    /// Two units, or none, on one step. Both refused: guessing would put an
    /// input on the wire that no game could sample.
    #[test]
    fn a_step_needs_exactly_one_duration_unit() {
        for steps in [
            "{ hold = [\"A\"], ms = 50, frames = 3 }",
            "{ hold = [\"A\"] }",
        ] {
            let file: PresetFile =
                toml::from_str(&format!("name = \"p\"\n[macros.m]\nsteps = [{steps}]\n")).unwrap();
            let err = file.to_core().unwrap_err();
            assert!(
                matches!(err, ConfigError::MacroStepDuration(_)),
                "{steps} gave {err}"
            );
            assert!(err.to_string().contains("frames"), "{err}");
        }
    }

    /// A macro trigger cannot be a chord — a sequence is started by a key.
    #[test]
    fn a_guarded_macro_trigger_is_refused() {
        let file: PresetFile = toml::from_str(
            "name = \"p\"\n[macros.m]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n\
             [bindings]\n\"macro.m\" = { key = \"P\", when = [\"Q\"] }\n",
        )
        .unwrap();
        assert!(matches!(
            file.to_core(),
            Err(ConfigError::GuardedMacroTrigger(_))
        ));
    }

    /// The regression guarantee at the file layer: a preset without macros
    /// emits exactly the bytes it always did.
    #[test]
    fn a_macro_free_preset_emits_no_macro_syntax() {
        for preset in Preset::builtins() {
            let text = toml::to_string(&PresetFile::from_core(&preset)).unwrap();
            assert!(!text.contains("macro"), "{text}");
        }
    }

    #[test]
    fn unknown_guard_keys_are_errors() {
        let file: PresetFile = toml::from_str(
            "name = \"p\"\n[bindings]\nrt = { key = \"A\", when = [\"NotAKey\"] }\n",
        )
        .unwrap();
        assert!(matches!(file.to_core(), Err(ConfigError::UnknownKey(_))));
    }

    #[test]
    fn unknown_names_are_errors() {
        let bad_function: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nwarp = \"S\"\n").unwrap();
        assert!(matches!(
            bad_function.to_core(),
            Err(ConfigError::UnknownFunction(_))
        ));

        let bad_key: PresetFile =
            toml::from_str("name = \"p\"\n[bindings]\nA = \"eight\"\n").unwrap();
        assert!(matches!(bad_key.to_core(), Err(ConfigError::UnknownKey(_))));
    }

    // ---- repeat / turbo (docs/INPUT-TRANSFORMS.md §1c) ---------------------

    /// The file shape for an auto-fire button, and its round trip. The UI pass
    /// is trivial exactly because this is the whole surface.
    #[test]
    fn repeat_and_the_turbo_rate_round_trip() {
        let file: PresetFile = toml::from_str(
            r#"
name = "p"
[bindings]
macro.fire = "P"

[macros.fire]
repeat = "turbo"
turbo_hz = 10
steps = [{ hold = ["A"], frames = 2 }]
"#,
        )
        .unwrap();
        let core = file.to_core().unwrap();
        let mac = &core.macros.defs[0];
        assert_eq!(mac.repeat, ksx_core::Repeat::Turbo);
        assert_eq!(mac.turbo, Some(ksx_core::TurboRate::Hz(10)));
        // 10 Hz on a 33 ms run is a 100 ms cycle, so a 67 ms released window.
        assert_eq!(mac.turbo_gap_ms(), 67);

        let text = toml::to_string(&PresetFile::from_core(&core)).unwrap();
        assert!(text.contains("repeat = \"turbo\""), "{text}");
        assert!(text.contains("turbo_hz = 10"), "{text}");
        assert_eq!(
            toml::from_str::<PresetFile>(&text)
                .unwrap()
                .to_core()
                .unwrap()
                .macros,
            core.macros,
            "{text}"
        );
    }

    /// `gap_ms` is the other spelling of the same number, and a round trip must
    /// not silently rewrite one into the other.
    #[test]
    fn the_gap_spelling_survives_as_the_gap_spelling() {
        let file: PresetFile = toml::from_str(
            "name = \"p\"\n[macros.fire]\nrepeat = \"turbo\"\ngap_ms = 50\n\
             steps = [{ hold = [\"A\"], ms = 50 }]\n",
        )
        .unwrap();
        let core = file.to_core().unwrap();
        assert_eq!(
            core.macros.defs[0].turbo,
            Some(ksx_core::TurboRate::GapMs(50))
        );
        let text = toml::to_string(&PresetFile::from_core(&core)).unwrap();
        assert!(text.contains("gap_ms = 50"), "{text}");
        assert!(!text.contains("turbo_hz"), "{text}");
    }

    /// `while-held` needs no rate at all, and the aliases people type parse.
    #[test]
    fn while_held_needs_no_rate() {
        let file: PresetFile = toml::from_str(
            "name = \"p\"\n[macros.m]\nrepeat = \"While_Held\"\n\
             steps = [{ hold = [\"A\"], ms = 50 }]\n",
        )
        .unwrap();
        let core = file.to_core().unwrap();
        assert_eq!(core.macros.defs[0].repeat, ksx_core::Repeat::WhileHeld);
        assert_eq!(core.macros.defs[0].turbo, None);
        assert_eq!(core.macros.defs[0].turbo_gap_ms(), 0);
    }

    /// Two units, or none on a turbo. Both refused for the same reason a step's
    /// duration is: an auto-fire whose rate ksx picked is one nobody asked for.
    #[test]
    fn a_turbo_needs_exactly_one_rate_unit() {
        for extra in ["turbo_hz = 10\ngap_ms = 50", ""] {
            let file: PresetFile = toml::from_str(&format!(
                "name = \"p\"\n[macros.m]\nrepeat = \"turbo\"\n{extra}\n\
                 steps = [{{ hold = [\"A\"], ms = 50 }}]\n"
            ))
            .unwrap();
            let err = file.to_core().unwrap_err();
            assert!(
                matches!(err, ConfigError::MacroTurboRate(_)),
                "{extra:?} gave {err}"
            );
            assert!(err.to_string().contains("turbo_hz"), "{err}");
        }
    }

    /// The regression guarantee: `once` is the default and is never written, so
    /// every preset that predates the setting serializes to the bytes it did.
    #[test]
    fn a_macro_that_does_not_repeat_writes_no_repeat_syntax() {
        let file: PresetFile = toml::from_str(HADOUKEN).unwrap();
        let core = file.to_core().unwrap();
        assert_eq!(core.macros.defs[0].repeat, ksx_core::Repeat::Once);
        let text = toml::to_string(&PresetFile::from_core(&core)).unwrap();
        assert!(!text.contains("repeat"), "{text}");
        assert!(!text.contains("turbo"), "{text}");
        assert!(!text.contains("gap_ms"), "{text}");
    }
}
