//! The staged setup, on the wire — `docs/FIRST-RUN.md` §2.
//!
//! [`ksx_core::StagedSetup`] is the domain value: a setup a user is still
//! deciding on, held in the daemon for the length of a visit, with no path to a
//! file. This module is how a surface drives it and how a surface renders it.
//!
//! # The shape is [`crate::control::MacroWrite`]'s, deliberately
//!
//! A [`StageEdit`] is what arrives from a form or a JSON body: strings a human
//! or a browser produced, which may be blank and may be wrong.
//! [`StageEdit::apply`] is the ONE place a wire word becomes a core operation,
//! and a bad word is refused THERE — in ksx-core's own sentence, with no round
//! trip and nothing changed. That is the same split `MacroWrite::to_request`
//! makes, and for the same reason: the daemon's parser stays strict, so a
//! genuine typo is still refused in words that name the options.
//!
//! # Every number here is SERVED, never known by the surface
//!
//! [`StagedSetupView`] carries `max_slots`, `max_xinput_slots`, `xinput_used`
//! and the whole persona roster with a `can_plug` flag on each. `docs/CLAUDE.md`
//! and `SURFACES.md` §1 make that a rule rather than a courtesy: a `16` typed
//! into TypeScript is the specific bug the rule exists for, and the day
//! HIDMaestro's shared section gets a mapper the roster changes underneath
//! every surface at once, with nothing to edit.
//!
//! # Looking is never a commitment
//!
//! Nothing on this module writes, claims or plugs. [`StageOutcome`] reports what
//! a *save* did only when [`crate::ControlSource::stage_commit`] was called, and
//! [`crate::ControlSource::stage_play`] starts a session from a plan built with
//! no file write at all — `FIRST-RUN.md` §2's "saving and playing are separate
//! acts", stated as two verbs.

use serde::{Deserialize, Serialize};

use ksx_core::stage::{StagedDevice, StagedSetup};
use ksx_core::{Blocking, DeviceSelector, Persona, MAX_SLOTS, MAX_XINPUT_SLOTS};

use crate::refusal::{codes, Refusal};

/// The chosen input device, as a surface shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedDeviceView {
    /// What a human calls it — "Ultimarc I-PAC 4". **This is the identifier on
    /// screen** (`FIRST-RUN.md` §5); the selector below is small print.
    pub label: String,
    /// The short name saved `[[slot]]` rows will refer to it by.
    pub alias: String,
    /// The `[[device]] id` a save would write: which BOARD, not which socket.
    pub selector: String,
    /// `model` | `serial` | `port` — how much of the board the selector pins
    /// down. Served so a surface never has to parse the selector to say it.
    pub rung: String,
    /// Does this selector still name the board after it is unplugged and
    /// plugged in elsewhere? `false` for a `port=` rung, which is the one that
    /// does not travel.
    pub survives_replug: bool,
}

impl From<&StagedDevice> for StagedDeviceView {
    fn from(device: &StagedDevice) -> Self {
        Self {
            label: device.label.clone(),
            alias: device.alias.clone(),
            selector: device.selector.to_string(),
            rung: device.selector.rung().to_owned(),
            survives_replug: device.selector.survives_replug(),
        }
    }
}

/// One staged controller, as a surface shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedSlotView {
    pub number: u8,
    /// Canonical persona name (`xbox360`, `playstation`…) — what a
    /// [`StageEdit`] sends back.
    pub persona: String,
    /// Human label ("Xbox 360", "PlayStation").
    pub persona_label: String,
    /// Does this persona occupy one of Windows' four XInput slots? The fact
    /// that explains why a fifth Xbox pad is refused and a fifth PlayStation
    /// one is not.
    pub is_xinput: bool,
    /// The preset name — and the name of the file a save would write.
    pub preset: String,
    /// How many controls this slot binds so far. `0` is a real answer: a pad
    /// with no bindings plugs and does nothing, and a surface should be able
    /// to say so before the user finds out in a game.
    pub bindings: usize,
}

/// One persona, as an option a surface offers.
///
/// The ROSTER, served. A surface that hardcoded five names would keep offering
/// `dualsense` after it started plugging, or keep offering it while it cannot —
/// and [`Self::can_plug`] plus [`Self::gap`] are the two halves of the honest
/// answer `ksx_core::Persona` already holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonaOption {
    pub name: String,
    pub label: String,
    pub is_xinput: bool,
    /// Can THIS BUILD create it? A fact about the binary, never a driver probe.
    pub can_plug: bool,
    /// What is missing, when it cannot — `PadBackend::gap()`'s own sentence, so
    /// no surface paraphrases it into "install HIDMaestro".
    pub gap: Option<String>,
    /// The nearest persona this build CAN plug, so an option that is greyed out
    /// still points somewhere.
    pub instead: String,
}

impl PersonaOption {
    /// Every persona ksx knows, in [`Persona::ALL`] order.
    pub fn roster() -> Vec<Self> {
        Persona::ALL
            .iter()
            .map(|&persona| Self {
                name: persona.as_str().to_owned(),
                label: persona.label().to_owned(),
                is_xinput: persona.is_xinput(),
                can_plug: persona.can_plug(),
                gap: persona.backend().gap().map(str::to_owned),
                instead: persona.nearest_pluggable().as_str().to_owned(),
            })
            .collect()
    }
}

/// One blocking answer, as `FIRST-RUN.md` §3 words it for a user.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockingOption {
    /// `whole` | `bound-keys` | `off` — what a [`StageEdit`] sends back.
    pub name: String,
    /// The question in the user's words: "Freeze this keyboard".
    pub title: String,
    /// What it means for them, not for the config file.
    pub detail: String,
}

impl BlockingOption {
    /// The two answers §3 asks about, plus the third the setting has always
    /// had. The wording lives HERE, once, so the browser and the cabinet cannot
    /// describe the same choice differently.
    pub fn roster() -> Vec<Self> {
        vec![
            Self {
                name: Blocking::Whole.as_str().to_owned(),
                title: "Freeze this keyboard".to_owned(),
                detail: "Every key on it drives the pad and nothing else. No typos into the \
                         game, no accidental Windows shortcuts. This is what most people want \
                         for a dedicated arcade panel."
                    .to_owned(),
            },
            Self {
                name: Blocking::BoundKeys.as_str().to_owned(),
                title: "Split this keyboard".to_owned(),
                detail: "Mapped keys drive the pad; everything else still types. This is what \
                         lets one keyboard serve player 1 and player 2, and what lets someone \
                         keep using their only keyboard."
                    .to_owned(),
            },
            Self {
                name: Blocking::Off.as_str().to_owned(),
                title: "Take nothing".to_owned(),
                detail: "The pads are driven and the keyboard keeps typing as well. Every \
                         mapped key does both at once."
                    .to_owned(),
            },
        ]
    }
}

/// What the staging screens render. Presentation-shaped, like
/// [`crate::StatusSnapshot`]: the provider composes it, a surface only places
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedSetupView {
    /// A daemon answered. `false` renders every control disabled WITH THE
    /// REASON — never hidden, never silently inert, and never as an empty
    /// setup, because "I could not reach the daemon" and "you have staged
    /// nothing" are different sentences (`SURFACES.md` §1b).
    pub reachable: bool,
    /// Why not, when [`Self::reachable`] is false.
    pub error: Option<String>,
    /// Nothing staged at all — a fresh visit, or just after "Start over".
    pub empty: bool,
    pub device: Option<StagedDeviceView>,
    /// Slot order.
    pub slots: Vec<StagedSlotView>,
    /// The split-or-freeze answer, or `None` when the question has not been
    /// asked. A surface must render those differently: pre-selecting an option
    /// answers §3's one question for the user.
    pub blocking: Option<String>,
    /// The lowest slot number "Add a controller" would use, or `None` when
    /// every slot is staged.
    pub next_slot: Option<u8>,
    /// How many staged slots occupy one of Windows' four XInput slots.
    pub xinput_used: usize,
    /// **Served, never hardcoded**: `ksx_core::MAX_SLOTS`.
    pub max_slots: u8,
    /// **Served, never hardcoded**: `ksx_core::MAX_XINPUT_SLOTS`.
    pub max_xinput_slots: u8,
    /// Every persona, with the flags that decide whether it may be offered.
    pub personas: Vec<PersonaOption>,
    /// The blocking answers, in §3's own words.
    pub blocking_options: Vec<BlockingOption>,
    /// Is this setup complete enough to save or play? A surface enables the two
    /// buttons off this rather than re-deriving the rule.
    pub ready: bool,
    /// Why not, when it is not — ksx-core's own refusal sentence.
    pub not_ready: Option<String>,
}

impl StagedSetupView {
    /// The no-channel view. Every roster is still served, so a disabled screen
    /// renders the real options greyed out rather than an empty page.
    pub fn unreachable(reason: impl Into<String>) -> Self {
        Self {
            reachable: false,
            error: Some(reason.into()),
            ..Self::of(&StagedSetup::new())
        }
    }

    /// The view of a live staged setup.
    pub fn of(setup: &StagedSetup) -> Self {
        let ready = setup.commit();
        Self {
            reachable: true,
            error: None,
            empty: setup.is_empty(),
            device: setup.device().map(StagedDeviceView::from),
            slots: setup
                .slots()
                .iter()
                .map(|slot| StagedSlotView {
                    number: slot.number,
                    persona: slot.persona.as_str().to_owned(),
                    persona_label: slot.persona.label().to_owned(),
                    is_xinput: slot.persona.is_xinput(),
                    preset: slot.preset.name.clone(),
                    bindings: slot.preset.entries.len() + slot.preset.chords.len(),
                })
                .collect(),
            blocking: setup.blocking().map(|b| b.as_str().to_owned()),
            next_slot: setup.next_free_slot(),
            xinput_used: setup.xinput_slots(),
            max_slots: MAX_SLOTS,
            max_xinput_slots: MAX_XINPUT_SLOTS,
            personas: PersonaOption::roster(),
            blocking_options: BlockingOption::roster(),
            not_ready: ready.as_ref().err().map(ToString::to_string),
            ready: ready.is_ok(),
        }
    }
}

/// One edit to a staged setup, as a surface spells it.
///
/// Kept apart from [`ksx_core::StagedSetup`]'s own operations on purpose: this
/// is what arrives from a form or a JSON body — strings, which may be blank and
/// may be wrong. [`Self::apply`] is the one place they become a typed
/// operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "kebab-case")]
pub enum StageEdit {
    /// Moment 4: they pick a keyboard. Replaces any earlier choice — freely,
    /// because nothing was written.
    ChooseDevice {
        /// A `ksx_core::DeviceSelector` spelling (`usb:d209:0430:00`), which is
        /// what `ksx device scan` prints for every board it lists. **Never a
        /// raw path a user typed**: `FIRST-RUN.md` §6 forbids ever asking for
        /// one, so a surface sends back the selector from the row they clicked.
        selector: String,
        alias: String,
        label: String,
    },
    /// Moment 5: they pick what it should become. `number` omitted means the
    /// lowest free slot, so an "Add a controller" button never has to make a
    /// first-run user choose a number.
    AddSlot {
        #[serde(default)]
        number: Option<u8>,
        persona: String,
        /// The preset name this controller's bindings live under.
        preset: String,
    },
    /// Moment 5 again: change their mind. Free, and the whole point.
    SetPersona { number: u8, persona: String },
    /// Moment 6: the bindings so far, as a whole preset table — the same
    /// whole-value rule `ControlSource::bind_keys` and `MacroWrite` follow.
    SetBindings {
        number: u8,
        /// Boxed because a preset file is much larger than the other variants
        /// and an enum is as big as its widest arm.
        preset: Box<ksx_config::PresetFile>,
    },
    /// Delete a staged controller. Free and complete: no file, no backup, no
    /// trace.
    RemoveSlot { number: u8 },
    /// Moment 6's one question: `whole` (freeze) or `bound-keys` (split).
    SetBlocking { blocking: String },
    /// **Start over.** Always works.
    Discard,
}

impl StageEdit {
    /// Apply this edit to `setup`, or refuse.
    ///
    /// Returns a NEW setup — the caller still holds the old one on refusal,
    /// which is what makes "a refusal changes nothing" true across the wire as
    /// well as inside ksx-core.
    ///
    /// Every refusal carries a stable code: ksx-core's own
    /// (`too-many-xinput-slots`, `persona-not-implemented`, …) for a domain
    /// rule, and [`codes::BAD_REQUEST`] for a word this build cannot parse —
    /// which is a different failure and must not be dressed as a domain one.
    pub fn apply(&self, setup: &StagedSetup) -> Result<StagedSetup, Refusal> {
        match self {
            Self::ChooseDevice {
                selector,
                alias,
                label,
            } => {
                let selector = DeviceSelector::parse(selector.trim()).map_err(|err| {
                    Refusal::with_remedy(
                        codes::BAD_REQUEST,
                        err.to_string(),
                        "send the selector `ksx device scan` prints for the board (for example \
                         `usb:d209:0430:00`) — never a path anybody typed",
                    )
                })?;
                setup
                    .choose_device(StagedDevice {
                        selector,
                        alias: alias.trim().to_owned(),
                        label: label.trim().to_owned(),
                    })
                    .map_err(refuse)
            }
            Self::AddSlot {
                number,
                persona,
                preset,
            } => {
                let persona = parse_persona(persona)?;
                let preset = ksx_core::Preset {
                    name: preset.trim().to_owned(),
                    entries: Vec::new(),
                    chords: Vec::new(),
                    macros: Default::default(),
                    turbo: Vec::new(),
                    protected: false,
                };
                match number {
                    Some(number) => setup.add_slot(*number, persona, preset),
                    None => setup.add_next_slot(persona, preset),
                }
                .map_err(refuse)
            }
            Self::SetPersona { number, persona } => setup
                .set_persona(*number, parse_persona(persona)?)
                .map_err(refuse),
            Self::SetBindings { number, preset } => {
                // Through the preset file's OWN serde types, so a binding that
                // would be refused on disk is refused here in the identical
                // words rather than staged and rejected at save time.
                let core = preset.to_core().map_err(|err| {
                    Refusal::with_remedy(
                        codes::BAD_REQUEST,
                        err.to_string(),
                        "fix the binding and send the table again",
                    )
                })?;
                setup.set_bindings(*number, core).map_err(refuse)
            }
            Self::RemoveSlot { number } => setup.remove_slot(*number).map_err(refuse),
            Self::SetBlocking { blocking } => {
                let blocking: Blocking =
                    blocking
                        .trim()
                        .parse()
                        .map_err(|err: ksx_core::UnknownBlocking| {
                            Refusal::new(codes::BAD_REQUEST, err.to_string())
                        })?;
                Ok(setup.set_blocking(blocking))
            }
            Self::Discard => Ok(setup.discard()),
        }
    }
}

fn parse_persona(name: &str) -> Result<Persona, Refusal> {
    name.trim()
        .parse()
        .map_err(|err: ksx_core::UnknownPersona| Refusal::new(codes::BAD_REQUEST, err.to_string()))
}

/// A ksx-core refusal, carrying its own code and its own sentence.
///
/// Never re-worded: the sentence already names what would make the choice
/// legal, and a surface that paraphrased it would be the second description of
/// a rule that has one.
fn refuse(refusal: ksx_core::StageRefusal) -> Refusal {
    Refusal {
        code: refusal.code().to_owned(),
        message: refusal.to_string(),
        remedy: None,
    }
}

/// The answer to a staging verb: what it did, and what the setup looks like
/// now.
///
/// Structured rather than `Result<String, Refusal>` for the reason
/// [`crate::control::BindOutcome`] is: a surface re-renders from
/// [`Self::setup`] without a second read, which is what makes "change your mind
/// freely" feel free.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageOutcome {
    pub ok: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    /// Stable refusal word (`too-many-xinput-slots`, `persona-not-implemented`,
    /// `bad-request`…).
    pub code: Option<String>,
    /// The setup AFTER the verb — unchanged when it was refused.
    pub setup: StagedSetupView,
    /// A save happened, and this is the config file it wrote. `None` for every
    /// other verb, including Play: **saving and playing are separate acts**,
    /// and a Play that reported a path would be claiming a write it did not
    /// make.
    #[serde(default)]
    pub saved: Option<String>,
    /// The timestamped copy taken before that write.
    #[serde(default)]
    pub backup: Option<String>,
    /// A session was started from the staged setup, with nothing written.
    #[serde(default)]
    pub playing: bool,
}

impl StageOutcome {
    /// Success, carrying the new setup.
    pub fn ok(setup: &StagedSetup, message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: Some(message.into()),
            setup: StagedSetupView::of(setup),
            ..Self::default()
        }
    }

    /// A refusal, carrying the setup UNCHANGED — the caller sees exactly what
    /// they still have.
    pub fn refused(setup: &StagedSetup, refusal: &Refusal) -> Self {
        Self {
            ok: false,
            error: Some(refusal.message.clone()),
            code: Some(refusal.code.clone()),
            setup: StagedSetupView::of(setup),
            ..Self::default()
        }
    }

    /// The honest answer from a surface that has no staged setup at all — never
    /// a silent no-op, and never an empty setup that reads as "you staged
    /// nothing".
    pub fn unavailable(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            ok: false,
            error: Some(reason.clone()),
            code: Some(codes::NOT_HERE.to_owned()),
            setup: StagedSetupView::unreachable(reason),
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
                .unwrap_or_else(|| "the staged setup was not changed".to_owned()),
        ))
    }

    /// The one line a surface prints.
    pub fn headline(&self) -> String {
        if let Some(refusal) = self.refusal() {
            return refusal.message;
        }
        self.message
            .clone()
            .unwrap_or_else(|| "the staged setup was updated".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged() -> StagedSetup {
        StageEdit::ChooseDevice {
            selector: "usb:d209:0430:00".into(),
            alias: "panel".into(),
            label: "Ultimarc I-PAC 4".into(),
        }
        .apply(&StagedSetup::new())
        .unwrap()
    }

    /// **Every number a surface would otherwise hardcode is served.**
    ///
    /// Breaks against a view that omitted them: the browser then holds its own
    /// `16` and its own list of five persona names, and the day `MAX_SLOTS`
    /// moves or a persona starts plugging, the page is wrong with nothing in
    /// Rust to catch it. That is the specific bug `CLAUDE.md`'s one rule exists
    /// for.
    #[test]
    fn the_view_serves_the_ceilings_and_the_persona_roster() {
        let view = StagedSetupView::of(&StagedSetup::new());
        assert_eq!(view.max_slots, MAX_SLOTS);
        assert_eq!(view.max_xinput_slots, MAX_XINPUT_SLOTS);
        assert_eq!(view.personas.len(), Persona::ALL.len());
        assert_eq!(view.blocking_options.len(), Blocking::ALL.len());

        // Each persona carries the two halves of the honest answer.
        for option in &view.personas {
            let persona: Persona = option.name.parse().expect("a canonical name");
            assert_eq!(option.can_plug, persona.can_plug(), "{}", option.name);
            assert_eq!(option.gap.is_none(), option.can_plug, "{}", option.name);
            assert!(
                view.personas
                    .iter()
                    .any(|p| p.name == option.instead && p.can_plug),
                "{} suggests {}, which must itself plug",
                option.name,
                option.instead
            );
        }
        // The gap is `PadBackend::gap()`'s own sentence, not a paraphrase.
        let dualsense = view
            .personas
            .iter()
            .find(|p| p.name == "dualsense")
            .expect("the roster lists it even though it cannot plug");
        assert!(!dualsense.can_plug);
        assert!(dualsense
            .gap
            .as_deref()
            .unwrap()
            .contains("installing HIDMaestro does not change it"));
    }

    /// An unreachable daemon renders as an unreachable daemon, with the reason
    /// — never as an empty setup.
    ///
    /// Breaks against `StagedSetupView::default()` as the no-channel answer:
    /// `reachable: false` with `empty: true` and no error reads on screen as
    /// "you have staged nothing", which is `SURFACES.md` §1b's exact bug —
    /// a failed read rendered as an absence.
    #[test]
    fn an_unreachable_daemon_is_not_an_empty_setup() {
        let view = StagedSetupView::unreachable("no daemon answered the control pipe");
        assert!(!view.reachable);
        assert_eq!(
            view.error.as_deref(),
            Some("no daemon answered the control pipe")
        );
        // ...and the rosters are still served, so the disabled screen shows the
        // real options rather than a blank page.
        assert_eq!(view.max_slots, MAX_SLOTS);
        assert_eq!(view.personas.len(), Persona::ALL.len());
    }

    /// The whole first-run flow as a surface drives it, ending in a setup that
    /// is ready to save or play.
    #[test]
    fn the_edits_walk_moments_four_five_and_six() {
        let setup = staged();
        let view = StagedSetupView::of(&setup);
        assert_eq!(view.device.as_ref().unwrap().label, "Ultimarc I-PAC 4");
        assert_eq!(view.device.as_ref().unwrap().rung, "model");
        assert!(view.device.as_ref().unwrap().survives_replug);
        assert_eq!(view.next_slot, Some(1));
        assert!(!view.ready, "a keyboard with no controller drives nothing");
        assert!(view.not_ready.as_deref().unwrap().contains("controller"));

        let setup = StageEdit::AddSlot {
            number: None,
            persona: "ps4".into(), // an alias a human would type
            preset: "Player 1".into(),
        }
        .apply(&setup)
        .unwrap();
        let view = StagedSetupView::of(&setup);
        assert_eq!(view.slots[0].number, 1);
        assert_eq!(view.slots[0].persona, "playstation");
        assert_eq!(view.slots[0].persona_label, "PlayStation");
        assert!(!view.slots[0].is_xinput);
        assert_eq!(view.slots[0].bindings, 0, "nothing mapped yet");
        assert_eq!(view.xinput_used, 0);
        assert!(view.ready, "a device and a controller is enough to play");
        assert_eq!(view.blocking, None, "§3 has not been asked yet");

        let setup = StageEdit::SetBlocking {
            blocking: "bound-keys".into(),
        }
        .apply(&setup)
        .unwrap();
        assert_eq!(
            StagedSetupView::of(&setup).blocking.as_deref(),
            Some("bound-keys")
        );

        // Change of mind, then delete — both free.
        let setup = StageEdit::SetPersona {
            number: 1,
            persona: "xbox360".into(),
        }
        .apply(&setup)
        .unwrap();
        assert_eq!(StagedSetupView::of(&setup).xinput_used, 1);
        let setup = StageEdit::RemoveSlot { number: 1 }.apply(&setup).unwrap();
        assert!(StagedSetupView::of(&setup).slots.is_empty());

        // ...and Start over clears everything, from any state.
        let fresh = StageEdit::Discard.apply(&setup).unwrap();
        assert!(StagedSetupView::of(&fresh).empty);
    }

    /// A word this build cannot parse is `bad-request`; a rule this build
    /// enforces keeps ksx-core's own code. Two different failures, and a
    /// surface that routes on the code has to be able to tell them apart.
    ///
    /// Breaks against an `apply` that mapped everything to one code: "you typed
    /// gamecube" and "Windows has only four XInput slots" would then flash the
    /// same colour and offer the same next step.
    #[test]
    fn a_typo_and_a_domain_rule_carry_different_codes() {
        let setup = staged();
        let typo = StageEdit::AddSlot {
            number: None,
            persona: "gamecube".into(),
            preset: "P1".into(),
        }
        .apply(&setup)
        .unwrap_err();
        assert_eq!(typo.code, codes::BAD_REQUEST);
        assert!(typo.message.contains("playstation"), "{typo}");

        let refused = StageEdit::AddSlot {
            number: None,
            persona: "dualsense".into(),
            preset: "P1".into(),
        }
        .apply(&setup)
        .unwrap_err();
        assert_eq!(refused.code, "persona-not-implemented");
        assert!(
            refused
                .message
                .contains("installing HIDMaestro does not change it"),
            "{refused}"
        );

        let bad_blocking = StageEdit::SetBlocking {
            blocking: "sometimes".into(),
        }
        .apply(&setup)
        .unwrap_err();
        assert_eq!(bad_blocking.code, codes::BAD_REQUEST);
        assert!(
            bad_blocking.message.contains("bound-keys"),
            "{bad_blocking}"
        );

        // A selector nobody could have meant refuses with the shape to send.
        let bad_device = StageEdit::ChooseDevice {
            selector: "  ".into(),
            alias: "panel".into(),
            label: "x".into(),
        }
        .apply(&setup)
        .unwrap_err();
        assert_eq!(bad_device.code, codes::BAD_REQUEST);
        assert!(bad_device
            .remedy
            .as_deref()
            .unwrap()
            .contains("ksx device scan"));
    }

    /// A refused edit leaves the outcome carrying the setup UNCHANGED, so the
    /// screen the user is looking at is still true.
    #[test]
    fn a_refused_outcome_carries_the_setup_the_caller_still_has() {
        let mut setup = staged();
        for n in 1..=4 {
            setup = StageEdit::AddSlot {
                number: Some(n),
                persona: "xbox360".into(),
                preset: format!("P{n}"),
            }
            .apply(&setup)
            .unwrap();
        }
        let refusal = StageEdit::AddSlot {
            number: Some(5),
            persona: "xbox360".into(),
            preset: "P5".into(),
        }
        .apply(&setup)
        .unwrap_err();
        let outcome = StageOutcome::refused(&setup, &refusal);
        assert!(!outcome.ok);
        assert_eq!(outcome.code.as_deref(), Some("too-many-xinput-slots"));
        assert_eq!(outcome.setup.slots.len(), 4, "still four, not five");
        assert_eq!(outcome.setup.xinput_used, 4);
        assert!(outcome.headline().contains("is_xinput()"));
        // Nothing was saved and nothing is playing.
        assert_eq!(outcome.saved, None);
        assert!(!outcome.playing);

        // The fifth slot as a PlayStation pad is how players 5+ exist, and the
        // outcome says so.
        let fifth = StageEdit::AddSlot {
            number: Some(5),
            persona: "playstation".into(),
            preset: "P5".into(),
        }
        .apply(&setup)
        .unwrap();
        let ok = StageOutcome::ok(&fifth, "staged slot 5 as a PlayStation pad");
        assert!(ok.ok);
        assert_eq!(ok.setup.slots.len(), 5);
        assert_eq!(ok.setup.xinput_used, 4);
    }

    /// A surface with no staged setup says so, per click — never a silent
    /// no-op, and never an empty setup that reads as "you staged nothing".
    #[test]
    fn an_unavailable_stage_says_so_rather_than_rendering_nothing() {
        let outcome = StageOutcome::unavailable("this control source has no staged setup");
        assert!(!outcome.ok);
        assert_eq!(outcome.code.as_deref(), Some(codes::NOT_HERE));
        assert!(!outcome.setup.reachable);
        assert!(outcome.setup.error.is_some());
        assert!(outcome.refusal().is_some());
    }

    /// The wire shape survives a JSON round trip — a browser POSTs these.
    #[test]
    fn the_edits_round_trip_as_json() {
        let edits = vec![
            StageEdit::ChooseDevice {
                selector: "usb:d209:0430:00".into(),
                alias: "panel".into(),
                label: "I-PAC".into(),
            },
            StageEdit::AddSlot {
                number: None,
                persona: "xbox360".into(),
                preset: "P1".into(),
            },
            StageEdit::SetPersona {
                number: 1,
                persona: "playstation".into(),
            },
            StageEdit::RemoveSlot { number: 1 },
            StageEdit::SetBlocking {
                blocking: "whole".into(),
            },
            StageEdit::Discard,
        ];
        for edit in edits {
            let text = serde_json::to_string(&edit).unwrap();
            assert_eq!(serde_json::from_str::<StageEdit>(&text).unwrap(), edit);
        }
        // The tag is the field a surface switches on.
        let text = serde_json::to_string(&StageEdit::Discard).unwrap();
        assert_eq!(text, r#"{"edit":"discard"}"#);
    }
}
