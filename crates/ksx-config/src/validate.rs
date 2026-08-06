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
use crate::function::parse_function;
use crate::games::GamesFile;
use crate::preset::{BindingEntry, GuardedEntry, PresetFile};
use ksx_core::{Key, Persona, Preset, MAX_SLOTS, MAX_XINPUT_SLOTS};

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
    /// that works exactly as written and merely has a cost the user should
    /// know about — today that is only the chord flash
    /// ([`Issue::ChordConstituentAlsoBound`]): binding a chord over keys that
    /// already do something is a legitimate, deliberate choice, so it prints
    /// as a warning and the session starts.
    pub fn is_advisory(&self) -> bool {
        matches!(self, Issue::ChordConstituentAlsoBound { .. })
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
    issues
}

/// Validate `games.toml` against the loaded preset files.
pub fn validate_games(games: &GamesFile, presets: &[PresetFile]) -> Vec<Issue> {
    let mut issues = Vec::new();
    let known_presets = known_preset_names(presets);
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

    let mut checked_functions = BTreeSet::new();
    for (function, flat) in &pairs {
        if checked_functions.insert(function.clone()) {
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
    }

    validate_chords(preset, &pairs, issues);
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
        // normalizes it), so it carries none of these rules.
        .filter(|(_, guard)| !(guard.when.is_empty() && guard.unless.is_empty()))
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
                },
                SlotEntry {
                    number: 1,
                    keyboard: None,
                    mouse: None,
                    preset: "default".into(),
                    persona: Persona::default(),
                },
                SlotEntry {
                    number: MAX_SLOTS + 1,
                    keyboard: None,
                    mouse: None,
                    preset: "empty".into(),
                    persona: Persona::default(),
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
