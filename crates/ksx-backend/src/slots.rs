//! **`ksx slot assign` — which preset a slot uses.**
//!
//! docs/CONTROL-SURFACE.md's honest gaps 1 and 5, closed. The mapper edits
//! bindings *inside* a preset; nothing until now could say which preset slot 3
//! points at except a hand edit of `config.toml` — or, since the interop verbs
//! landed, a whole-file `ksx config export | edit | import`, which works and
//! **loses every comment in the file**. That is exactly the loss the TOML-is-
//! canonical decision exists to prevent, so the narrow verb is worth having:
//! this one rewrites one field of one entry.
//!
//! # One writer, like every other write
//!
//! Same shape as [`crate::mapping`]: a typed spec in, a typed "here is what
//! landed" out, a timestamped backup taken before the write, and the store's
//! atomic save doing the actual I/O. The CLI verb, the pipe verb and any GUI
//! all call [`assign`] — there is no second editor, which is the standing rule
//! ("every front-door action maps to an existing backend verb").
//!
//! # Why a slot assignment BOUNCES the pads
//!
//! Every other write on the control surface is a key→function table and the
//! live engine takes it in place. This one is not, and the surface has to say
//! so *before* it is used. See [`ksx_api::SlotOutcome::restarted`] for the full
//! reasoning; the short version is that this verb writes the slot ENTRY, and a
//! verb whose pad behaviour depends on which field you happened to touch is a
//! verb nobody can predict.

use std::path::PathBuf;

use ksx_config::{ConfigError, GameSlotEntry, SlotEntry, Store};
use ksx_core::{Persona, MAX_SLOTS, MAX_XINPUT_SLOTS};

/// One slot assignment, as any surface spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSpec {
    /// 1..=[`MAX_SLOTS`].
    pub slot: u8,
    /// The preset's `name`, matched case-insensitively against what is on disk
    /// (a preset that exists keeps its own spelling — the same rule the macro
    /// writer uses for table names), or `None` for "the one it already uses".
    ///
    /// Optional for the same reason [`Self::persona`] is, and it arrived for
    /// the same reason: `ksx slot assign --slot 5 --persona playstation` must
    /// not oblige anyone to re-type a preset name they are not changing, and a
    /// surface that filled it in from what it last read would write back a
    /// preset the file may have moved on from.
    ///
    /// A slot that does not exist yet has no preset to keep, and that is a
    /// refusal ([`SlotError::NoPresetToKeep`]) rather than a silent default:
    /// there is no honest guess at which preset a brand-new player 5 wants.
    pub preset: Option<String>,
    /// A games.toml profile title, or `None` for `config.toml`'s `[[slot]]`
    /// list. The same either/or `ksx setup` asks about.
    pub profile: Option<String>,
    /// **Which controller this slot presents itself as**, or `None` for "leave
    /// it exactly as it is".
    ///
    /// `None` rather than [`Persona::default`] on purpose, all the way down
    /// from the wire: the default is `xbox360`, so a spec that could not
    /// express "not asked about" would turn every preset re-point into a
    /// silent re-personaing of that slot back to Xbox. A cabinet whose slots
    /// 5–8 are PlayStation pads (the only way past Windows' four XInput slots)
    /// would lose them to a preset change nobody connected to personas.
    ///
    /// An EXISTING slot keeps its persona when this is `None`; a slot this
    /// call CREATES gets [`Persona::default`], which is what a hand-written
    /// `[[slot]]` with no `persona` key means and therefore the only answer
    /// that keeps the two spellings of "a new slot" identical.
    pub persona: Option<Persona>,
}

/// What a successful [`assign`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedSlot {
    pub path: PathBuf,
    pub slot: u8,
    /// The preset as the FILE now spells it.
    pub preset: String,
    /// What the slot pointed at before; `None` when the slot was created.
    pub previous: Option<String>,
    /// The persona the slot presents itself as NOW — whether or not this call
    /// changed it, so a surface can render the row without a second read.
    pub persona: Persona,
    /// What it was before, and `None` when this call left the persona alone
    /// (including when the slot was created). The pair, not just the new half:
    /// "xbox360 → playstation" is the sentence, and the new half on its own
    /// reads as an assertion that nothing changed.
    pub previous_persona: Option<Persona>,
    pub profile: Option<String>,
    /// The slot was added rather than repointed.
    pub created: bool,
    /// The slot already used that preset AND that persona, so the file was
    /// left alone.
    pub unchanged: bool,
    /// The timestamped copy taken before the write. `None` when nothing was
    /// written (`unchanged`) or the file did not exist yet.
    pub backup: Option<PathBuf>,
}

impl AppliedSlot {
    /// The one sentence a surface prints. It always names the pad bounce,
    /// because the pad bounce is the part a user must not have to infer.
    pub fn message(&self) -> String {
        let where_ = match &self.profile {
            Some(profile) => format!(" in profile \"{profile}\""),
            None => String::new(),
        };
        if self.unchanged {
            return format!(
                "slot {} already uses \"{}\" as a {} pad{where_} — nothing was written",
                self.slot,
                self.preset,
                self.persona.label(),
            );
        }
        // The persona half is named ONLY when it moved. Appending "— now a
        // PlayStation pad" to every preset re-point would read as a change
        // this call did not make, which is the same lie in the other
        // direction as omitting a change it did.
        let persona_change = match self.previous_persona {
            Some(previous) => format!(
                ", now a {} pad (was {})",
                self.persona.label(),
                previous.label()
            ),
            None => String::new(),
        };
        let change = match &self.previous {
            Some(previous) if *previous == self.preset => format!(
                "slot {}{where_} keeps \"{previous}\"{persona_change}",
                self.slot
            ),
            Some(previous) => format!(
                "slot {}{where_}: \"{previous}\" → \"{}\"{persona_change}",
                self.slot, self.preset
            ),
            None => format!(
                "slot {} added{where_}, using \"{}\" as a {} pad",
                self.slot,
                self.preset,
                self.persona.label()
            ),
        };
        format!("{change} — a slot change needs the pads replugged")
    }
}

/// Why an assignment was refused. Every variant carries what the caller should
/// have said instead — a refusal that lists the presets on disk is the
/// difference between a dead end and a next step.
#[derive(Debug, thiserror::Error)]
pub enum SlotError {
    #[error("slot number must be 1..={MAX_SLOTS}, got {given}")]
    BadSlot { given: u8 },
    #[error(
        "no preset called \"{preset}\"{}",
        list_or_none(available, "presets on disk")
    )]
    UnknownPreset {
        preset: String,
        available: Vec<String>,
    },
    #[error(
        "no games.toml profile called \"{profile}\"{}",
        list_or_none(available, "profiles in games.toml")
    )]
    UnknownProfile {
        profile: String,
        available: Vec<String>,
    },
    /// The persona is a real persona, and this build cannot create it.
    ///
    /// The gap is [`ksx_core::PadBackend::gap`]'s own sentence, not a
    /// paraphrase, so the CLI, the pipe, Studio and `ksx doctor` all say the
    /// identical thing — and so that the day HIDMaestro's shared section gets
    /// a mapper, one `is_implemented` flips and every one of them stops
    /// refusing at once.
    #[error(
        "persona '{persona}' is a persona ksx knows and this build cannot create — {reason}. \
         What decides it is the BINARY, not the machine: `Persona::backend()` sends {persona} to \
         {backend}, and `PadBackend::is_implemented` is false for {backend} in this build, so \
         this refusal is identical on every PC and on this one tomorrow. It changes when a ksx \
         release ships code that can map {backend}'s shared section — nothing you can install \
         moves it. Use persona '{instead}' for now"
    )]
    PersonaNotImplemented {
        persona: Persona,
        backend: &'static str,
        reason: &'static str,
        instead: Persona,
    },
    /// The write would leave the destination with a fifth slot on an XInput
    /// persona. Refused the way `ksx pads` refuses a fifth pad, and for the
    /// same measured reason.
    #[error(
        "slot {slot} would make {after} slots in {destination} use an XInput persona, and Windows \
         exposes only {MAX_XINPUT_SLOTS} XInput slots — the fifth pad plugs and no game reads it \
         (measured on this cabinet, docs/research/m6.5-ds4-findings.md). What decides it is \
         `Persona::is_xinput()`: '{}' and '{}' each take one of the four; '{}' is plain HID and \
         takes none, which is how players 5+ exist at all. This counts the SLOT LIST in that one \
         file — a real Xbox pad plugged into the cabinet takes a slot too, and `ksx pads` is what \
         reads those. Give slot {slot} persona '{}', or move one of the other {after} off XInput",
        Persona::Xbox360,
        Persona::XboxSeries,
        Persona::PlayStation,
        Persona::PlayStation
    )]
    TooManyXinputSlots {
        slot: u8,
        /// How many slots in the destination would be on an XInput persona
        /// once this write landed.
        after: usize,
        /// `config.toml`, or `profile "Steam" (games.toml)`.
        destination: String,
    },
    /// A persona-only write named a slot that is not in the file yet.
    ///
    /// "Keep the preset it uses" has no answer for a slot that uses none, and
    /// the two alternatives are both worse than a refusal: inventing a default
    /// wires a player to a key map nobody chose, and creating the slot with an
    /// empty preset writes a config that refuses to start at the next boot.
    #[error(
        "slot {slot} is not in {destination} yet, so there is no preset for it to keep — \
         name one (`--preset NAME`) and this call will create the slot with it"
    )]
    NoPresetToKeep { slot: u8, destination: String },
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl SlotError {
    /// The stable refusal word — the same one `--json` prints and the pipe
    /// answers with.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadSlot { .. } => ksx_api::codes::BAD_SLOT,
            Self::UnknownPreset { .. } => ksx_api::codes::UNKNOWN_PRESET,
            Self::UnknownProfile { .. } => ksx_api::codes::UNKNOWN_PROFILE,
            // The same word `ksx pads` answers a refused spawn with
            // (`SpawnPlan::code`), because it is the same fact about the same
            // build refusing the same persona. A second spelling would make a
            // surface that routes on the code handle one and not the other.
            Self::PersonaNotImplemented { .. } => "persona-not-implemented",
            Self::TooManyXinputSlots { .. } => "too-many-xinput-slots",
            Self::NoPresetToKeep { .. } => "no-preset-to-keep",
            Self::Config(_) => "config-error",
        }
    }
}

/// `" — presets on disk: a, b, c"`, or a plain "(none)" note when the cabinet
/// genuinely has none. Never an empty tail that reads as if the list was
/// forgotten.
fn list_or_none(available: &[String], what: &str) -> String {
    if available.is_empty() {
        format!(" — there are no {what}")
    } else {
        format!(" — {what}: {}", available.join(", "))
    }
}

/// Point `spec.slot` at `spec.preset`, in `config.toml` or in one games.toml
/// profile.
///
/// Order of checks is deliberate and matches the mapper's: everything that can
/// refuse does so **before** anything is copied or written, so a refusal never
/// leaves a stray backup behind.
pub fn assign(store: &Store, spec: &SlotSpec) -> Result<AppliedSlot, SlotError> {
    if spec.slot == 0 || spec.slot > MAX_SLOTS {
        return Err(SlotError::BadSlot { given: spec.slot });
    }
    // Before the disk is touched at all: a persona this build cannot create is
    // refused here rather than at the next session start, which is where the
    // config validator would catch it — long after whoever typed this walked
    // away, and with four pads that did not appear as the only symptom.
    if let Some(persona) = spec.persona {
        check_pluggable(persona)?;
    }
    // The preset has to EXIST. A slot pointing at a preset that is not there is
    // a cabinet that refuses to start, and it would refuse at the next boot —
    // long after the person who typed this walked away.
    let preset = match &spec.preset {
        Some(wanted) => Some(resolve_preset(store, wanted)?),
        None => None,
    };

    match &spec.profile {
        None => assign_in_config(store, spec.slot, preset.as_deref(), spec.persona),
        Some(profile) => {
            assign_in_profile(store, spec.slot, preset.as_deref(), spec.persona, profile)
        }
    }
}

/// Refuse a persona this BUILD cannot create, in [`ksx_core::PadBackend`]'s own
/// words.
///
/// Reads [`Persona::can_plug`] and nothing else — never a driver probe, for the
/// reason that method's doc comment gives: a probe answers "is HIDMaestro
/// installed", which is a different question whose answer flips to yes while
/// `create_controller` still has nothing to call. A probe-based gate would go
/// quiet exactly in the case it exists for, and send the user back to the
/// driver they had just installed.
fn check_pluggable(persona: Persona) -> Result<(), SlotError> {
    if persona.can_plug() {
        return Ok(());
    }
    let backend = persona.backend();
    Err(SlotError::PersonaNotImplemented {
        persona,
        backend: backend.label(),
        reason: backend
            .gap()
            .unwrap_or("this build ships no code that can create it"),
        instead: persona.nearest_pluggable(),
    })
}

/// Refuse a write that would leave `destination` with a fifth slot on an XInput
/// persona.
///
/// **`after > before` is the whole condition, not `after > ceiling` alone.** A
/// file that is already over the ceiling — hand-edited, or written by a ksx
/// that had not learned to count — must not have every slot write refused: the
/// most likely next act on such a file is moving one of those slots to
/// `playstation`, and a check that only looked at the total would refuse the
/// repair along with the damage. So this refuses the write that MAKES it worse,
/// and leaves `ksx doctor` / the config validator to keep complaining about a
/// state this call did not create.
fn check_xinput_ceiling(
    slot: u8,
    before: usize,
    after: usize,
    destination: String,
) -> Result<(), SlotError> {
    if after > usize::from(MAX_XINPUT_SLOTS) && after > before {
        return Err(SlotError::TooManyXinputSlots {
            slot,
            after,
            destination,
        });
    }
    Ok(())
}

/// The preset's name as the FILE spells it, or the refusal listing what is
/// there. Case-insensitive on the way in, canonical on the way out — so
/// `--preset "ipac p1"` writes `IPAC P1` and the config keeps one spelling.
fn resolve_preset(store: &Store, wanted: &str) -> Result<String, SlotError> {
    let loaded = store.load_presets()?;
    let mut available: Vec<String> = loaded.value.iter().map(|p| p.name.clone()).collect();
    match loaded
        .value
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(wanted))
    {
        Some(found) => Ok(found.name.clone()),
        None => {
            available.sort_by_key(|name| name.to_ascii_lowercase());
            Err(SlotError::UnknownPreset {
                preset: wanted.to_owned(),
                available,
            })
        }
    }
}

fn assign_in_config(
    store: &Store,
    slot: u8,
    preset: Option<&str>,
    persona: Option<Persona>,
) -> Result<AppliedSlot, SlotError> {
    let mut config = store.load_config()?.value;
    let before_xinput = xinput_count(config.slots.iter().map(|s| s.persona));
    let existing = config.slots.iter_mut().find(|s| s.number == slot);
    let (previous, previous_persona, preset, now, created) = match existing {
        Some(entry) => {
            // `None` means "not asked about" here too: the slot keeps the
            // preset it has, which is what makes a persona-only write possible.
            let preset = preset.unwrap_or(&entry.preset).to_owned();
            let same_preset = entry.preset.eq_ignore_ascii_case(&preset);
            // `None` means "not asked about", so it can never make a write
            // necessary — the slot's own persona is the answer.
            let wanted = persona.unwrap_or(entry.persona);
            if same_preset && wanted == entry.persona {
                return Ok(AppliedSlot {
                    path: store.root().config_path(),
                    slot,
                    preset: entry.preset.clone(),
                    previous: Some(entry.preset.clone()),
                    persona: entry.persona,
                    previous_persona: None,
                    profile: None,
                    created: false,
                    unchanged: true,
                    backup: None,
                });
            }
            let previous = std::mem::replace(&mut entry.preset, preset.clone());
            let was = std::mem::replace(&mut entry.persona, wanted);
            (
                Some(previous),
                (was != wanted).then_some(was),
                preset,
                wanted,
                false,
            )
        }
        None => {
            let Some(preset) = preset else {
                return Err(SlotError::NoPresetToKeep {
                    slot,
                    destination: "config.toml".to_owned(),
                });
            };
            // A brand-new slot inherits nothing: no keyboard (so it accepts
            // any), the default persona unless one was asked for, the default
            // SOCD and macro switch. Wiring a DEVICE to it is `ksx setup`'s
            // job — it identifies the board by press, which is the only honest
            // way to do it and is deliberately not something a preset picker
            // can guess.
            let wanted = persona.unwrap_or_default();
            config.slots.push(SlotEntry {
                number: slot,
                keyboard: None,
                mouse: None,
                preset: preset.to_owned(),
                persona: wanted,
                socd: Default::default(),
                macros: Default::default(),
            });
            config.slots.sort_by_key(|s| s.number);
            // No `previous_persona`: a slot that did not exist did not present
            // itself as anything, and "xbox360 → xbox360" is a change that
            // never happened.
            (None, None, preset.to_owned(), wanted, true)
        }
    };

    // After the edit, before the write — the whole file's answer, counted from
    // the same `is_xinput` flag the validator and `ksx pads` count from.
    // Nothing has touched the disk yet, so a refusal here leaves no backup and
    // no half-written config.
    check_xinput_ceiling(
        slot,
        before_xinput,
        xinput_count(config.slots.iter().map(|s| s.persona)),
        "config.toml".to_owned(),
    )?;

    let path = store.root().config_path();
    let backup = store.backup(&path)?;
    let written = store.save_config(&config)?;
    Ok(AppliedSlot {
        path: written,
        slot,
        preset,
        previous,
        persona: now,
        previous_persona,
        profile: None,
        created,
        unchanged: false,
        backup,
    })
}

/// How many of these slots occupy one of Windows' four XInput slots.
///
/// One helper for both destinations, reading [`Persona::is_xinput`] — the same
/// flag `ksx-config::validate` counts and `ksx pads` plans against. Three
/// callers, one rule: a second `matches!(p, Xbox360 | XboxSeries)` written here
/// would be a fourth persona's chance to be forgotten.
fn xinput_count(personas: impl Iterator<Item = Persona>) -> usize {
    personas.filter(|p| p.is_xinput()).count()
}

fn assign_in_profile(
    store: &Store,
    slot: u8,
    preset: Option<&str>,
    persona: Option<Persona>,
    profile: &str,
) -> Result<AppliedSlot, SlotError> {
    let mut games = store.load_games()?.value;
    let titles: Vec<String> = games.games.iter().map(|g| g.title.clone()).collect();
    let Some(game) = games
        .games
        .iter_mut()
        .find(|g| g.title.eq_ignore_ascii_case(profile))
    else {
        return Err(SlotError::UnknownProfile {
            profile: profile.to_owned(),
            available: titles,
        });
    };
    let title = game.title.clone();
    // ONE PROFILE's slot list, not every profile's. Two games.toml profiles
    // never run at once — switching profiles stops the session and starts
    // another — so four XInput slots in each is four at a time, and counting
    // the file whole would refuse a perfectly ordinary second profile. Same
    // scope `ksx-config::validate_games` uses, per `[[game]]`.
    let before_xinput = xinput_count(game.slots.iter().map(|s| s.persona));

    let existing = game.slots.iter_mut().find(|s| s.number == slot);
    let (previous, previous_persona, preset, now, created) = match existing {
        Some(entry) => {
            let preset = preset.unwrap_or(&entry.preset).to_owned();
            let same_preset = entry.preset.eq_ignore_ascii_case(&preset);
            let wanted = persona.unwrap_or(entry.persona);
            if same_preset && wanted == entry.persona {
                return Ok(AppliedSlot {
                    path: store.root().games_path(),
                    slot,
                    preset: entry.preset.clone(),
                    previous: Some(entry.preset.clone()),
                    persona: entry.persona,
                    previous_persona: None,
                    profile: Some(title),
                    created: false,
                    unchanged: true,
                    backup: None,
                });
            }
            let previous = std::mem::replace(&mut entry.preset, preset.clone());
            let was = std::mem::replace(&mut entry.persona, wanted);
            (
                Some(previous),
                (was != wanted).then_some(was),
                preset,
                wanted,
                false,
            )
        }
        None => {
            let Some(preset) = preset else {
                return Err(SlotError::NoPresetToKeep {
                    slot,
                    destination: format!("profile \"{title}\" (games.toml)"),
                });
            };
            let wanted = persona.unwrap_or_default();
            game.slots.push(GameSlotEntry {
                number: slot,
                // Advisory only (the real XInput index comes from ViGEm's
                // callback), and `ksx setup` writes it the same way.
                user_index: Some(slot),
                keyboard: None,
                mouse: None,
                preset: preset.to_owned(),
                persona: wanted,
                socd: Default::default(),
                macros: Default::default(),
            });
            game.slots.sort_by_key(|s| s.number);
            (None, None, preset.to_owned(), wanted, true)
        }
    };
    let after_xinput = xinput_count(game.slots.iter().map(|s| s.persona));
    check_xinput_ceiling(
        slot,
        before_xinput,
        after_xinput,
        format!("profile \"{title}\" (games.toml)"),
    )?;

    let path = store.root().games_path();
    let backup = store.backup(&path)?;
    let written = store.save_games(&games)?;
    Ok(AppliedSlot {
        path: written,
        slot,
        preset,
        previous,
        persona: now,
        previous_persona,
        profile: Some(title),
        created,
        unchanged: false,
        backup,
    })
}

/// One slot as a picker shows it: the number, the preset it uses, and where
/// that wiring is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRow {
    pub slot: u8,
    pub preset: String,
    /// `None` = config.toml.
    pub profile: Option<String>,
    /// The keyboard alias or id, `"(any)"` when unassigned — the same wording
    /// the mapper's slot strip uses.
    pub keyboard: String,
    /// What this slot presents itself as. Always populated: a file with no
    /// `persona` key means [`Persona::default`], which is a fact about the
    /// slot and not an absence, so there is nothing here to render as "(none)".
    pub persona: Persona,
}

/// Every slot of one destination, in slot order. The read side of this verb,
/// and daemon-free like every other read.
pub fn list(store: &Store, profile: Option<&str>) -> Result<Vec<SlotRow>, SlotError> {
    match profile {
        None => Ok(store
            .load_config()?
            .value
            .slots
            .iter()
            .map(|s| SlotRow {
                slot: s.number,
                preset: s.preset.clone(),
                profile: None,
                keyboard: s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
                persona: s.persona,
            })
            .collect()),
        Some(profile) => {
            let games = store.load_games()?.value;
            let titles: Vec<String> = games.games.iter().map(|g| g.title.clone()).collect();
            let Some(game) = games
                .games
                .iter()
                .find(|g| g.title.eq_ignore_ascii_case(profile))
            else {
                return Err(SlotError::UnknownProfile {
                    profile: profile.to_owned(),
                    available: titles,
                });
            };
            Ok(game
                .slots
                .iter()
                .map(|s| SlotRow {
                    slot: s.number,
                    preset: s.preset.clone(),
                    profile: Some(game.title.clone()),
                    keyboard: s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
                    persona: s.persona,
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::{ConfigFile, ConfigRoot, GamesFile, PresetFile};

    /// A throwaway config root. Same shape as `mapping.rs`'s, so the two
    /// writers' tests read the same way.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-slots-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// An empty config, an empty games file and two presets — the smallest
        /// cabinet an assignment can be made on.
        fn store(&self) -> Store {
            let store = Store::new(ConfigRoot::at(&self.0));
            store.save_config(&ConfigFile::default()).unwrap();
            store.save_games(&GamesFile::default()).unwrap();
            for name in ["IPAC P1", "IPAC P2"] {
                let file: PresetFile =
                    toml::from_str(&format!("name = \"{name}\"\n[bindings]\nA = \"S\"\n")).unwrap();
                store.save_preset(&file).unwrap();
            }
            store
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn one_profile(store: &Store) {
        let file: GamesFile =
            toml::from_str("[[game]]\ntitle = \"Steam\"\npath = 'C:\\steam.exe'\n").unwrap();
        store.save_games(&file).unwrap();
    }

    fn spec(slot: u8, preset: &str) -> SlotSpec {
        SlotSpec {
            slot,
            preset: Some(preset.to_owned()),
            persona: None,
            profile: None,
        }
    }

    /// A persona-only write: no preset named at all, which is the shape
    /// `ksx slot assign --slot N --persona P` produces.
    fn persona_spec(slot: u8, persona: Persona) -> SlotSpec {
        SlotSpec {
            slot,
            preset: None,
            persona: Some(persona),
            profile: None,
        }
    }

    /// The happy path in config.toml — and the fact the message leads with:
    /// the pads replug.
    #[test]
    fn assigning_a_slot_writes_config_toml_and_says_the_pads_bounce() {
        let root = TempRoot::new("assign");
        let store = root.store();

        let applied = assign(&store, &spec(1, "IPAC P1")).expect("a preset that exists");
        assert!(applied.created, "slot 1 did not exist yet");
        assert_eq!(applied.previous, None);
        assert!(
            applied.message().contains("pads replugged"),
            "the bounce must never be something a user has to infer: {}",
            applied.message()
        );

        let config = store.load_config().unwrap().value;
        assert_eq!(config.slots.len(), 1);
        assert_eq!(config.slots[0].preset, "IPAC P1");

        // Repointing reports BOTH halves — what it was and what it is.
        let moved = assign(&store, &spec(1, "IPAC P2")).unwrap();
        assert_eq!(moved.previous.as_deref(), Some("IPAC P1"));
        assert!(!moved.created);
        assert!(moved.backup.is_some(), "a whole-file write backs up first");
        assert!(moved.message().contains("\"IPAC P1\" → \"IPAC P2\""));
    }

    /// Re-asserting the state a slot is already in is success and a NO-OP —
    /// nothing written, no backup, and the message says so rather than letting
    /// someone wait for a pad bounce that is never coming.
    #[test]
    fn assigning_the_preset_a_slot_already_uses_writes_nothing() {
        let root = TempRoot::new("noop");
        let store = root.store();
        assign(&store, &spec(1, "IPAC P1")).unwrap();
        let again = assign(&store, &spec(1, "IPAC P1")).unwrap();
        assert!(again.unchanged);
        assert!(again.backup.is_none(), "nothing was overwritten");
        assert!(again.message().contains("nothing was written"));
    }

    /// A preset name is matched case-insensitively and stored in the FILE's
    /// spelling, so a config never grows two spellings of one preset.
    #[test]
    fn the_preset_is_written_in_the_spelling_the_file_uses() {
        let root = TempRoot::new("spelling");
        let store = root.store();
        let applied = assign(&store, &spec(2, "ipac p2")).unwrap();
        assert_eq!(applied.preset, "IPAC P2");
    }

    /// A slot pointing at a preset that is not there is a cabinet that refuses
    /// to start — at the next boot, long after whoever typed this walked away.
    /// So it is refused now, with the list.
    #[test]
    fn a_preset_that_does_not_exist_is_refused_with_the_ones_that_do() {
        let root = TempRoot::new("unknown-preset");
        let store = root.store();
        let err = assign(&store, &spec(1, "Nope")).unwrap_err();
        assert_eq!(err.code(), ksx_api::codes::UNKNOWN_PRESET);
        let message = err.to_string();
        assert!(
            message.contains("IPAC P1") && message.contains("IPAC P2"),
            "{message}"
        );
        assert!(
            store.load_config().unwrap().value.slots.is_empty(),
            "a refusal writes nothing"
        );
    }

    #[test]
    fn a_slot_number_off_the_end_is_refused_before_anything_is_read() {
        let root = TempRoot::new("bad-slot");
        let store = root.store();
        for slot in [0, MAX_SLOTS + 1] {
            let err = assign(&store, &spec(slot, "IPAC P1")).unwrap_err();
            assert_eq!(err.code(), ksx_api::codes::BAD_SLOT);
        }
    }

    /// The games.toml half: the same write, in one profile, and it never
    /// touches config.toml.
    #[test]
    fn assigning_inside_a_profile_writes_only_that_profile() {
        let root = TempRoot::new("profile");
        let store = root.store();
        one_profile(&store);

        let applied = assign(
            &store,
            &SlotSpec {
                slot: 3,
                preset: Some("IPAC P1".into()),
                persona: None,
                // Title matching is case-insensitive; the FILE's spelling wins.
                profile: Some("steam".into()),
            },
        )
        .unwrap();
        assert_eq!(applied.profile.as_deref(), Some("Steam"));
        assert!(applied.created);
        assert!(applied.message().contains("profile \"Steam\""));

        let games = store.load_games().unwrap().value;
        assert_eq!(games.games[0].slots.len(), 1);
        assert_eq!(games.games[0].slots[0].preset, "IPAC P1");
        assert!(
            store.load_config().unwrap().value.slots.is_empty(),
            "a profile write must not touch config.toml"
        );
    }

    #[test]
    fn an_unknown_profile_is_refused_with_the_titles_that_exist() {
        let root = TempRoot::new("unknown-profile");
        let store = root.store();
        one_profile(&store);
        let err = assign(
            &store,
            &SlotSpec {
                slot: 1,
                preset: Some("IPAC P1".into()),
                persona: None,
                profile: Some("MAME".into()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), ksx_api::codes::UNKNOWN_PROFILE);
        assert!(err.to_string().contains("Steam"), "{err}");
    }

    /// The read side a picker uses: slot order, whatever order they were
    /// written in.
    #[test]
    fn list_reports_the_slots_of_one_destination_in_order() {
        let root = TempRoot::new("list");
        let store = root.store();
        assign(&store, &spec(2, "IPAC P2")).unwrap();
        assign(&store, &spec(1, "IPAC P1")).unwrap();
        let rows = list(&store, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].slot, 1);
        assert_eq!(rows[0].preset, "IPAC P1");
        assert_eq!(rows[0].keyboard, "(any)");
        assert_eq!(rows[0].persona, Persona::Xbox360, "the file said nothing");
        assert_eq!(rows[1].slot, 2);
    }

    // ── The persona half (task #8) ────────────────────────────────────────

    /// **The bug this whole optionality exists for.**
    ///
    /// Breaks against: `SlotSpec { persona: Persona }` — any shape where "not
    /// asked about" and "xbox360" are the same value. `Persona::default()` is
    /// `Xbox360`, so a non-optional field turns every ordinary preset re-point
    /// into a silent re-personaing, and a cabinet's slots 5–8 (the PlayStation
    /// pads that are the only way past Windows' four XInput slots) would go
    /// back to Xbox the next time somebody changed a key map. Nothing in the
    /// output would say so; the pads would just stop being readable.
    #[test]
    fn a_preset_repoint_leaves_the_persona_alone() {
        let root = TempRoot::new("keeps-persona");
        let store = root.store();
        assign(&store, &spec(5, "IPAC P1")).unwrap();
        assign(&store, &persona_spec(5, Persona::PlayStation)).unwrap();

        // A preset change, with no persona named at all.
        let applied = assign(&store, &spec(5, "IPAC P2")).unwrap();
        assert_eq!(applied.persona, Persona::PlayStation, "not re-personaed");
        assert_eq!(applied.previous_persona, None, "nothing to report");
        assert_eq!(
            store.load_config().unwrap().value.slots[0].persona,
            Persona::PlayStation
        );
        // ...and the sentence does not claim a persona change either.
        assert!(
            !applied.message().contains("now a"),
            "{}",
            applied.message()
        );
    }

    /// A persona-only write: no preset named, the slot keeps the one it has,
    /// and the sentence names BOTH ends of the change.
    ///
    /// Breaks against: a `preset: String` that forces a caller to restate the
    /// preset. That version cannot express `ksx slot assign --slot 5 --persona
    /// playstation` at all, and a surface that filled the preset in from its
    /// last read would write back a name the file may have moved on from.
    #[test]
    fn a_persona_only_write_keeps_the_preset_and_names_both_ends() {
        let root = TempRoot::new("persona-only");
        let store = root.store();
        assign(&store, &spec(5, "IPAC P2")).unwrap();

        let applied = assign(&store, &persona_spec(5, Persona::PlayStation)).unwrap();
        assert_eq!(applied.preset, "IPAC P2", "the preset it already used");
        assert_eq!(applied.persona, Persona::PlayStation);
        assert_eq!(applied.previous_persona, Some(Persona::Xbox360));
        assert!(!applied.created);
        assert!(!applied.unchanged);
        let message = applied.message();
        assert!(message.contains("PlayStation"), "{message}");
        assert!(message.contains("was Xbox 360"), "{message}");
        assert!(message.contains("pads replugged"), "{message}");

        // The FILE, in the one spelling ksx-config writes.
        let text = std::fs::read_to_string(store.root().config_path()).unwrap();
        assert!(text.contains("persona = \"playstation\""), "{text}");
    }

    /// Re-asserting the persona a slot already has is a no-op, exactly like
    /// re-asserting its preset — no write, no backup, no promised pad bounce.
    #[test]
    fn asking_for_the_persona_a_slot_already_has_writes_nothing() {
        let root = TempRoot::new("persona-noop");
        let store = root.store();
        assign(&store, &spec(1, "IPAC P1")).unwrap();
        let again = assign(&store, &persona_spec(1, Persona::Xbox360)).unwrap();
        assert!(again.unchanged);
        assert!(again.backup.is_none());
        assert!(again.message().contains("nothing was written"));
        assert!(again.message().contains("Xbox 360"), "{}", again.message());
    }

    /// A slot that does not exist has no preset to keep, and no honest guess
    /// exists — so a persona-only write on one is refused rather than
    /// inventing a key map for a player nobody set up.
    ///
    /// Breaks against: a writer that creates the slot with an empty preset
    /// (a config that refuses to start at the next boot) or with some
    /// arbitrary first-preset-on-disk.
    #[test]
    fn a_persona_only_write_on_a_slot_that_does_not_exist_is_refused() {
        let root = TempRoot::new("no-preset-to-keep");
        let store = root.store();
        let err = assign(&store, &persona_spec(7, Persona::PlayStation)).unwrap_err();
        assert_eq!(err.code(), "no-preset-to-keep");
        assert!(err.to_string().contains("--preset"), "{err}");
        assert!(
            store.load_config().unwrap().value.slots.is_empty(),
            "a refusal writes nothing"
        );
    }

    /// The three HIDMaestro personas are refused BEFORE the disk is touched,
    /// in `PadBackend::gap()`'s own words — including the sentence that closes
    /// off the wrong fix, because "not installed" is the reading that costs a
    /// pointless driver install.
    ///
    /// Breaks against: a writer that accepts them (the config then refuses to
    /// start at the next boot, with four missing pads as the only symptom) and
    /// against any refusal that paraphrases the gap instead of quoting it.
    #[test]
    fn a_persona_this_build_cannot_plug_is_refused_with_the_reason_and_a_way_out() {
        let root = TempRoot::new("cannot-plug");
        let store = root.store();
        assign(&store, &spec(1, "IPAC P1")).unwrap();

        for (persona, instead) in [
            (Persona::DualSense, "playstation"),
            (Persona::SwitchPro, "xbox360"),
            (Persona::XboxSeries, "xbox360"),
        ] {
            let err = assign(&store, &persona_spec(1, persona)).unwrap_err();
            assert_eq!(err.code(), "persona-not-implemented", "{persona}");
            let message = err.to_string();
            // The gap, verbatim — the half that stops a wasted driver install.
            assert!(
                message.contains("installing HIDMaestro does not change it"),
                "{message}"
            );
            // What DECIDES it: the binary, not this machine.
            assert!(message.contains("BINARY"), "{message}");
            // ...and a way out that itself works.
            assert!(message.contains(instead), "{message}");
        }
        // Nothing was written by any of them.
        assert_eq!(
            store.load_config().unwrap().value.slots[0].persona,
            Persona::Xbox360
        );
    }

    /// A fifth XInput slot is refused the way `ksx pads` refuses a fifth pad.
    ///
    /// Breaks against: a writer with no ceiling check. Without it a cabinet
    /// ends up with five `xbox360` slots, ksx starts, four pads appear, and the
    /// fifth player's panel does nothing — with the config validator's warning
    /// the only clue, at the next boot.
    #[test]
    fn a_fifth_xinput_slot_is_refused_naming_what_decides_it() {
        let root = TempRoot::new("fifth-xinput");
        let store = root.store();
        for slot in 1..=4 {
            assign(&store, &spec(slot, "IPAC P1")).unwrap();
        }
        // Four Xbox slots is fine; the fifth is not.
        let err = assign(&store, &spec(5, "IPAC P2")).unwrap_err();
        assert_eq!(err.code(), "too-many-xinput-slots");
        let message = err.to_string();
        assert!(message.contains("is_xinput"), "names the rule: {message}");
        assert!(message.contains("playstation"), "{message}");
        assert!(
            message.contains("a real Xbox pad plugged into the cabinet takes a slot too"),
            "the count is of the FILE, and must not be read as a machine reading: {message}"
        );
        assert_eq!(
            store.load_config().unwrap().value.slots.len(),
            4,
            "a refusal writes nothing"
        );

        // The same slot as a PlayStation pad is exactly how players 5+ exist.
        let fifth = assign(
            &store,
            &SlotSpec {
                slot: 5,
                preset: Some("IPAC P2".into()),
                persona: Some(Persona::PlayStation),
                profile: None,
            },
        )
        .unwrap();
        assert_eq!(fifth.persona, Persona::PlayStation);
        assert!(fifth.created);
    }

    /// Xbox Series counts against the four as well — it is HID, but its GIP
    /// companion maps it onto an XInput slot, so five of THOSE is the same
    /// refusal. Written as its own case because "it's not an Xbox 360" is the
    /// intuition that would wave it through.
    ///
    /// Breaks against: a ceiling that counted `Persona::Xbox360` instead of
    /// `Persona::is_xinput()`.
    #[test]
    fn the_ceiling_counts_is_xinput_not_the_xbox360_variant() {
        // Stated against ksx-core rather than by writing five xboxseries slots
        // — this build refuses to WRITE an xboxseries slot at all
        // (`can_plug` is false), so the ceiling cannot be reached that way
        // today. What must not drift is the flag the count reads.
        assert!(Persona::XboxSeries.is_xinput());
        assert_eq!(
            xinput_count([Persona::Xbox360, Persona::XboxSeries, Persona::PlayStation].into_iter()),
            2
        );
    }

    /// A file that is ALREADY over the ceiling must not have every write
    /// refused — the most likely next act on one is the repair.
    ///
    /// Breaks against: `if after > MAX_XINPUT_SLOTS` with no `after > before`.
    /// That version refuses the very write that fixes the file, and leaves a
    /// hand-edited config with no way out but another hand edit.
    #[test]
    fn a_file_already_over_the_ceiling_can_still_be_repaired() {
        let root = TempRoot::new("repair");
        let store = root.store();
        // Five Xbox slots, written the way a hand edit or an older ksx would.
        let mut config = ksx_config::ConfigFile::default();
        for number in 1..=5 {
            config.slots.push(SlotEntry {
                number,
                keyboard: None,
                mouse: None,
                preset: "IPAC P1".into(),
                persona: Persona::Xbox360,
                socd: Default::default(),
                macros: Default::default(),
            });
        }
        store.save_config(&config).unwrap();

        // The repair: move slot 5 to PlayStation. 5 → 4, so it lands.
        let fixed = assign(&store, &persona_spec(5, Persona::PlayStation)).unwrap();
        assert_eq!(fixed.persona, Persona::PlayStation);

        // And a write that neither adds nor removes an XInput slot is not
        // blocked by a state it did not create.
        store.save_config(&config).unwrap();
        assert!(assign(&store, &spec(3, "IPAC P2")).is_ok());
    }

    /// The profile half: the ceiling is ONE profile's slot list, because two
    /// games.toml profiles never run at once.
    ///
    /// Breaks against: a count over every `[[game]]` in the file, which would
    /// refuse a perfectly ordinary second four-player profile.
    #[test]
    fn the_xinput_ceiling_is_counted_per_profile_not_per_file() {
        let root = TempRoot::new("per-profile");
        let store = root.store();
        let games: ksx_config::GamesFile = toml::from_str(
            "[[game]]\ntitle = \"Steam\"\npath = 'C:\\steam.exe'\n\
             [[game]]\ntitle = \"MAME\"\npath = 'C:\\mame.exe'\n",
        )
        .unwrap();
        store.save_games(&games).unwrap();

        for title in ["Steam", "MAME"] {
            for slot in 1..=4 {
                assign(
                    &store,
                    &SlotSpec {
                        slot,
                        preset: Some("IPAC P1".into()),
                        persona: None,
                        profile: Some(title.to_owned()),
                    },
                )
                .expect("four Xbox slots per profile is four at a time");
            }
        }

        // Eight XInput slots across the file, and neither profile is over.
        // The fifth in ONE profile still is.
        let err = assign(
            &store,
            &SlotSpec {
                slot: 5,
                preset: Some("IPAC P2".into()),
                persona: None,
                profile: Some("MAME".into()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "too-many-xinput-slots");
        assert!(err.to_string().contains("MAME"), "{err}");
    }

    /// A new slot created WITH a persona gets it, and reports no previous —
    /// a slot that did not exist did not present itself as anything.
    #[test]
    fn a_slot_created_with_a_persona_reports_no_previous_one() {
        let root = TempRoot::new("created-with");
        let store = root.store();
        let applied = assign(
            &store,
            &SlotSpec {
                slot: 6,
                preset: Some("IPAC P1".into()),
                persona: Some(Persona::PlayStation),
                profile: None,
            },
        )
        .unwrap();
        assert!(applied.created);
        assert_eq!(applied.persona, Persona::PlayStation);
        assert_eq!(applied.previous_persona, None, "xbox360 -> was never true");
        assert!(applied.message().contains("PlayStation"));
    }
}
