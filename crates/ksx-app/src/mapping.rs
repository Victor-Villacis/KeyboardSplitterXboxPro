//! The one mapping writer: `ksx map`, the daemon's pipe `map` verb and (through
//! it) Studio's mapper all call [`apply`] — no surface gets a private editing
//! path (docs/CONTROL-SURFACE.md's standing rule).
//!
//! Semantics:
//!
//! - **Replace per function.** `map --function A --key G` makes G the ONLY key
//!   bound to A — the mapper's tile shows one tag per control, and the file
//!   says what the tile says. (Multi-key fan-in stays expressible by hand;
//!   this verb does not author it.)
//! - **`--clear`** leaves the function in the file bound to the inert `"None"`
//!   placeholder — the same convention as the built-in empty preset, so a
//!   cleared control stays visible instead of silently vanishing.
//! - **Conflicts block, the caller decides** (the PadForge gap this closes —
//!   docs/research/padforge-code-audit.md §1.2 "Conflict handling: none"). Two
//!   scopes are checked: the key already bound to ANOTHER function in the same
//!   preset, and the key bound in another slot's preset within any games.toml
//!   profile that uses the target preset. `force` proceeds: same-preset
//!   conflicts are stolen (the key is removed from the old function, which
//!   keeps a `"None"` placeholder if emptied); **other presets are never
//!   edited** — a cross-profile conflict under `force` is written anyway and
//!   reported, because silently rewriting a file the caller did not name is
//!   worse than a double binding the response spells out.
//! - **Writes are canonical.** The store serializes `PresetFile` afresh:
//!   bindings come back sorted with flat quoted dotted keys (`"dpad.up"`), and
//!   hand-written comments do not survive. That is the documented trade for
//!   atomic, validated writes (store.rs); the file remains hand-editable.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ksx_config::{parse_function, ConfigError, PresetFile, Store};
use ksx_core::Key;

/// One requested edit.
#[derive(Clone, Debug)]
pub struct MapSpec {
    /// Preset name (the `name` field, e.g. `"IPAC P1"`), not a file name.
    pub preset: String,
    /// Function name, any case (`A`, `dpad.up`, `lx.min`, `lx.-16384`).
    pub function: String,
    /// `Some(key name)` binds, `None` clears.
    pub key: Option<String>,
    /// Proceed despite conflicts (see module docs for exactly what that does).
    pub force: bool,
}

/// Where a conflicting binding lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictScope {
    /// Another function in the SAME preset. `force` steals from it.
    Preset,
    /// A different slot's preset inside a games.toml profile that also uses
    /// the target preset. Never auto-edited; reported for the caller.
    Profile,
}

/// One conflicting binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapConflict {
    pub scope: ConflictScope,
    /// Preset holding the conflicting binding.
    pub preset: String,
    /// Canonical function name the key is bound to there.
    pub function: String,
    /// Profile title (Profile scope only).
    pub profile: Option<String>,
    /// Slot number inside that profile (Profile scope only).
    pub slot: Option<u8>,
}

impl MapConflict {
    /// One human line, e.g. `G is "IPAC P2"'s A (slot 2 of "Steam")`.
    pub fn describe(&self, key: &str) -> String {
        match self.scope {
            ConflictScope::Preset => format!("{key} is already this preset's {}", self.function),
            ConflictScope::Profile => format!(
                "{key} is \"{}\"'s {}{}",
                self.preset,
                self.function,
                match (&self.profile, self.slot) {
                    (Some(profile), Some(slot)) => format!(" (slot {slot} of \"{profile}\")"),
                    (Some(profile), None) => format!(" (\"{profile}\")"),
                    _ => String::new(),
                }
            ),
        }
    }
}

/// What a successful [`apply`] did.
#[derive(Clone, Debug)]
pub struct AppliedMap {
    /// The file that was written.
    pub path: PathBuf,
    pub preset: String,
    /// Canonical function spelling (what the file now says).
    pub function: String,
    /// Canonical key name, `None` for a clear.
    pub key: Option<String>,
    /// Same-preset functions the key was stolen from (`force`).
    pub stolen_from: Vec<String>,
    /// Cross-profile conflicts that were overridden by `force` — written
    /// anyway, reported so the caller can say so.
    pub overridden: Vec<MapConflict>,
}

impl AppliedMap {
    /// The one-line confirmation every surface prints.
    pub fn message(&self) -> String {
        let mut line = match &self.key {
            Some(key) => format!("\"{}\": {} = {}", self.preset, self.function, key),
            None => format!("\"{}\": {} cleared", self.preset, self.function),
        };
        if !self.stolen_from.is_empty() {
            line.push_str(&format!(" (taken from {})", self.stolen_from.join(", ")));
        }
        if let Some(key) = &self.key {
            for conflict in &self.overridden {
                line.push_str(&format!("; still {}", conflict.describe(key)));
            }
        }
        line
    }
}

/// Why an [`apply`] refused or failed.
#[derive(Debug)]
pub enum MapError {
    /// No preset with that name — nothing is guessed, nothing is created.
    UnknownPreset {
        name: String,
        known: Vec<String>,
    },
    UnknownFunction(String),
    UnknownKey(String),
    /// Conflicts found and `force` not given. The write did NOT happen.
    Conflicts {
        key: String,
        conflicts: Vec<MapConflict>,
    },
    Config(ConfigError),
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapError::UnknownPreset { name, known } => {
                write!(f, "unknown preset \"{name}\"")?;
                if !known.is_empty() {
                    write!(f, " (presets on disk: {})", known.join(", "))?;
                }
                Ok(())
            }
            MapError::UnknownFunction(name) => write!(
                f,
                "unknown function \"{name}\" (buttons A B X Y start back guide lb rb \
                 lthumb rthumb, triggers lt rt, axes lx/ly/rx/ry with .min/.max/.<i16>, \
                 dpad.up/.down/.left/.right)"
            ),
            MapError::UnknownKey(name) => write!(
                f,
                "unknown key \"{name}\" — key names use the legacy spelling \
                 (`ksx monitor` shows the name for any key you press)"
            ),
            MapError::Conflicts { key, conflicts } => {
                write!(f, "refusing to bind {key}: ")?;
                for (i, conflict) in conflicts.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}", conflict.describe(key))?;
                }
                write!(
                    f,
                    " — use --force to bind anyway (same-preset conflicts are \
                     stolen; other presets are never edited)"
                )
            }
            MapError::Config(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for MapError {}

impl From<ConfigError> for MapError {
    fn from(err: ConfigError) -> Self {
        Self::Config(err)
    }
}

/// Load, validate, conflict-check, edit, write. See the module docs for the
/// exact semantics of every step.
pub fn apply(store: &Store, spec: &MapSpec) -> Result<AppliedMap, MapError> {
    // Validate the function and key names FIRST — a typo must not depend on
    // which preset it was aimed at.
    let binding = parse_function(&spec.function)
        .map_err(|_| MapError::UnknownFunction(spec.function.clone()))?;
    let canonical = ksx_config::function_name(&binding);
    let key = spec.key.as_deref().map(resolve_key).transpose()?;

    // The preset must exist; `ksx map` creates bindings, never presets.
    let file = load_preset_by_name(store, &spec.preset)?;
    let mut entries = file.to_core()?.entries;

    let mut stolen_from = Vec::new();
    let mut overridden = Vec::new();
    if let Some(key) = key {
        let conflicts = find_conflicts(store, &spec.preset, &entries, key, &canonical);
        if !conflicts.is_empty() && !spec.force {
            return Err(MapError::Conflicts {
                key: key.name().to_owned(),
                conflicts,
            });
        }
        for conflict in conflicts {
            match conflict.scope {
                ConflictScope::Preset => stolen_from.push(conflict.function.clone()),
                ConflictScope::Profile => overridden.push(conflict),
            }
        }
        // Steal (same preset only): drop the key wherever else it appears,
        // leaving a "None" placeholder if a function would end up empty.
        if !stolen_from.is_empty() {
            let victims: Vec<ksx_core::Binding> = entries
                .iter()
                .filter(|(k, b)| *k == key && ksx_config::function_name(b) != canonical)
                .map(|(_, b)| *b)
                .collect();
            entries.retain(|(k, b)| !(*k == key && ksx_config::function_name(b) != canonical));
            for victim in victims {
                if !entries.iter().any(|(_, b)| {
                    ksx_config::function_name(b) == ksx_config::function_name(&victim)
                }) {
                    entries.push((Key::None, victim));
                }
            }
        }
    }

    // Replace-per-function: out with every old key for this function...
    entries.retain(|(_, b)| ksx_config::function_name(b) != canonical);
    // ...in with the new one (or the inert placeholder for a clear).
    entries.push((key.unwrap_or(Key::None), binding));

    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries,
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedMap {
        path,
        preset: file.name,
        function: canonical,
        key: key.map(|k| k.name().to_owned()),
        stolen_from,
        overridden,
    })
}

/// Exact legacy spelling first; a UNIQUE case-insensitive match is accepted
/// (panel keys get typed as `g` at a shell); anything else is refused.
fn resolve_key(name: &str) -> Result<Key, MapError> {
    if let Some(key) = Key::from_name(name) {
        return Ok(key);
    }
    let mut matches = Key::ALL
        .iter()
        .copied()
        .filter(|k| k.name().eq_ignore_ascii_case(name));
    match (matches.next(), matches.next()) {
        (Some(key), None) => Ok(key),
        _ => Err(MapError::UnknownKey(name.to_owned())),
    }
}

/// Find the preset by its `name` field. The file name is derived storage
/// (store.rs), so this scans `load_presets` rather than guessing a path.
fn load_preset_by_name(store: &Store, name: &str) -> Result<PresetFile, MapError> {
    let loaded = store.load_presets()?;
    let known: Vec<String> = loaded.value.iter().map(|p| p.name.clone()).collect();
    loaded
        .value
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| MapError::UnknownPreset {
            name: name.to_owned(),
            known,
        })
}

/// Both conflict scopes for `key` aimed at `canonical` in `preset_name`.
fn find_conflicts(
    store: &Store,
    preset_name: &str,
    entries: &[(Key, ksx_core::Binding)],
    key: Key,
    canonical: &str,
) -> Vec<MapConflict> {
    let mut conflicts = Vec::new();

    // Same preset: the key on any OTHER function.
    for (k, b) in entries {
        let function = ksx_config::function_name(b);
        if *k == key && function != canonical {
            conflicts.push(MapConflict {
                scope: ConflictScope::Preset,
                preset: preset_name.to_owned(),
                function,
                profile: None,
                slot: None,
            });
        }
    }

    // Profiles that use this preset: the key in any OTHER slot's preset.
    // games.toml being unreadable is not a mapping error — the preset write
    // stands on its own; an unreadable profile list just cannot warn.
    let Ok(games) = store.load_games() else {
        return conflicts;
    };
    let mut cache: BTreeMap<String, Vec<(Key, String)>> = BTreeMap::new();
    let mut seen: Vec<(String, String)> = Vec::new(); // dedupe (preset, function)
    for game in &games.value.games {
        if !game.slots.iter().any(|s| s.preset == preset_name) {
            continue;
        }
        for slot in &game.slots {
            if slot.preset == preset_name {
                continue; // same-preset scope already covers it
            }
            let bound = cache.entry(slot.preset.clone()).or_insert_with(|| {
                store
                    .load_preset(&slot.preset)
                    .ok()
                    .flatten()
                    .and_then(|loaded| loaded.value.to_core().ok())
                    .map(|core| {
                        core.entries
                            .iter()
                            .map(|(k, b)| (*k, ksx_config::function_name(b)))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            for (k, function) in bound.iter() {
                if *k != key {
                    continue;
                }
                let dedupe = (slot.preset.clone(), function.clone());
                if seen.contains(&dedupe) {
                    continue;
                }
                seen.push(dedupe);
                conflicts.push(MapConflict {
                    scope: ConflictScope::Profile,
                    preset: slot.preset.clone(),
                    function: function.clone(),
                    profile: Some(game.title.clone()),
                    slot: Some(slot.number),
                });
            }
        }
    }
    conflicts
}

/// The conflicts as pipe/Studio JSON rows — one shape everywhere.
pub fn conflicts_json(conflicts: &[MapConflict]) -> serde_json::Value {
    serde_json::Value::Array(
        conflicts
            .iter()
            .map(|c| {
                serde_json::json!({
                    "scope": match c.scope {
                        ConflictScope::Preset => "preset",
                        ConflictScope::Profile => "profile",
                    },
                    "preset": c.preset,
                    "function": c.function,
                    "profile": c.profile,
                    "slot": c.slot,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::{ConfigRoot, GamesFile};

    struct TempRoot(std::path::PathBuf);
    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-mapping-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> Store {
            Store::new(ConfigRoot::at(&self.0))
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn preset(store: &Store, name: &str, toml: &str) {
        let file: PresetFile =
            toml::from_str(&format!("name = \"{name}\"\n[bindings]\n{toml}")).unwrap();
        store.save_preset(&file).unwrap();
    }

    fn games(store: &Store, toml: &str) {
        let file: GamesFile = toml::from_str(toml).unwrap();
        store.save_games(&file).unwrap();
    }

    fn spec(preset: &str, function: &str, key: Option<&str>, force: bool) -> MapSpec {
        MapSpec {
            preset: preset.into(),
            function: function.into(),
            key: key.map(str::to_owned),
            force,
        }
    }

    #[test]
    fn a_clean_bind_writes_canonical_toml() {
        let root = TempRoot::new("clean");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");

        let applied = apply(&store, &spec("P1", "a", Some("g"), false)).unwrap();
        assert_eq!(applied.function, "A", "function name canonicalized");
        assert_eq!(applied.key.as_deref(), Some("G"), "key name canonicalized");
        assert!(applied.stolen_from.is_empty());
        assert_eq!(applied.message(), "\"P1\": A = G");

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(
            !on_disk.contains("\"S\""),
            "replace-per-function: {on_disk}"
        );
    }

    #[test]
    fn replace_per_function_collapses_multi_key_bindings() {
        let root = TempRoot::new("multi");
        let store = root.store();
        preset(&store, "P1", "A = [\"S\", \"Enter\"]\n");
        let applied = apply(&store, &spec("P1", "A", Some("G"), false)).unwrap();
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(!on_disk.contains("Enter"), "{on_disk}");
    }

    #[test]
    fn clear_leaves_the_inert_placeholder() {
        let root = TempRoot::new("clear");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");
        let applied = apply(&store, &spec("P1", "A", None, false)).unwrap();
        assert_eq!(applied.key, None);
        assert_eq!(applied.message(), "\"P1\": A cleared");
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"None\""), "{on_disk}");
        assert!(
            on_disk.contains("B = \"D\""),
            "untouched sibling: {on_disk}"
        );
    }

    #[test]
    fn dotted_functions_write_the_flat_quoted_form() {
        let root = TempRoot::new("dotted");
        let store = root.store();
        preset(&store, "P1", "dpad.up = \"I\"\n");
        let applied = apply(&store, &spec("P1", "DPAD.UP", Some("W"), false)).unwrap();
        assert_eq!(applied.function, "dpad.up");
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("\"dpad.up\" = \"W\""), "{on_disk}");
        // And the rewrite still parses back through the store.
        assert_eq!(store.load_preset("P1").unwrap().unwrap().value.name, "P1");
    }

    #[test]
    fn unknown_names_are_refused_before_any_write() {
        let root = TempRoot::new("unknown");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        assert!(matches!(
            apply(&store, &spec("Nope", "A", Some("G"), false)),
            Err(MapError::UnknownPreset { .. })
        ));
        assert!(matches!(
            apply(&store, &spec("P1", "warp", Some("G"), false)),
            Err(MapError::UnknownFunction(_))
        ));
        assert!(matches!(
            apply(&store, &spec("P1", "A", Some("NotAKey"), false)),
            Err(MapError::UnknownKey(_))
        ));
        let after = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert_eq!(before, after, "no refusal may leave a changed file");
    }

    #[test]
    fn unknown_preset_lists_what_exists() {
        let root = TempRoot::new("known-list");
        let store = root.store();
        preset(&store, "IPAC P1", "A = \"S\"\n");
        let err = apply(&store, &spec("IPAC P9", "A", Some("G"), false)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("IPAC P9"), "{text}");
        assert!(text.contains("IPAC P1"), "{text}");
    }

    #[test]
    fn a_same_preset_conflict_blocks_without_force_and_steals_with_it() {
        let root = TempRoot::new("steal");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\n");

        // Without force: refused, file untouched.
        let err = apply(&store, &spec("P1", "A", Some("G"), false)).unwrap_err();
        let MapError::Conflicts { key, conflicts } = &err else {
            panic!("expected conflicts, got {err:?}");
        };
        assert_eq!(key, "G");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].scope, ConflictScope::Preset);
        assert_eq!(conflicts[0].function, "B");
        assert!(err.to_string().contains("--force"), "{err}");

        // With force: stolen, and the victim keeps a "None" placeholder.
        let applied = apply(&store, &spec("P1", "A", Some("G"), true)).unwrap();
        assert_eq!(applied.stolen_from, vec!["B".to_owned()]);
        assert!(
            applied.message().contains("taken from B"),
            "{}",
            applied.message()
        );
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(on_disk.contains("B = \"None\""), "{on_disk}");
    }

    #[test]
    fn a_cross_profile_conflict_reports_but_never_edits_the_other_preset() {
        let root = TempRoot::new("profile");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        preset(&store, "P2", "A = \"G\"\n");
        games(
            &store,
            r#"
[[game]]
title = "Steam"
path = "C:\\steam.exe"
[[game.slot]]
number = 1
preset = "P1"
[[game.slot]]
number = 2
preset = "P2"
"#,
        );

        let err = apply(&store, &spec("P1", "B", Some("G"), false)).unwrap_err();
        let MapError::Conflicts { conflicts, .. } = &err else {
            panic!("expected conflicts, got {err:?}");
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].scope, ConflictScope::Profile);
        assert_eq!(conflicts[0].preset, "P2");
        assert_eq!(conflicts[0].function, "A");
        assert_eq!(conflicts[0].profile.as_deref(), Some("Steam"));
        assert_eq!(conflicts[0].slot, Some(2));
        assert!(err.to_string().contains("\"P2\"'s A"), "{err}");

        // Force writes the target, reports the override, leaves P2 alone.
        let p2_before = std::fs::read_to_string(store.preset_path("P2").unwrap()).unwrap();
        let applied = apply(&store, &spec("P1", "B", Some("G"), true)).unwrap();
        assert_eq!(applied.overridden.len(), 1);
        assert!(
            applied.message().contains("still G is \"P2\"'s A"),
            "{}",
            applied.message()
        );
        let p2_after = std::fs::read_to_string(store.preset_path("P2").unwrap()).unwrap();
        assert_eq!(p2_before, p2_after, "other presets are never edited");
    }

    #[test]
    fn profiles_not_using_the_target_preset_are_not_conflict_scope() {
        let root = TempRoot::new("scope");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        preset(&store, "Other", "A = \"G\"\n");
        games(
            &store,
            r#"
[[game]]
title = "Solo"
path = "C:\\solo.exe"
[[game.slot]]
number = 1
preset = "Other"
"#,
        );
        // "Solo" does not use P1, so Other's G is not in scope.
        assert!(apply(&store, &spec("P1", "B", Some("G"), false)).is_ok());
    }

    #[test]
    fn keys_resolve_case_insensitively_when_unique() {
        assert_eq!(resolve_key("g").unwrap(), Key::G);
        assert_eq!(resolve_key("enter").unwrap(), Key::Enter);
        assert_eq!(resolve_key("Left").unwrap(), Key::Left);
        assert!(resolve_key("not a key").is_err());
    }

    #[test]
    fn conflicts_serialize_to_the_documented_rows() {
        let rows = conflicts_json(&[MapConflict {
            scope: ConflictScope::Profile,
            preset: "P2".into(),
            function: "A".into(),
            profile: Some("Steam".into()),
            slot: Some(2),
        }]);
        assert_eq!(
            rows,
            serde_json::json!([{
                "scope": "profile", "preset": "P2", "function": "A",
                "profile": "Steam", "slot": 2
            }])
        );
    }
}
