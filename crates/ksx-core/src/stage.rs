//! The STAGED SETUP — a setup the user is still deciding on
//! (`docs/FIRST-RUN.md` §2).
//!
//! # Why this is a type and not a UI trick
//!
//! Before this module, choosing a persona *was* a `[[slot]]` written to
//! `config.toml`, and a pad appeared when a session started. "Pick PS4, look at
//! it, change to Xbox 360" was three file writes and two timestamped backups —
//! for a decision the user had not made yet. Deleting the controller they never
//! wanted left a fourth write and a backup behind it.
//!
//! A staged setup is its own value and **never touches disk**. It holds the
//! chosen device, the chosen persona per slot, the bindings so far, and the
//! blocking answer. It lives in the daemon for the length of a visit. Nothing
//! is claimed, nothing is plugged, no config file is written, until the user
//! says so — and even then, [`StagedSetup::commit`] (save) and the play path
//! are separate acts.
//!
//! The consequences §2 makes requirements, and where each one is:
//!
//! - deleting a staged controller is free and complete — [`StagedSetup::remove_slot`];
//! - "Start over" always works — [`StagedSetup::discard`], which cannot refuse;
//! - leaving without saving loses only what was typed — nothing here writes;
//! - **what is staged is what plays** — [`StagedSetup::commit`] produces ONE
//!   [`CommitSpec`], and the save path and the play path are both built from
//!   that one value. There is no second translation in which a saved file could
//!   mean something different from what the screen showed. `ksx-app`'s
//!   `crate::stage` pins that with a test.
//!
//! # Validation is reused, never restated
//!
//! Every rule here already existed somewhere and is *read*, not re-derived:
//!
//! - [`Persona::can_plug`] gates the three HIDMaestro personas, and the refusal
//!   quotes [`crate::PadBackend::gap`] verbatim — the same sentence
//!   `ksx slot assign` prints, so the day HIDMaestro's shared section gets a
//!   mapper, one `is_implemented` flips and both stop refusing at once;
//! - [`MAX_XINPUT_SLOTS`] caps the personas that occupy one of Windows' four
//!   XInput slots, counted through [`Persona::is_xinput`] — which is why
//!   `playstation` is plain HID, takes none, and is how players 5+ exist;
//! - [`MAX_SLOTS`] caps the total, through [`SlotSpec::new`]'s own check.
//!
//! Refusals name what would make the choice legal. A refusal with no way
//! forward is just an error message.

use crate::blocking::Blocking;
use crate::engine::ResolvedSlot;
use crate::persona::Persona;
use crate::preset::Preset;
use crate::selector::DeviceSelector;
use crate::slot::{SlotSpec, MAX_SLOTS, MAX_XINPUT_SLOTS};

/// The input device a staged setup is built on.
///
/// **A [`DeviceSelector`], never a raw path** (`docs/DEVICE-IDENTITY.md` §1):
/// what is staged is *which board*, so the setup keeps meaning the same thing
/// after the board is replugged into another socket. The raw instance path a
/// scan happened to see is not stored here at all, which is what makes it
/// impossible for it to leak onto a screen as the identifier
/// (`docs/FIRST-RUN.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedDevice {
    /// Which board — the weakest rung that is still unique, as
    /// `ksx device pick` computes it.
    pub selector: DeviceSelector,
    /// The short name `[[slot]]` rows will refer to it by once this is saved
    /// ("panel", "P1"). Never contains a backslash: everywhere else in the
    /// config a value with one is read as a literal device path, so such an
    /// alias could never resolve back to its own entry.
    pub alias: String,
    /// What a human calls it on screen — "Ultimarc I-PAC 4", "Logitech
    /// keyboard". Presentation only; nothing resolves against it.
    pub label: String,
}

/// One staged controller: the slot number, what it presents itself as, and the
/// bindings so far.
///
/// The bindings are a whole [`Preset`] because that is the unit the rest of ksx
/// already speaks — the engine takes one, the file format is one, and a staged
/// slot that held some smaller edit shape would need a translation into a
/// preset at save time, which is the second translation §2 forbids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedSlot {
    /// 1..=[`MAX_SLOTS`].
    pub number: u8,
    /// What this slot presents itself as. Gated by [`Persona::can_plug`].
    pub persona: Persona,
    /// The bindings accumulated so far, carrying their own name — which is the
    /// name of the preset file a save will write.
    pub preset: Preset,
}

/// A setup being explored. In memory only; **nothing here can write**.
///
/// Every mutating operation takes `&self` and returns a *new* validated value.
/// That is not a style preference: it makes "a refusal changes nothing" a fact
/// about the type rather than a discipline the implementation has to keep, so a
/// user who is told "no" is provably left holding exactly what they had.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StagedSetup {
    device: Option<StagedDevice>,
    /// Kept sorted by slot number.
    slots: Vec<StagedSlot>,
    /// `None` = **not asked yet**, which is a different fact from "the user
    /// chose the default" and a surface must be able to tell them apart:
    /// `docs/FIRST-RUN.md` §3 asks the split-or-freeze question once, and a
    /// screen that renders "Freeze" pre-selected has answered it for them.
    blocking: Option<Blocking>,
}

/// Why a staging operation was refused. Nothing is ever half-applied: the
/// caller still holds the setup it passed in.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StageRefusal {
    #[error("slot number must be 1..={MAX_SLOTS}, got {given}")]
    BadSlot { given: u8 },
    #[error("slot {number} is already staged")]
    SlotTaken { number: u8 },
    #[error("slot {number} is not staged")]
    NoSuchSlot { number: u8 },
    #[error(
        "all {MAX_SLOTS} slots are already staged — {MAX_SLOTS} is ksx's own ceiling \
         (ksx_core::MAX_SLOTS), not a Windows one"
    )]
    NoFreeSlot,
    /// The persona is a real persona, and this build cannot create it. Worded
    /// from [`crate::PadBackend::gap`] so this refusal and `ksx slot assign`'s
    /// say the identical thing.
    #[error(
        "persona '{persona}' is a persona ksx knows and this build cannot create — {reason}. \
         What decides it is the BINARY, not the machine: `Persona::backend()` sends {persona} to \
         {backend}, and `PadBackend::is_implemented` is false for {backend} in this build. \
         Stage persona '{instead}' instead"
    )]
    PersonaNotImplemented {
        persona: Persona,
        backend: &'static str,
        reason: &'static str,
        instead: Persona,
    },
    /// Staging this would leave a fifth slot on an XInput persona.
    #[error(
        "that would make {after} staged slots use an XInput persona, and Windows exposes only \
         {MAX_XINPUT_SLOTS} XInput slots — the fifth pad plugs and no game reads it (measured, \
         docs/research/m6.5-ds4-findings.md). What decides it is `Persona::is_xinput()`: '{}' and \
         '{}' each take one of the four; '{}' is plain HID and takes none, which is how players \
         5+ exist at all. Give slot {number} persona '{}', or move one of the other {after} off \
         XInput. Nothing has been written, so changing your mind costs nothing",
        Persona::Xbox360,
        Persona::XboxSeries,
        Persona::PlayStation,
        Persona::PlayStation
    )]
    TooManyXinputSlots {
        number: u8,
        /// How many staged slots would be on an XInput persona.
        after: usize,
    },
    /// An alias that could never resolve back to its own `[[device]]` entry.
    #[error("\"{alias}\" cannot name this device: {problem}")]
    BadAlias {
        alias: String,
        problem: &'static str,
    },
    /// Two staged slots hold presets that share a NAME but not a BODY.
    ///
    /// One name is one preset file, so saving these would collapse them into
    /// one and the setup that came back off disk would not be the one on
    /// screen. That is exactly the second translation §2 forbids, caught while
    /// it is still free to fix.
    #[error(
        "slots {number} and {other} both stage a preset called \"{name}\", with different \
         bindings — one name is one preset file, so saving would keep one set of bindings and \
         silently lose the other. Rename one of them"
    )]
    PresetNameClash { number: u8, other: u8, name: String },
    #[error("a preset needs a name — it is what the saved file is called, and what a [[slot]] refers to")]
    UnnamedPreset { number: u8 },
    /// Committing (or planning) with no device chosen.
    #[error(
        "no keyboard has been chosen yet, so there is nothing for these {slots} slot(s) to \
         listen to — pick a device first"
    )]
    NoDevice { slots: usize },
    /// Committing (or planning) with no slots.
    #[error(
        "no controller has been staged yet — a setup with a keyboard and no controller drives \
         nothing. Add one and choose what it should become"
    )]
    NoSlots,
}

impl StageRefusal {
    /// The stable refusal word. The three that name a rule shared with
    /// `ksx slot assign` deliberately reuse ITS word: a surface that routes on
    /// the code must handle one fact once, not once per verb that states it.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadSlot { .. } => "bad-slot",
            Self::SlotTaken { .. } => "slot-taken",
            Self::NoSuchSlot { .. } => "no-such-slot",
            Self::NoFreeSlot => "no-free-slot",
            Self::PersonaNotImplemented { .. } => "persona-not-implemented",
            Self::TooManyXinputSlots { .. } => "too-many-xinput-slots",
            Self::BadAlias { .. } => "bad-alias",
            Self::PresetNameClash { .. } => "preset-name-clash",
            Self::UnnamedPreset { .. } => "unnamed-preset",
            Self::NoDevice { .. } => "no-device",
            Self::NoSlots => "no-slots",
        }
    }
}

/// Everything a finished staging means, as one typed value.
///
/// **The single source both paths read.** `ksx-app`'s `crate::stage` turns this
/// into the `ConfigFile` a save writes AND into the `RunPlan` a play runs, with
/// one translation used by both — which is what makes "what is staged is what
/// plays" a structural fact instead of a promise.
///
/// [`Self::slots`] is [`ResolvedSlot`], the very type `RunPlan::slots` holds:
/// a slot spec with its preset already beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSpec {
    pub device: StagedDevice,
    /// Slot order. Every one names [`Self::device`] as its keyboard.
    pub slots: Vec<ResolvedSlot>,
    /// How much of the keyboard a session takes away from Windows. Falls back
    /// to [`Blocking::default`] when the user was never asked — which is what
    /// an omitted `block_keyboards` key means on disk, so the fallback and the
    /// file agree.
    pub blocking: Blocking,
}

impl StagedSetup {
    /// An empty stage: no device, no controllers, nothing asked.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn device(&self) -> Option<&StagedDevice> {
        self.device.as_ref()
    }

    /// The staged controllers, in slot order.
    pub fn slots(&self) -> &[StagedSlot] {
        &self.slots
    }

    pub fn slot(&self, number: u8) -> Option<&StagedSlot> {
        self.slots.iter().find(|s| s.number == number)
    }

    /// The split-or-freeze answer, or `None` if the question has not been
    /// asked. See the field docs for why those are not the same.
    pub fn blocking(&self) -> Option<Blocking> {
        self.blocking
    }

    /// What a session would actually do — the answer, or the default an
    /// omitted `block_keyboards` key means.
    pub fn effective_blocking(&self) -> Blocking {
        self.blocking.unwrap_or_default()
    }

    /// Nothing staged at all. `true` for a fresh visit and after
    /// [`Self::discard`].
    pub fn is_empty(&self) -> bool {
        self.device.is_none() && self.slots.is_empty() && self.blocking.is_none()
    }

    /// The lowest slot number not staged yet, or `None` when all
    /// [`MAX_SLOTS`] are taken. What an "Add a controller" button offers.
    pub fn next_free_slot(&self) -> Option<u8> {
        (1..=MAX_SLOTS).find(|n| self.slot(*n).is_none())
    }

    /// How many staged slots occupy one of Windows' four XInput slots.
    ///
    /// Counted through [`Persona::is_xinput`] — the same flag `ksx-config`'s
    /// validator and `ksx pads` count from. A second
    /// `matches!(p, Xbox360 | XboxSeries)` written here would be a fourth
    /// persona's chance to be forgotten.
    pub fn xinput_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.persona.is_xinput()).count()
    }

    /// Choose the input device. Replaces any earlier choice — that is the whole
    /// point of moment 4, and it costs nothing because nothing was written.
    pub fn choose_device(&self, device: StagedDevice) -> Result<Self, StageRefusal> {
        let alias = device.alias.trim();
        if alias.is_empty() {
            return Err(StageRefusal::BadAlias {
                alias: device.alias.clone(),
                problem: "an alias is the short name a [[slot]] refers to, and an empty one \
                          refers to nothing",
            });
        }
        if alias.contains('\\') {
            return Err(StageRefusal::BadAlias {
                alias: device.alias.clone(),
                problem: "a value containing a backslash is read as a literal device path \
                          everywhere else in the config, so it could never resolve back to this \
                          entry",
            });
        }
        let mut next = self.clone();
        next.device = Some(StagedDevice {
            alias: alias.to_owned(),
            ..device
        });
        Ok(next)
    }

    /// Stage a controller in `number`, presenting itself as `persona`, with
    /// `preset` as its bindings so far.
    pub fn add_slot(
        &self,
        number: u8,
        persona: Persona,
        preset: Preset,
    ) -> Result<Self, StageRefusal> {
        check_slot_number(number)?;
        if self.slot(number).is_some() {
            return Err(StageRefusal::SlotTaken { number });
        }
        check_pluggable(persona)?;
        let mut next = self.clone();
        next.slots.push(StagedSlot {
            number,
            persona,
            preset,
        });
        next.slots.sort_by_key(|s| s.number);
        next.check_xinput_ceiling(number)?;
        Ok(next)
    }

    /// Stage a controller in the lowest free slot — the "Add a controller"
    /// button, which must not make a first-run user pick a number.
    pub fn add_next_slot(&self, persona: Persona, preset: Preset) -> Result<Self, StageRefusal> {
        let number = self.next_free_slot().ok_or(StageRefusal::NoFreeSlot)?;
        self.add_slot(number, persona, preset)
    }

    /// Change what a staged slot presents itself as — moment 5's "and they can
    /// change their mind freely".
    pub fn set_persona(&self, number: u8, persona: Persona) -> Result<Self, StageRefusal> {
        check_slot_number(number)?;
        check_pluggable(persona)?;
        let mut next = self.clone();
        let slot = next
            .slots
            .iter_mut()
            .find(|s| s.number == number)
            .ok_or(StageRefusal::NoSuchSlot { number })?;
        slot.persona = persona;
        next.check_xinput_ceiling(number)?;
        Ok(next)
    }

    /// Replace a staged slot's bindings wholesale.
    ///
    /// Whole-preset, never per-key, for the reason `ControlSource::bind_keys`
    /// gives: the editor already holds the whole grid, so it can always send
    /// all of it, and a partial protocol carries indices computed against a
    /// value that may have moved.
    pub fn set_bindings(&self, number: u8, preset: Preset) -> Result<Self, StageRefusal> {
        check_slot_number(number)?;
        let mut next = self.clone();
        let slot = next
            .slots
            .iter_mut()
            .find(|s| s.number == number)
            .ok_or(StageRefusal::NoSuchSlot { number })?;
        slot.preset = preset;
        Ok(next)
    }

    /// **Delete a staged controller. Free and complete** — no file, no backup,
    /// no trace, because there was never one to begin with.
    pub fn remove_slot(&self, number: u8) -> Result<Self, StageRefusal> {
        check_slot_number(number)?;
        if self.slot(number).is_none() {
            return Err(StageRefusal::NoSuchSlot { number });
        }
        let mut next = self.clone();
        next.slots.retain(|s| s.number != number);
        Ok(next)
    }

    /// Answer the split-or-freeze question (`docs/FIRST-RUN.md` §3).
    #[must_use]
    pub fn set_blocking(&self, blocking: Blocking) -> Self {
        let mut next = self.clone();
        next.blocking = Some(blocking);
        next
    }

    /// **"Start over".** Infallible on purpose: §2 requires that a staged setup
    /// can be discarded wholesale, and a discard that could refuse would be a
    /// way to get stuck in a setup you did not want.
    #[must_use]
    pub fn discard(&self) -> Self {
        Self::default()
    }

    /// Turn this into the one typed value a save and a play are both built
    /// from, or refuse.
    ///
    /// Re-checks every rule an operation already checked. That is deliberate
    /// and cheap: it makes the spec's validity a property of the SPEC rather
    /// than of the sequence of calls that produced it, so a future operation
    /// that forgets a rule cannot leak a bad setup past here.
    pub fn commit(&self) -> Result<CommitSpec, StageRefusal> {
        let Some(device) = self.device.clone() else {
            return Err(StageRefusal::NoDevice {
                slots: self.slots.len(),
            });
        };
        if self.slots.is_empty() {
            return Err(StageRefusal::NoSlots);
        }
        self.check_preset_names()?;

        // The device as every slot will name it. `from_selector`, not
        // `parse(&to_string())`: this is a value ksx is writing for the first
        // time, so the canonical spelling IS the written one.
        let keyboard =
            crate::selector::DeviceRef::from_selector(device.selector.clone()).as_device_id();

        let mut slots = Vec::with_capacity(self.slots.len());
        for staged in &self.slots {
            check_pluggable(staged.persona)?;
            let spec = SlotSpec::new(
                staged.number,
                Some(keyboard.clone()),
                None,
                staged.preset.name.clone(),
            )
            .map_err(|err| StageRefusal::BadSlot { given: err.0 })?
            .with_persona(staged.persona);
            slots.push(ResolvedSlot {
                spec,
                preset: staged.preset.clone(),
            });
        }
        let xinput = self.xinput_slots();
        if xinput > usize::from(MAX_XINPUT_SLOTS) {
            return Err(StageRefusal::TooManyXinputSlots {
                number: self
                    .slots
                    .iter()
                    .filter(|s| s.persona.is_xinput())
                    .map(|s| s.number)
                    .next_back()
                    .unwrap_or(0),
                after: xinput,
            });
        }
        Ok(CommitSpec {
            device,
            slots,
            blocking: self.effective_blocking(),
        })
    }

    /// Refuse a state where a fifth slot would sit on an XInput persona.
    ///
    /// `number` is the slot the caller just touched, so the refusal can name
    /// the one to change.
    fn check_xinput_ceiling(&self, number: u8) -> Result<(), StageRefusal> {
        let after = self.xinput_slots();
        if after > usize::from(MAX_XINPUT_SLOTS) {
            return Err(StageRefusal::TooManyXinputSlots { number, after });
        }
        Ok(())
    }

    /// One preset name is one preset file. Two staged slots may share a name
    /// only when they share the bindings too.
    fn check_preset_names(&self) -> Result<(), StageRefusal> {
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.preset.name.trim().is_empty() {
                return Err(StageRefusal::UnnamedPreset {
                    number: slot.number,
                });
            }
            for other in &self.slots[i + 1..] {
                if other.preset.name.eq_ignore_ascii_case(&slot.preset.name)
                    && other.preset != slot.preset
                {
                    return Err(StageRefusal::PresetNameClash {
                        number: slot.number,
                        other: other.number,
                        name: slot.preset.name.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn check_slot_number(number: u8) -> Result<(), StageRefusal> {
    if number == 0 || number > MAX_SLOTS {
        return Err(StageRefusal::BadSlot { given: number });
    }
    Ok(())
}

/// Refuse a persona this BUILD cannot create, in [`crate::PadBackend`]'s own
/// words.
///
/// Reads [`Persona::can_plug`] and nothing else — never a driver probe, for the
/// reason that method's doc comment gives: a probe answers "is HIDMaestro
/// installed", whose answer flips to yes while `create_controller` still has
/// nothing to call.
fn check_pluggable(persona: Persona) -> Result<(), StageRefusal> {
    if persona.can_plug() {
        return Ok(());
    }
    let backend = persona.backend();
    Err(StageRefusal::PersonaNotImplemented {
        persona,
        backend: backend.label(),
        reason: backend
            .gap()
            .unwrap_or("this build ships no code that can create it"),
        instead: persona.nearest_pluggable(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::Key;
    use crate::pad::XButton;
    use crate::preset::Binding;

    fn device() -> StagedDevice {
        StagedDevice {
            selector: DeviceSelector::parse("usb:d209:0430:00").unwrap(),
            alias: "panel".to_owned(),
            label: "Ultimarc I-PAC 4".to_owned(),
        }
    }

    fn preset(name: &str) -> Preset {
        Preset {
            name: name.to_owned(),
            entries: vec![(Key::A, Binding::Button(XButton::A))],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        }
    }

    fn staged() -> StagedSetup {
        StagedSetup::new()
            .choose_device(device())
            .unwrap()
            .add_slot(1, Persona::Xbox360, preset("Player 1"))
            .unwrap()
    }

    /// **The moment §2 exists for.** Pick PS4, look at it, change to Xbox 360,
    /// then delete the controller entirely — and every one of those is a value
    /// in memory.
    ///
    /// Breaks against the pre-staging design, in which each of these four acts
    /// was a `config.toml` write plus a timestamped backup. There is no file
    /// here to assert on, which IS the assertion: the type has no path to one.
    #[test]
    fn a_user_can_change_their_mind_four_times_and_nothing_is_written() {
        let ps4 = staged()
            .set_persona(1, Persona::PlayStation)
            .expect("PlayStation plugs");
        assert_eq!(ps4.slot(1).unwrap().persona, Persona::PlayStation);

        let xbox = ps4.set_persona(1, Persona::Xbox360).unwrap();
        assert_eq!(xbox.slot(1).unwrap().persona, Persona::Xbox360);

        // Deleting is free and COMPLETE: no trace of slot 1 anywhere.
        let gone = xbox.remove_slot(1).unwrap();
        assert!(gone.slots().is_empty());
        assert_eq!(gone.slot(1), None);
        // ...and the device survives, because deleting a controller is not
        // starting over.
        assert_eq!(gone.device(), Some(&device()));

        // "Start over" always works, from any state, and cannot refuse.
        assert!(xbox.discard().is_empty());
        assert!(StagedSetup::new().discard().is_empty());
    }

    /// A refusal leaves the caller holding exactly what it had.
    ///
    /// Breaks against a `&mut self` design that validates as it mutates: those
    /// leave the fifth Xbox slot pushed onto the list and then report failure,
    /// so the next read shows a slot the user was told they could not have.
    #[test]
    fn a_refusal_changes_nothing_at_all() {
        let four = (1..=4).fold(
            StagedSetup::new().choose_device(device()).unwrap(),
            |setup, n| {
                setup
                    .add_slot(n, Persona::Xbox360, preset(&format!("P{n}")))
                    .unwrap()
            },
        );
        let refused = four
            .add_slot(5, Persona::Xbox360, preset("P5"))
            .unwrap_err();
        assert_eq!(refused.code(), "too-many-xinput-slots");
        assert_eq!(
            four.slots().len(),
            4,
            "the setup the caller holds is intact"
        );
        assert_eq!(four.xinput_slots(), 4);
    }

    /// The XInput ceiling is Windows', the slot ceiling is ours, and the two
    /// are different numbers — which is exactly how players 5+ exist.
    ///
    /// Breaks against a stage that counted slots instead of
    /// `Persona::is_xinput()`: that version refuses the PlayStation slot 5 that
    /// is the entire point of the persona, or waves through a fifth Xbox pad
    /// that plugs and that no game will ever read.
    #[test]
    fn a_fifth_xbox_slot_is_refused_and_a_fifth_playstation_slot_is_not() {
        let four = (1..=4).fold(
            StagedSetup::new().choose_device(device()).unwrap(),
            |setup, n| {
                setup
                    .add_slot(n, Persona::Xbox360, preset(&format!("P{n}")))
                    .unwrap()
            },
        );
        let refused = four
            .add_slot(5, Persona::Xbox360, preset("P5"))
            .unwrap_err();
        let message = refused.to_string();
        assert!(message.contains("is_xinput()"), "names the rule: {message}");
        assert!(message.contains("playstation"), "{message}");
        assert!(
            message.contains("Nothing has been written"),
            "a staged refusal must say the change is still free: {message}"
        );

        let fifth = four
            .add_slot(5, Persona::PlayStation, preset("P5"))
            .expect("plain HID takes none of the four");
        assert_eq!(fifth.slots().len(), 5);
        assert_eq!(fifth.xinput_slots(), 4);

        // ...and re-personaing that fifth slot back to Xbox is the same
        // refusal, so the two doors into the state are one rule.
        assert_eq!(
            fifth.set_persona(5, Persona::Xbox360).unwrap_err().code(),
            "too-many-xinput-slots"
        );
    }

    /// The three HIDMaestro personas are refused in `PadBackend::gap()`'s own
    /// words — including the half that closes off the wrong fix.
    ///
    /// Breaks against a stage that accepted them: the user picks DualSense, the
    /// screen says ready, and the pad never appears — with a driver install as
    /// the obvious and useless next step.
    #[test]
    fn a_persona_this_build_cannot_plug_is_refused_with_a_way_out() {
        for (persona, instead) in [
            (Persona::DualSense, Persona::PlayStation),
            (Persona::SwitchPro, Persona::Xbox360),
            (Persona::XboxSeries, Persona::Xbox360),
        ] {
            let refused = staged().add_slot(2, persona, preset("P2")).unwrap_err();
            assert_eq!(refused.code(), "persona-not-implemented", "{persona}");
            let message = refused.to_string();
            assert!(
                message.contains("installing HIDMaestro does not change it"),
                "the gap verbatim: {message}"
            );
            assert!(message.contains("BINARY"), "{message}");
            assert!(message.contains(instead.as_str()), "{message}");
            // The same gate on the other door.
            assert_eq!(
                staged().set_persona(1, persona).unwrap_err().code(),
                "persona-not-implemented"
            );
        }
    }

    /// MAX_SLOTS is the total ceiling, and it is ksx's own — the refusal says
    /// so rather than implying Windows imposed it.
    #[test]
    fn the_slot_ceiling_is_max_slots_and_the_refusals_name_the_live_bound() {
        let mut setup = StagedSetup::new().choose_device(device()).unwrap();
        for n in 1..=MAX_SLOTS {
            // PlayStation throughout: this test is about the TOTAL, and four
            // XInput slots would stop it at five for the other reason.
            setup = setup
                .add_slot(n, Persona::PlayStation, preset(&format!("P{n}")))
                .expect("every slot up to the ceiling");
        }
        assert_eq!(setup.next_free_slot(), None);
        assert_eq!(
            setup
                .add_next_slot(Persona::PlayStation, preset("P17"))
                .unwrap_err(),
            StageRefusal::NoFreeSlot
        );
        // A number off the end is refused by the same check `SlotSpec::new`
        // makes, quoting the live bound.
        let off = setup
            .add_slot(MAX_SLOTS + 1, Persona::PlayStation, preset("X"))
            .unwrap_err();
        assert_eq!(off.code(), "bad-slot");
        assert!(off.to_string().contains(&MAX_SLOTS.to_string()));
        assert_eq!(
            StagedSetup::new()
                .add_slot(0, Persona::Xbox360, preset("X"))
                .unwrap_err()
                .code(),
            "bad-slot"
        );
    }

    /// "Not asked yet" and "the user chose the default" are different facts.
    ///
    /// Breaks against `blocking: Blocking` (no Option): that version cannot
    /// tell a surface whether §3's one question has been put to the user, so
    /// the screen renders Freeze pre-selected and the question is answered for
    /// them — while `effective_blocking` still reports the same value, which is
    /// why only the pair of accessors proves it.
    #[test]
    fn the_split_or_freeze_question_starts_unanswered() {
        let setup = staged();
        assert_eq!(setup.blocking(), None, "nobody has been asked");
        assert_eq!(
            setup.effective_blocking(),
            Blocking::Whole,
            "an omitted block_keyboards key means the whole device"
        );

        let split = setup.set_blocking(Blocking::BoundKeys);
        assert_eq!(split.blocking(), Some(Blocking::BoundKeys));
        assert_eq!(split.effective_blocking(), Blocking::BoundKeys);
        // Answering "freeze" is an ANSWER, not a return to silence.
        let freeze = split.set_blocking(Blocking::Whole);
        assert_eq!(freeze.blocking(), Some(Blocking::Whole));
        assert!(!freeze.is_empty());
    }

    /// A device is staged as a SELECTOR — which board — and never as the raw
    /// path a scan happened to see.
    ///
    /// Breaks against a stage that carried the instance path: that value pins
    /// the setup to one USB socket, and it is the string §5 bans from ever
    /// being the identifier on screen.
    #[test]
    fn the_staged_device_is_a_selector_and_its_alias_can_never_be_a_path() {
        let setup = staged();
        let staged_device = setup.device().unwrap();
        assert_eq!(staged_device.selector.rung(), "model");
        assert!(staged_device.selector.survives_replug());
        assert_eq!(staged_device.label, "Ultimarc I-PAC 4");

        // An alias with a backslash is read as a literal device path
        // everywhere else in the config, so it could never resolve back here.
        let refused = StagedSetup::new()
            .choose_device(StagedDevice {
                alias: r"USB\VID_D209".to_owned(),
                ..device()
            })
            .unwrap_err();
        assert_eq!(refused.code(), "bad-alias");
        assert!(refused.to_string().contains("literal device path"));

        let blank = StagedSetup::new()
            .choose_device(StagedDevice {
                alias: "  ".to_owned(),
                ..device()
            })
            .unwrap_err();
        assert_eq!(blank.code(), "bad-alias");

        // Changing the device is free, and replaces the earlier choice.
        let other = DeviceSelector::parse("usb:046d:c31c:00").unwrap();
        let moved = setup
            .choose_device(StagedDevice {
                selector: other.clone(),
                alias: " logitech ".to_owned(),
                label: "Logitech keyboard".to_owned(),
            })
            .unwrap();
        assert_eq!(moved.device().unwrap().selector, other);
        assert_eq!(moved.device().unwrap().alias, "logitech", "trimmed");
        assert_eq!(moved.slots().len(), 1, "the controllers are untouched");
    }

    /// Commit produces slots that name the chosen device and carry the staged
    /// persona and the staged bindings — the three things the screen showed.
    ///
    /// Breaks against a commit that dropped the keyboard (which is what
    /// `slots::assign` does for a slot it creates): the saved config plans to
    /// zero usable slots, because a slot with no input device is dropped with
    /// `NoInputDeviceSelected`, and the cabinet is dead with four pads missing.
    #[test]
    fn commit_carries_the_device_the_persona_and_the_bindings() {
        let spec = staged()
            .add_slot(2, Persona::PlayStation, preset("Player 2"))
            .unwrap()
            .set_blocking(Blocking::BoundKeys)
            .commit()
            .expect("a device and two controllers");

        assert_eq!(spec.blocking, Blocking::BoundKeys);
        assert_eq!(spec.slots.len(), 2);
        assert_eq!(spec.device.alias, "panel");
        for slot in &spec.slots {
            assert_eq!(
                slot.spec.keyboard.as_ref().map(|k| k.as_str()),
                Some("usb:d209:0430:00"),
                "every staged slot listens to the staged device"
            );
            assert_eq!(slot.spec.preset, slot.preset.name);
        }
        assert_eq!(spec.slots[0].spec.persona, Persona::Xbox360);
        assert_eq!(spec.slots[1].spec.persona, Persona::PlayStation);
        assert_eq!(spec.slots[0].preset.entries.len(), 1);
    }

    /// Committing an incomplete stage refuses in words that name the missing
    /// step, rather than saving a config that cannot run.
    #[test]
    fn committing_without_a_device_or_without_a_controller_is_refused() {
        let no_device = StagedSetup::new()
            .add_slot(1, Persona::Xbox360, preset("P1"))
            .unwrap()
            .commit()
            .unwrap_err();
        assert_eq!(no_device.code(), "no-device");
        assert!(no_device.to_string().contains("pick a device"));

        let no_slots = StagedSetup::new()
            .choose_device(device())
            .unwrap()
            .commit()
            .unwrap_err();
        assert_eq!(no_slots.code(), "no-slots");
        assert!(no_slots.to_string().contains("Add one"));
    }

    /// Two staged slots may share a preset NAME only when they share the
    /// bindings, because one name is one file.
    ///
    /// Breaks against a commit with no name check: saving writes "Player 1"
    /// twice, the second write wins, and slot 1 comes back off disk holding
    /// slot 2's bindings — the second translation §2 forbids, landing silently.
    #[test]
    fn two_slots_cannot_stage_different_bindings_under_one_preset_name() {
        let mut two = preset("Player 1");
        two.entries = vec![(Key::B, Binding::Button(XButton::B))];
        let clash = staged()
            .add_slot(2, Persona::PlayStation, two)
            .unwrap()
            .commit()
            .unwrap_err();
        assert_eq!(clash.code(), "preset-name-clash");
        assert!(clash.to_string().contains("silently lose the other"));

        // Deliberate sharing — the SAME preset in both slots — is fine: that
        // is two players on one key map, which is an ordinary thing to want.
        let shared = staged()
            .add_slot(2, Persona::PlayStation, preset("Player 1"))
            .unwrap()
            .commit()
            .expect("one preset, two slots");
        assert_eq!(shared.slots.len(), 2);

        // A nameless preset has no file to be written to.
        let unnamed = staged()
            .add_slot(2, Persona::PlayStation, preset(" "))
            .unwrap()
            .commit()
            .unwrap_err();
        assert_eq!(unnamed.code(), "unnamed-preset");
    }

    /// Editing bindings and removing slots only ever touch the slot named.
    #[test]
    fn bindings_and_removal_address_exactly_one_slot() {
        let two = staged()
            .add_slot(2, Persona::PlayStation, preset("Player 2"))
            .unwrap();

        let edited = two
            .set_bindings(2, preset("Player 2 v2"))
            .expect("slot 2 is staged");
        assert_eq!(edited.slot(2).unwrap().preset.name, "Player 2 v2");
        assert_eq!(edited.slot(1).unwrap().preset.name, "Player 1");

        assert_eq!(
            two.set_bindings(3, preset("ghost")).unwrap_err(),
            StageRefusal::NoSuchSlot { number: 3 }
        );
        assert_eq!(
            two.remove_slot(3).unwrap_err(),
            StageRefusal::NoSuchSlot { number: 3 }
        );
        assert_eq!(
            two.set_persona(3, Persona::Xbox360).unwrap_err(),
            StageRefusal::NoSuchSlot { number: 3 }
        );

        // Removing the middle of a set leaves the rest, and the freed number
        // is offered again.
        let one = two.remove_slot(1).unwrap();
        assert_eq!(one.slots().len(), 1);
        assert_eq!(one.slots()[0].number, 2);
        assert_eq!(one.next_free_slot(), Some(1));
    }

    /// "Add a controller" must not make a first-run user pick a number, and it
    /// must never reuse one.
    #[test]
    fn add_next_slot_fills_the_lowest_free_number() {
        let mut setup = StagedSetup::new().choose_device(device()).unwrap();
        for expected in 1..=3 {
            setup = setup
                .add_next_slot(Persona::PlayStation, preset(&format!("P{expected}")))
                .unwrap();
            assert_eq!(setup.slots().last().unwrap().number, expected);
        }
        let gap = setup.remove_slot(2).unwrap();
        assert_eq!(gap.next_free_slot(), Some(2));
        let filled = gap
            .add_next_slot(Persona::PlayStation, preset("P2b"))
            .unwrap();
        assert_eq!(filled.slot(2).unwrap().preset.name, "P2b");
        assert_eq!(filled.slots().len(), 3);
    }

    /// Every refusal has a stable, distinct code — a surface routes on these.
    #[test]
    fn refusal_codes_are_distinct_and_never_empty() {
        let all = [
            StageRefusal::BadSlot { given: 0 },
            StageRefusal::SlotTaken { number: 1 },
            StageRefusal::NoSuchSlot { number: 1 },
            StageRefusal::NoFreeSlot,
            StageRefusal::PersonaNotImplemented {
                persona: Persona::DualSense,
                backend: "HIDMaestro",
                reason: "x",
                instead: Persona::PlayStation,
            },
            StageRefusal::TooManyXinputSlots {
                number: 5,
                after: 5,
            },
            StageRefusal::BadAlias {
                alias: "a".into(),
                problem: "x",
            },
            StageRefusal::PresetNameClash {
                number: 1,
                other: 2,
                name: "P".into(),
            },
            StageRefusal::UnnamedPreset { number: 1 },
            StageRefusal::NoDevice { slots: 1 },
            StageRefusal::NoSlots,
        ];
        let codes: std::collections::HashSet<&str> = all.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), all.len(), "one code per refusal");
        assert!(all.iter().all(|r| !r.to_string().is_empty()));
    }

    /// Staging the same slot twice is a refusal, not a silent overwrite — a
    /// second "Add a controller" click must not replace the one being edited.
    #[test]
    fn staging_a_slot_that_is_already_staged_is_refused() {
        let refused = staged()
            .add_slot(1, Persona::PlayStation, preset("other"))
            .unwrap_err();
        assert_eq!(refused, StageRefusal::SlotTaken { number: 1 });
        assert_eq!(staged().slot(1).unwrap().preset.name, "Player 1");
    }
}
