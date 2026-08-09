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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use ksx_core::stage::{StagedDevice, StagedSetup};
use ksx_core::{Blocking, DeviceSelector, Key, Persona, TurboBinding, MAX_SLOTS, MAX_XINPUT_SLOTS};

use crate::control::{BindConflict, BindOutcome, MacroOutcome, MacroWrite};
use crate::machine::TemplateRow;
use crate::refusal::{codes, Refusal};
use crate::status::{MacroSnapshot, MapperSlot, MapperSnapshot};
use crate::wire::MacroWriteKind;

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
    /// The complete preset table being authored in memory.
    ///
    /// `Some` on every view composed from a live [`StagedSetup`]. Optional on
    /// the wire so a newer surface can still read a staged view sent by an
    /// older daemon; such a view remains readable, but cannot safely prepare a
    /// mapper edit until it is refreshed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoring: Option<ksx_config::PresetFile>,
    /// **How many bindings a key can actually reach**
    /// ([`ksx_core::Preset::live_bindings`]).
    ///
    /// `0` is a real answer and it is the one that matters: a pad with no
    /// bindings plugs, appears in the game, and does nothing — so
    /// `StagedSetup::commit` refuses it and a surface must be able to say why
    /// before anyone finds out with a stick in their hand.
    ///
    /// It counts LIVE rows, not `entries.len()`: `Preset::builtin_empty` lists
    /// every control with a `Key::None` placeholder, so the obvious count
    /// reports two dozen bindings for a pad on which nothing works. This field
    /// shipped as `entries.len() + chords.len()` and did exactly that.
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

/// **The escape hatch, and it is always live.**
///
/// `docs/FIRST-RUN.md` §3: "Two things must be said on that screen, not
/// buried." This is the first, and it is here rather than on a page because it
/// is a fact about `ksx-capture`'s escape latch (`escape.rs` — the capture
/// thread flips its OWN passthrough, which is why no UI can break it), not a
/// reassurance a surface is free to word.
pub const ESCAPE_HATCH_LINE: &str =
    "LeftCtrl five times always stops emulation and gives every keyboard back — in both modes. It \
     is handled in the capture thread itself, so no screen, no browser and no crashed UI can take \
     it away.";

/// **Freezing is not permanent and not global.** §3's second must-say.
pub const BLOCKING_SCOPE_LINE: &str =
    "This applies to the keyboard you picked, for this session only. Stopping the session ends it, \
     and no other keyboard on this PC is affected either way.";

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
    /// The preset name "Add a controller" would use, or `None` when there is no
    /// free slot.
    ///
    /// **Served, because it becomes a FILE NAME.** A save writes one preset per
    /// staged slot (`ksx-backend`'s `stage::apply`), so this string is the name of
    /// something that lands on disk — and a surface that invented it would be
    /// deciding, in TypeScript, what a first-run user's files are called. The
    /// first-run user is the whole audience here and `FIRST-RUN.md` §1 gives
    /// them no keyboard-and-shell step to rename anything afterwards.
    pub next_preset: Option<String>,
    /// How many staged slots occupy one of Windows' four XInput slots.
    pub xinput_used: usize,
    /// **Served, never hardcoded**: `ksx_core::MAX_SLOTS`.
    pub max_slots: u8,
    /// **Served, never hardcoded**: `ksx_core::MAX_XINPUT_SLOTS`.
    pub max_xinput_slots: u8,
    /// Every persona, with the flags that decide whether it may be offered.
    pub personas: Vec<PersonaOption>,
    /// **Every in-box layout a staged controller can be dressed in**, served
    /// for the reason the persona roster is: a list of panels typed into a
    /// surface is a second description of `ksx_core::templates`.
    ///
    /// It travels with the STAGED SETUP rather than being read alongside it,
    /// and that matters: layouts live in the binary, so a surface must be able
    /// to offer them on a machine whose presets folder cannot be read. The
    /// same rows are served by [`crate::PresetsView`] for seeding a new preset
    /// FILE — one [`TemplateRow::roster`], two consumers.
    ///
    /// This is what makes moment 6 reachable at all without a mapper: a pad
    /// has to bind something before `commit()` will play it, and an I-PAC on
    /// its factory chart is already described by `arcade-6button`
    /// (`docs/MAPPER-UX.md` commandment 9 — "the best mapping session is
    /// none").
    pub layouts: Vec<TemplateRow>,
    /// The layout id "Add a controller" offers first.
    ///
    /// **Served, because a first-run user must not have to choose one to get
    /// moving.** It is the named `keyboard-2p` desktop layout: unlike the
    /// declaration-order `arcade-6button` choice it replaced, it gives both
    /// player blocks distinct keys and includes Guide.
    pub default_layout: String,
    /// The blocking answers, in §3's own words.
    pub blocking_options: Vec<BlockingOption>,
    /// [`ESCAPE_HATCH_LINE`], served so it cannot be paraphrased on the way to
    /// a screen. §3 requires it beside the question, not buried.
    pub escape_hatch: String,
    /// [`BLOCKING_SCOPE_LINE`], same rule.
    pub blocking_scope: String,
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
                    authoring: Some(ksx_config::PresetFile::from_core(&slot.preset)),
                    bindings: slot.preset.live_bindings(),
                })
                .collect(),
            blocking: setup.blocking().map(|b| b.as_str().to_owned()),
            next_slot: setup.next_free_slot(),
            next_preset: setup.next_free_slot().map(preset_name_for_slot),
            xinput_used: setup.xinput_slots(),
            max_slots: MAX_SLOTS,
            max_xinput_slots: MAX_XINPUT_SLOTS,
            personas: PersonaOption::roster(),
            layouts: TemplateRow::roster(),
            default_layout: default_layout(),
            blocking_options: BlockingOption::roster(),
            escape_hatch: ESCAPE_HATCH_LINE.to_owned(),
            blocking_scope: BLOCKING_SCOPE_LINE.to_owned(),
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
        /// **The layout it starts from** — a [`TemplateRow::id`], served.
        ///
        /// Absent stages a controller that binds NOTHING, which
        /// `StagedSetup::commit` then refuses by name: that is the honest
        /// shape for a caller that means to map from scratch, and it is not
        /// something a first-run flow should reach by omission. The wire keeps
        /// it optional rather than required because the alternative is a
        /// caller inventing a template id.
        #[serde(default)]
        layout: Option<String>,
    },
    /// Moment 5 again: change their mind. Free, and the whole point.
    SetPersona { number: u8, persona: String },
    /// **Moment 6, the menu half: dress a staged controller in an in-box
    /// layout.**
    ///
    /// Instantiates one of [`ksx_core::templates`]'s tables into the slot's
    /// preset, keeping its NAME (the name is the file a save writes, and
    /// changing the layout does not rename anything). It lands through
    /// `StagedSetup::set_bindings` — the same core operation
    /// [`Self::SetBindings`] takes — so a layout is bindings in the stage, in
    /// memory, with no file written and no mapper opened.
    ///
    /// The surface names a template; it never composes a preset. That split is
    /// why a keyboard chart is not describable in TypeScript.
    SetLayout {
        number: u8,
        /// A [`TemplateRow::id`] (`arcade-6button`, `keyboard-wasd`…).
        layout: String,
        /// Which player block of the layout to use. Absent takes the block of
        /// the slot's own number, so a two-player panel dresses slots 1 and 2
        /// with the two halves it was authored for and nobody has to know that
        /// is what "player 2" means in the chart.
        #[serde(default)]
        player: Option<u8>,
    },
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
                layout,
            } => {
                let persona = parse_persona(persona)?;
                let name = preset.trim();
                // The slot this will take, decided BEFORE the layout is built,
                // because the player block follows the slot number.
                let number = match number {
                    Some(number) => *number,
                    None => setup
                        .next_free_slot()
                        .ok_or_else(|| refuse(ksx_core::StageRefusal::NoFreeSlot))?,
                };
                let preset = match layout.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                    Some(layout) => instantiate(layout, name, number, None)?,
                    // No layout named: a controller that binds nothing, which
                    // `commit()` refuses by name. Kept reachable on purpose —
                    // see the variant's docs.
                    None => ksx_core::Preset {
                        name: name.to_owned(),
                        entries: Vec::new(),
                        chords: Vec::new(),
                        macros: Default::default(),
                        turbo: Vec::new(),
                        protected: false,
                    },
                };
                setup.add_slot(number, persona, preset).map_err(refuse)
            }
            Self::SetPersona { number, persona } => setup
                .set_persona(*number, parse_persona(persona)?)
                .map_err(refuse),
            Self::SetLayout {
                number,
                layout,
                player,
            } => {
                // The NAME survives: it is the file a save writes, and
                // changing what a controller does must not rename it.
                let name = setup
                    .slot(*number)
                    .map(|slot| slot.preset.name.clone())
                    .ok_or_else(|| {
                        refuse(ksx_core::StageRefusal::NoSuchSlot { number: *number })
                    })?;
                let preset = instantiate(layout, &name, *number, *player)?;
                setup.set_bindings(*number, preset).map_err(refuse)
            }
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

/// One whole binding-row edit aimed at an in-memory staged controller.
///
/// This deliberately mirrors the existing mapper's request shape. `number`
/// selects the player and `preset` pins the controller-layout identity that
/// was visible when the action began.
/// An empty `keys` list (or the single canonical placeholder `None`) clears
/// the row. `turbo_hz = None` preserves an existing rate, while `Some(0)`
/// removes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedBindRequest {
    pub number: u8,
    /// Controller-layout name observed with `number` when the action began.
    /// The daemon checks both pieces of identity so removing and recreating a
    /// player at the same number cannot receive a stale browser write.
    #[serde(default)]
    pub preset: String,
    pub function: String,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub turbo_hz: Option<u32>,
}

/// One whole macro edit aimed at an exact in-memory staged controller.
///
/// The slot number is deliberately part of the request instead of being
/// inferred from the preset name. Two staged controllers may legitimately
/// author presets with similar names, and a stale browser tab must be refused
/// rather than redirected to whichever controller happens to be first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedMacroRequest {
    pub number: u8,
    #[serde(flatten)]
    pub write: MacroWrite,
}

/// A binding edit prepared entirely in memory.
///
/// A server sends [`Self::edit`] through `ControlSource::stage_edit`, then
/// passes that answer to [`Self::finish`]. The mapper can therefore return its
/// existing [`BindOutcome`] without inventing a staged-only response shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedBindEdit {
    pub edit: StageEdit,
    pub outcome: BindOutcome,
}

impl StagedBindEdit {
    /// Keep the precomposed mapper success when the stage accepted the edit,
    /// or amend it with the stage's exact refusal when the transport/daemon did
    /// not. No reload is ever claimed: staged bindings are memory only.
    pub fn finish(mut self, stage: &StageOutcome) -> BindOutcome {
        if !stage.ok {
            self.outcome.ok = false;
            self.outcome.message = None;
            self.outcome.error = Some(
                stage
                    .error
                    .clone()
                    .unwrap_or_else(|| "the staged setup was not changed".to_owned()),
            );
            self.outcome.code = Some(
                stage
                    .code
                    .clone()
                    .unwrap_or_else(|| codes::REFUSED.to_owned()),
            );
        }
        self.outcome.reloaded = false;
        self.outcome
    }
}

/// A macro edit prepared entirely in memory, with the existing macro editor's
/// outcome already composed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedMacroEdit {
    pub edit: StageEdit,
    pub outcome: MacroOutcome,
}

impl StagedMacroEdit {
    /// The macro counterpart of [`StagedBindEdit::finish`]. A staged write has
    /// neither a disk backup nor a live-session reload, even after success.
    pub fn finish(mut self, stage: &StageOutcome) -> MacroOutcome {
        if !stage.ok {
            self.outcome.ok = false;
            self.outcome.message = None;
            self.outcome.error = Some(
                stage
                    .error
                    .clone()
                    .unwrap_or_else(|| "the staged setup was not changed".to_owned()),
            );
            self.outcome.code = Some(
                stage
                    .code
                    .clone()
                    .unwrap_or_else(|| codes::REFUSED.to_owned()),
            );
        }
        self.outcome.backup = None;
        self.outcome.reloaded = false;
        self.outcome
    }
}

/// Prepare a staged binding edit from the complete setup view.
///
/// This is the convenient server entry point: it locates `request.number`,
/// checks every other staged slot for duplicate keys, and returns either one
/// atomic [`StageEdit::SetBindings`] or the ordinary mapper refusal.
#[allow(clippy::result_large_err)] // the refusal intentionally is the existing complete outcome
pub fn staged_bind_edit(
    setup: &StagedSetupView,
    request: &StagedBindRequest,
) -> Result<StagedBindEdit, BindOutcome> {
    if !setup.reachable {
        return Err(bind_refusal(
            codes::NOT_HERE,
            setup
                .error
                .clone()
                .unwrap_or_else(|| "this unsaved setup is unavailable".to_owned()),
            Vec::new(),
        ));
    }
    let Some(slot) = setup
        .slots
        .iter()
        .find(|slot| slot.number == request.number)
    else {
        return Err(bind_refusal(
            codes::BAD_SLOT,
            format!(
                "Player {} is no longer in this unsaved setup. Nothing changed.",
                request.number
            ),
            Vec::new(),
        ));
    };
    staged_slot_bind_edit(slot, &setup.slots, request)
}

/// Prepare a binding edit when the caller already selected the target slot.
/// `slots` must be the setup's complete staged roster so cross-slot duplicate
/// detection cannot silently omit a controller.
#[allow(clippy::result_large_err)] // the refusal intentionally is the existing complete outcome
pub fn staged_slot_bind_edit(
    slot: &StagedSlotView,
    slots: &[StagedSlotView],
    request: &StagedBindRequest,
) -> Result<StagedBindEdit, BindOutcome> {
    if request.number != slot.number {
        return Err(bind_refusal(
            codes::BAD_SLOT,
            format!(
                "This binding was aimed at Player {}, not Player {}. Nothing changed.",
                request.number, slot.number
            ),
            Vec::new(),
        ));
    }
    let Some(file) = slot.authoring.as_ref() else {
        return Err(bind_refusal(
            codes::NOT_HERE,
            format!(
                "Player {}'s controller layout is not available. Refresh the unsaved setup before mapping.",
                slot.number
            ),
            Vec::new(),
        ));
    };
    if !request.preset.trim().is_empty() && !request.preset.trim().eq_ignore_ascii_case(&file.name)
    {
        return Err(bind_refusal(
            codes::BAD_SLOT,
            format!(
                "This binding was opened for controller layout \"{}\", but Player {} now uses \"{}\". Nothing changed.",
                request.preset.trim(),
                slot.number,
                file.name
            ),
            Vec::new(),
        ));
    }
    let mut core = file.to_core().map_err(|err| {
        bind_refusal(
            codes::BAD_REQUEST,
            format!(
                "Player {}'s controller layout cannot be edited: {err}",
                slot.number
            ),
            Vec::new(),
        )
    })?;
    let keys = canonical_keys(&request.keys)
        .map_err(|message| bind_refusal(codes::BAD_REQUEST, message, Vec::new()))?;

    enum Target {
        Pad {
            binding: ksx_core::Binding,
            canonical: String,
        },
        Macro {
            index: u16,
            canonical: String,
        },
    }

    let function = request.function.trim();
    let target = if let Some(name) = ksx_config::macro_name(function) {
        let name = name.trim();
        if name.is_empty() {
            return Err(bind_refusal(
                codes::BAD_REQUEST,
                "a macro trigger needs a name after `macro.`",
                Vec::new(),
            ));
        }
        let Some(index) = core.macros.index_of(name) else {
            let known = core
                .macros
                .defs
                .iter()
                .map(|mac| mac.name.clone())
                .collect::<Vec<_>>();
            return Err(bind_refusal(
                codes::UNKNOWN_MACRO,
                format!(
                    "Player {} has no macro \"{name}\"{}",
                    slot.number,
                    if known.is_empty() {
                        String::new()
                    } else {
                        format!(" (known: {})", known.join(", "))
                    }
                ),
                Vec::new(),
            ));
        };
        let canonical = ksx_config::macro_function_name(&core.macros.defs[usize::from(index)].name);
        Target::Macro { index, canonical }
    } else {
        let binding = ksx_config::parse_function(function)
            .map_err(|err| bind_refusal(codes::BAD_REQUEST, err.to_string(), Vec::new()))?;
        Target::Pad {
            binding,
            canonical: ksx_config::function_name(&binding),
        }
    };

    let mut found = staged_cross_conflicts(slots, slot.number, &keys)
        .map_err(|message| bind_refusal(codes::BAD_REQUEST, message, Vec::new()))?;
    if let Target::Macro { index, .. } = &target {
        for key in &keys {
            for trigger in &core.macros.triggers {
                if trigger.index == *index || trigger.key != *key || trigger.key == Key::None {
                    continue;
                }
                if let Some(other) = core.macros.get(trigger.index) {
                    found.push((
                        key.name().to_owned(),
                        BindConflict {
                            // A pad multi-bind inside one controller layout is
                            // intentional and needs no question. Two MACRO
                            // triggers on one key are different: they launch
                            // two sequences, so the browser must ask before a
                            // forced fan-out instead of taking its legacy
                            // same-preset auto-retry.
                            scope: "macro".to_owned(),
                            preset: file.name.clone(),
                            function: ksx_config::macro_function_name(&other.name),
                            file: String::new(),
                            profile: None,
                            slot: Some(slot.number),
                        },
                    ));
                }
            }
        }
    }
    dedupe_staged_conflicts(&mut found);
    let conflicts: Vec<BindConflict> = found.iter().map(|(_, row)| row.clone()).collect();
    if !request.force {
        if let Some((key, row)) = found.first() {
            return Err(bind_refusal(
                codes::CONFLICT,
                format!(
                    "{}; choose “Use anyway” only if every listed control or macro should receive that key",
                    row.describe(key)
                ),
                conflicts,
            ));
        }
    }

    let (canonical, also_drives, turbo_hz, turbo_effective_hz) = match target {
        Target::Pad { binding, canonical } => {
            core.entries
                .retain(|(_, bound)| ksx_config::function_name(bound) != canonical);
            core.chords
                .retain(|chord| ksx_config::function_name(&chord.binding) != canonical);
            if keys.is_empty() {
                // Keep the mapper row visible after a clear. `None` is the
                // preset format's canonical inert placeholder.
                core.entries.push((Key::None, binding));
            } else {
                core.entries
                    .extend(keys.iter().copied().map(|key| (key, binding)));
            }

            match request.turbo_hz {
                Some(0) | None if keys.is_empty() => {
                    core.turbo.retain(|row| row.binding != binding)
                }
                None => {}
                Some(0) => core.turbo.retain(|row| row.binding != binding),
                Some(hz) => {
                    core.turbo.retain(|row| row.binding != binding);
                    core.turbo.push(TurboBinding::new(binding, hz));
                }
            }
            let also = other_functions_for_keys(&core, &keys, &canonical);
            let turbo = core
                .turbo
                .iter()
                .copied()
                .find(|row| row.binding == binding);
            (
                canonical,
                also,
                turbo.map(|row| row.hz),
                turbo.map(TurboBinding::effective_hz),
            )
        }
        Target::Macro { index, canonical } => {
            if request.turbo_hz.is_some_and(|hz| hz != 0) {
                return Err(bind_refusal(
                    codes::BAD_REQUEST,
                    format!(
                        "{canonical} is a macro trigger; its repeat rate belongs in the macro body, not on its trigger key"
                    ),
                    Vec::new(),
                ));
            }
            core.macros
                .triggers
                .retain(|trigger| trigger.index != index);
            core.macros.triggers.extend(
                keys.iter()
                    .copied()
                    .map(|key| ksx_core::MacroTrigger::new(key, index)),
            );
            let also = other_functions_for_keys(&core, &keys, &canonical);
            (canonical, also, None, None)
        }
    };

    let rewritten = ksx_config::PresetFile::from_core(&core);
    // The conversion is expected to be valid by construction. Keep the check
    // at the API edge so a future core field cannot produce a stage edit the
    // daemon alone would reject.
    rewritten.to_core().map_err(|err| {
        bind_refusal(
            codes::BAD_REQUEST,
            format!(
                "Player {}'s controller layout would be invalid: {err}",
                slot.number
            ),
            Vec::new(),
        )
    })?;
    let message = if keys.is_empty() {
        format!("Cleared {canonical} for Player {}.", slot.number)
    } else {
        format!(
            "Player {}: {canonical} = {}",
            slot.number,
            keys.iter()
                .map(|key| key.name())
                .collect::<Vec<_>>()
                .join(" · ")
        )
    };
    Ok(StagedBindEdit {
        edit: StageEdit::SetBindings {
            number: slot.number,
            preset: Box::new(rewritten),
        },
        outcome: BindOutcome {
            ok: true,
            message: Some(message),
            conflicts,
            also_drives,
            turbo_hz,
            turbo_effective_hz,
            // Staging is neither a disk write nor a live hot reload.
            reloaded: false,
            ..BindOutcome::default()
        },
    })
}

/// Prepare a whole-macro write for one staged slot.
///
/// Body replacement, deletion (including every trigger row), and enable
/// toggles follow [`MacroWrite`]'s existing meanings. Validation is the same
/// `ksx_config::validate` rule set used for files, but this helper never opens
/// a store, takes a backup, or reloads a session.
#[allow(clippy::result_large_err)] // the refusal intentionally is the existing complete outcome
pub fn staged_macro_edit(
    slot: &StagedSlotView,
    write: &MacroWrite,
) -> Result<StagedMacroEdit, MacroOutcome> {
    let Some(file) = slot.authoring.as_ref() else {
        return Err(macro_refusal(
            codes::NOT_HERE,
            format!(
                "Player {}'s controller layout is not available. Refresh the unsaved setup before editing macros.",
                slot.number
            ),
            Vec::new(),
        ));
    };
    if !write.preset.trim().eq_ignore_ascii_case(&file.name) {
        return Err(macro_refusal(
            codes::UNKNOWN_PRESET,
            format!(
                "This macro edit was opened for controller layout \"{}\", but Player {} now uses \"{}\". Nothing changed.",
                write.preset.trim(),
                slot.number,
                file.name
            ),
            Vec::new(),
        ));
    }
    let request = write
        .to_request()
        .map_err(|refusal| macro_refusal(&refusal.code, refusal.message, Vec::new()))?;
    let name = request.name.trim();
    if name.is_empty() {
        let message = "a macro needs a name; it is the name used by its `macro.<name>` trigger row";
        return Err(macro_refusal(
            codes::MACRO_INVALID,
            message,
            vec![message.to_owned()],
        ));
    }

    let existing = file
        .macros
        .keys()
        .find(|known| known.eq_ignore_ascii_case(name))
        .cloned();
    let mut next = file.clone();
    let mut warnings = Vec::new();
    let (deleted, enabled, toggled, message) = match request.write {
        MacroWriteKind::Toggle(enabled) => {
            let Some(key) = existing else {
                return Err(unknown_staged_macro(slot.number, file, name));
            };
            next.macros
                .get_mut(&key)
                .expect("the existing macro key came from this map")
                .enabled = enabled;
            (
                false,
                enabled,
                true,
                format!(
                    "Macro \"{key}\" is {} for Player {}.",
                    if enabled { "enabled" } else { "disabled" },
                    slot.number
                ),
            )
        }
        MacroWriteKind::Delete => {
            let Some(key) = existing else {
                return Err(unknown_staged_macro(slot.number, file, name));
            };
            next.macros.remove(&key);
            next.bindings.retain(|function, _| {
                !ksx_config::macro_name(function)
                    .is_some_and(|bound| bound.eq_ignore_ascii_case(&key))
            });
            (
                true,
                true,
                false,
                format!(
                    "Macro \"{key}\" and its trigger keys were removed for Player {}.",
                    slot.number
                ),
            )
        }
        MacroWriteKind::Body(body) => {
            let (problems, advisories) = staged_macro_body_issues(&file.name, name, &body);
            if !problems.is_empty() {
                return Err(macro_refusal(
                    codes::MACRO_INVALID,
                    format!("macro \"{name}\" is invalid"),
                    problems,
                ));
            }
            warnings = advisories;
            let key = existing.unwrap_or_else(|| name.to_owned());
            let enabled = body.enabled;
            next.macros.insert(key.clone(), *body);
            (
                false,
                enabled,
                false,
                format!("Macro \"{key}\" was updated for Player {}.", slot.number),
            )
        }
    };

    if !toggled {
        let before = staged_preset_problems(file);
        let broke: Vec<String> = staged_preset_problems(&next)
            .into_iter()
            .filter(|problem| !before.contains(problem))
            .filter(|problem| !warnings.contains(problem))
            .collect();
        if !broke.is_empty() {
            return Err(macro_refusal(
                codes::MACRO_INVALID,
                format!(
                    "Macro \"{name}\" would make Player {}'s controller layout invalid",
                    slot.number
                ),
                broke,
            ));
        }
        if let Err(err) = next.to_core() {
            return Err(macro_refusal(
                codes::MACRO_INVALID,
                err.to_string(),
                vec![err.to_string()],
            ));
        }
    }

    Ok(StagedMacroEdit {
        edit: StageEdit::SetBindings {
            number: slot.number,
            preset: Box::new(next),
        },
        outcome: MacroOutcome {
            ok: true,
            message: Some(message),
            warnings,
            deleted,
            enabled,
            toggled,
            // A staged edit has no disk road-home and does not hot reload.
            backup: None,
            reloaded: false,
            ..MacroOutcome::default()
        },
    })
}

/// Prepare a staged macro edit from the complete setup view, selecting only
/// the exact slot named by the request.
///
/// This setup-scoped entry point is the macro counterpart of
/// [`staged_bind_edit`]. It exists so a daemon can perform selection,
/// preparation, validation, and application while holding its one staged
/// state lock; no caller has to guess by preset name or fall back to slot 1.
#[allow(clippy::result_large_err)] // the refusal intentionally is the existing complete outcome
pub fn staged_macro_edit_for_setup(
    setup: &StagedSetupView,
    request: &StagedMacroRequest,
) -> Result<StagedMacroEdit, MacroOutcome> {
    if !setup.reachable {
        return Err(macro_refusal(
            codes::NOT_HERE,
            setup
                .error
                .clone()
                .unwrap_or_else(|| "this unsaved setup is unavailable".to_owned()),
            Vec::new(),
        ));
    }
    let Some(slot) = setup
        .slots
        .iter()
        .find(|slot| slot.number == request.number)
    else {
        return Err(macro_refusal(
            codes::BAD_SLOT,
            format!(
                "Player {} is no longer in this unsaved setup. Nothing changed.",
                request.number
            ),
            Vec::new(),
        ));
    };
    staged_macro_edit(slot, &request.write)
}

/// Convert one staged slot to the saved mapper's existing slot shape.
pub fn staged_mapper_slot(slot: &StagedSlotView, keyboard: &str) -> Result<MapperSlot, Refusal> {
    let file = slot.authoring.as_ref().ok_or_else(|| {
        Refusal::new(
            codes::NOT_HERE,
            format!(
                "Player {}'s controller layout is not available. Refresh the unsaved setup.",
                slot.number
            ),
        )
    })?;
    let core = file.to_core().map_err(|err| {
        Refusal::new(
            codes::BAD_REQUEST,
            format!(
                "Player {}'s controller layout cannot be shown: {err}",
                slot.number
            ),
        )
    })?;
    let mut bindings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, binding) in &core.entries {
        let keys = bindings
            .entry(ksx_config::function_name(binding))
            .or_default();
        if *key != Key::None {
            keys.push(key.name().to_owned());
        }
    }
    let turbo = core
        .turbo
        .iter()
        .map(|row| (ksx_config::function_name(&row.binding), row.hz))
        .collect();
    Ok(MapperSlot {
        number: slot.number,
        persona: slot.persona.clone(),
        persona_label: slot.persona_label.clone(),
        preset: file.name.clone(),
        keyboard: keyboard.to_owned(),
        bindings,
        // Nothing staged has ever been backed up to disk.
        backup: None,
        session_backup: false,
        turbo,
        macros_off: false,
    })
}

/// Convert a complete staged setup to the mapper's existing point-in-time
/// snapshot. All labels are stable in-memory facts; no clock or config root is
/// read, which keeps this conversion pure and deterministic.
pub fn staged_mapper_snapshot(setup: &StagedSetupView) -> MapperSnapshot {
    if !setup.reachable {
        return MapperSnapshot::unavailable(
            setup
                .error
                .as_deref()
                .unwrap_or("the staged setup is unavailable"),
        );
    }
    let keyboard = setup
        .device
        .as_ref()
        .map(|device| device.alias.as_str())
        .unwrap_or("(any)");
    let mut slots = Vec::with_capacity(setup.slots.len());
    for slot in &setup.slots {
        match staged_mapper_slot(slot, keyboard) {
            Ok(slot) => slots.push(slot),
            Err(refusal) => return MapperSnapshot::unavailable(&refusal.message),
        }
    }
    MapperSnapshot {
        generated_at: "(staged)".to_owned(),
        source: "unsaved setup".to_owned(),
        profile: None,
        config_root: "(not saved)".to_owned(),
        slots,
    }
}

/// Convert one staged preset to the macro editor's existing snapshot.
pub fn staged_macro_snapshot(slot: &StagedSlotView) -> MacroSnapshot {
    match &slot.authoring {
        Some(file) => MacroSnapshot::from_preset(file),
        None => MacroSnapshot::unavailable(&format!(
            "Player {}'s controller layout is not available. Refresh the unsaved setup.",
            slot.number
        )),
    }
}

fn bind_refusal(
    code: &str,
    message: impl Into<String>,
    conflicts: Vec<BindConflict>,
) -> BindOutcome {
    BindOutcome {
        ok: false,
        error: Some(message.into()),
        code: Some(code.to_owned()),
        conflicts,
        reloaded: false,
        ..BindOutcome::default()
    }
}

fn macro_refusal(code: &str, message: impl Into<String>, problems: Vec<String>) -> MacroOutcome {
    MacroOutcome {
        ok: false,
        error: Some(message.into()),
        code: Some(code.to_owned()),
        problems,
        backup: None,
        reloaded: false,
        ..MacroOutcome::default()
    }
}

fn unknown_staged_macro(number: u8, file: &ksx_config::PresetFile, name: &str) -> MacroOutcome {
    let known = file.macros.keys().cloned().collect::<Vec<_>>();
    macro_refusal(
        codes::UNKNOWN_MACRO,
        format!(
            "Player {number} has no macro \"{name}\"{}",
            if known.is_empty() {
                String::new()
            } else {
                format!(" (known: {})", known.join(", "))
            }
        ),
        Vec::new(),
    )
}

fn canonical_keys(words: &[String]) -> Result<Vec<Key>, String> {
    let mut out = Vec::new();
    let mut none = false;
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            return Err(
                "a binding key name cannot be blank; send an empty key list to clear it".to_owned(),
            );
        }
        let Some(key) = Key::ALL
            .iter()
            .copied()
            .find(|key| key.name().eq_ignore_ascii_case(word))
        else {
            return Err(format!("unknown key name '{word}'"));
        };
        if key == Key::None {
            none = true;
        } else if !out.contains(&key) {
            out.push(key);
        }
    }
    if none && !out.is_empty() {
        return Err(
            "`None` is the clear placeholder and cannot be mixed with live keys".to_owned(),
        );
    }
    Ok(out)
}

fn staged_cross_conflicts(
    slots: &[StagedSlotView],
    target: u8,
    keys: &[Key],
) -> Result<Vec<(String, BindConflict)>, String> {
    let mut found = Vec::new();
    for slot in slots.iter().filter(|slot| slot.number != target) {
        let file = slot.authoring.as_ref().ok_or_else(|| {
            format!(
                "staged slot {} has no authoring snapshot; refresh before checking duplicate keys",
                slot.number
            )
        })?;
        let core = file.to_core().map_err(|err| {
            format!(
                "staged preset \"{}\" cannot be checked for duplicate keys: {err}",
                file.name
            )
        })?;
        for key in keys {
            for function in functions_for_key(&core, *key) {
                found.push((
                    key.name().to_owned(),
                    BindConflict {
                        scope: "stage".to_owned(),
                        preset: file.name.clone(),
                        function,
                        file: String::new(),
                        profile: None,
                        slot: Some(slot.number),
                    },
                ));
            }
        }
    }
    Ok(found)
}

fn functions_for_key(preset: &ksx_core::Preset, key: Key) -> Vec<String> {
    let mut functions = Vec::new();
    functions.extend(
        preset
            .entries
            .iter()
            .filter(|(bound, _)| *bound == key)
            .map(|(_, binding)| ksx_config::function_name(binding)),
    );
    functions.extend(
        preset
            .chords
            .iter()
            .filter(|chord| chord.key == key)
            .map(|chord| ksx_config::function_name(&chord.binding)),
    );
    functions.extend(
        preset
            .macros
            .triggers
            .iter()
            .filter(|trigger| trigger.key == key)
            .filter_map(|trigger| preset.macros.get(trigger.index))
            .map(|mac| ksx_config::macro_function_name(&mac.name)),
    );
    functions.sort();
    functions.dedup();
    functions
}

fn other_functions_for_keys(preset: &ksx_core::Preset, keys: &[Key], target: &str) -> Vec<String> {
    let mut functions = keys
        .iter()
        .flat_map(|key| functions_for_key(preset, *key))
        .filter(|function| function != target)
        .collect::<Vec<_>>();
    functions.sort();
    functions.dedup();
    functions
}

fn dedupe_staged_conflicts(conflicts: &mut Vec<(String, BindConflict)>) {
    let mut seen = BTreeSet::new();
    conflicts.retain(|(key, row)| {
        seen.insert((
            key.clone(),
            row.scope.clone(),
            row.preset.clone(),
            row.function.clone(),
            row.slot,
        ))
    });
}

fn staged_macro_body_issues(
    preset_name: &str,
    name: &str,
    body: &ksx_config::MacroFile,
) -> (Vec<String>, Vec<String>) {
    let solo = ksx_config::PresetFile {
        name: preset_name.to_owned(),
        bindings: BTreeMap::new(),
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
        problems.push(
            "an empty step list is not a macro delete; use the delete flag if removal is intended"
                .to_owned(),
        );
    }
    (problems, advisories)
}

fn staged_preset_problems(file: &ksx_config::PresetFile) -> BTreeSet<String> {
    ksx_config::validate(
        &ksx_config::ConfigFile::default(),
        std::slice::from_ref(file),
    )
    .into_iter()
    .filter(|issue| !issue.is_advisory())
    .map(|issue| issue.to_string())
    .collect()
}

/// The preset a controller staged into slot `number` binds, by default.
///
/// "Player 1", not "slot1" or "preset-1": it is shown to someone who has never
/// seen ksx, and it is the name of the file the mapper will then list. One
/// implementation, because the string is on the staging screen, in the flash
/// after a save, and on a file.
pub fn preset_name_for_slot(number: u8) -> String {
    format!("Player {number}")
}

/// The layout id "Add a controller" offers first.
///
/// # Why this is a NAME and not "the first roster entry"
///
/// It used to be `roster().find(|l| !l.blank)`, which is declaration order
/// wearing a rule's clothing: it returned `arcade-6button` because that
/// `Template` happens to be written first in `ksx_core::templates`. So the
/// person `docs/FIRST-RUN.md` is about — at a desk, on the keyboard they
/// already own — was offered an I-PAC six-button *cabinet* chart, keyed to
/// arrows plus `LeftControl/LeftAlt/Space/Z/X` and `1`/`5` for start and coin.
///
/// Two things fell out of that, and the second is why this is a defect rather
/// than a preference:
///
/// 1. It is the wrong chart for the machine. An arcade template describes a
///    panel wired to an encoder's factory chart, and a laptop is not one.
/// 2. **It silently broke FIRST-RUN.md moment 7.** No arcade template binds
///    `Guide` — a real panel has no spare button for it — so the default path
///    could not send the controller button that asks Windows to open Game Bar
///    when the user's Game Bar setting allows it.
///
/// # Why `keyboard-2p` and not `keyboard-wasd`
///
/// `keyboard-wasd` binds Guide and was the obvious pick, and it is wrong: it is
/// `players: 1`. Staging a second controller takes the layout's SECOND player
/// block, and a one-player layout has none — so slot 2 fell back to another
/// chart and the two collided on `D`. A test already guarded that ("two players
/// on one panel must not share a key") and caught it.
///
/// `keyboard-2p` is the two-player desktop layout, so slot 2 is a real second
/// half rather than a fallback — and it now binds Guide too (`LeftWindows` /
/// `NumpadAsterisk`), which it should have from the start: every persona
/// exposes Guide, so a desktop layout omitting it was an oversight rather than
/// a decision.
///
/// An arcade owner still picks their chart in one click. They came here knowing
/// they have a cabinet, which is exactly the knowledge a first-run desktop user
/// does not have.
const DEFAULT_LAYOUT: &str = "keyboard-2p";

fn default_layout() -> String {
    debug_assert!(
        TemplateRow::roster()
            .iter()
            .any(|layout| layout.id == DEFAULT_LAYOUT && !layout.blank),
        "DEFAULT_LAYOUT must name a roster entry that binds something"
    );
    DEFAULT_LAYOUT.to_owned()
}

/// One in-box layout as a real preset, or the refusal that names what to send.
///
/// `player` defaults to the SLOT NUMBER: a two-player panel dresses slots 1
/// and 2 with the two halves it was authored for, which is what the chart
/// means and not something a first-run user should have to know.
fn instantiate(
    layout: &str,
    name: &str,
    number: u8,
    player: Option<u8>,
) -> Result<ksx_core::Preset, Refusal> {
    let player = player.unwrap_or(number);
    ksx_core::templates::instantiate(layout.trim(), name, player).map_err(|err| {
        let remedy = match &err {
            // The template exists and does not reach this slot. Name the ones
            // that do rather than leaving "player 3" as the user's problem —
            // this is the ordinary case for player 3 on a two-player panel.
            ksx_core::templates::TemplateError::NoSuchPlayer { players, .. } => format!(
                "this layout carries {players} player block(s), so slot {number} has none of its \
                 own — pick a layout with more blocks ({}), or send an explicit player block to \
                 share one deliberately",
                blocks_at_least(number).join(" | ")
            ),
            _ => format!(
                "send one of the served layout ids ({})",
                TemplateRow::roster()
                    .iter()
                    .map(|l| l.id.clone())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        };
        Refusal::with_remedy(codes::BAD_REQUEST, err.to_string(), remedy)
    })
}

/// The layout ids carrying a block for slot `number` — read off the roster, so
/// a refusal never names a layout this build does not have.
fn blocks_at_least(number: u8) -> Vec<String> {
    TemplateRow::roster()
        .into_iter()
        .filter(|layout| layout.players.contains(&number) && !layout.blank)
        .map(|layout| layout.id)
        .collect()
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

    fn authored_preset() -> ksx_config::PresetFile {
        toml::from_str(
            r#"
name = "Player 1"

[bindings]
A = "S"
B = { key = "D", turbo_hz = 7 }
rt = { key = "Q", when = ["W"] }
"macro.hadouken" = "P"

[macros.hadouken]
steps = [{ hold = ["dpad.down", "A"], frames = 3, allow_short = true }]
"#,
        )
        .expect("the authoring fixture is a preset file")
    }

    fn staged_with_preset(file: &ksx_config::PresetFile) -> StagedSetup {
        staged()
            .add_slot(
                1,
                Persona::Xbox360,
                file.to_core().expect("the authoring fixture loads"),
            )
            .expect("slot 1 stages")
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

    /// **§3's two must-says travel with the question.**
    ///
    /// Breaks against a view that served only the three options: the escape
    /// hatch and the "this is per-keyboard, per-session" scope would then be
    /// composed on whichever screen asked, in whatever words that screen chose,
    /// and the browser's copy would be a second description of what the capture
    /// thread does. The first one is not reassurance — it is the only thing
    /// standing between a frozen keyboard and a reboot.
    #[test]
    fn the_blocking_question_carries_its_two_must_says() {
        let view = StagedSetupView::of(&StagedSetup::new());
        assert_eq!(view.escape_hatch, ESCAPE_HATCH_LINE);
        assert_eq!(view.blocking_scope, BLOCKING_SCOPE_LINE);
        assert!(view.escape_hatch.contains("LeftCtrl five times"));
        assert!(
            view.escape_hatch.contains("both modes"),
            "the hatch works under Freeze AND Split; a sentence that said it only \
             for one would be worse than none"
        );
        assert!(view.blocking_scope.contains("this session only"));
        // A screen with no daemon still has to be able to say them — that
        // screen is exactly where somebody is reading about how to get out.
        let down = StagedSetupView::unreachable("no daemon answered");
        assert_eq!(down.escape_hatch, ESCAPE_HATCH_LINE);
        assert_eq!(down.blocking_scope, BLOCKING_SCOPE_LINE);
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
        // The preset name travels WITH the slot number, because it is the name
        // of the file a save writes. A surface that composed "Player 1" for
        // itself would be naming someone's files in TypeScript.
        assert_eq!(view.next_preset.as_deref(), Some("Player 1"));
        assert!(!view.ready, "a keyboard with no controller drives nothing");
        assert!(view.not_ready.as_deref().unwrap().contains("controller"));

        let setup = StageEdit::AddSlot {
            number: None,
            persona: "ps4".into(), // an alias a human would type
            preset: "Player 1".into(),
            layout: Some("arcade-6button".into()),
        }
        .apply(&setup)
        .unwrap();
        let view = StagedSetupView::of(&setup);
        assert_eq!(view.slots[0].number, 1);
        assert_eq!(view.slots[0].persona, "playstation");
        assert_eq!(view.slots[0].persona_label, "PlayStation");
        assert!(!view.slots[0].is_xinput);
        assert!(
            view.slots[0].bindings > 0,
            "a controller staged from a layout binds something"
        );
        assert_eq!(view.xinput_used, 0);
        assert!(
            !view.ready,
            "§3's question has not been asked, and it decides whether the keyboard can \
             still type"
        );
        assert_eq!(view.blocking, None, "§3 has not been asked yet");
        assert!(
            view.not_ready
                .as_deref()
                .unwrap()
                .contains("split-or-freeze"),
            "{:?}",
            view.not_ready
        );

        let setup = StageEdit::SetBlocking {
            blocking: "bound-keys".into(),
        }
        .apply(&setup)
        .unwrap();
        let view = StagedSetupView::of(&setup);
        assert_eq!(view.blocking.as_deref(), Some("bound-keys"));
        assert!(view.ready, "a keyboard, a mapped controller and an answer");

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
            layout: None,
        }
        .apply(&setup)
        .unwrap_err();
        assert_eq!(typo.code, codes::BAD_REQUEST);
        assert!(typo.message.contains("playstation"), "{typo}");

        let refused = StageEdit::AddSlot {
            number: None,
            persona: "dualsense".into(),
            preset: "P1".into(),
            layout: None,
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
                layout: Some("arcade-4way".into()),
            }
            .apply(&setup)
            .unwrap();
        }
        let refusal = StageEdit::AddSlot {
            number: Some(5),
            persona: "xbox360".into(),
            preset: "P5".into(),
            layout: None,
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
            layout: None,
        }
        .apply(&setup)
        .unwrap();
        let ok = StageOutcome::ok(&fifth, "staged slot 5 as a PlayStation pad");
        assert!(ok.ok);
        assert_eq!(ok.setup.slots.len(), 5);
        assert_eq!(ok.setup.xinput_used, 4);
    }

    /// **A controller staged from a layout binds real keys, in the stage, with
    /// no file written — and the roster it was picked from is SERVED.**
    ///
    /// Breaks against the shipped `AddSlot`, which staged
    /// `entries: Vec::new()` unconditionally. That version left mapping as
    /// something that could only happen in the mapper, and the mapper edits
    /// preset FILES — so the only journey to a working pad was Save (a write
    /// nobody asked for) → leave → map → come back → and then Play started the
    /// STAGED, still-empty preset anyway.
    ///
    /// It also breaks against a page that listed the layouts itself: the panel
    /// text and the player counts are `ksx_core::templates`', and a second copy
    /// in TypeScript is `CLAUDE.md`'s one rule, broken.
    #[test]
    fn a_controller_can_be_dressed_in_a_served_layout_without_touching_a_file() {
        let view = StagedSetupView::of(&ksx_core::stage::StagedSetup::new());
        assert_eq!(view.layouts.len(), ksx_core::TEMPLATES.len());
        assert!(
            !view.default_layout.is_empty(),
            "a first-run user must not have to choose a layout to get moving"
        );
        let default = view
            .layouts
            .iter()
            .find(|l| l.id == view.default_layout)
            .expect("the default is one of the served options");
        assert!(
            !default.blank,
            "the offered default must bind something, or Add-a-controller is one click \
             from a pad that does nothing"
        );
        assert!(!default.detail.is_empty(), "a layout nobody can identify");
        // The one option that binds nothing is FLAGGED, not hidden: hiding it
        // would decide for the user, and offering it silently would offer the
        // one choice that cannot play.
        assert!(view.layouts.iter().any(|l| l.blank && l.id == "empty"));

        // Staged, and the bindings are really there — in memory.
        let setup = StageEdit::AddSlot {
            number: None,
            persona: "xbox360".into(),
            preset: "Player 1".into(),
            layout: Some(view.default_layout.clone()),
        }
        .apply(&staged())
        .unwrap();
        assert!(setup.slot(1).unwrap().preset.live_bindings() > 10);
        assert_eq!(
            setup.slot(1).unwrap().preset.name,
            "Player 1",
            "the layout dresses the preset; it does not rename it"
        );

        // Slot 2 takes the layout's SECOND player block, so a two-player panel
        // gives two players different keys without anybody being asked what a
        // "player block" is.
        //
        // BOTH slots take `view.default_layout`, and that is the point: this
        // asserted non-collision across a hardcoded `"arcade-6button"` and
        // whatever the default happened to be, which passed only while those
        // two were the same string. When the default moved it failed — not
        // because the property broke, but because two DIFFERENT charts share
        // keys by construction and always did. The invariant worth holding is
        // the one the comment above describes: one layout, two players, no
        // overlap.
        let two = StageEdit::AddSlot {
            number: None,
            persona: "playstation".into(),
            preset: "Player 2".into(),
            layout: Some(view.default_layout.clone()),
        }
        .apply(&setup)
        .unwrap();
        let p1: Vec<_> = two.slot(1).unwrap().preset.bound_keys().collect();
        let p2: Vec<_> = two.slot(2).unwrap().preset.bound_keys().collect();
        assert!(!p2.is_empty());
        assert!(
            p1.iter().all(|key| !p2.contains(key)),
            "two players on one panel must not share a key: {p1:?} vs {p2:?}"
        );

        // Changing the layout is as free as changing the persona — §2.
        let rewired = StageEdit::SetLayout {
            number: 1,
            layout: "keyboard-wasd".into(),
            player: None,
        }
        .apply(&two)
        .unwrap();
        assert!(rewired
            .slot(1)
            .unwrap()
            .preset
            .bound_keys()
            .any(|key| key == ksx_core::Key::W));
    }

    /// A layout that has no block for this slot refuses by naming the layouts
    /// that do — never by silently reusing player 1's keys, which would give
    /// two players the same stick.
    #[test]
    fn a_layout_with_no_block_for_this_slot_names_the_ones_that_have_it() {
        let setup = StageEdit::AddSlot {
            number: Some(3),
            persona: "playstation".into(),
            // One player block, and slot 3 is asking for a third.
            preset: "Player 3".into(),
            layout: Some("keyboard-wasd".into()),
        }
        .apply(&staged())
        .unwrap_err();
        assert_eq!(setup.code, codes::BAD_REQUEST);
        assert!(setup.message.contains("player block"), "{setup}");
        let remedy = setup.remedy.as_deref().unwrap();
        assert!(remedy.contains("arcade-4way"), "{remedy}");
        assert!(
            !remedy.contains("empty"),
            "a remedy must not send anybody at the layout that binds nothing: {remedy}"
        );

        // An id nobody could have meant names the served ids.
        let one = StageEdit::AddSlot {
            number: None,
            persona: "xbox360".into(),
            preset: "Player 1".into(),
            layout: Some("arcade-6button".into()),
        }
        .apply(&staged())
        .unwrap();
        let unknown = StageEdit::SetLayout {
            number: 1,
            layout: "hitbox".into(),
            player: None,
        }
        .apply(&one)
        .unwrap_err();
        assert_eq!(unknown.code, codes::BAD_REQUEST);
        assert!(
            unknown
                .remedy
                .as_deref()
                .unwrap()
                .contains("arcade-6button"),
            "{unknown}"
        );
    }

    /// **`ready` is false until the setup can actually be played, and the
    /// reason is ksx-core's own sentence.**
    ///
    /// Breaks against the shipped view in two places at once: a slot with no
    /// bindings was `ready` (Play plugged a dead pad), and an unanswered
    /// split-or-freeze question was `ready` (Save wrote Freeze from a question
    /// nobody was shown). Both were reachable from the page as it shipped, in
    /// two clicks.
    #[test]
    fn ready_is_false_for_a_dead_pad_and_for_an_unanswered_question() {
        let blank = StageEdit::AddSlot {
            number: None,
            persona: "xbox360".into(),
            preset: "Player 1".into(),
            layout: None,
        }
        .apply(&staged())
        .unwrap();
        let view = StagedSetupView::of(&blank);
        assert_eq!(view.slots[0].bindings, 0);
        assert!(!view.ready, "a pad with no bindings is not ready to play");
        let why = view.not_ready.as_deref().unwrap();
        assert!(why.contains("slot 1"), "it names the slot: {why}");

        // Answering the question does NOT make a dead pad playable — the two
        // gates are independent, and the dead pad is the one that gets named.
        let answered = StageEdit::SetBlocking {
            blocking: "whole".into(),
        }
        .apply(&blank)
        .unwrap();
        let view = StagedSetupView::of(&answered);
        assert!(!view.ready);
        assert!(view.not_ready.as_deref().unwrap().contains("slot 1"));

        // A real layout, no answer: still not ready, and now the QUESTION is
        // what it says.
        let mapped = StageEdit::SetLayout {
            number: 1,
            layout: "arcade-6button".into(),
            player: None,
        }
        .apply(&blank)
        .unwrap();
        let view = StagedSetupView::of(&mapped);
        assert!(!view.ready);
        assert!(view
            .not_ready
            .as_deref()
            .unwrap()
            .contains("split-or-freeze"));
        assert_eq!(view.blocking, None, "and it is still NOT pre-answered");

        // Both, and only then.
        let ready = StageEdit::SetBlocking {
            blocking: "bound-keys".into(),
        }
        .apply(&mapped)
        .unwrap();
        let view = StagedSetupView::of(&ready);
        assert!(view.ready, "{:?}", view.not_ready);
        assert_eq!(view.not_ready, None);
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
                layout: Some("arcade-6button".into()),
            },
            StageEdit::SetLayout {
                number: 1,
                layout: "keyboard-wasd".into(),
                player: None,
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

    #[test]
    fn every_live_slot_carries_a_full_authoring_snapshot_with_old_wire_fallback() {
        let file = authored_preset();
        let setup = staged_with_preset(&file);
        let view = StagedSetupView::of(&setup);
        let slot = &view.slots[0];
        assert_eq!(
            slot.authoring.as_ref(),
            Some(&ksx_config::PresetFile::from_core(
                &setup.slot(1).unwrap().preset
            ))
        );
        assert!(slot
            .authoring
            .as_ref()
            .unwrap()
            .macros
            .contains_key("hadouken"));

        let mut old_wire = serde_json::to_value(slot).unwrap();
        old_wire
            .as_object_mut()
            .expect("a slot is an object")
            .remove("authoring");
        let old: StagedSlotView = serde_json::from_value(old_wire).unwrap();
        assert_eq!(old.authoring, None, "an older daemon remains readable");
    }

    #[test]
    fn staged_bindings_canonicalize_multi_keys_and_round_trip_turbo_chords_and_macros() {
        let setup = staged_with_preset(&authored_preset());
        let before = StagedSetupView::of(&setup);
        let prepared = staged_bind_edit(
            &before,
            &StagedBindRequest {
                number: 1,
                preset: "Player 1".into(),
                function: "a".into(),
                keys: vec!["g".into(), "enter".into(), "G".into()],
                force: false,
                turbo_hz: Some(12),
            },
        )
        .expect("a free multi-key binding stages");
        assert!(prepared.outcome.ok);
        assert_eq!(
            prepared.outcome.message.as_deref(),
            Some("Player 1: A = G · Enter")
        );
        assert_eq!(prepared.outcome.turbo_hz, Some(12));
        assert!(!prepared.outcome.reloaded, "no live reload claim");

        let StageEdit::SetBindings { preset, .. } = &prepared.edit else {
            panic!("the helper must compose one whole SetBindings edit")
        };
        let core = preset.to_core().unwrap();
        let a_keys = core
            .entries
            .iter()
            .filter(|(_, binding)| ksx_config::function_name(binding) == "A")
            .map(|(key, _)| key.name())
            .collect::<Vec<_>>();
        assert_eq!(a_keys, vec!["G", "Enter"], "canonical and deduplicated");
        assert_eq!(core.chords.len(), 1, "an unrelated chord survives");
        assert_eq!(core.macros.defs.len(), 1, "macro bodies survive");
        assert_eq!(core.macros.triggers.len(), 1, "macro triggers survive");
        assert!(core
            .entries
            .iter()
            .any(|(key, binding)| *key == Key::D && ksx_config::function_name(binding) == "B"));

        let changed = prepared.edit.apply(&setup).unwrap();
        let changed_view = StagedSetupView::of(&changed);
        let round_trip = changed_view.slots[0].authoring.as_ref().unwrap();
        assert_eq!(round_trip.to_core().unwrap(), core);

        // The clear spelling is the mapper's empty list or the preset's
        // explicit `None`; either way the edit writes one inert row and drops
        // that function's turbo clock.
        let cleared = staged_bind_edit(
            &changed_view,
            &StagedBindRequest {
                number: 1,
                preset: "Player 1".into(),
                function: "A".into(),
                keys: vec!["none".into()],
                force: false,
                turbo_hz: None,
            },
        )
        .expect("None is the canonical clear placeholder");
        assert_eq!(
            cleared.outcome.message.as_deref(),
            Some("Cleared A for Player 1.")
        );
        let StageEdit::SetBindings { preset, .. } = &cleared.edit else {
            unreachable!()
        };
        let core = preset.to_core().unwrap();
        assert!(core.entries.iter().any(|(key, binding)| {
            *key == Key::None && ksx_config::function_name(binding) == "A"
        }));
        assert!(!core
            .turbo
            .iter()
            .any(|row| ksx_config::function_name(&row.binding) == "A"));
    }

    #[test]
    fn missing_and_recreated_player_binding_targets_are_refused_in_customer_words() {
        let setup = staged_with_preset(&authored_preset());
        let view = StagedSetupView::of(&setup);
        let missing = staged_bind_edit(
            &view,
            &StagedBindRequest {
                number: 9,
                preset: "Player 9".into(),
                function: "A".into(),
                keys: vec!["G".into()],
                ..StagedBindRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(missing.code.as_deref(), Some(codes::BAD_SLOT));
        assert_eq!(
            missing.error.as_deref(),
            Some("Player 9 is no longer in this unsaved setup. Nothing changed.")
        );

        let recycled = staged_bind_edit(
            &view,
            &StagedBindRequest {
                number: 1,
                preset: "The layout that was removed".into(),
                function: "A".into(),
                keys: vec!["G".into()],
                ..StagedBindRequest::default()
            },
        )
        .unwrap_err();
        assert_eq!(recycled.code.as_deref(), Some(codes::BAD_SLOT));
        let error = recycled.error.as_deref().unwrap_or_default();
        assert!(error.contains("controller layout"), "{error}");
        assert!(error.contains("Nothing changed"), "{error}");
        assert!(
            !error.contains("staged") && !error.contains("preset file"),
            "{error}"
        );
    }

    #[test]
    fn a_cross_staged_slot_duplicate_refuses_without_mutating_and_force_is_explicit() {
        let layout = default_layout();
        let one = StageEdit::AddSlot {
            number: Some(1),
            persona: "xbox360".into(),
            preset: "Player 1".into(),
            layout: Some(layout.clone()),
        }
        .apply(&staged())
        .unwrap();
        let two = StageEdit::AddSlot {
            number: Some(2),
            persona: "playstation".into(),
            preset: "Player 2".into(),
            layout: Some(layout),
        }
        .apply(&one)
        .unwrap();
        let view = StagedSetupView::of(&two);
        let before = view.clone();
        let occupied = view.slots[1]
            .authoring
            .as_ref()
            .unwrap()
            .to_core()
            .unwrap()
            .entries
            .iter()
            .find(|(key, _)| *key != Key::None)
            .map(|(key, _)| key.name().to_owned())
            .unwrap();
        let request = StagedBindRequest {
            number: 1,
            preset: "Player 1".into(),
            function: "A".into(),
            keys: vec![occupied],
            force: false,
            turbo_hz: None,
        };
        let refused = staged_bind_edit(&view, &request).unwrap_err();
        assert_eq!(refused.code.as_deref(), Some(codes::CONFLICT));
        assert_eq!(refused.conflicts[0].scope, "stage");
        assert_eq!(refused.conflicts[0].slot, Some(2));
        assert_eq!(view, before, "a pure refusal changes no staged value");
        let error = refused.error.as_deref().unwrap_or_default();
        assert!(error.contains("Use anyway"), "{error}");
        assert!(
            !error.contains("force") && !error.contains("staged"),
            "{error}"
        );

        let forced = staged_bind_edit(
            &view,
            &StagedBindRequest {
                force: true,
                ..request
            },
        )
        .expect("force stages the duplicate without touching the other slot");
        assert!(forced.outcome.ok);
        assert!(!forced.outcome.conflicts.is_empty());
        assert!(!forced.outcome.reloaded);
    }

    #[test]
    fn staged_macro_body_toggle_delete_and_trigger_rows_are_one_memory_only_round_trip() {
        let setup = staged_with_preset(&authored_preset());
        let view = StagedSetupView::of(&setup);
        let body = MacroWrite {
            preset: "Player 1".into(),
            name: "HADOUKEN".into(),
            steps: vec![crate::MacroStepView {
                hold: vec!["dpad.down".into(), "A".into()],
                frames: Some(4),
                allow_short: true,
                ..crate::MacroStepView::default()
            }],
            repeat: "turbo".into(),
            turbo_hz: Some(10),
            enabled: Some(false),
            // Even if a saved-preset form asks for reload, staging never claims
            // that it happened.
            reload: true,
            ..MacroWrite::default()
        };
        let prepared = staged_macro_edit(&view.slots[0], &body).expect("valid macro body");
        assert!(prepared.outcome.ok);
        assert_eq!(
            prepared.outcome.message.as_deref(),
            Some("Macro \"hadouken\" was updated for Player 1.")
        );
        assert!(!prepared.outcome.enabled);
        assert_eq!(prepared.outcome.backup, None);
        assert!(!prepared.outcome.reloaded);
        let with_body = prepared.edit.apply(&setup).unwrap();

        let with_body_view = StagedSetupView::of(&with_body);
        let trigger = staged_bind_edit(
            &with_body_view,
            &StagedBindRequest {
                number: 1,
                preset: "Player 1".into(),
                function: "MACRO.Hadouken".into(),
                keys: vec!["p".into(), "leftcontrol".into()],
                force: false,
                turbo_hz: None,
            },
        )
        .expect("macro trigger rows use the same binding helper");
        let with_trigger = trigger.edit.apply(&with_body).unwrap();
        let macro_view = staged_macro_snapshot(&StagedSetupView::of(&with_trigger).slots[0]);
        assert!(macro_view.available);
        assert_eq!(macro_view.macros[0].name, "hadouken");
        assert_eq!(macro_view.macros[0].triggers, vec!["P", "LeftControl"]);
        assert!(macro_view.macros[0].disabled);
        assert_eq!(macro_view.macros[0].repeat, "turbo");
        assert_eq!(macro_view.macros[0].turbo_hz, Some(10));

        let with_trigger_view = StagedSetupView::of(&with_trigger);
        let toggle = staged_macro_edit(
            &with_trigger_view.slots[0],
            &MacroWrite {
                preset: "Player 1".into(),
                name: "hadouken".into(),
                enabled: Some(true),
                ..MacroWrite::default()
            },
        )
        .expect("an enable toggle keeps body and triggers");
        assert!(toggle.outcome.toggled && toggle.outcome.enabled);
        let enabled = toggle.edit.apply(&with_trigger).unwrap();
        let enabled_macro = staged_macro_snapshot(&StagedSetupView::of(&enabled).slots[0]);
        assert_eq!(enabled_macro.macros[0].steps[0].frames, Some(4));
        assert_eq!(enabled_macro.macros[0].triggers, vec!["P", "LeftControl"]);
        assert!(!enabled_macro.macros[0].disabled);

        let enabled_view = StagedSetupView::of(&enabled);
        let delete = staged_macro_edit(
            &enabled_view.slots[0],
            &MacroWrite {
                preset: "Player 1".into(),
                name: "HADOUKEN".into(),
                delete: true,
                ..MacroWrite::default()
            },
        )
        .expect("delete removes the body and its triggers");
        assert!(delete.outcome.deleted);
        assert_eq!(delete.outcome.backup, None);
        let deleted = delete.edit.apply(&enabled).unwrap();
        let deleted_view = StagedSetupView::of(&deleted);
        let file = deleted_view.slots[0].authoring.as_ref().unwrap();
        assert!(file.macros.is_empty());
        assert!(!file.bindings.keys().any(|function| {
            ksx_config::macro_name(function)
                .is_some_and(|name| name.eq_ignore_ascii_case("hadouken"))
        }));
    }

    #[test]
    fn a_key_that_already_starts_another_macro_needs_an_explicit_decision() {
        let setup = staged_with_preset(&authored_preset());
        let view = StagedSetupView::of(&setup);
        let second = staged_macro_edit(
            &view.slots[0],
            &MacroWrite {
                preset: "Player 1".into(),
                name: "uppercut".into(),
                steps: vec![crate::MacroStepView {
                    hold: vec!["A".into()],
                    ms: Some(50),
                    ..crate::MacroStepView::default()
                }],
                ..MacroWrite::default()
            },
        )
        .expect("the second macro is valid")
        .edit
        .apply(&setup)
        .unwrap();
        let view = StagedSetupView::of(&second);
        let request = StagedBindRequest {
            number: 1,
            preset: "Player 1".into(),
            function: "macro.uppercut".into(),
            keys: vec!["P".into()],
            ..StagedBindRequest::default()
        };
        let refused = staged_bind_edit(&view, &request).unwrap_err();
        assert_eq!(refused.code.as_deref(), Some(codes::CONFLICT));
        assert_eq!(refused.conflicts.len(), 1);
        assert_eq!(refused.conflicts[0].scope, "macro");
        assert_eq!(refused.conflicts[0].function, "macro.hadouken");
        let error = refused.error.as_deref().unwrap_or_default();
        assert!(error.contains("Use anyway"), "{error}");
        assert!(!error.contains("force"), "{error}");

        let forced = staged_bind_edit(
            &view,
            &StagedBindRequest {
                force: true,
                ..request
            },
        )
        .expect("an explicit decision may share a macro trigger");
        assert!(forced.outcome.ok);
    }

    #[test]
    fn invalid_staged_macro_is_refused_with_no_mutation_backup_or_reload_claim() {
        let setup = staged_with_preset(&authored_preset());
        let view = StagedSetupView::of(&setup);
        let before = view.slots[0].clone();
        let refused = staged_macro_edit(
            &view.slots[0],
            &MacroWrite {
                preset: "Player 1".into(),
                name: "hadouken".into(),
                steps: vec![crate::MacroStepView {
                    hold: vec!["warp".into()],
                    ms: Some(10),
                    frames: Some(1),
                    ..crate::MacroStepView::default()
                }],
                ..MacroWrite::default()
            },
        )
        .unwrap_err();
        assert_eq!(refused.code.as_deref(), Some(codes::MACRO_INVALID));
        assert!(!refused.problems.is_empty());
        assert_eq!(refused.backup, None);
        assert!(!refused.reloaded);
        assert_eq!(view.slots[0], before);
    }

    #[test]
    fn staged_mapper_and_macro_views_match_the_existing_saved_shapes_without_io() {
        let setup = staged_with_preset(&authored_preset());
        let view = StagedSetupView::of(&setup);
        let slot = staged_mapper_slot(&view.slots[0], "panel").unwrap();
        assert_eq!(slot.number, 1);
        assert_eq!(slot.preset, "Player 1");
        assert_eq!(slot.keyboard, "panel");
        assert_eq!(slot.bindings.get("A"), Some(&vec!["S".to_owned()]));
        assert_eq!(slot.bindings.get("B"), Some(&vec!["D".to_owned()]));
        assert_eq!(slot.turbo.get("B"), Some(&7));
        assert_eq!(slot.backup, None, "an in-memory preset has no disk backup");

        let snapshot = staged_mapper_snapshot(&view);
        assert_eq!(snapshot.generated_at, "(staged)");
        assert_eq!(snapshot.config_root, "(not saved)");
        assert_eq!(
            snapshot.slots,
            vec![staged_mapper_slot(&view.slots[0], "panel").unwrap()]
        );

        let macros = staged_macro_snapshot(&view.slots[0]);
        assert!(macros.available);
        assert_eq!(macros.preset, slot.preset);
        assert_eq!(macros.macros.len(), 1);
        assert_eq!(macros.macros[0].triggers, vec!["P"]);
        assert_eq!(macros.macros[0].steps[0].frames, Some(3));
        assert!(macros.macros[0].steps[0].allow_short);
    }

    #[test]
    fn prepared_mapper_outcomes_adopt_a_later_stage_refusal() {
        let setup = staged_with_preset(&authored_preset());
        let view = StagedSetupView::of(&setup);
        let prepared = staged_bind_edit(
            &view,
            &StagedBindRequest {
                number: 1,
                function: "A".into(),
                keys: vec!["G".into()],
                ..StagedBindRequest::default()
            },
        )
        .unwrap();
        let refused = StageOutcome {
            ok: false,
            error: Some("daemon rejected the stage edit".into()),
            code: Some("stage-raced".into()),
            setup: view,
            ..StageOutcome::default()
        };
        let outcome = prepared.finish(&refused);
        assert!(!outcome.ok);
        assert_eq!(outcome.code.as_deref(), Some("stage-raced"));
        assert_eq!(
            outcome.error.as_deref(),
            Some("daemon rejected the stage edit")
        );
        assert!(!outcome.reloaded);
    }

    /// **FIRST-RUN.md moment 7, and the reason `DEFAULT_LAYOUT` is a name.**
    ///
    /// Moment 7 offers Guide as the controller shortcut that can ask Windows to
    /// open Game Bar when the user's Windows setting allows it. A first-run
    /// user may take the offered layout unchanged, so both player blocks must
    /// carry the exact physical keys the screen describes.
    ///
    /// Fails against the previous `roster().find(|l| !l.blank)`: that returned
    /// `arcade-6button`, which binds Start and Back and no Guide, because a
    /// real arcade panel has no spare button for one.
    #[test]
    fn the_offered_layout_binds_the_documented_guide_key_for_both_players() {
        let offered = default_layout();
        let guide_keys = |player, name| {
            instantiate(&offered, name, player, None)
                .expect("the offered layout must instantiate")
                .entries
                .iter()
                .filter_map(|(key, binding)| {
                    (*binding == ksx_core::Binding::Button(ksx_core::XButton::Guide))
                        .then_some(*key)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            guide_keys(1, "Player 1"),
            vec![ksx_core::Key::LeftWindows],
            "the offered layout ({offered}) must keep P1's documented Guide key"
        );
        assert_eq!(
            guide_keys(2, "Player 2"),
            vec![ksx_core::Key::NumpadAsterisk],
            "the offered layout ({offered}) must keep P2's documented Guide key"
        );
    }

    /// The offered layout must also actually bind something — the property the
    /// old roster scan was protecting, kept now that the id is spelled.
    #[test]
    fn the_offered_layout_is_never_a_blank_one() {
        let offered = default_layout();
        let row = TemplateRow::roster()
            .into_iter()
            .find(|layout| layout.id == offered)
            .expect("DEFAULT_LAYOUT must name a real roster entry");
        assert!(
            !row.blank,
            "{offered} is blank: one click from a pad that does nothing"
        );
    }
}
