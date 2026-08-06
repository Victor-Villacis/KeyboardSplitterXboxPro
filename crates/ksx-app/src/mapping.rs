//! The one mapping writer: `ksx map`, the daemon's pipe `map` verb and (through
//! it) Studio's mapper all call [`apply`] — no surface gets a private editing
//! path (docs/CONTROL-SURFACE.md's standing rule).
//!
//! Semantics:
//!
//! - **Replace per function.** `map --function A --key G` makes G the ONLY key
//!   bound to A. It replaces the KEYS OF ONE FUNCTION and nothing else: the
//!   same key on OTHER functions is left exactly where it is (see multi-bind,
//!   below).
//! - **A function may be given a KEY LIST** — many keys → one control, the
//!   OR-chain the engine has always executed (`A = ["S", "Enter"]`,
//!   docs/INPUT-TRANSFORMS.md §1a). `--key S --key Enter` (or `--key S,Enter`,
//!   or the pipe's `"keys": ["S","Enter"]`) writes the WHOLE list in ONE
//!   write, so the mapper's "add another key" / per-key ✕ are atomic instead
//!   of read-modify-write. The caller's ORDER is kept and DUPLICATES are
//!   dropped (first occurrence wins; `s` and `S` are the same key, because the
//!   list is deduped after each name is resolved). A one-key list is exactly
//!   the old single-key write, byte for byte. `"key"` and `"keys"` are two
//!   spellings of the same field, so giving both is refused rather than
//!   guessed.
//! - **One key may drive many functions — that is a MULTI-BIND, not a
//!   conflict.** The engine has no uniqueness constraint in either direction:
//!   "many keys → one function and one key → many functions are both native"
//!   (`ksx-core/src/preset.rs`, docs/INPUT-TRANSFORMS.md §1a) — one key
//!   compiles to a `SmallVec` of targets and they all fire together. So
//!   writing G to A when G is already this preset's B leaves B alone, writes
//!   A, and REPORTS the co-binding (`also_drives: ["B"]`) as information.
//!   That is what makes the mapper's "Map all to one key" work: N ordinary
//!   writes of one key, and all N stick (MAPPER-UX commandment 7 — duplicates
//!   are information, fan-out is the product).
//! - **`--clear`** leaves the function in the file bound to the inert `"None"`
//!   placeholder — the same convention as the built-in empty preset, so a
//!   cleared control stays visible instead of silently vanishing. Guarded rows
//!   (chords) are replaced/cleared by the same rule, so a cleared control is
//!   really cleared and never keeps a chord the engine still obeys.
//! - **Chords** (`when`/`unless`, docs/INPUT-TRANSFORMS.md §1b) write a
//!   GUARDED binding: "this function, but only while these other keys are (or
//!   are not) also held". Two deliberate differences from a plain write:
//!   a chord **never conflicts** (layering it over keys that already do
//!   something is the point, and stealing from them would destroy what the
//!   chord sits on), and it reports a **flash advisory** instead — one entry
//!   per constituent that is also individually bound, because ksx does not
//!   defer input and the game will see that binding for a moment before the
//!   chord completes. Guards that cannot mean anything (the trigger in its own
//!   guard, a key required and forbidden at once, a guard with no key) are
//!   refused before any write.
//! - **`--move-from FUNCTION` is the only way this verb unbinds anything it
//!   was not asked to bind**, and it names its victim out loud: it takes THIS
//!   key away from THAT one function (which keeps a `"None"` placeholder if
//!   that emptied it) and touches nothing else. Never implicit, never a side
//!   effect of `force`. The response says exactly what it unbound
//!   (`moved_from`).
//! - **The one remaining conflict is CROSS-SLOT, and it blocks** (the PadForge
//!   gap this closes — docs/research/padforge-code-audit.md §1.2 "Conflict
//!   handling: none"): the key is also bound in ANOTHER slot's preset inside a
//!   games.toml profile that uses the target preset. `force` is the caller
//!   saying "yes, I mean both slots to see that key" — it writes the target,
//!   keeps reporting the double binding, and **never edits the other preset**,
//!   because silently rewriting a file the caller did not name is worse than a
//!   double binding the response spells out. `force` therefore removes no
//!   binding, anywhere, ever: it is an acknowledgement, not a hammer. (The
//!   genuinely destructive verbs are their own: [`clear_all`] and [`restore`],
//!   each of which takes a timestamped backup first.)
//! - **Writes are canonical.** The store serializes `PresetFile` afresh:
//!   bindings come back sorted with flat quoted dotted keys (`"dpad.up"`), and
//!   hand-written comments do not survive. That is the documented trade for
//!   atomic, validated writes (store.rs); the file remains hand-editable.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ksx_config::{parse_function, BindingEntry, ConfigError, MacroFile, PresetFile, Store};
use ksx_core::Key;

/// One requested edit.
#[derive(Clone, Debug, Default)]
pub struct MapSpec {
    /// Preset name (the `name` field, e.g. `"IPAC P1"`), not a file name.
    pub preset: String,
    /// Function name, any case (`A`, `dpad.up`, `lx.min`, `lx.-16384`).
    pub function: String,
    /// `Some(key name)` binds ONE key, `None` clears — the single-key
    /// spelling, and what every pre-list caller sends. Mutually exclusive with
    /// [`MapSpec::keys`]: both given is [`MapError::KeyAndKeys`].
    pub key: Option<String>,
    /// MANY KEYS → ONE CONTROL: the whole list this function should hold,
    /// in the caller's order (duplicates dropped, first occurrence wins).
    /// Empty means "not given" — the write then follows [`MapSpec::key`], and
    /// neither one is a clear.
    pub keys: Vec<String>,
    /// Write anyway when the key is already bound in ANOTHER SLOT's preset
    /// (the only conflict left). It removes nothing, anywhere: see module
    /// docs. Same-preset duplicates never need it — they are multi-binds.
    pub force: bool,
    /// Take this key away from exactly ONE other function of the same preset
    /// (`--move-from B`). The old, explicit "move the key" behaviour; the
    /// named function keeps a `"None"` placeholder if it is left with nothing.
    pub move_from: Option<String>,
    /// CHORD: extra keys that must ALL be held for this binding to apply.
    /// Empty means an ordinary unguarded binding — exactly as before.
    pub when: Vec<String>,
    /// CHORD: keys that must NOT be held (MAME's `NOT`).
    pub unless: Vec<String>,
}

/// One conflicting binding — always CROSS-SLOT.
///
/// There is deliberately no "same preset" variant any more: a key on another
/// function of the SAME preset is a multi-bind, reported as
/// [`AppliedMap::also_drives`], not as a conflict. The wire keeps emitting
/// `"scope": "profile"` (see [`conflicts_json`]) so existing readers that
/// switch on it are unaffected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapConflict {
    /// The key that conflicts. Carried per row because ONE write can now
    /// place several keys, and "still G is …" must name the key it is about.
    pub key: String,
    /// The OTHER slot's preset, which is never edited.
    pub preset: String,
    /// Canonical function name the key is bound to there.
    pub function: String,
    /// Profile title.
    pub profile: Option<String>,
    /// Slot number inside that profile.
    pub slot: Option<u8>,
}

impl MapConflict {
    /// One human line, e.g. `G is "IPAC P2"'s A (slot 2 of "Steam")`.
    pub fn describe(&self, key: &str) -> String {
        format!(
            "{key} is \"{}\"'s {}{}",
            self.preset,
            self.function,
            match (&self.profile, self.slot) {
                (Some(profile), Some(slot)) => format!(" (slot {slot} of \"{profile}\")"),
                (Some(profile), None) => format!(" (\"{profile}\")"),
                _ => String::new(),
            }
        )
    }

    /// The same line, about this row's OWN key — what a multi-key write has to
    /// print, since its conflicts can come from different keys.
    pub fn line(&self) -> String {
        self.describe(&self.key)
    }
}

/// What `--move-from` unbound: the function the key was taken from, and the
/// keys that function has LEFT. `remaining` empty = it now holds the inert
/// `"None"` placeholder, exactly as `--clear` leaves one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedFrom {
    pub function: String,
    pub remaining: Vec<String>,
}

/// What a successful [`apply`] did.
#[derive(Clone, Debug)]
pub struct AppliedMap {
    /// The file that was written.
    pub path: PathBuf,
    pub preset: String,
    /// Canonical function spelling (what the file now says).
    pub function: String,
    /// Canonical name of the FIRST key — `None` for a clear. The single-key
    /// spelling of [`AppliedMap::keys`], kept because every existing reader
    /// (and the wire's `"key"`) says exactly this for a one-key write.
    pub key: Option<String>,
    /// The function's WHOLE key list as the file now holds it, in order.
    /// Empty for a clear; one entry for an ordinary write; one entry for a
    /// chord (the trigger).
    pub keys: Vec<String>,
    /// Canonical `when` key names — empty for an unguarded binding.
    pub when: Vec<String>,
    /// Canonical `unless` key names — empty for an unguarded binding.
    pub unless: Vec<String>,
    /// The OTHER functions of this same preset that this key also drives now
    /// that the write is done — INFORMATION, not a problem (the engine fires
    /// all of them; docs/INPUT-TRANSFORMS.md §1a). Empty for a clear, for a
    /// chord, and for a key that drives this control only.
    pub also_drives: Vec<String>,
    /// What `--move-from` unbound — `None` unless it was asked for.
    pub moved_from: Option<MovedFrom>,
    /// Cross-slot conflicts that were overridden by `force` — written
    /// anyway, reported so the caller can say so. The other preset is
    /// untouched.
    pub overridden: Vec<MapConflict>,
    /// The honest caveat, per constituent: `(key, the function it is also
    /// bound to on its own)`. ksx does not defer input, so that binding shows
    /// for a moment before the chord completes.
    pub flash: Vec<(String, String)>,
}

impl AppliedMap {
    /// The chord as a human reads it: `A+B`, `A+B unless LeftShift`.
    pub fn chord(&self) -> Option<String> {
        let key = self.key.as_ref()?;
        if self.when.is_empty() && self.unless.is_empty() {
            return None;
        }
        let mut text = std::iter::once(key.as_str())
            .chain(self.when.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("+");
        if !self.unless.is_empty() {
            text.push_str(&format!(" unless {}", self.unless.join("+")));
        }
        Some(text)
    }

    /// The one-line confirmation every surface prints.
    pub fn message(&self) -> String {
        let mut line = match (&self.key, self.chord()) {
            (Some(_), Some(chord)) => format!("\"{}\": {} = {}", self.preset, self.function, chord),
            // A list reads as a list — "A = S, Enter" — and a one-key list is
            // the same sentence it always was.
            (Some(_), None) => format!(
                "\"{}\": {} = {}",
                self.preset,
                self.function,
                self.keys.join(", ")
            ),
            (None, _) => format!("\"{}\": {} cleared", self.preset, self.function),
        };
        if let Some(moved) = &self.moved_from {
            line.push_str(&match moved.remaining.as_slice() {
                [] => format!(
                    " (taken from {} — {} is now unbound)",
                    moved.function, moved.function
                ),
                keys => format!(
                    " (taken from {} — {} still has {})",
                    moved.function,
                    moved.function,
                    keys.join(", ")
                ),
            });
        }
        if self.key.is_some() {
            // Multi-bind: say what else the key drives, in the same words the
            // mapper's legend uses ("G also drives A, B"). With a key LIST the
            // subject is the list, because any of them can be the one sharing.
            if !self.also_drives.is_empty() {
                line.push_str(&format!(
                    "; {} also drives {}",
                    self.keys.join(", "),
                    self.also_drives.join(", ")
                ));
            }
            for conflict in &self.overridden {
                line.push_str(&format!("; still {}", conflict.line()));
            }
        }
        for (key, bound_to) in &self.flash {
            line.push_str(&format!(
                "; note: {key} is also {bound_to} on its own, so the game sees {bound_to} for a \
                 moment before the chord completes (ksx does not defer input)"
            ));
        }
        line
    }
}

/// Why an [`apply`] (or [`restore`]) refused or failed.
#[derive(Debug)]
pub enum MapError {
    /// No preset with that name — nothing is guessed, nothing is created.
    UnknownPreset {
        name: String,
        known: Vec<String>,
    },
    UnknownFunction(String),
    UnknownKey(String),
    /// `key` AND `keys` in the same request. They are two spellings of the
    /// same field, so one of them would have to be ignored — refused instead.
    KeyAndKeys,
    /// `--when`/`--unless` that cannot mean anything (the trigger guarding
    /// itself, a key required and forbidden at once, a guard with no key).
    InvalidGuard(String),
    /// A `--move-from` that would unbind something the caller did not mean:
    /// the function being bound, a function that does not hold this key at
    /// all, a clear, or a chord. Refused BEFORE any write — the one path that
    /// removes a binding never guesses.
    BadMoveFrom(String),
    /// Cross-slot conflicts found and `force` not given. The write did NOT
    /// happen.
    Conflicts {
        key: String,
        conflicts: Vec<MapConflict>,
    },
    /// `macro.<name>` for a macro this preset does not define. `ksx map` binds
    /// a TRIGGER; the sequence itself is authored in the file's `[macros]`
    /// table, so there is nothing sensible to create here.
    UnknownMacro {
        preset: String,
        name: String,
        known: Vec<String>,
    },
    /// `restore session-backup` with no backup on disk: nothing was mapped
    /// through the daemon this session, so there is nothing to undo.
    NoSessionBackup {
        preset: String,
    },
    /// `restore latest-backup` with no `*.toml.bak-*` file on disk: nothing has
    /// ever been restored for this preset, so no timestamped backup exists.
    NoBackup {
        preset: String,
    },
    /// A backup file exists but does not parse/validate — restoring it would
    /// trade a good file for a bad one, so it is refused. `source` names which
    /// backup ("the session-start backup", a file name).
    BadBackup {
        preset: String,
        source: String,
        reason: String,
    },
    /// A macro BODY that cannot be written, in the words validation already
    /// uses for it: a step holding something that is not a pad function, a
    /// table with no steps at all, a step with two duration units or none, a
    /// macro with no name. Every one of them is refused BEFORE the backup is
    /// taken and before a byte is written — see [`save_macro`].
    BadMacro {
        preset: String,
        name: String,
        problems: Vec<String>,
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
            MapError::KeyAndKeys => write!(
                f,
                "a key and a key LIST were both given — \"keys\" already holds every key this \
                 control should fire on (\"key\" is the one-key spelling of it), so honouring \
                 both would mean ignoring one. Nothing was written"
            ),
            MapError::InvalidGuard(reason) => write!(f, "refusing to write that chord: {reason}"),
            MapError::BadMoveFrom(reason) => write!(f, "refusing that --move-from: {reason}"),
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
                    " — that is another SLOT's preset; use --force to bind here \
                     anyway (both slots then see {key}; other presets are never \
                     edited)"
                )
            }
            MapError::UnknownMacro {
                preset,
                name,
                known,
            } => {
                write!(
                    f,
                    "preset \"{preset}\" defines no macro called \"{name}\" — `ksx map` binds the \
                     key that STARTS a macro; write the sequence itself as a [macros.{name}] \
                     table in the preset file (docs/INPUT-TRANSFORMS.md §1c)"
                )?;
                if !known.is_empty() {
                    write!(f, ". Macros in this preset: {}", known.join(", "))?;
                }
                Ok(())
            }
            MapError::NoSessionBackup { preset } => write!(
                f,
                "no session backup for \"{preset}\" — nothing has been mapped through the \
                 daemon this session, so there is nothing to undo"
            ),
            MapError::NoBackup { preset } => write!(
                f,
                "no timestamped backup for \"{preset}\" — one is written next to the preset \
                 (\"<preset>.toml.bak-YYYYMMDD-HHMMSS\") before every restore, so the first \
                 restore of a preset has nothing older to go back to"
            ),
            MapError::BadBackup {
                preset,
                source,
                reason,
            } => write!(
                f,
                "{source} for \"{preset}\" is unreadable ({reason}) — refusing to replace a \
                 good preset with it"
            ),
            MapError::BadMacro {
                preset,
                name,
                problems,
            } => write!(
                f,
                "refusing to write macro \"{name}\" of preset \"{preset}\": {} — nothing was \
                 written and no backup was taken",
                problems.join("; ")
            ),
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
    // A `macro.<name>` row is not a pad function at all — it starts a timed
    // sequence the preset defines (docs/INPUT-TRANSFORMS.md §1c). Binding and
    // clearing the TRIGGER is what `ksx map` does; authoring the steps stays
    // TOML-only for now (the `[macros]` table is the editing surface).
    if let Some(name) = ksx_config::macro_name(&spec.function) {
        return apply_macro_trigger(store, spec, name);
    }
    // Validate the function and key names FIRST — a typo must not depend on
    // which preset it was aimed at.
    let binding = parse_function(&spec.function)
        .map_err(|_| MapError::UnknownFunction(spec.function.clone()))?;
    let canonical = ksx_config::function_name(&binding);
    // The WHOLE list this function will hold: caller order, duplicates gone,
    // empty = a clear. One key here is the single-key write, unchanged.
    let keys = resolve_key_list(spec)?;
    let key = keys.first().copied();
    let when = resolve_keys(&spec.when)?;
    let unless = resolve_keys(&spec.unless)?;
    let guarded = !when.is_empty() || !unless.is_empty();
    if guarded {
        let Some(trigger) = key else {
            return Err(MapError::InvalidGuard(
                "--when/--unless need a --key to guard (a cleared function has nothing to guard)"
                    .to_owned(),
            ));
        };
        // A chord has ONE trigger — "either of these two keys while B is held"
        // is two chords, and writing it as one would have to pick a key to
        // drop or a message that lies about which key fires.
        if keys.len() > 1 {
            return Err(MapError::InvalidGuard(format!(
                "a chord is ONE trigger key plus its guard, and {} keys were given ({}) — write \
                 one chord per trigger",
                keys.len(),
                keys.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
            )));
        }
        check_guard(trigger, &when, &unless)?;
    }
    // The only path that unbinds a function the caller did not name in
    // `--function` is validated here, before a file is even read.
    let move_from = resolve_move_from(spec, &canonical, &keys, guarded)?;

    // The preset must exist; `ksx map` creates bindings, never presets.
    let file = load_preset_by_name(store, &spec.preset)?;
    let core = file.to_core()?;
    let mut entries = core.entries;
    let mut chords = core.chords;

    let mut moved_from = None;
    let mut also_drives = Vec::new();
    let mut overridden = Vec::new();
    // A chord deliberately does NOT run the conflict machinery: its whole
    // point is to reuse keys that already do something, so "G is already this
    // preset's A" is the normal case, not a refusal — and stealing G from A
    // would destroy the binding the chord is layered over. What the caller
    // gets instead is the flash advisory below, which names the cost.
    let flash = if guarded {
        let key = key.expect("a guard requires a key");
        std::iter::once(key)
            .chain(when.iter().copied())
            .filter_map(|k| {
                entries
                    .iter()
                    .find(|(bound, _)| *bound == k)
                    .map(|(_, b)| (k.name().to_owned(), ksx_config::function_name(b)))
            })
            .collect()
    } else {
        Vec::new()
    };
    if !guarded && !keys.is_empty() {
        // Same-preset duplicates are NOT checked here on purpose: one key
        // driving several functions is a multi-bind the engine executes as
        // written, so it is reported (`also_drives`, below) and never refused.
        // The only conflict left is the one that crosses into a preset this
        // writer refuses to edit — and with a key LIST every key faces it, in
        // the order given, so the refusal names the first key that hits one
        // and nothing is written for any of them.
        for key in &keys {
            let conflicts = find_profile_conflicts(store, &spec.preset, *key);
            if !conflicts.is_empty() && !spec.force {
                return Err(MapError::Conflicts {
                    key: key.name().to_owned(),
                    conflicts,
                });
            }
            overridden.extend(conflicts);
        }

        // `--move-from B`: take THIS key off B, and nothing else off anybody.
        // (Validated to a single key above — a list has no "this key".)
        let key = keys[0];
        if let Some(victim) = &move_from {
            let Some(victim_binding) = entries
                .iter()
                .find(|(k, b)| *k == key && ksx_config::function_name(b) == *victim)
                .map(|(_, b)| *b)
            else {
                return Err(MapError::BadMoveFrom(not_holding(
                    &entries,
                    victim,
                    key.name(),
                )));
            };
            entries.retain(|(k, b)| !(*k == key && ksx_config::function_name(b) == *victim));
            let remaining: Vec<String> = entries
                .iter()
                .filter(|(k, b)| ksx_config::function_name(b) == *victim && *k != Key::None)
                .map(|(k, _)| k.name().to_owned())
                .collect();
            // Emptied? Keep the control visible as the inert placeholder —
            // the same convention `--clear` uses, so nothing silently vanishes
            // from the file or the mapper's legend.
            if !entries
                .iter()
                .any(|(_, b)| ksx_config::function_name(b) == *victim)
            {
                entries.push((Key::None, victim_binding));
            }
            moved_from = Some(MovedFrom {
                function: victim.clone(),
                remaining,
            });
        }
    }

    // Replace-per-function: out with every old key for this function —
    // guarded rows included, so `--clear` and a re-map both wipe a chord
    // instead of leaving a ghost the file still obeys...
    entries.retain(|(_, b)| ksx_config::function_name(b) != canonical);
    chords.retain(|c| ksx_config::function_name(&c.binding) != canonical);
    // ...in with the new one (or the inert placeholder for a clear).
    if guarded {
        chords.push(ksx_core::Chord {
            key: key.expect("a guard requires a key"),
            binding,
            when: when.clone(),
            unless: unless.clone(),
        });
    } else if keys.is_empty() {
        entries.push((Key::None, binding));
    } else {
        // The list, IN ORDER — `from_core` groups a function's keys in the
        // order they were pushed, so the file reads the way the caller wrote
        // it (`A = ["S", "Enter"]`).
        for key in &keys {
            entries.push((*key, binding));
        }
    }

    // The co-binding report, read off the file as it is ABOUT TO BE WRITTEN —
    // so it already excludes anything `--move-from` took away, and it says
    // what the preset will really do rather than what the caller assumed.
    // With a key list it is the UNION: what any of these keys also drives.
    if !guarded {
        for key in &keys {
            also_drives.extend(
                entries
                    .iter()
                    .filter(|(k, b)| k == key && ksx_config::function_name(b) != canonical)
                    .map(|(_, b)| ksx_config::function_name(b)),
            );
        }
        also_drives.sort();
        also_drives.dedup();
    }

    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries,
        chords,
        // Untouched: editing a binding never disturbs the preset's macros.
        macros: core.macros,
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedMap {
        path,
        preset: file.name,
        function: canonical,
        key: key.map(|k| k.name().to_owned()),
        keys: keys.iter().map(|k| k.name().to_owned()).collect(),
        when: when.iter().map(|k| k.name().to_owned()).collect(),
        unless: unless.iter().map(|k| k.name().to_owned()).collect(),
        also_drives,
        moved_from,
        overridden,
        flash,
    })
}

/// Bind (or clear) the key that STARTS a macro the preset already defines.
///
/// Deliberately narrow. `ksx map` owns the trigger because that is the part a
/// player rebinds on a cabinet; the steps themselves are a timeline with
/// durations and two interruption policies, and a flag-per-field CLI for that
/// would be worse than the TOML it would write. Authoring stays in
/// `[macros.<name>]`.
fn apply_macro_trigger(store: &Store, spec: &MapSpec, name: &str) -> Result<AppliedMap, MapError> {
    if !spec.when.is_empty() || !spec.unless.is_empty() {
        return Err(MapError::InvalidGuard(
            "a macro is started by a key; a chord that starts a sequence is not implemented \
             (docs/INPUT-TRANSFORMS.md §1c)"
                .to_owned(),
        ));
    }
    if spec.move_from.is_some() {
        return Err(MapError::BadMoveFrom(
            "--move-from takes a key from another pad function; a macro trigger is not one"
                .to_owned(),
        ));
    }
    // A macro takes a key LIST exactly like a button does: several triggers
    // for one sequence are ordinary `macro.<name>` rows in the file.
    let keys = resolve_key_list(spec)?;

    let file = load_preset_by_name(store, &spec.preset)?;
    let mut core = file.to_core()?;
    let Some(index) = core.macros.index_of(name) else {
        return Err(MapError::UnknownMacro {
            preset: file.name.clone(),
            name: name.to_owned(),
            known: core.macros.defs.iter().map(|m| m.name.clone()).collect(),
        });
    };
    let canonical = ksx_config::macro_function_name(&core.macros.defs[usize::from(index)].name);

    // A macro trigger is a key like any other, so the one conflict that
    // survives — the key already doing something in ANOTHER slot's preset —
    // is checked with exactly the same rule and the same `--force` escape.
    let mut overridden = Vec::new();
    for key in &keys {
        let conflicts = find_profile_conflicts(store, &spec.preset, *key);
        if !conflicts.is_empty() && !spec.force {
            return Err(MapError::Conflicts {
                key: key.name().to_owned(),
                conflicts,
            });
        }
        overridden.extend(conflicts);
    }

    // Replace-per-function, as everywhere else: this macro's old triggers go,
    // the new one(s) arrive (or nothing, for a clear).
    core.macros.triggers.retain(|t| t.index != index);
    for key in &keys {
        core.macros
            .triggers
            .push(ksx_core::MacroTrigger::new(*key, index));
    }

    // Multi-bind reads the same as anywhere: what else do these keys do now?
    let mut also_drives: Vec<String> = Vec::new();
    for key in &keys {
        also_drives.extend(
            core.entries
                .iter()
                .filter(|(k, _)| k == key)
                .map(|(_, b)| ksx_config::function_name(b)),
        );
        also_drives.extend(
            core.macros
                .triggers
                .iter()
                .filter(|t| t.key == *key && t.index != index)
                .filter_map(|t| core.macros.get(t.index))
                .map(|m| ksx_config::macro_function_name(&m.name)),
        );
    }
    if !keys.is_empty() {
        also_drives.sort();
        also_drives.dedup();
    }

    let rewritten = PresetFile::from_core(&core);
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedMap {
        path,
        preset: file.name,
        function: canonical,
        key: keys.first().map(|k| k.name().to_owned()),
        keys: keys.iter().map(|k| k.name().to_owned()).collect(),
        when: Vec::new(),
        unless: Vec::new(),
        also_drives,
        moved_from: None,
        overridden,
        flash: Vec::new(),
    })
}

/// Validate `--move-from` before anything is read or written. Everything it
/// can refuse is refused here except "that function does not hold this key",
/// which needs the file ([`not_holding`]).
fn resolve_move_from(
    spec: &MapSpec,
    canonical: &str,
    keys: &[Key],
    guarded: bool,
) -> Result<Option<String>, MapError> {
    let Some(name) = spec.move_from.as_deref() else {
        return Ok(None);
    };
    let victim = parse_function(name).map_err(|_| MapError::UnknownFunction(name.to_owned()))?;
    let victim = ksx_config::function_name(&victim);
    if victim == canonical {
        return Err(MapError::BadMoveFrom(format!(
            "{canonical} is the function being bound — a control cannot take a key from itself"
        )));
    }
    if keys.is_empty() {
        return Err(MapError::BadMoveFrom(
            "it needs a --key (it takes THAT key away from the named function, and a clear \
             takes nothing from anyone)"
                .to_owned(),
        ));
    }
    if keys.len() > 1 {
        return Err(MapError::BadMoveFrom(format!(
            "it takes ONE key away from {victim}, and {} were given ({}) — say which key moves, \
             or unbind {victim} yourself with --clear",
            keys.len(),
            keys.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
        )));
    }
    if guarded {
        return Err(MapError::BadMoveFrom(format!(
            "a chord layers over the keys it uses instead of taking them, so nothing is moved \
             off {victim}; drop --when/--unless, or unbind {victim} yourself with --clear"
        )));
    }
    Ok(Some(victim))
}

/// The refusal for `--move-from B` when B does not hold that key — it names
/// what B actually has, because the alternative is unbinding a control the
/// caller was not thinking about.
fn not_holding(entries: &[(Key, ksx_core::Binding)], victim: &str, key: &str) -> String {
    let held: Vec<String> = entries
        .iter()
        .filter(|(k, b)| ksx_config::function_name(b) == victim && *k != Key::None)
        .map(|(k, _)| k.name().to_owned())
        .collect();
    match held.as_slice() {
        [] => format!("{victim} is not bound to anything, so {key} cannot be taken from it"),
        keys => format!(
            "{victim} is not bound to {key} (it has {}) — refusing to unbind a control the \
             write was not about",
            keys.join(", ")
        ),
    }
}

/// The keys a [`MapSpec`] asks this function to hold, resolved and in the
/// caller's order. `Ok(vec![])` is a clear.
///
/// `key` and `keys` are the same field spelled two ways, so BOTH is a refusal
/// rather than a silent choice. Duplicates are dropped AFTER resolution —
/// `--key s --key S` is one key, not a file with the same row twice — and the
/// FIRST occurrence keeps its place, so the order the mapper shows is the
/// order the file holds.
fn resolve_key_list(spec: &MapSpec) -> Result<Vec<Key>, MapError> {
    if spec.key.is_some() && !spec.keys.is_empty() {
        return Err(MapError::KeyAndKeys);
    }
    let mut keys: Vec<Key> = Vec::new();
    for name in spec.key.iter().chain(spec.keys.iter()) {
        let key = resolve_key(name)?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    Ok(keys)
}

/// Resolve a list of key names the same way [`resolve_key`] does one.
fn resolve_keys(names: &[String]) -> Result<Vec<Key>, MapError> {
    names.iter().map(|n| resolve_key(n)).collect()
}

/// The guard rules `ksx map` refuses rather than writes — the same ones
/// `ksx doctor` reports for a hand-edited file.
fn check_guard(trigger: Key, when: &[Key], unless: &[Key]) -> Result<(), MapError> {
    for key in when.iter().chain(unless.iter()) {
        if *key == trigger {
            return Err(MapError::InvalidGuard(format!(
                "{} is the key being bound; the trigger is always required, so listing it in \
                 --when/--unless says nothing",
                key.name()
            )));
        }
    }
    for key in when {
        if unless.contains(key) {
            return Err(MapError::InvalidGuard(format!(
                "{} is in both --when and --unless, so the chord could never fire",
                key.name()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Restore: the mapper's THREE destinations (docs/CONTROL-SURFACE.md
// "map-restore"). All of them go through the same store writer as `apply` — no
// private editing path — and all of them take a timestamped backup first.
//
// The three are deliberately different distances back, and the UI must name
// them by their DESTINATION, never by the word "defaults" (MAPPER-UX
// commandment 5: honest labels, guaranteed road home). "Defaults" is the one
// that surprised Victor: it does NOT mean "how this preset shipped" — it means
// the LEGACY GENERIC KEYBOARD layout (S=A, D=B, A=X, W=Y, Q/E triggers, arrows
// = left stick, Esc=Start), which on an arcade cabinet replaces an I-PAC
// panel map with a desktop-keyboard map. It stays available because it is the
// always-there floor, but it is spelled out everywhere it appears.
// ---------------------------------------------------------------------------

/// Which safety net [`restore`] pulls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreKind {
    /// Rewrite the preset's bindings to `ksx_core::Preset::builtin_default()` —
    /// the LEGACY GENERIC KEYBOARD layout, not "this preset as it shipped".
    /// Keeps the preset's name.
    Defaults,
    /// Rewrite the preset from its `<file>.session-bak` — the snapshot
    /// [`take_session_backup`] made before the daemon's FIRST map write to
    /// that preset this daemon lifetime ("undo everything since the daemon
    /// started").
    SessionBackup,
    /// Rewrite the preset from the NEWEST `<file>.bak-YYYYMMDD-HHMMSS` — the
    /// snapshot taken automatically before the previous restore. This is the
    /// undo for a restore itself.
    LatestBackup,
    /// Not a restore at all: [`clear_all`]'s destination, sharing this type so
    /// every whole-preset write reports the same way (and takes the same
    /// backup). Deliberately NOT parseable from a `--restore`/`"mode"` word —
    /// "clear everything" is its own verb, never a spelling of "restore".
    ClearAll,
}

impl RestoreKind {
    /// The wire word (`ksx map --restore <mode>`, pipe `map-restore` `"mode"`,
    /// and the `"mode"` field of every whole-preset response).
    pub fn as_str(self) -> &'static str {
        match self {
            RestoreKind::Defaults => "defaults",
            RestoreKind::SessionBackup => "session-backup",
            RestoreKind::LatestBackup => "latest-backup",
            RestoreKind::ClearAll => "clear-all",
        }
    }

    /// Parse a RESTORE mode. `None` for anything else — callers report the
    /// three valid spellings rather than guessing, and `clear-all` is
    /// deliberately not one of them.
    pub fn parse(mode: &str) -> Option<Self> {
        match mode {
            "defaults" => Some(RestoreKind::Defaults),
            "session-backup" => Some(RestoreKind::SessionBackup),
            "latest-backup" => Some(RestoreKind::LatestBackup),
            _ => None,
        }
    }

    /// What this destination WRITES, in one clause — the sentence every
    /// confirm dialog and every CLI line is built from. Never the bare word
    /// "defaults".
    pub fn destination(self) -> &'static str {
        match self {
            RestoreKind::Defaults => {
                "the generic keyboard layout (S=A, D=B, A=X, W=Y, Q/E triggers, arrow keys \
                 = left stick, Esc=Start) — NOT this preset's original panel map"
            }
            RestoreKind::SessionBackup => {
                "this preset as it was before the daemon's first change this session"
            }
            RestoreKind::LatestBackup => {
                "this preset as it was before the most recent restore (the newest \
                 timestamped backup)"
            }
            RestoreKind::ClearAll => {
                "an empty preset — every control still listed, none of them bound"
            }
        }
    }
}

/// One `<preset>.toml.bak-YYYYMMDD-HHMMSS` on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetBackup {
    pub path: PathBuf,
    /// `YYYYMMDD-HHMMSS` — sortable, and the file name's suffix verbatim.
    pub stamp: String,
}

impl PresetBackup {
    /// The stamp spelled for a human: `2026-08-05 14:32:07 UTC`.
    ///
    /// UTC, like every other timestamp ksx prints (the studio pages, the
    /// snapshot lines): converting to local time needs a Win32 call that would
    /// make this module platform-specific for a cosmetic gain, and a mixed
    /// UTC/local page is worse than a consistent UTC one.
    pub fn label(&self) -> String {
        let (date, time) = match self.stamp.split_once('-') {
            Some(split) => split,
            None => return self.stamp.clone(),
        };
        if date.len() != 8 || time.len() < 6 {
            return self.stamp.clone();
        }
        format!(
            "{}-{}-{} {}:{}:{} UTC",
            &date[0..4],
            &date[4..6],
            &date[6..8],
            &time[0..2],
            &time[2..4],
            &time[4..6],
        )
    }
}

/// The suffix that marks a timestamped backup: `<preset file>.bak-<stamp>`.
const BACKUP_MARK: &str = ".bak-";

/// `YYYYMMDD-HHMMSS`, UTC — sortable as a plain string, which is what makes
/// "newest" a lexicographic max rather than a filesystem-mtime guess.
fn stamp_now() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// Every timestamped backup of `preset_name`, NEWEST FIRST.
///
/// Backups are never pruned: they are small text files, a restore is a rare
/// deliberate act, and silently deleting a user's only copy of a panel map to
/// save a few kilobytes is not a trade ksx gets to make. The mapper shows the
/// newest one; the rest sit next to the preset for anyone who needs them.
pub fn list_backups(store: &Store, preset_name: &str) -> Result<Vec<PresetBackup>, MapError> {
    let preset_path = store.preset_path(preset_name)?;
    let Some(dir) = preset_path.parent() else {
        return Ok(Vec::new());
    };
    let Some(file_name) = preset_path.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let prefix = format!("{file_name}{BACKUP_MARK}");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new()); // no presets dir yet = no backups, not an error
    };
    let mut backups: Vec<PresetBackup> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_owned();
            let stamp = name.strip_prefix(&prefix)?.to_owned();
            (!stamp.is_empty()).then_some(PresetBackup { path, stamp })
        })
        .collect();
    // Newest first. The stamp is fixed-width and zero-padded, so a plain
    // descending string sort IS chronological (and the collision suffix
    // `-2`, `-3`… sorts after the bare stamp of the same second).
    backups.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(backups)
}

/// Copy the preset file to `<file>.bak-YYYYMMDD-HHMMSS`.
///
/// Called before EVERY restore (see [`restore`]) — that is the whole point:
/// no restore can be the last word, because the thing it overwrote is on disk
/// with a timestamp on it. `Ok(None)` means there was no preset file to copy
/// (a caller that is about to fail with `UnknownPreset` anyway).
///
/// Two restores inside one second get `-2`, `-3`… appended, so a backup is
/// never silently overwritten by the restore that follows it.
pub fn take_backup(store: &Store, preset_name: &str) -> Result<Option<PresetBackup>, MapError> {
    let source = store.preset_path(preset_name)?;
    if !source.exists() {
        return Ok(None);
    }
    let base = stamp_now();
    let (path, stamp) = (1u32..)
        .map(|n| {
            let stamp = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            (
                PathBuf::from(format!("{}{BACKUP_MARK}{stamp}", source.display())),
                stamp,
            )
        })
        .find(|(path, _)| !path.exists())
        .expect("an unbounded suffix search always finds a free name");
    std::fs::copy(&source, &path).map_err(|err| MapError::BadBackup {
        preset: preset_name.to_owned(),
        source: "the pre-restore backup".to_owned(),
        reason: format!("could not write {}: {err}", path.display()),
    })?;
    Ok(Some(PresetBackup { path, stamp }))
}

/// What a successful [`restore`] did.
#[derive(Clone, Debug)]
pub struct AppliedRestore {
    pub path: PathBuf,
    pub preset: String,
    pub kind: RestoreKind,
    /// The timestamped backup taken immediately before the write — the road
    /// back from this very restore. `None` only when there was no file to copy.
    pub backup: Option<PresetBackup>,
}

impl AppliedRestore {
    /// The one-line confirmation every surface prints. It names what was
    /// WRITTEN and what was BACKED UP, in that order — a restore that does not
    /// say both is a restore somebody will be afraid of.
    pub fn message(&self) -> String {
        let wrote = match self.kind {
            RestoreKind::Defaults => format!(
                "\"{}\": bindings reset to the generic keyboard layout (S/D/A/W…)",
                self.preset
            ),
            RestoreKind::SessionBackup => format!(
                "\"{}\": bindings restored from the session-start backup",
                self.preset
            ),
            RestoreKind::LatestBackup => format!(
                "\"{}\": bindings restored from the newest timestamped backup",
                self.preset
            ),
            RestoreKind::ClearAll => format!(
                "\"{}\": every binding cleared (all controls still listed, none bound)",
                self.preset
            ),
        };
        match &self.backup {
            Some(backup) => format!(
                "{wrote} — the previous file is backed up as {}",
                backup.stamp
            ),
            None => wrote,
        }
    }
}

/// `<preset file>.session-bak`, next to the preset itself.
pub fn session_backup_path(store: &Store, preset_name: &str) -> Result<PathBuf, MapError> {
    let path = store.preset_path(preset_name)?;
    Ok(PathBuf::from(format!("{}.session-bak", path.display())))
}

/// Snapshot the preset file to `<file>.session-bak` — called by the daemon's
/// map writer before its FIRST write to that preset in this daemon lifetime
/// (the caller keeps the once-per-lifetime set; this function just copies).
/// A missing preset file is not an error here: `apply` will name it properly.
pub fn take_session_backup(store: &Store, preset_name: &str) -> Result<(), MapError> {
    let Ok(source) = store.preset_path(preset_name) else {
        return Ok(());
    };
    if !source.exists() {
        return Ok(());
    }
    let backup = PathBuf::from(format!("{}.session-bak", source.display()));
    std::fs::copy(&source, &backup).map_err(|err| MapError::BadBackup {
        preset: preset_name.to_owned(),
        source: "the session-start backup".to_owned(),
        reason: format!("could not write {}: {err}", backup.display()),
    })?;
    Ok(())
}

/// Read a backup file and validate it the way every other write is validated —
/// a hand-damaged backup must never be swapped in for a good preset.
fn bindings_from_backup(
    path: &std::path::Path,
    preset_name: &str,
    source: &str,
) -> Result<Vec<(Key, ksx_core::Binding)>, MapError> {
    let bad = |reason: String| MapError::BadBackup {
        preset: preset_name.to_owned(),
        source: source.to_owned(),
        reason,
    };
    let text = std::fs::read_to_string(path).map_err(|err| bad(err.to_string()))?;
    let parsed: PresetFile = toml::from_str(&text).map_err(|err| bad(err.to_string()))?;
    let core = parsed.to_core().map_err(|err| bad(err.to_string()))?;
    Ok(core.entries)
}

/// Restore a preset's bindings from one of the three destinations.
///
/// The order is the safety property: the replacement is resolved and validated
/// FIRST (so a refusal leaves no pointless backup lying around), then the
/// current file is copied to `<file>.bak-YYYYMMDD-HHMMSS`, and only then is the
/// preset overwritten. The write is canonical (same serializer as [`apply`]);
/// the preset must already exist — restore edits presets, it never creates them.
pub fn restore(
    store: &Store,
    preset_name: &str,
    kind: RestoreKind,
) -> Result<AppliedRestore, MapError> {
    let file = load_preset_by_name(store, preset_name)?;
    let entries = match kind {
        RestoreKind::Defaults => ksx_core::Preset::builtin_default().entries,
        RestoreKind::SessionBackup => {
            let backup = session_backup_path(store, preset_name)?;
            if !backup.exists() {
                return Err(MapError::NoSessionBackup {
                    preset: preset_name.to_owned(),
                });
            }
            bindings_from_backup(&backup, preset_name, "the session-start backup")?
        }
        RestoreKind::LatestBackup => {
            let newest = list_backups(store, preset_name)?
                .into_iter()
                .next()
                .ok_or_else(|| MapError::NoBackup {
                    preset: preset_name.to_owned(),
                })?;
            let source = format!("the backup from {}", newest.label());
            bindings_from_backup(&newest.path, preset_name, &source)?
        }
        // `clear-all` is its own verb ([`clear_all`]) precisely so it cannot be
        // reached by anything spelled "restore".
        RestoreKind::ClearAll => ksx_core::Preset::builtin_empty().entries,
    };

    // Everything below this line WILL write: take the road home first.
    let backup = take_backup(store, preset_name)?;
    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries,
        // Whole-preset writes replace the bindings wholesale, chords
        // included: "restore" and "clear all" must not leave a guard behind.
        chords: Vec::new(),
        macros: Default::default(),
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedRestore {
        path,
        preset: file.name,
        kind,
        backup,
    })
}

/// Unbind every function of a preset, keeping the file structurally valid.
///
/// This writes the `empty` built-in's SHAPE — every function present, keyed
/// `Key::None` — rather than deleting rows, which is the same convention
/// `--clear` uses for one function: a cleared control stays visible in the file
/// and in the mapper's legend instead of silently vanishing.
///
/// Like every whole-preset write it takes a timestamped backup first, so
/// "Clear all bindings" has the same one-click road home as a restore.
pub fn clear_all(store: &Store, preset_name: &str) -> Result<AppliedRestore, MapError> {
    let file = load_preset_by_name(store, preset_name)?;
    let backup = take_backup(store, preset_name)?;
    let rewritten = PresetFile::from_core(&ksx_core::Preset {
        name: file.name.clone(),
        entries: ksx_core::Preset::builtin_empty().entries,
        chords: Vec::new(),
        macros: Default::default(),
        protected: false,
    });
    let path = store.save_preset(&rewritten)?;
    Ok(AppliedRestore {
        path,
        preset: file.name,
        kind: RestoreKind::ClearAll,
        backup,
    })
}

// ---------------------------------------------------------------------------
// Macro BODIES — the whole `[macros.<name>]` table
// (docs/INPUT-TRANSFORMS.md §1c)
//
// `apply` writes the key that STARTS a macro; this writes the sequence itself.
// One more surface on the same writer, not a second editor: it loads through
// the store, validates with `ksx_config::validate`, takes the same timestamped
// backup every whole-preset write takes, and saves through `Store::save_preset`
// — so `ksx macro`, the pipe's `map-macro` verb and Studio's macro editor are
// three faces of this one function, exactly as they are for `apply`.
// ---------------------------------------------------------------------------

/// One requested macro-body edit: the WHOLE `[macros.<name>]` table.
///
/// Whole-macro, never per-step, on purpose. An editor holds the entire grid in
/// front of the user, so it can always send the whole thing; a per-step
/// protocol would have to carry indices computed against a file that may have
/// moved underneath it, and the first dropped or reordered message would leave
/// a sequence nobody authored. One table in, one table out — the same
/// replace-per-function rule [`apply`] uses for a binding.
#[derive(Clone, Debug, Default)]
pub struct MacroSpec {
    /// Preset name (the `name` field, e.g. `"IPAC P1"`), not a file name.
    pub preset: String,
    /// `[macros.<name>]`. Matched case-insensitively against the tables on
    /// disk, like every other name in ksx; a table that already exists KEEPS
    /// its own spelling, so replacing a macro never silently renames it.
    pub name: String,
    /// The body in the FILE's own shape ([`ksx_config::MacroFile`]) — the same
    /// type the preset parser reads and `ksx config export` emits, so there is
    /// no second macro schema anywhere in ksx. Durations stay in the unit they
    /// were authored in (`ms` or `frames`, never both).
    pub body: MacroFile,
    /// DELETE the table instead of writing it, together with the
    /// `macro.<name>` trigger rows that would otherwise dangle (a trigger for
    /// a macro that no longer exists does not load at all).
    ///
    /// Deletion is an explicit FLAG and NOT "a write with no steps". An empty
    /// `steps` list is a fault validation already names ("triggering it does
    /// nothing"), so it is refused here — which means an editor that lost its
    /// grid, or a caller whose `steps` field was misspelled, gets a refusal
    /// instead of quietly deleting the user's macro.
    pub delete: bool,
}

/// What a successful [`save_macro`] did.
#[derive(Clone, Debug)]
pub struct AppliedMacro {
    pub path: PathBuf,
    pub preset: String,
    /// The table's name as the file now spells it.
    pub name: String,
    /// Steps written (0 for a delete).
    pub steps: usize,
    /// Run length at the durations the engine will use (the sampling floor
    /// applied) — 0 for a delete.
    pub total_ms: u64,
    pub deleted: bool,
    /// The keys that START this macro. Untouched by this verb either way:
    /// on a write they are what the trigger rows already say (`ksx map
    /// --function macro.<name>` is what changes them), and on a delete they
    /// are the rows that had to go with the table.
    pub triggers: Vec<String>,
    /// The timestamped backup taken immediately before the write — the road
    /// back from this very edit. `None` only when there was no file to copy.
    pub backup: Option<PresetBackup>,
    /// Validation ADVISORIES, passed through rather than swallowed: a step
    /// shorter than the sampling floor is raised (or, with `allow_short`, run
    /// as written and possibly missed). Neither outcome is ever silent.
    pub warnings: Vec<String>,
}

impl AppliedMacro {
    /// The one-line confirmation every surface prints — what was written, what
    /// starts it, what was backed up, and every advisory, in that order.
    pub fn message(&self) -> String {
        let mut line = if self.deleted {
            let mut line = format!("\"{}\": macro \"{}\" deleted", self.preset, self.name);
            if !self.triggers.is_empty() {
                line.push_str(&format!(
                    " — its trigger row(s) went with it ({})",
                    self.triggers.join(", ")
                ));
            }
            line
        } else {
            let mut line = format!(
                "\"{}\": macro \"{}\" = {} step(s) · {} ms",
                self.preset, self.name, self.steps, self.total_ms
            );
            line.push_str(&match self.triggers.as_slice() {
                [] => {
                    " — no trigger key yet (bind one with `ksx map --function macro.".to_owned()
                        + &self.name
                        + " --key <KEY>`)"
                }
                keys => format!(" — started by {}", keys.join(", ")),
            });
            line
        };
        if let Some(backup) = &self.backup {
            line.push_str(&format!(
                " — the previous file is backed up as {}",
                backup.stamp
            ));
        }
        for warning in &self.warnings {
            line.push_str(&format!("; note: {warning}"));
        }
        line
    }
}

/// Write (or delete) one preset's whole `[macros.<name>]` table.
///
/// The order is the safety property, and it is the same one [`restore`] uses:
///
/// 1. the preset is loaded (this verb edits presets, it never creates them);
/// 2. the BODY is validated on its own through `ksx_config::validate` — the
///    very rules `ksx doctor` reports for a hand-edited file, so a step holding
///    `warp`, a table with no steps, or a step with two duration units is
///    refused in the same words. Advisories (a step below the sampling floor)
///    are collected, not refused;
/// 3. the edited file is validated as a WHOLE, and any NEW fault the edit
///    introduced elsewhere refuses it too — nothing this verb writes may leave
///    a preset that will not load;
/// 4. only then is the current file copied to
///    `<preset>.toml.bak-YYYYMMDD-HHMMSS` and overwritten. A refusal therefore
///    leaves no pointless backup, and a success always has one.
///
/// Bindings, chords and every OTHER macro of the preset are carried through
/// untouched: this is a whole-MACRO write, not a whole-preset one.
pub fn save_macro(store: &Store, spec: &MacroSpec) -> Result<AppliedMacro, MapError> {
    let name = spec.name.trim();
    let file = load_preset_by_name(store, &spec.preset)?;
    let refuse = |problems: Vec<String>| MapError::BadMacro {
        preset: file.name.clone(),
        name: name.to_owned(),
        problems,
    };
    if name.is_empty() {
        return Err(refuse(vec![
            "a macro needs a name — it is half of the `macro.<name>` function that starts it"
                .to_owned(),
        ]));
    }

    // The table this edit is about, in the FILE's own spelling when it is
    // already there (names match case-insensitively, so replacing "Hadouken"
    // with "hadouken" must not leave two tables behind).
    let existing = file
        .macros
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned();
    // Read before the edit: on a delete these are the rows that go with the
    // table, and on a write they are what the response reports as "started by".
    let triggers = macro_trigger_keys(&file, name);

    let mut next = file.clone();
    let mut warnings = Vec::new();
    if spec.delete {
        let Some(key) = existing else {
            return Err(MapError::UnknownMacro {
                preset: file.name.clone(),
                name: name.to_owned(),
                known: file.macros.keys().cloned().collect(),
            });
        };
        next.macros.remove(&key);
        // A `macro.<name>` row whose table is gone does not load at all
        // (`ConfigError::UnknownMacro`), so the rows leave with it. That is
        // the one binding this verb removes, it is never a surprise — deleting
        // the sequence a key starts is the whole request — and the response
        // names every key it took.
        remove_macro_triggers(&mut next.bindings, &key);
    } else {
        // Step 2: the body on its own. Validated as a preset that contains
        // NOTHING but this macro, so a fault the file already had somewhere
        // else can neither mask this write's problems nor block it.
        let (problems, advisories) = macro_body_issues(&file.name, name, &spec.body);
        if !problems.is_empty() {
            return Err(refuse(problems));
        }
        warnings = advisories;
        next.macros.insert(
            existing.unwrap_or_else(|| name.to_owned()),
            spec.body.clone(),
        );
    }

    // Step 3: nothing this verb writes may leave a preset that will not load.
    // Compared against the file as it was, so a fault that was ALREADY there
    // is not this edit's to refuse (and not this edit's to fix).
    let before: std::collections::BTreeSet<String> = validate_preset_file(&file);
    let broke: Vec<String> = validate_preset_file(&next)
        .into_iter()
        .filter(|issue| !before.contains(issue))
        .filter(|issue| !warnings.contains(issue))
        .collect();
    if !broke.is_empty() {
        return Err(refuse(broke));
    }

    // Everything below this line WILL write: take the road home first.
    let backup = take_backup(store, &file.name)?;
    let path = store.save_preset(&next)?;
    // The table as the file now holds it — which is the file's spelling of the
    // name, not necessarily the caller's, and nothing at all after a delete.
    let written = next
        .macros
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name));
    Ok(AppliedMacro {
        path,
        preset: file.name,
        name: written.map_or_else(|| name.to_owned(), |(key, _)| key.clone()),
        steps: written.map_or(0, |(_, body)| body.steps.len()),
        total_ms: written.map_or(0, |(_, body)| body_total_ms(body)),
        deleted: spec.delete,
        triggers,
        backup,
        warnings,
    })
}

/// One macro body's own issues, split into refusals and advisories.
///
/// Validated as a preset containing NOTHING but this table, which is what
/// keeps the answer about the body: `ksx_config::validate` is the only rule
/// set (no parallel checker), and an unrelated fault elsewhere in the file can
/// neither hide a problem here nor block a write that is fine.
fn macro_body_issues(
    preset_name: &str,
    name: &str,
    body: &MacroFile,
) -> (Vec<String>, Vec<String>) {
    let solo = PresetFile {
        name: preset_name.to_owned(),
        bindings: Default::default(),
        macros: std::iter::once((name.to_owned(), body.clone())).collect(),
    };
    let mut problems = Vec::new();
    let mut advisories = Vec::new();
    for issue in ksx_config::validate(
        &ksx_config::ConfigFile::default(),
        std::slice::from_ref(&solo),
    ) {
        if issue.is_advisory() {
            advisories.push(issue.to_string());
        } else {
            problems.push(issue.to_string());
        }
    }
    if body.steps.is_empty() {
        // Validation already says "triggering it does nothing"; this adds the
        // one thing it cannot know — that the caller may have meant `delete`.
        problems.push(format!(
            "an empty step list is not how a macro is removed — pass the delete flag \
             (`ksx macro --preset … --name {name} --delete`) if that is what you meant"
        ));
    }
    (problems, advisories)
}

/// Every issue in one preset file, as text — the set the whole-file check
/// compares before and after.
fn validate_preset_file(file: &PresetFile) -> std::collections::BTreeSet<String> {
    ksx_config::validate(
        &ksx_config::ConfigFile::default(),
        std::slice::from_ref(file),
    )
    .into_iter()
    .filter(|issue| !issue.is_advisory())
    .map(|issue| issue.to_string())
    .collect()
}

/// A macro body's run length in milliseconds, at the durations the engine will
/// actually use — the sampling floor (`ksx_core::MIN_STEP_MS`) applied exactly
/// as `MacroStep::effective_ms` applies it.
fn body_total_ms(body: &MacroFile) -> u64 {
    body.steps
        .iter()
        .map(|step| {
            // A step with no readable duration contributes nothing here; it is
            // a refusal, and this number is only ever computed for a body that
            // passed validation.
            let Ok(duration) = step.duration() else {
                return 0;
            };
            let ms = duration.ms();
            if step.allow_short || ms >= ksx_core::MIN_STEP_MS {
                u64::from(ms)
            } else {
                u64::from(ksx_core::MIN_STEP_MS)
            }
        })
        .sum()
}

/// The keys that START `name` in this preset file — the `macro.<name>` rows of
/// `[bindings]`, in file order, with the inert `"None"` placeholder dropped.
///
/// Reads the FILE rather than the core model on purpose: a preset with an
/// unknown key or function somewhere else still has readable macro triggers,
/// and the macro editor's whole job is to show what the file says.
pub fn macro_trigger_keys(file: &PresetFile, name: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for (function, entry) in &file.bindings {
        collect_macro_keys(function, entry, name, &mut keys);
    }
    keys
}

fn collect_macro_keys(function: &str, entry: &BindingEntry, name: &str, out: &mut Vec<String>) {
    let mut push = |key: &String| {
        if is_trigger_row(function, name) && key != "None" {
            out.push(key.clone());
        }
    };
    match entry {
        BindingEntry::Key(key) => push(key),
        BindingEntry::Keys(keys) => keys.iter().for_each(push),
        BindingEntry::Guarded(guarded) => push(&guarded.key),
        BindingEntry::Many(entries) => {
            for entry in entries {
                collect_macro_keys(function, entry, name, out);
            }
        }
        // TOML dotted keys: `macro.hadouken = "P"` parses as a `macro` table
        // holding `hadouken`, which is the same row spelled differently.
        BindingEntry::Group(group) => {
            for (sub, entry) in group {
                collect_macro_keys(&format!("{function}.{sub}"), entry, name, out);
            }
        }
    }
}

/// Is this function name the `macro.<name>` row of THIS macro?
fn is_trigger_row(function: &str, name: &str) -> bool {
    ksx_config::macro_name(function).is_some_and(|target| target.eq_ignore_ascii_case(name))
}

/// Drop every `macro.<name>` row from a `[bindings]` table — both spellings
/// (the flat `"macro.hadouken"` key and the nested `macro = { hadouken = … }`
/// group a TOML dotted key produces).
fn remove_macro_triggers(bindings: &mut BTreeMap<String, BindingEntry>, name: &str) {
    bindings.retain(|function, _| !is_trigger_row(function, name));
    let Some(group_key) = bindings
        .keys()
        .find(|function| {
            ksx_config::MACRO_PREFIX
                .strip_suffix('.')
                .is_some_and(|head| function.eq_ignore_ascii_case(head))
        })
        .cloned()
    else {
        return;
    };
    let Some(BindingEntry::Group(group)) = bindings.get_mut(&group_key) else {
        return;
    };
    group.retain(|sub, _| !sub.eq_ignore_ascii_case(name));
    if group.is_empty() {
        bindings.remove(&group_key);
    }
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

/// The one conflict scope left: `key` bound in ANOTHER slot's preset, inside a
/// games.toml profile that also uses `preset_name`.
///
/// The same key on another function of the SAME preset is deliberately not
/// here — it is a multi-bind, reported as `also_drives`.
fn find_profile_conflicts(store: &Store, preset_name: &str, key: Key) -> Vec<MapConflict> {
    let mut conflicts = Vec::new();

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
                    key: key.name().to_owned(),
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

/// The flash advisories as pipe/Studio JSON rows — one shape everywhere.
/// `[{ "key": "G", "bound_to": "A" }]`, empty for any unguarded write.
pub fn flash_json(flash: &[(String, String)]) -> serde_json::Value {
    serde_json::Value::Array(
        flash
            .iter()
            .map(|(key, bound_to)| serde_json::json!({ "key": key, "bound_to": bound_to }))
            .collect(),
    )
}

/// What `--move-from` unbound, as pipe/CLI JSON — one shape everywhere.
/// `null` when nothing was moved (the normal multi-bind write), otherwise
/// `{"function":"B","remaining":[],"unbound":true}`: `unbound` is the flag a
/// UI needs (the control now shows the inert `"None"`), `remaining` is what it
/// kept when the key was one of several.
pub fn moved_from_json(moved: Option<&MovedFrom>) -> serde_json::Value {
    match moved {
        None => serde_json::Value::Null,
        Some(moved) => serde_json::json!({
            "function": moved.function,
            "remaining": moved.remaining,
            "unbound": moved.remaining.is_empty(),
        }),
    }
}

/// The conflicts as pipe/Studio JSON rows — one shape everywhere.
///
/// `scope` is the constant `"profile"`: every conflict this writer reports is
/// cross-slot now, and the field stays on the wire so readers that switch on
/// it (studio's `BindConflict`, studio-ui's dialog) keep working unchanged.
pub fn conflicts_json(conflicts: &[MapConflict]) -> serde_json::Value {
    serde_json::Value::Array(
        conflicts
            .iter()
            .map(|c| {
                serde_json::json!({
                    "scope": "profile",
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
            ..MapSpec::default()
        }
    }

    /// The list spelling: the WHOLE set of keys this control should hold
    /// (empty = a clear), which is what Studio's mapper computes and sends.
    fn keys_spec(preset: &str, function: &str, keys: &[&str]) -> MapSpec {
        MapSpec {
            preset: preset.into(),
            function: function.into(),
            keys: keys.iter().map(|k| (*k).to_owned()).collect(),
            ..MapSpec::default()
        }
    }

    /// The explicit move: `--function F --key K --move-from VICTIM`.
    fn move_spec(preset: &str, function: &str, key: &str, victim: &str) -> MapSpec {
        MapSpec {
            preset: preset.into(),
            function: function.into(),
            key: Some(key.to_owned()),
            move_from: Some(victim.to_owned()),
            ..MapSpec::default()
        }
    }

    /// The chord shape: `--function F --key K --when …`.
    fn chord_spec(preset: &str, function: &str, key: &str, when: &[&str]) -> MapSpec {
        MapSpec {
            preset: preset.into(),
            function: function.into(),
            key: Some(key.to_owned()),
            when: when.iter().map(|k| (*k).to_owned()).collect(),
            ..MapSpec::default()
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
        assert!(applied.moved_from.is_none());
        assert!(applied.also_drives.is_empty());
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

    /// `--function F --key K` and the one-entry list are the SAME write —
    /// same bytes on disk, same message — so the list spelling costs nothing.
    #[test]
    fn a_one_key_list_is_byte_for_byte_the_single_key_write() {
        let root = TempRoot::new("one-key-list");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");
        let single = apply(&store, &spec("P1", "A", Some("G"), false)).unwrap();
        let after_single = std::fs::read_to_string(&single.path).unwrap();

        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");
        let listed = apply(&store, &keys_spec("P1", "A", &["G"])).unwrap();
        assert_eq!(
            std::fs::read_to_string(&listed.path).unwrap(),
            after_single,
            "a one-key list must not write a different file"
        );
        assert_eq!(listed.message(), single.message());
        assert_eq!(listed.key.as_deref(), Some("G"));
        assert_eq!(listed.keys, vec!["G".to_owned()]);
    }

    /// MANY KEYS → ONE CONTROL, in ONE write: the file gets a list, in the
    /// order asked for, and the engine reads two entries for that function.
    #[test]
    fn two_keys_land_as_a_list_in_one_write() {
        let root = TempRoot::new("two-keys");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");

        let applied = apply(&store, &keys_spec("P1", "A", &["Enter", "g"])).unwrap();
        assert_eq!(applied.keys, vec!["Enter".to_owned(), "G".to_owned()]);
        assert_eq!(
            applied.key.as_deref(),
            Some("Enter"),
            "\"key\" stays the FIRST key for pre-list readers"
        );
        assert_eq!(applied.message(), "\"P1\": A = Enter, G");

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(
            on_disk.contains("A = [\"Enter\", \"G\"]"),
            "caller order, as a list: {on_disk}"
        );
        assert!(
            on_disk.contains("B = \"D\""),
            "sibling untouched: {on_disk}"
        );

        // And the engine sees BOTH keys driving A (the OR-chain).
        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        let on_a: Vec<Key> = core
            .entries
            .iter()
            .filter(|(_, b)| ksx_config::function_name(b) == "A")
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(on_a, vec![Key::Enter, Key::G], "{:?}", core.entries);
    }

    /// The mapper's "add another key" then its per-key ✕, as the two writes
    /// Studio actually sends: each one carries the WHOLE list, so removing the
    /// added key restores exactly the file that was there before.
    #[test]
    fn add_then_remove_a_key_round_trips_to_the_original_file() {
        let root = TempRoot::new("add-remove");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        // "Add another key": current keys ∪ {Enter}.
        let added = apply(&store, &keys_spec("P1", "A", &["S", "Enter"])).unwrap();
        let with_both = std::fs::read_to_string(&added.path).unwrap();
        assert!(with_both.contains("A = [\"S\", \"Enter\"]"), "{with_both}");

        // The per-key ✕ on Enter: current keys ∖ {Enter}.
        let removed = apply(&store, &keys_spec("P1", "A", &["S"])).unwrap();
        assert_eq!(
            std::fs::read_to_string(&removed.path).unwrap(),
            before,
            "add then remove must land back on the original file"
        );

        // And the ✕ on the LAST key is an honest clear, not a vanished row.
        let cleared = apply(&store, &keys_spec("P1", "A", &[])).unwrap();
        assert!(cleared.keys.is_empty());
        assert_eq!(cleared.message(), "\"P1\": A cleared");
        let on_disk = std::fs::read_to_string(&cleared.path).unwrap();
        assert!(on_disk.contains("A = \"None\""), "{on_disk}");
    }

    /// Duplicates are dropped AFTER the key name is resolved (so `s` and `S`
    /// are one key), and the FIRST occurrence keeps its place: the file never
    /// holds the same key twice for one control, whatever the caller sent.
    #[test]
    fn a_key_list_keeps_its_order_and_drops_duplicates() {
        let root = TempRoot::new("dedup");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        let applied = apply(
            &store,
            &keys_spec("P1", "A", &["Enter", "g", "enter", "G", "S"]),
        )
        .unwrap();
        assert_eq!(
            applied.keys,
            vec!["Enter".to_owned(), "G".to_owned(), "S".to_owned()]
        );
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(
            on_disk.contains("A = [\"Enter\", \"G\", \"S\"]"),
            "{on_disk}"
        );
    }

    /// `key` and `keys` are two spellings of one field. Sending both is
    /// refused BEFORE any write — merging them would silently invent a
    /// binding the caller never asked for.
    #[test]
    fn a_key_and_a_key_list_together_are_refused_and_write_nothing() {
        let root = TempRoot::new("key-and-keys");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "A".into(),
                key: Some("G".into()),
                keys: vec!["Enter".into()],
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::KeyAndKeys), "{err:?}");
        assert!(err.to_string().contains("Nothing was written"), "{err}");
        assert_eq!(
            before,
            std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap()
        );
    }

    /// A key LIST and CHORDS live side by side: chords are their own rows
    /// (`Preset::chords`), so writing a list to one control leaves another
    /// control's guard exactly where it was — and the two can be read back
    /// together.
    #[test]
    fn a_key_list_leaves_another_functions_chord_alone() {
        let root = TempRoot::new("list-and-chords");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");
        apply(&store, &chord_spec("P1", "rt", "D", &["F"])).unwrap();

        let applied = apply(&store, &keys_spec("P1", "A", &["S", "Enter"])).unwrap();
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = [\"S\", \"Enter\"]"), "{on_disk}");

        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert_eq!(core.chords.len(), 1, "the chord survived: {on_disk}");
        assert_eq!(core.chords[0].key, Key::D);
        assert_eq!(core.chords[0].when, vec![Key::F]);
        assert_eq!(
            core.entries
                .iter()
                .filter(|(_, b)| ksx_config::function_name(b) == "A")
                .count(),
            2
        );
    }

    /// The two edits that mean "this ONE key": a chord's trigger and
    /// `--move-from`'s victim. Given a list they refuse in words instead of
    /// picking a key for the caller.
    #[test]
    fn a_key_list_is_refused_where_exactly_one_key_is_meant() {
        let root = TempRoot::new("list-refusals");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "rt".into(),
                keys: vec!["S".into(), "Enter".into()],
                when: vec!["F".into()],
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::InvalidGuard(_)), "{err:?}");
        assert!(err.to_string().contains("ONE trigger key"), "{err}");

        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "A".into(),
                keys: vec!["G".into(), "Enter".into()],
                move_from: Some("B".into()),
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::BadMoveFrom(_)), "{err:?}");
        assert!(err.to_string().contains("ONE key away from B"), "{err}");

        assert_eq!(
            before,
            std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap(),
            "no refusal may leave a changed file"
        );
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

    // ---- multi-bind: one key → many controls (docs/INPUT-TRANSFORMS.md §1a)

    /// THE bug this replaced: writing a key that another control of the SAME
    /// preset already holds used to be a conflict, and `force` MOVED the key.
    /// It is a multi-bind — both controls keep it, no flag needed, and the
    /// response says what else the key drives.
    #[test]
    fn a_same_preset_duplicate_is_a_multi_bind_not_a_conflict() {
        let root = TempRoot::new("multibind");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\n");

        // No --force anywhere in sight.
        let applied = apply(&store, &spec("P1", "A", Some("G"), false)).unwrap();
        assert_eq!(applied.also_drives, vec!["B".to_owned()]);
        assert!(applied.moved_from.is_none(), "nothing was unbound");
        assert_eq!(applied.message(), "\"P1\": A = G; G also drives B");

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(
            on_disk.contains("B = \"G\""),
            "the original binding must be untouched: {on_disk}"
        );
        assert!(!on_disk.contains("None"), "nothing was cleared: {on_disk}");
    }

    /// The acceptance test for the mapper's "Map all to one key": N sequential
    /// ordinary writes of ONE key must all stick (v7's multi-select arm writes
    /// exactly this, one `map` call per selected control).
    #[test]
    fn one_key_can_drive_three_controls_written_one_at_a_time() {
        let root = TempRoot::new("map-all");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\nrt = \"E\"\n");

        let mut last = None;
        for function in ["A", "B", "rt"] {
            last = Some(apply(&store, &spec("P1", function, Some("P"), false)).unwrap());
        }
        let last = last.unwrap();
        assert_eq!(
            last.also_drives,
            vec!["A".to_owned(), "B".to_owned()],
            "the last write names the two controls already on P"
        );

        let on_disk = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        for row in ["A = \"P\"", "B = \"P\"", "rt = \"P\""] {
            assert!(on_disk.contains(row), "missing {row} in:\n{on_disk}");
        }
        // And the engine sees one key with three targets — the thing the file
        // is FOR (many keys → one function and one key → many functions are
        // both native, ksx-core/src/preset.rs).
        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        let on_p = core.entries.iter().filter(|(k, _)| *k == Key::P).count();
        assert_eq!(on_p, 3, "{:?}", core.entries);
    }

    /// Re-binding the SAME control is still replace-per-function: the key it
    /// used to hold goes, and the OTHER controls sharing that old key stay.
    #[test]
    fn rebinding_one_control_never_disturbs_the_others_sharing_its_key() {
        let root = TempRoot::new("multibind-rebind");
        let store = root.store();
        preset(&store, "P1", "A = \"P\"\nB = \"P\"\nrt = \"P\"\n");

        let applied = apply(&store, &spec("P1", "B", Some("Q"), false)).unwrap();
        assert!(applied.also_drives.is_empty(), "Q drives B alone");
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("B = \"Q\""), "{on_disk}");
        assert!(on_disk.contains("A = \"P\""), "{on_disk}");
        assert!(on_disk.contains("rt = \"P\""), "{on_disk}");
    }

    // ---- the explicit move: --move-from ----------------------------------

    /// `--move-from B` unbinds exactly B, and exactly of this key.
    #[test]
    fn move_from_unbinds_the_named_function_and_nothing_else() {
        let root = TempRoot::new("move-from");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\nX = \"G\"\nY = \"G\"\n");

        let applied = apply(&store, &move_spec("P1", "A", "G", "b")).unwrap();
        let moved = applied.moved_from.as_ref().expect("a move was requested");
        assert_eq!(moved.function, "B", "the victim is named, canonically");
        assert!(moved.remaining.is_empty(), "B had only G");
        assert_eq!(
            applied.also_drives,
            vec!["X".to_owned(), "Y".to_owned()],
            "the OTHER co-bindings are untouched and reported"
        );
        assert!(
            applied
                .message()
                .contains("taken from B — B is now unbound"),
            "{}",
            applied.message()
        );

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("A = \"G\""), "{on_disk}");
        assert!(on_disk.contains("B = \"None\""), "{on_disk}");
        assert!(on_disk.contains("X = \"G\""), "{on_disk}");
        assert!(on_disk.contains("Y = \"G\""), "{on_disk}");
    }

    /// A victim with several keys keeps the others — the move takes ONE key,
    /// not the control's whole binding — and the message says so.
    #[test]
    fn move_from_takes_only_that_key_when_the_victim_has_more() {
        let root = TempRoot::new("move-from-multi");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = [\"G\", \"H\"]\n");

        let applied = apply(&store, &move_spec("P1", "A", "G", "B")).unwrap();
        let moved = applied.moved_from.as_ref().unwrap();
        assert_eq!(moved.remaining, vec!["H".to_owned()]);
        assert!(
            applied.message().contains("B still has H"),
            "{}",
            applied.message()
        );
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("B = \"H\""), "{on_disk}");
        assert!(!on_disk.contains("None"), "{on_disk}");
    }

    /// Everything `--move-from` refuses, it refuses BEFORE writing: the one
    /// path that removes a binding never guesses which one.
    #[test]
    fn move_from_refuses_anything_it_would_have_to_guess() {
        let root = TempRoot::new("move-from-refuse");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\nX = \"Q\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        // A control that does not hold this key — moving would unbind
        // something the write was not about.
        let err = apply(&store, &move_spec("P1", "A", "G", "X")).unwrap_err();
        assert!(matches!(err, MapError::BadMoveFrom(_)), "{err:?}");
        assert!(err.to_string().contains("X is not bound to G"), "{err}");
        assert!(err.to_string().contains("(it has Q)"), "{err}");

        // A control that is not bound at all.
        let err = apply(&store, &move_spec("P1", "A", "G", "Y")).unwrap_err();
        assert!(
            err.to_string().contains("Y is not bound to anything"),
            "{err}"
        );

        // The function being bound.
        let err = apply(&store, &move_spec("P1", "A", "G", "a")).unwrap_err();
        assert!(err.to_string().contains("being bound"), "{err}");

        // A clear takes nothing from anyone.
        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "A".into(),
                key: None,
                move_from: Some("B".into()),
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("needs a --key"), "{err}");

        // A chord layers over its keys instead of taking them.
        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "rt".into(),
                key: Some("G".into()),
                when: vec!["F".into()],
                move_from: Some("B".into()),
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("a chord layers over"), "{err}");

        // An unknown victim is refused like any other function name.
        let err = apply(&store, &move_spec("P1", "A", "G", "warp")).unwrap_err();
        assert!(matches!(err, MapError::UnknownFunction(_)), "{err:?}");

        assert_eq!(
            before,
            std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap(),
            "no refusal may leave a changed file"
        );
    }

    /// `force` is not a spelling of "move": it only concerns the cross-slot
    /// case, and it removes nothing.
    #[test]
    fn force_never_takes_a_key_away_from_anyone() {
        let root = TempRoot::new("force-harmless");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"G\"\n");
        let applied = apply(&store, &spec("P1", "A", Some("G"), true)).unwrap();
        assert!(applied.moved_from.is_none());
        assert_eq!(applied.also_drives, vec!["B".to_owned()]);
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("B = \"G\""), "{on_disk}");
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
        assert_eq!(conflicts[0].preset, "P2");
        assert_eq!(conflicts[0].function, "A");
        assert_eq!(conflicts[0].profile.as_deref(), Some("Steam"));
        assert_eq!(conflicts[0].slot, Some(2));
        assert!(err.to_string().contains("\"P2\"'s A"), "{err}");
        assert!(err.to_string().contains("another SLOT's preset"), "{err}");

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

    // ---- chords (docs/INPUT-TRANSFORMS.md §1b) ----------------------------

    #[test]
    fn a_chord_writes_the_guarded_form_and_says_so() {
        let root = TempRoot::new("chord");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");

        let applied = apply(&store, &chord_spec("P1", "rt", "d", &["f"])).unwrap();
        assert_eq!(applied.key.as_deref(), Some("D"), "canonicalized");
        assert_eq!(applied.when, vec!["F".to_owned()]);
        assert_eq!(applied.chord().as_deref(), Some("D+F"));
        assert_eq!(applied.message(), "\"P1\": rt = D+F");
        assert!(
            applied.flash.is_empty(),
            "F and D bind nothing on their own"
        );

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("key = \"D\""), "{on_disk}");
        assert!(on_disk.contains("when = [\"F\"]"), "{on_disk}");
        assert!(
            on_disk.contains("A = \"S\""),
            "untouched sibling: {on_disk}"
        );

        // And it reloads as a chord, not as anything else.
        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert_eq!(
            core.chords,
            vec![ksx_core::Chord::new(
                Key::D,
                ksx_core::Binding::Trigger(ksx_core::Trigger::Right),
                vec![Key::F]
            )]
        );
    }

    /// A chord over keys that already do something is ALLOWED — that is the
    /// point — so it must not trip the conflict machinery, and must not steal
    /// the bindings it is layered over. What the caller gets is the flash.
    #[test]
    fn a_chord_over_bound_keys_is_written_with_the_flash_named() {
        let root = TempRoot::new("chord-flash");
        let store = root.store();
        preset(&store, "P1", "X = \"A\"\nY = \"B\"\n");

        let applied = apply(&store, &chord_spec("P1", "rt", "A", &["B"])).unwrap();
        assert!(applied.moved_from.is_none(), "a chord takes nothing");
        assert_eq!(
            applied.flash,
            vec![
                ("A".to_owned(), "X".to_owned()),
                ("B".to_owned(), "Y".to_owned())
            ]
        );
        let message = applied.message();
        assert!(message.contains("does not defer input"), "{message}");

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(
            on_disk.contains("X = \"A\""),
            "layered over, not stolen: {on_disk}"
        );
        assert!(on_disk.contains("Y = \"B\""), "{on_disk}");
    }

    /// Replace-per-function covers guarded rows: re-mapping and `--clear`
    /// both wipe the chord instead of leaving a ghost the engine still obeys.
    #[test]
    fn clear_and_rebind_remove_a_chord() {
        let root = TempRoot::new("chord-clear");
        let store = root.store();
        preset(&store, "P1", "rt = { key = \"D\", when = [\"F\"] }\n");

        // Plain re-bind of the same function drops the guard.
        let applied = apply(&store, &spec("P1", "rt", Some("Q"), false)).unwrap();
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("rt = \"Q\""), "{on_disk}");
        assert!(!on_disk.contains("when"), "{on_disk}");

        // Back to a chord, then clear it.
        apply(&store, &chord_spec("P1", "rt", "D", &["F"])).unwrap();
        let applied = apply(&store, &spec("P1", "rt", None, false)).unwrap();
        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("rt = \"None\""), "{on_disk}");
        assert!(!on_disk.contains("when"), "{on_disk}");
        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert!(core.chords.is_empty());
    }

    /// A plain multi-bind and a chord on the SAME key coexist, in both write
    /// orders: the chord never joins the conflict machinery (it was already
    /// exempt), and the plain write treats the key like any other shared key.
    /// The engine's consumption rules decide what fires (§1b); the file just
    /// has to hold both, and the flash advisory has to name the cost.
    #[test]
    fn a_plain_multi_bind_and_a_chord_share_one_key() {
        let root = TempRoot::new("chord-multibind");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");

        // Plain first, then the chord on top of the same trigger.
        apply(&store, &spec("P1", "A", Some("D"), false)).unwrap();
        apply(&store, &spec("P1", "B", Some("D"), false)).unwrap();
        let applied = apply(&store, &chord_spec("P1", "rt", "D", &["F"])).unwrap();
        assert!(applied.moved_from.is_none(), "a chord takes nothing");
        assert!(
            applied.also_drives.is_empty(),
            "co-bindings are a PLAIN write's report; a chord reports the flash"
        );
        // The flash names the trigger's own binding — one row per constituent
        // that is also individually bound, whichever function it is.
        assert_eq!(applied.flash.len(), 1, "{:?}", applied.flash);
        assert_eq!(applied.flash[0].0, "D");

        // Then a THIRD plain control joins the same key: still no conflict,
        // and the chord is still there afterwards.
        let applied = apply(&store, &spec("P1", "X", Some("D"), false)).unwrap();
        assert_eq!(applied.also_drives, vec!["A".to_owned(), "B".to_owned()]);
        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert_eq!(
            core.entries.iter().filter(|(k, _)| *k == Key::D).count(),
            3,
            "{:?}",
            core.entries
        );
        assert_eq!(core.chords.len(), 1, "{:?}", core.chords);
        assert_eq!(core.chords[0].key, Key::D);
    }

    /// A chord on one function must not disturb another function's chord.
    #[test]
    fn chords_on_different_functions_coexist() {
        let root = TempRoot::new("chord-coexist");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        apply(&store, &chord_spec("P1", "rt", "D", &["F"])).unwrap();
        apply(&store, &chord_spec("P1", "lt", "D", &["C"])).unwrap();
        let core = store
            .load_preset("P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert_eq!(core.chords.len(), 2, "{:?}", core.chords);
    }

    #[test]
    fn impossible_guards_are_refused_before_any_write() {
        let root = TempRoot::new("chord-refuse");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        // The trigger guarding itself.
        let err = apply(&store, &chord_spec("P1", "rt", "D", &["D"])).unwrap_err();
        assert!(matches!(err, MapError::InvalidGuard(_)), "{err:?}");
        assert!(
            err.to_string().contains("the trigger is always required"),
            "{err}"
        );

        // Required and forbidden at once.
        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "rt".into(),
                key: Some("D".into()),
                when: vec!["F".into()],
                unless: vec!["F".into()],
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::InvalidGuard(_)), "{err:?}");

        // A guard with nothing to guard.
        let err = apply(
            &store,
            &MapSpec {
                preset: "P1".into(),
                function: "rt".into(),
                key: None,
                when: vec!["F".into()],
                ..MapSpec::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::InvalidGuard(_)), "{err:?}");

        // An unknown guard key is refused like any other key name.
        let err = apply(&store, &chord_spec("P1", "rt", "D", &["NotAKey"])).unwrap_err();
        assert!(matches!(err, MapError::UnknownKey(_)), "{err:?}");

        assert_eq!(
            before,
            std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap(),
            "no refusal may leave a changed file"
        );
    }

    #[test]
    fn flash_advisories_serialize_to_the_documented_rows() {
        assert_eq!(
            flash_json(&[("G".into(), "A".into())]),
            serde_json::json!([{ "key": "G", "bound_to": "A" }])
        );
        assert_eq!(flash_json(&[]), serde_json::json!([]));
    }

    #[test]
    fn keys_resolve_case_insensitively_when_unique() {
        assert_eq!(resolve_key("g").unwrap(), Key::G);
        assert_eq!(resolve_key("enter").unwrap(), Key::Enter);
        assert_eq!(resolve_key("Left").unwrap(), Key::Left);
        assert!(resolve_key("not a key").is_err());
    }

    #[test]
    fn restore_defaults_rewrites_the_bindings_and_keeps_the_name() {
        let root = TempRoot::new("restore-defaults");
        let store = root.store();
        preset(&store, "P1", "A = \"Q\"\nB = \"W\"\n");

        let applied = restore(&store, "P1", RestoreKind::Defaults).unwrap();
        assert_eq!(applied.preset, "P1");
        // The label names the DESTINATION, never the bare word "defaults" —
        // Victor's cabinet would otherwise read "restore defaults" as "put my
        // I-PAC map back" and get a desktop-keyboard layout instead.
        let message = applied.message();
        assert!(message.contains("generic keyboard layout"), "{message}");
        assert!(message.contains("S/D/A/W"), "{message}");
        assert!(
            message.contains("backed up as"),
            "a restore must say where the old file went: {message}"
        );
        let reloaded = store.load_preset("P1").unwrap().unwrap().value;
        assert_eq!(reloaded.name, "P1", "name survives a defaults restore");
        let defaults = PresetFile::from_core(&ksx_core::Preset {
            name: "P1".into(),
            entries: ksx_core::Preset::builtin_default().entries,
            chords: Vec::new(),
            macros: Default::default(),
            protected: false,
        });
        assert_eq!(
            reloaded.bindings, defaults.bindings,
            "bindings are exactly the built-in default layout"
        );
    }

    #[test]
    fn restore_refuses_unknown_presets() {
        let root = TempRoot::new("restore-unknown");
        let store = root.store();
        assert!(matches!(
            restore(&store, "Nope", RestoreKind::Defaults),
            Err(MapError::UnknownPreset { .. })
        ));
    }

    #[test]
    fn session_backup_round_trip_undoes_later_writes() {
        let root = TempRoot::new("session-bak");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\nB = \"D\"\n");

        // No backup yet: restore says so and writes nothing.
        let err = restore(&store, "P1", RestoreKind::SessionBackup).unwrap_err();
        assert!(matches!(err, MapError::NoSessionBackup { .. }), "{err:?}");
        assert!(err.to_string().contains("nothing to undo"), "{err}");

        // The daemon's first-write snapshot, then two edits.
        take_session_backup(&store, "P1").unwrap();
        apply(&store, &spec("P1", "A", Some("G"), false)).unwrap();
        apply(&store, &spec("P1", "B", None, false)).unwrap();
        let edited = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert!(edited.contains("A = \"G\""), "{edited}");
        assert!(edited.contains("B = \"None\""), "{edited}");

        // Undo this session: both edits gone.
        let applied = restore(&store, "P1", RestoreKind::SessionBackup).unwrap();
        assert!(applied.message().contains("session-start backup"));
        let restored = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert!(restored.contains("A = \"S\""), "{restored}");
        assert!(restored.contains("B = \"D\""), "{restored}");
    }

    #[test]
    fn a_corrupt_session_backup_is_refused_not_written() {
        let root = TempRoot::new("session-bak-corrupt");
        let store = root.store();
        preset(&store, "P1", "A = \"S\"\n");
        std::fs::write(
            session_backup_path(&store, "P1").unwrap(),
            "this is not a preset",
        )
        .unwrap();
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        let err = restore(&store, "P1", RestoreKind::SessionBackup).unwrap_err();
        assert!(matches!(err, MapError::BadBackup { .. }), "{err:?}");
        let after = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();
        assert_eq!(before, after, "refusal must not touch the preset");
        // A refusal must not leave a pointless timestamped backup behind
        // either — the copy happens only once the write is certain.
        assert!(list_backups(&store, "P1").unwrap().is_empty());
    }

    // -- timestamped backups (FIX 2) ---------------------------------------

    /// The road home from a restore. "Reset to the generic keyboard layout"
    /// is the most destructive button on the page; `latest-backup` is what
    /// makes pressing it survivable.
    #[test]
    fn every_restore_backs_the_preset_up_first_and_latest_backup_undoes_it() {
        let root = TempRoot::new("bak-undo");
        let store = root.store();
        preset(&store, "IPAC P1", "A = \"G\"\nB = \"F\"\n");
        let original = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();

        // Nothing to go back to yet: the refusal names the mechanism.
        let err = restore(&store, "IPAC P1", RestoreKind::LatestBackup).unwrap_err();
        assert!(matches!(err, MapError::NoBackup { .. }), "{err:?}");
        assert!(err.to_string().contains("bak-YYYYMMDD-HHMMSS"), "{err}");

        // The scary one: reset to the generic keyboard layout.
        let applied = restore(&store, "IPAC P1", RestoreKind::Defaults).unwrap();
        let backup = applied.backup.expect("a restore always backs up first");
        assert!(backup.path.exists(), "{}", backup.path.display());
        assert_eq!(
            std::fs::read_to_string(&backup.path).unwrap(),
            original,
            "the backup must be the file as it was before the restore"
        );
        let generic = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        // The legacy generic layout: S (and Enter) on A, W on Y, arrows on the
        // left stick — a desktop keyboard, nothing like the panel map above.
        assert!(generic.contains(r#"A = ["S", "Enter"]"#), "{generic}");
        assert!(generic.contains(r#""lx.min" = "Left""#), "{generic}");

        // …and one click back to the panel map.
        let undone = restore(&store, "IPAC P1", RestoreKind::LatestBackup).unwrap();
        assert!(
            undone.message().contains("newest timestamped backup"),
            "{}",
            undone.message()
        );
        let back = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(back.contains("A = \"G\""), "{back}");
        assert!(back.contains("B = \"F\""), "{back}");
        // Two restores, two backups, newest first.
        let backups = list_backups(&store, "IPAC P1").unwrap();
        assert_eq!(backups.len(), 2, "{backups:?}");
        assert!(backups[0].stamp >= backups[1].stamp, "{backups:?}");
    }

    /// Two restores inside one second must not overwrite each other's backup —
    /// the whole chain would collapse to one entry.
    #[test]
    fn backups_taken_in_the_same_second_do_not_collide() {
        let root = TempRoot::new("bak-collide");
        let store = root.store();
        preset(&store, "P1", "A = \"G\"\n");
        for _ in 0..3 {
            take_backup(&store, "P1").unwrap().expect("preset exists");
        }
        let backups = list_backups(&store, "P1").unwrap();
        assert_eq!(backups.len(), 3, "{backups:?}");
        let mut stamps: Vec<&str> = backups.iter().map(|b| b.stamp.as_str()).collect();
        stamps.sort_unstable();
        stamps.dedup();
        assert_eq!(stamps.len(), 3, "stamps must be distinct: {backups:?}");
    }

    /// Backups sit next to the preset with a non-`.toml` extension, so the
    /// store's preset scan must never pick one up as a second preset.
    #[test]
    fn backups_are_invisible_to_the_preset_loader() {
        let root = TempRoot::new("bak-invisible");
        let store = root.store();
        preset(&store, "P1", "A = \"G\"\n");
        take_backup(&store, "P1").unwrap();
        take_session_backup(&store, "P1").unwrap();
        let loaded = store.load_presets().unwrap();
        assert_eq!(loaded.value.len(), 1, "{:?}", loaded.value);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    /// "Clear all bindings" empties the preset without breaking it — every
    /// function is still listed (as the inert `"None"`), so the mapper's
    /// legend keeps 25 rows and the file keeps parsing.
    #[test]
    fn clear_all_empties_the_preset_and_leaves_a_backup() {
        let root = TempRoot::new("clear-all");
        let store = root.store();
        preset(&store, "IPAC P1", "A = \"G\"\nB = \"F\"\n");

        let applied = clear_all(&store, "IPAC P1").unwrap();
        assert_eq!(applied.kind, RestoreKind::ClearAll);
        assert!(applied.backup.is_some(), "clearing must be undoable");
        let message = applied.message();
        assert!(message.contains("every binding cleared"), "{message}");
        assert!(message.contains("backed up as"), "{message}");

        let on_disk = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(!on_disk.contains("\"G\""), "{on_disk}");
        assert!(on_disk.contains("A = \"None\""), "{on_disk}");
        assert!(on_disk.contains("\"dpad.up\" = \"None\""), "{on_disk}");
        // Still a valid preset, and every function is still present.
        let core = store
            .load_preset("IPAC P1")
            .unwrap()
            .unwrap()
            .value
            .to_core()
            .unwrap();
        assert_eq!(core.entries.len(), 25);

        // …and one click back to the panel map.
        restore(&store, "IPAC P1", RestoreKind::LatestBackup).unwrap();
        let back = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(back.contains("A = \"G\""), "{back}");
    }

    #[test]
    fn clear_all_refuses_an_unknown_preset() {
        let root = TempRoot::new("clear-all-unknown");
        assert!(matches!(
            clear_all(&root.store(), "Nope"),
            Err(MapError::UnknownPreset { .. })
        ));
    }

    #[test]
    fn a_backup_stamp_spells_itself_out_for_a_human() {
        let backup = PresetBackup {
            path: PathBuf::from("x"),
            stamp: "20260805-143207".to_owned(),
        };
        assert_eq!(backup.label(), "2026-08-05 14:32:07 UTC");
        // Anything unexpected degrades to the raw stamp, never to a lie.
        let odd = PresetBackup {
            path: PathBuf::from("x"),
            stamp: "nonsense".to_owned(),
        };
        assert_eq!(odd.label(), "nonsense");
    }

    /// The wire words are contract: CLI `--restore`, pipe `"mode"`, Studio's
    /// three buttons all speak the same three strings.
    #[test]
    fn restore_kinds_round_trip_their_wire_words_and_name_their_destination() {
        for kind in [
            RestoreKind::Defaults,
            RestoreKind::SessionBackup,
            RestoreKind::LatestBackup,
        ] {
            assert_eq!(RestoreKind::parse(kind.as_str()), Some(kind));
            assert!(!kind.destination().is_empty());
        }
        assert_eq!(RestoreKind::parse("yolo"), None);
        // "clear everything" is its own verb, never a spelling of "restore" —
        // otherwise `--restore clear-all` would read as a way BACK.
        assert_eq!(RestoreKind::parse("clear-all"), None);
        assert_eq!(RestoreKind::ClearAll.as_str(), "clear-all");
        // The one label that must never be vague.
        assert!(RestoreKind::Defaults
            .destination()
            .contains("generic keyboard layout"));
        assert!(RestoreKind::Defaults
            .destination()
            .contains("NOT this preset's original panel map"));
    }

    #[test]
    fn conflicts_serialize_to_the_documented_rows() {
        let rows = conflicts_json(&[MapConflict {
            key: "G".into(),
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

    // ---- macro BODIES (docs/INPUT-TRANSFORMS.md §1c) ----------------------

    /// A preset written from raw TOML, so a test can carry `[macros]` tables
    /// and trigger rows the `preset` helper's bindings-only shape cannot.
    fn preset_toml(store: &Store, toml: &str) -> PresetFile {
        let file: PresetFile = toml::from_str(toml).unwrap();
        store.save_preset(&file).unwrap();
        file
    }

    /// The macro body a JSON caller (or the editor) sends.
    fn body(json: &str) -> MacroFile {
        serde_json::from_str(json).unwrap()
    }

    const SF_PRESET: &str = r#"
name = "P1"
[bindings]
A = "S"
"dpad.down" = "Down"
"dpad.right" = "Right"
macro.hadouken = ["P", "O"]

[macros.hadouken]
steps = [{ hold = ["A"], ms = 50 }]
"#;

    /// The headline: a four-step macro goes in as JSON, lands in the preset's
    /// TOML, and comes back out of the file as the same four steps — with the
    /// unit each one was authored in intact (a sequence written in frames must
    /// still read in frames) and the policies alongside it.
    #[test]
    fn a_four_step_macro_round_trips_to_toml_and_back() {
        let root = TempRoot::new("macro-roundtrip");
        let store = root.store();
        preset_toml(&store, SF_PRESET);

        let applied = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "hadouken".into(),
                body: body(
                    r#"{ "steps": [
                          { "hold": ["dpad.down"], "ms": 50 },
                          { "hold": ["dpad.down","dpad.right"], "ms": 50 },
                          { "hold": ["dpad.right"], "ms": 50 },
                          { "hold": ["A"], "frames": 3 } ],
                        "on_release": "abort", "retrigger": "restart",
                        "interrupt": "opposing" }"#,
                ),
                delete: false,
            },
        )
        .unwrap();

        assert_eq!(applied.steps, 4);
        assert_eq!(applied.total_ms, 200, "50 + 50 + 50 + 3 frames (50)");
        assert!(!applied.deleted);
        assert!(applied.warnings.is_empty(), "{:?}", applied.warnings);
        // The trigger rows are this verb's business only to REPORT.
        assert_eq!(applied.triggers, ["P", "O"]);
        assert!(
            applied.message().contains("started by P, O"),
            "{}",
            applied.message()
        );

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(on_disk.contains("frames = 3"), "{on_disk}");
        assert!(on_disk.contains("on_release = \"abort\""), "{on_disk}");

        // ...and the file the engine loads says exactly what was asked for.
        let core = load_preset_by_name(&store, "P1")
            .unwrap()
            .to_core()
            .unwrap();
        let mac = &core.macros.defs[0];
        assert_eq!(mac.steps.len(), 4);
        assert_eq!(mac.steps[1].hold.len(), 2, "the diagonal is ONE step");
        assert_eq!(mac.steps[3].duration, ksx_core::StepDuration::Frames(3));
        assert_eq!(mac.total_ms(), 200);
        assert_eq!(mac.on_release, ksx_core::OnRelease::Abort);
        assert_eq!(mac.retrigger, ksx_core::Retrigger::Restart);
        assert_eq!(mac.interrupt, ksx_core::Interrupt::Opposing);
        // Everything ELSE the preset held is untouched: this is a whole-MACRO
        // write, not a whole-preset one.
        assert!(core
            .entries
            .contains(&(Key::S, ksx_core::Binding::Button(ksx_core::XButton::A))));
        assert_eq!(core.macros.triggers.len(), 2);
    }

    /// A macro that did not exist is CREATED by the same call, next to the
    /// preset's other tables — and it starts with no trigger, which the
    /// message says out loud rather than implying the key is bound.
    #[test]
    fn writing_a_name_the_preset_lacks_adds_a_second_macro() {
        let root = TempRoot::new("macro-new");
        let store = root.store();
        preset_toml(&store, SF_PRESET);
        let applied = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "shoryuken".into(),
                body: body(r#"{ "steps": [{ "hold": ["dpad.right"], "ms": 50 }] }"#),
                delete: false,
            },
        )
        .unwrap();
        assert!(applied.triggers.is_empty());
        assert!(
            applied.message().contains("no trigger key yet"),
            "{}",
            applied.message()
        );
        let file = load_preset_by_name(&store, "P1").unwrap();
        assert_eq!(file.macros.len(), 2);
        assert!(file.macros.contains_key("hadouken"));
        assert!(file.macros.contains_key("shoryuken"));
    }

    /// A step holding something that is not a pad function is REFUSED, in the
    /// words validation already uses for it — and the refusal is total: no
    /// write, and no pointless backup left behind either.
    #[test]
    fn an_unknown_binding_in_a_hold_set_is_refused_and_nothing_is_written() {
        let root = TempRoot::new("macro-unknown-hold");
        let store = root.store();
        preset_toml(&store, SF_PRESET);
        let before = std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap();

        let err = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "hadouken".into(),
                body: body(
                    r#"{ "steps": [{ "hold": ["dpad.down"], "ms": 50 },
                                   { "hold": ["warp"], "ms": 50 }] }"#,
                ),
                delete: false,
            },
        )
        .unwrap_err();

        let MapError::BadMacro { ref problems, .. } = err else {
            panic!("expected a body refusal, got {err}");
        };
        assert!(problems.iter().any(|p| p.contains("warp")), "{problems:?}");
        assert!(err.to_string().contains("nothing was written"), "{err}");
        assert_eq!(
            std::fs::read_to_string(store.preset_path("P1").unwrap()).unwrap(),
            before,
            "a refusal must not touch the file"
        );
        assert!(
            list_backups(&store, "P1").unwrap().is_empty(),
            "a refusal must not leave a backup behind"
        );
    }

    /// Every other refusal the body itself can carry: no steps at all (which
    /// also points at the flag the caller probably meant), and a step with two
    /// duration units or none.
    #[test]
    fn a_body_that_cannot_run_is_refused_before_the_write() {
        let root = TempRoot::new("macro-bad-body");
        let store = root.store();
        preset_toml(&store, SF_PRESET);
        for (json, needle) in [
            (r#"{ "steps": [] }"#, "delete"),
            (
                r#"{ "steps": [{ "hold": ["A"], "ms": 50, "frames": 3 }] }"#,
                "frames",
            ),
            (r#"{ "steps": [{ "hold": ["A"] }] }"#, "duration"),
        ] {
            let err = save_macro(
                &store,
                &MacroSpec {
                    preset: "P1".into(),
                    name: "hadouken".into(),
                    body: body(json),
                    delete: false,
                },
            )
            .unwrap_err();
            assert!(
                matches!(err, MapError::BadMacro { .. }),
                "{json} gave {err}"
            );
            assert!(err.to_string().contains(needle), "{json} gave {err}");
        }
        // A macro with no name is the same class of refusal.
        let err = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "   ".into(),
                body: body(r#"{ "steps": [{ "hold": ["A"], "ms": 50 }] }"#),
                delete: false,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("needs a name"), "{err}");
    }

    /// The sampling rule (§0.2) is an ADVISORY on both sides, never a refusal:
    /// the write lands and the answer says what the engine will really do.
    #[test]
    fn a_short_step_is_a_warning_the_write_carries_not_a_refusal() {
        let root = TempRoot::new("macro-short");
        let store = root.store();
        preset_toml(&store, SF_PRESET);
        let applied = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "hadouken".into(),
                body: body(
                    r#"{ "steps": [{ "hold": ["A"], "ms": 5 },
                                   { "hold": [], "ms": 5, "allow_short": true }] }"#,
                ),
                delete: false,
            },
        )
        .unwrap();
        assert_eq!(applied.warnings.len(), 2, "{:?}", applied.warnings);
        assert!(
            applied.warnings.iter().any(|w| w.contains("raised")),
            "{:?}",
            applied.warnings
        );
        assert!(
            applied.warnings.iter().any(|w| w.contains("allow_short")),
            "{:?}",
            applied.warnings
        );
        // ...and the reported length is what the ENGINE will run: the first
        // step was raised to the floor, the opted-out one was not.
        assert_eq!(applied.total_ms, u64::from(ksx_core::MIN_STEP_MS) + 5);
        assert!(applied.message().contains("note:"), "{}", applied.message());
    }

    /// Every macro write takes the same timestamped backup a restore does —
    /// so "I just overwrote my sequence" has the same one-click road home as
    /// every other whole-file write, through the SAME `--restore
    /// latest-backup`.
    #[test]
    fn a_macro_write_takes_a_timestamped_backup_first() {
        let root = TempRoot::new("macro-backup");
        let store = root.store();
        preset_toml(&store, SF_PRESET);

        let applied = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "hadouken".into(),
                body: body(r#"{ "steps": [{ "hold": ["dpad.down"], "ms": 50 }] }"#),
                delete: false,
            },
        )
        .unwrap();
        let backup = applied.backup.clone().expect("a backup was taken");
        assert!(backup.path.exists(), "{}", backup.path.display());
        assert_eq!(list_backups(&store, "P1").unwrap().len(), 1);
        // It holds the file as it was BEFORE this write.
        let saved = std::fs::read_to_string(&backup.path).unwrap();
        assert!(saved.contains("hold = [\"A\"]"), "{saved}");
        assert!(
            applied.message().contains(&backup.stamp),
            "{}",
            applied.message()
        );

        // ...and the restore verb that walks it back really does.
        restore(&store, "P1", RestoreKind::LatestBackup).unwrap();
    }

    /// DELETE takes the `macro.<name>` trigger rows with it — a trigger whose
    /// table is gone does not load at all, so leaving one behind would be
    /// writing a file the engine refuses.
    #[test]
    fn deleting_a_macro_takes_its_trigger_rows_with_it() {
        let root = TempRoot::new("macro-delete");
        let store = root.store();
        preset_toml(&store, SF_PRESET);

        let applied = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "HADOUKEN".into(), // names match case-insensitively
                body: MacroFile::default(),
                delete: true,
            },
        )
        .unwrap();
        assert!(applied.deleted);
        assert_eq!(applied.steps, 0);
        assert_eq!(applied.triggers, ["P", "O"], "the rows it removed");
        assert!(
            applied.backup.is_some(),
            "a delete is backed up like any write"
        );
        assert!(
            applied.message().contains("trigger row(s) went with it"),
            "{}",
            applied.message()
        );

        let on_disk = std::fs::read_to_string(&applied.path).unwrap();
        assert!(!on_disk.contains("macro"), "{on_disk}");
        let file = load_preset_by_name(&store, "P1").unwrap();
        assert!(file.macros.is_empty());
        // The preset still LOADS — the whole point of taking the rows too.
        let core = file.to_core().unwrap();
        assert!(core.macros.triggers.is_empty());
        assert!(core
            .entries
            .contains(&(Key::S, ksx_core::Binding::Button(ksx_core::XButton::A))));
    }

    /// Deleting something that is not there is a refusal that names what IS.
    #[test]
    fn deleting_a_macro_the_preset_does_not_define_is_refused() {
        let root = TempRoot::new("macro-delete-missing");
        let store = root.store();
        preset_toml(&store, SF_PRESET);
        let err = save_macro(
            &store,
            &MacroSpec {
                preset: "P1".into(),
                name: "shoryuken".into(),
                body: MacroFile::default(),
                delete: true,
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::UnknownMacro { .. }), "{err}");
        assert!(err.to_string().contains("hadouken"), "{err}");
        assert!(list_backups(&store, "P1").unwrap().is_empty());
    }

    /// The trigger reader sees both spellings of the same row — the flat
    /// quoted key ksx writes and the dotted key a hand-edited file uses.
    #[test]
    fn macro_triggers_are_read_in_either_toml_spelling() {
        let dotted: PresetFile = toml::from_str(
            "name = \"p\"\n[macros.m]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n\
             [bindings]\nmacro.m = \"P\"\n",
        )
        .unwrap();
        let flat: PresetFile = toml::from_str(
            "name = \"p\"\n[macros.m]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n\
             [bindings]\n\"macro.m\" = [\"P\", \"None\"]\n",
        )
        .unwrap();
        assert_eq!(macro_trigger_keys(&dotted, "m"), ["P"]);
        // The inert "None" placeholder is not a trigger.
        assert_eq!(macro_trigger_keys(&flat, "M"), ["P"]);
        assert!(macro_trigger_keys(&flat, "other").is_empty());
    }

    #[test]
    fn a_macro_write_needs_a_preset_that_exists() {
        let root = TempRoot::new("macro-no-preset");
        let store = root.store();
        let err = save_macro(
            &store,
            &MacroSpec {
                preset: "nope".into(),
                name: "m".into(),
                body: body(r#"{ "steps": [{ "hold": ["A"], "ms": 50 }] }"#),
                delete: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, MapError::UnknownPreset { .. }), "{err}");
    }

    #[test]
    fn a_move_serializes_to_the_documented_row() {
        assert_eq!(moved_from_json(None), serde_json::Value::Null);
        assert_eq!(
            moved_from_json(Some(&MovedFrom {
                function: "B".into(),
                remaining: Vec::new(),
            })),
            serde_json::json!({ "function": "B", "remaining": [], "unbound": true })
        );
        assert_eq!(
            moved_from_json(Some(&MovedFrom {
                function: "B".into(),
                remaining: vec!["H".into()],
            })),
            serde_json::json!({ "function": "B", "remaining": ["H"], "unbound": false })
        );
    }
}
