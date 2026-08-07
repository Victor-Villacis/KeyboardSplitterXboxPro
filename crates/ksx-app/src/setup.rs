//! `ksx setup` — the first-contact wizard: a fresh machine to a working panel
//! without hand-writing a line of TOML.
//!
//! This is Build B of `docs/MAPPER-UX.md`, pointed at a new install rather
//! than at one slot of an existing one, and it copies EmulationStation's shape
//! deliberately (`docs/research/mapping-ux-field-study.md` §Track 2 — the
//! strings were read off this cabinet's own RetroBat install):
//!
//! 1. **Identify by PRESS, never by a numbered list.** "Hold a key on the
//!    panel for player 1" — on a cabinet with two identical encoders, a list
//!    of hardware ids is not an answer anyone can give (`docs/USE-CASES.md`
//!    T4, and commandment 1: the physical press is the pointer).
//! 2. **Sequential, POSITION-NAMED prompts.** The prompt says `SOUTH`, not
//!    `A`, because the letter on a Nintendo pad is in the other place and half
//!    the arcade world's panels are labelled by position anyway. What gets
//!    STORED is our own function vocabulary (`A`, `dpad.up`, `lx.min`) — the
//!    prompt is for the human, the name is for the file (commandment 6).
//! 3. **Auto-advance on press**, with the press echoed as it lands.
//! 4. **Inline ALREADY TAKEN.** A key already used by this run is refused at
//!    the moment it is pressed, with the control that holds it named, and the
//!    prompt stays put. (Deliberate multi-binds are a `ksx map` job — inside a
//!    first-contact run a duplicate is a misfire nine times out of ten.)
//! 5. **Completeness audit before commit.** A panel with no Start and no Back
//!    cannot leave a game; ES learned this the hard way with its hotkey audit.
//! 6. **Transactional.** Nothing is written until the review screen is
//!    confirmed. DISCARD is always the default answer.
//!
//! # The one deliberate deviation: skipping
//!
//! ES skips a control by HOLDING a key for two seconds. ksx cannot: its
//! observer is `ksx_capture::observe_next_key`, a Raw Input sink that reports
//! MAKES, and it re-baselines every key that is already down when it starts
//! (`docs/research/padforge-code-audit.md` §1.2) — so a held key becomes
//! invisible rather than continuous, and the only way to see a hold would be
//! to probe autorepeat for a second after EVERY press, taxing all twenty
//! bindings to make one skip cheap.
//!
//! So skipping is the same affordance inverted: **press nothing.** Each prompt
//! runs a visible per-second countdown ([`Options::step_secs`]); when it
//! expires the control is skipped and the wizard advances. Two skips in a row
//! end the run early and skip everything left — which makes bailing out of the
//! optional tail cost ~12 s instead of ES's ~40 s of holding.
//!
//! `Escape` cancels the whole run. It is the only reserved key; a panel that
//! genuinely has Escape wired can be given it afterwards with `ksx map`.
//!
//! # Exit codes
//!
//! 0 = written, discarded, or a dry run (all three are the user's answer being
//! honoured), 1 = error, [`EXIT_REFUSED`] (2) = refused before anything was
//! written (no device identified, nothing bound, an unusable `--profile`).

// Off Windows only the stub `run` is reachable, but the whole state machine
// (and its tests) stays compiled — that is the half worth testing everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use ksx_config::{ConfigRoot, DeviceEntry, GameSlotEntry, PresetFile, SlotEntry, Store};
use ksx_core::{
    Axis, Binding, DpadDirection, Key, Preset, Trigger, XButton, AXIS_MAX, AXIS_MIN, MAX_SLOTS,
};

/// Refused — nothing was written.
pub const EXIT_REFUSED: i32 = 2;

/// How long each prompt listens before the control is skipped.
pub const DEFAULT_STEP_SECS: u64 = 6;

/// How long the "hold a key on the panel" identify prompt listens. Longer than
/// a control prompt: the user is still working out which panel is which.
pub const IDENTIFY_SECS: u64 = 20;

/// Consecutive silent prompts that end the run early. Two, because one is
/// "I have no button for that" and two is "I am done".
const IDLE_STREAK_TO_FINISH: usize = 2;

// ---------------------------------------------------------------------------
// The prompt table
// ---------------------------------------------------------------------------

/// One prompt: what the human is asked, and what the press binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Stable id — `--json`, and what the tests name.
    pub id: &'static str,
    /// POSITION-NAMED prompt (commandment 6).
    pub prompt: &'static str,
    /// The plain-language gloss printed under the prompt.
    pub hint: &'static str,
    /// Everything one press binds. A stick direction binds TWO outputs — the
    /// hat and the axis — because some games read only one of them and
    /// keyboard fan-out is native (`docs/INPUT-TRANSFORMS.md` §1a).
    pub bindings: &'static [Binding],
    /// Does the completeness audit care about this one?
    pub essential: bool,
}

const fn axis(axis: Axis, value: i16) -> Binding {
    Binding::Axis { axis, value }
}

/// The run, in order: stick, faces, shoulders, triggers, system, thumbs, right
/// stick. Essentials first so that a run abandoned halfway is still a usable
/// panel, and the optional tail last so "press nothing twice" ends it cheaply.
pub const STEPS: &[Step] = &[
    Step {
        id: "up",
        prompt: "UP",
        hint: "push the stick up (binds the D-pad and the left stick)",
        bindings: &[Binding::Dpad(DpadDirection::Up), axis(Axis::Y, AXIS_MAX)],
        essential: true,
    },
    Step {
        id: "down",
        prompt: "DOWN",
        hint: "pull the stick down",
        bindings: &[Binding::Dpad(DpadDirection::Down), axis(Axis::Y, AXIS_MIN)],
        essential: true,
    },
    Step {
        id: "left",
        prompt: "LEFT",
        hint: "push the stick left",
        bindings: &[Binding::Dpad(DpadDirection::Left), axis(Axis::X, AXIS_MIN)],
        essential: true,
    },
    Step {
        id: "right",
        prompt: "RIGHT",
        hint: "push the stick right",
        bindings: &[Binding::Dpad(DpadDirection::Right), axis(Axis::X, AXIS_MAX)],
        essential: true,
    },
    Step {
        id: "south",
        prompt: "SOUTH",
        hint: "the BOTTOM face button (A on an Xbox pad, cross on a PlayStation one)",
        bindings: &[Binding::Button(XButton::A)],
        essential: true,
    },
    Step {
        id: "east",
        prompt: "EAST",
        hint: "the RIGHT face button (B / circle)",
        bindings: &[Binding::Button(XButton::B)],
        essential: true,
    },
    Step {
        id: "west",
        prompt: "WEST",
        hint: "the LEFT face button (X / square)",
        bindings: &[Binding::Button(XButton::X)],
        essential: true,
    },
    Step {
        id: "north",
        prompt: "NORTH",
        hint: "the TOP face button (Y / triangle)",
        bindings: &[Binding::Button(XButton::Y)],
        essential: true,
    },
    Step {
        id: "start",
        prompt: "START",
        hint: "the START button — on a cabinet this is how a game is left",
        bindings: &[Binding::Button(XButton::Start)],
        essential: true,
    },
    Step {
        id: "back",
        prompt: "BACK / SELECT",
        hint: "the COIN or SELECT button — the cabinet's other exit key",
        bindings: &[Binding::Button(XButton::Back)],
        essential: true,
    },
    Step {
        id: "shoulder-left",
        prompt: "LEFT SHOULDER",
        hint: "LB — button 7 on a six-button panel, if it has one",
        bindings: &[Binding::Button(XButton::LeftBumper)],
        essential: false,
    },
    Step {
        id: "shoulder-right",
        prompt: "RIGHT SHOULDER",
        hint: "RB — often the heavy punch on a fighting panel",
        bindings: &[Binding::Button(XButton::RightBumper)],
        essential: false,
    },
    Step {
        id: "trigger-left",
        prompt: "LEFT TRIGGER",
        hint: "LT",
        bindings: &[Binding::Trigger(Trigger::Left)],
        essential: false,
    },
    Step {
        id: "trigger-right",
        prompt: "RIGHT TRIGGER",
        hint: "RT — often the heavy kick on a fighting panel",
        bindings: &[Binding::Trigger(Trigger::Right)],
        essential: false,
    },
    Step {
        id: "guide",
        prompt: "GUIDE",
        hint: "the big centre button, if your panel has a spare one",
        bindings: &[Binding::Button(XButton::Guide)],
        essential: false,
    },
    Step {
        id: "thumb-left",
        prompt: "LEFT STICK CLICK",
        hint: "L3",
        bindings: &[Binding::Button(XButton::LeftThumb)],
        essential: false,
    },
    Step {
        id: "thumb-right",
        prompt: "RIGHT STICK CLICK",
        hint: "R3",
        bindings: &[Binding::Button(XButton::RightThumb)],
        essential: false,
    },
    Step {
        id: "rstick-up",
        prompt: "RIGHT STICK UP",
        hint: "a second stick, if the panel has one — skip on an arcade panel",
        bindings: &[axis(Axis::Ry, AXIS_MAX)],
        essential: false,
    },
    Step {
        id: "rstick-down",
        prompt: "RIGHT STICK DOWN",
        hint: "",
        bindings: &[axis(Axis::Ry, AXIS_MIN)],
        essential: false,
    },
    Step {
        id: "rstick-left",
        prompt: "RIGHT STICK LEFT",
        hint: "",
        bindings: &[axis(Axis::Rx, AXIS_MIN)],
        essential: false,
    },
    Step {
        id: "rstick-right",
        prompt: "RIGHT STICK RIGHT",
        hint: "",
        bindings: &[axis(Axis::Rx, AXIS_MAX)],
        essential: false,
    },
];

// ---------------------------------------------------------------------------
// The state machine (pure — this is what the tests drive)
// ---------------------------------------------------------------------------

/// What the observer heard, already classified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A key came down on `device`.
    Press { device: String, key: Key },
    /// The prompt's countdown expired with nothing pressed.
    Timeout,
    /// `Escape`, or the console's Ctrl+C.
    Cancel,
    /// The observer itself failed (no Raw Input, a lost window).
    Failed(String),
}

/// Why a control ended up unbound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The countdown expired.
    Idle,
    /// The run ended early and this control was never asked.
    NotAsked,
}

/// What one [`Input`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reaction {
    /// The panel answered; the control prompts start now.
    Identified { device: String },
    /// Still listening for the panel (a silent identify prompt).
    StillListening { attempts_left: usize },
    /// Bound and advanced.
    Bound { step: &'static str, key: Key },
    /// The key already drives another control in this run — ES's inline
    /// ALREADY TAKEN. The prompt does NOT advance.
    AlreadyTaken { key: Key, taken_by: &'static str },
    /// A press from a device other than the identified panel. Refused so a
    /// stray keystroke on the desk keyboard cannot land in a panel's preset.
    OtherDevice { device: String },
    /// Skipped and advanced.
    Skipped {
        step: &'static str,
        reason: SkipReason,
    },
    /// Two silent prompts in a row: everything left was skipped.
    FinishedEarly { skipped: usize },
    /// Every prompt has been answered or skipped — the review screen is next.
    Complete,
    /// The run is over and nothing will be written.
    Cancelled(CancelReason),
}

/// Why a run stopped without reaching the review screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// Escape.
    User,
    /// Nothing ever pressed at the identify prompt.
    NoDevice,
    /// The observer broke.
    Failed(String),
}

/// Where the run is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// "Hold a key on the panel for player N."
    Identify,
    /// Walking [`STEPS`].
    Control,
    /// Everything asked; audit + confirm.
    Review,
    /// Over, nothing to write.
    Cancelled(CancelReason),
}

/// One control's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Pending,
    Bound(Key),
    Skipped(SkipReason),
}

/// One slot's run. Holds everything in memory and writes nothing — assembling
/// the preset is [`Wizard::preset`], and only the caller's confirmed commit
/// touches a file.
#[derive(Clone, Debug)]
pub struct Wizard {
    slot: u8,
    steps: &'static [Step],
    cursor: usize,
    device: Option<String>,
    decisions: Vec<Decision>,
    phase: Phase,
    idle_streak: usize,
    identify_attempts: usize,
}

impl Wizard {
    pub fn new(slot: u8) -> Self {
        Self::with_steps(slot, STEPS)
    }

    pub fn with_steps(slot: u8, steps: &'static [Step]) -> Self {
        Self {
            slot,
            steps,
            cursor: 0,
            device: None,
            decisions: vec![Decision::Pending; steps.len()],
            phase: Phase::Identify,
            idle_streak: 0,
            identify_attempts: 0,
        }
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The device instance path the panel identified itself with.
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// The prompt on screen, or `None` once every control has been answered.
    pub fn current(&self) -> Option<&'static Step> {
        match self.phase {
            Phase::Control => self.steps.get(self.cursor),
            _ => None,
        }
    }

    /// `(answered, total)` — the progress line.
    pub fn progress(&self) -> (usize, usize) {
        (self.cursor.min(self.steps.len()), self.steps.len())
    }

    /// Every control that got a key, in prompt order.
    pub fn bound(&self) -> Vec<(&'static Step, Key)> {
        self.steps
            .iter()
            .zip(&self.decisions)
            .filter_map(|(step, decision)| match decision {
                Decision::Bound(key) => Some((step, *key)),
                _ => None,
            })
            .collect()
    }

    /// Every control that did not.
    pub fn skipped(&self) -> Vec<(&'static Step, SkipReason)> {
        self.steps
            .iter()
            .zip(&self.decisions)
            .filter_map(|(step, decision)| match decision {
                Decision::Skipped(reason) => Some((step, *reason)),
                _ => None,
            })
            .collect()
    }

    /// Is there anything worth writing?
    pub fn can_commit(&self) -> bool {
        self.device.is_some() && !self.bound().is_empty()
    }

    /// Feed one observation. Every state transition in the wizard goes through
    /// here, which is why the interactive loop can stay a printer.
    pub fn feed(&mut self, input: Input) -> Reaction {
        if let Input::Failed(error) = input {
            let reason = CancelReason::Failed(error);
            self.phase = Phase::Cancelled(reason.clone());
            return Reaction::Cancelled(reason);
        }
        if input == Input::Cancel {
            self.phase = Phase::Cancelled(CancelReason::User);
            return Reaction::Cancelled(CancelReason::User);
        }
        match self.phase {
            Phase::Identify => self.feed_identify(input),
            Phase::Control => self.feed_control(input),
            Phase::Review => Reaction::Complete,
            Phase::Cancelled(ref reason) => Reaction::Cancelled(reason.clone()),
        }
    }

    fn feed_identify(&mut self, input: Input) -> Reaction {
        match input {
            Input::Press { device, .. } => {
                self.device = Some(device.clone());
                self.phase = Phase::Control;
                self.idle_streak = 0;
                Reaction::Identified { device }
            }
            Input::Timeout => {
                self.identify_attempts += 1;
                let left = IDLE_STREAK_TO_FINISH.saturating_sub(self.identify_attempts);
                if left == 0 {
                    self.phase = Phase::Cancelled(CancelReason::NoDevice);
                    Reaction::Cancelled(CancelReason::NoDevice)
                } else {
                    Reaction::StillListening {
                        attempts_left: left,
                    }
                }
            }
            // Cancel/Failed are handled before the phase match.
            _ => unreachable!("cancel and failure are handled by feed()"),
        }
    }

    fn feed_control(&mut self, input: Input) -> Reaction {
        let Some(step) = self.steps.get(self.cursor) else {
            self.phase = Phase::Review;
            return Reaction::Complete;
        };
        match input {
            Input::Press { device, key } => {
                if self.device.as_deref() != Some(device.as_str()) {
                    return Reaction::OtherDevice { device };
                }
                if let Some(taken_by) = self.holder_of(key) {
                    return Reaction::AlreadyTaken { key, taken_by };
                }
                self.decisions[self.cursor] = Decision::Bound(key);
                self.idle_streak = 0;
                self.advance();
                Reaction::Bound { step: step.id, key }
            }
            Input::Timeout => {
                self.decisions[self.cursor] = Decision::Skipped(SkipReason::Idle);
                self.idle_streak += 1;
                self.advance();
                if self.idle_streak >= IDLE_STREAK_TO_FINISH && self.phase == Phase::Control {
                    let skipped = self.skip_the_rest();
                    Reaction::FinishedEarly { skipped }
                } else {
                    Reaction::Skipped {
                        step: step.id,
                        reason: SkipReason::Idle,
                    }
                }
            }
            _ => unreachable!("cancel and failure are handled by feed()"),
        }
    }

    /// The control already holding `key`, if any — the ALREADY TAKEN lookup.
    fn holder_of(&self, key: Key) -> Option<&'static str> {
        self.steps
            .iter()
            .zip(&self.decisions)
            .find_map(|(step, decision)| match decision {
                Decision::Bound(bound) if *bound == key => Some(step.id),
                _ => None,
            })
    }

    fn advance(&mut self) {
        self.cursor += 1;
        if self.cursor >= self.steps.len() {
            self.cursor = self.steps.len();
            self.phase = Phase::Review;
        }
    }

    /// Mark everything still pending as never-asked and go to review.
    fn skip_the_rest(&mut self) -> usize {
        let mut skipped = 0;
        let from = self.cursor.min(self.decisions.len());
        for decision in &mut self.decisions[from..] {
            if *decision == Decision::Pending {
                *decision = Decision::Skipped(SkipReason::NotAsked);
                skipped += 1;
            }
        }
        self.cursor = self.steps.len();
        self.phase = Phase::Review;
        skipped
    }

    /// TRANSACTIONAL ASSEMBLY: the preset this run would write, built fresh
    /// from the decisions every time it is asked. Nothing here touches disk,
    /// so the review screen and the commit are looking at the same object.
    pub fn preset(&self, name: &str) -> Preset {
        let mut entries = Vec::new();
        for (step, decision) in self.steps.iter().zip(&self.decisions) {
            if let Decision::Bound(key) = decision {
                for binding in step.bindings {
                    entries.push((*key, *binding));
                }
            }
        }
        Preset {
            name: name.to_owned(),
            entries,
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        }
    }

    /// The completeness audit, run before the commit is offered.
    pub fn audit(&self) -> Vec<AuditNote> {
        let mut notes = Vec::new();
        let bound = self.bound();
        if bound.is_empty() {
            notes.push(AuditNote::warn(
                "nothing-bound",
                "No control was bound — there is nothing to write.".to_owned(),
            ));
            return notes;
        }

        let has = |id: &str| bound.iter().any(|(step, _)| step.id == id);
        let start = has("start");
        let back = has("back");
        match (start, back) {
            (false, false) => notes.push(AuditNote::warn(
                "no-exit",
                "Neither START nor BACK is bound. On a cabinet those are the \
                 exit keys — with the panel captured there would be no way to \
                 leave a game from it. Bind at least one before you rely on \
                 this panel."
                    .to_owned(),
            )),
            (false, true) => notes.push(AuditNote::warn(
                "no-start",
                "START is not bound. Most games pause and quit through it.".to_owned(),
            )),
            (true, false) => notes.push(AuditNote::warn(
                "no-back",
                "BACK / SELECT is not bound (the coin button on a cabinet).".to_owned(),
            )),
            (true, true) => {}
        }

        let directions = ["up", "down", "left", "right"]
            .iter()
            .filter(|id| has(id))
            .count();
        if directions == 0 {
            notes.push(AuditNote::warn(
                "no-directions",
                "No direction is bound — this pad cannot move.".to_owned(),
            ));
        } else if directions < 4 {
            notes.push(AuditNote::warn(
                "partial-directions",
                format!("Only {directions} of 4 directions are bound."),
            ));
        }

        let faces = ["south", "east", "west", "north"]
            .iter()
            .filter(|id| has(id))
            .count();
        if faces < 2 {
            notes.push(AuditNote::warn(
                "few-buttons",
                format!("Only {faces} face button(s) bound; most games need two."),
            ));
        }

        // "Unbound", not "skipped": counted from what was BOUND, so the note
        // is honest whether a control was skipped, never asked, or the run is
        // being audited mid-flight.
        let unbound = self.steps.len() - bound.len();
        if unbound > 0 {
            notes.push(AuditNote::info(
                "skipped",
                format!(
                    "{unbound} control(s) left unbound. `ksx map` adds any of \
                     them later without redoing this."
                ),
            ));
        }
        notes
    }
}

/// How loud an audit note is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditLevel {
    Warn,
    Info,
}

impl AuditLevel {
    pub fn word(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Info => "info",
        }
    }
}

/// One finding of the completeness audit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditNote {
    pub level: AuditLevel,
    /// Stable code for `--json`.
    pub code: &'static str,
    pub message: String,
}

impl AuditNote {
    fn warn(code: &'static str, message: String) -> Self {
        Self {
            level: AuditLevel::Warn,
            code,
            message,
        }
    }

    fn info(code: &'static str, message: String) -> Self {
        Self {
            level: AuditLevel::Info,
            code,
            message,
        }
    }
}

/// Turn a raw observation into the wizard's vocabulary. `Escape` is the only
/// reserved key.
pub fn classify(device: String, key: Key) -> Input {
    if key == Key::Escape {
        Input::Cancel
    } else {
        Input::Press { device, key }
    }
}

// ---------------------------------------------------------------------------
// The observer: one slice at a time, so the countdown can tick
// ---------------------------------------------------------------------------

/// One second of listening. The countdown loop calls this repeatedly rather
/// than blocking for the whole prompt, which is what makes the countdown
/// visible — and, as a bonus, re-baselines every slice so a key left leaning
/// on the panel cannot autorepeat itself into a binding.
pub trait Observer {
    /// Wait up to `slice` for one press: `Ok(None)` = nothing in that slice.
    fn slice(&mut self, slice: Duration) -> Result<Option<(String, Key)>, String>;
}

/// Run one prompt: up to `secs` one-second slices, ticking the countdown, and
/// stopping the moment something is heard.
pub fn observe_step(observer: &mut dyn Observer, secs: u64, tick: &mut dyn FnMut(u64)) -> Input {
    for remaining in (1..=secs.max(1)).rev() {
        tick(remaining);
        match observer.slice(Duration::from_secs(1)) {
            Ok(Some((device, key))) => return classify(device, key),
            Ok(None) => {}
            Err(error) => return Input::Failed(error),
        }
    }
    Input::Timeout
}

/// The real thing: `ksx_capture::observe_next_key`, the same Raw Input sink
/// the daemon's learn service uses (`daemon/learn.rs`).
#[cfg(windows)]
pub struct RawInput;

#[cfg(windows)]
impl Observer for RawInput {
    fn slice(&mut self, slice: Duration) -> Result<Option<(String, Key)>, String> {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        ksx_capture::observe_next_key(slice, &cancel)
            .map(|hit| hit.map(|k| (k.instance_path, k.key)))
            .map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// What a finished run would write
// ---------------------------------------------------------------------------

/// Where a finished slot gets wired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wiring {
    /// Preset only — the user will wire it themselves.
    None,
    /// A `[[slot]]` in `config.toml`, plus a named `[[device]]` for the panel.
    Config,
    /// A `[[game.slot]]` inside this `games.toml` profile. Profiles persist
    /// device ids directly, so no alias is invented for them.
    Profile(String),
}

/// Everything one confirmed run intends to write, assembled but not yet
/// written — the transaction.
#[derive(Clone, Debug)]
pub struct Plan {
    pub slot: u8,
    pub preset_name: String,
    pub preset: Preset,
    pub device_id: String,
    pub device_alias: String,
    pub wiring: Wiring,
    pub bound: Vec<(&'static str, Key)>,
    pub skipped: Vec<&'static str>,
    pub audit: Vec<AuditNote>,
}

/// What actually landed on disk.
#[derive(Clone, Debug, Default)]
pub struct Written {
    pub preset: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub games: Option<PathBuf>,
}

/// Write a plan. Every file goes through the store's atomic writer, and the
/// preset is written FIRST: a `[[slot]]` pointing at a preset that does not
/// exist is the one ordering that leaves a machine worse than it started.
pub fn commit(store: &Store, plan: &Plan) -> anyhow::Result<Written> {
    let mut written = Written {
        preset: Some(store.save_preset(&PresetFile::from_core(&plan.preset))?),
        ..Written::default()
    };

    match &plan.wiring {
        Wiring::None => {}
        Wiring::Config => {
            let mut config = store.load_config()?.value;
            let alias = upsert_device(&mut config.devices, &plan.device_id, &plan.device_alias)?;
            upsert_slot(&mut config.slots, plan, &alias);
            written.config = Some(store.save_config(&config)?);
        }
        Wiring::Profile(title) => {
            let mut games = store.load_games()?.value;
            let Some(game) = games
                .games
                .iter_mut()
                .find(|g| g.title.eq_ignore_ascii_case(title))
            else {
                anyhow::bail!("no games.toml profile called \"{title}\"");
            };
            upsert_game_slot(&mut game.slots, plan);
            written.games = Some(store.save_games(&games)?);
        }
    }
    Ok(written)
}

/// Give the panel a `[[device]]` entry, keyed by its instance path, and return
/// the alias the slot should refer to.
///
/// An id that is already known keeps ITS alias — renaming a device the user
/// named would break every slot that refers to it by name — and a fresh id
/// whose preferred alias is taken gets a numbered one rather than stealing it.
///
/// Fallible now that `[[device]] id` is a selector: this refuses rather than
/// committing an id that would make the file itself unloadable next time. The
/// value comes from Raw Input, which is always a device path, so in practice
/// the wizard never meets this — but writing a config ksx cannot read back is
/// the one failure a setup command must not have.
fn upsert_device(
    devices: &mut Vec<DeviceEntry>,
    id: &str,
    preferred: &str,
) -> anyhow::Result<String> {
    if let Some(known) = devices.iter().find(|d| d.id.raw() == id) {
        return Ok(known.alias.clone());
    }
    let mut alias = preferred.to_owned();
    let mut n = 2;
    while devices.iter().any(|d| d.alias == alias) {
        alias = format!("{preferred} ({n})");
        n += 1;
    }
    devices.push(DeviceEntry {
        id: id.parse()?,
        alias: alias.clone(),
        backend: Default::default(),
    });
    Ok(alias)
}

fn upsert_slot(slots: &mut Vec<SlotEntry>, plan: &Plan, alias: &str) {
    if let Some(existing) = slots.iter_mut().find(|s| s.number == plan.slot) {
        existing.keyboard = Some(alias.to_owned());
        existing.preset = plan.preset_name.clone();
        return;
    }
    slots.push(SlotEntry {
        number: plan.slot,
        keyboard: Some(alias.to_owned()),
        mouse: None,
        preset: plan.preset_name.clone(),
        persona: Default::default(),
        socd: Default::default(),
        macros: Default::default(),
    });
    slots.sort_by_key(|s| s.number);
}

fn upsert_game_slot(slots: &mut Vec<GameSlotEntry>, plan: &Plan) {
    if let Some(existing) = slots.iter_mut().find(|s| s.number == plan.slot) {
        existing.keyboard = Some(plan.device_id.clone());
        existing.preset = plan.preset_name.clone();
        return;
    }
    slots.push(GameSlotEntry {
        number: plan.slot,
        user_index: Some(plan.slot),
        keyboard: Some(plan.device_id.clone()),
        mouse: None,
        preset: plan.preset_name.clone(),
        persona: Default::default(),
        socd: Default::default(),
        macros: Default::default(),
    });
    slots.sort_by_key(|s| s.number);
}

// ---------------------------------------------------------------------------
// The interactive driver (thin: it prints, it reads, it never decides)
// ---------------------------------------------------------------------------

pub struct Options {
    /// The slot this run starts at; later players continue from it.
    pub slot: u8,
    /// Preset name for the first slot (later ones get "<name> P<n>"), or the
    /// default "Player <n>".
    pub preset: Option<String>,
    /// Wire finished slots into this `games.toml` profile instead of
    /// `config.toml`.
    pub profile: Option<String>,
    /// Seconds each prompt listens before skipping the control.
    pub step_secs: u64,
    /// Assemble and report; write nothing.
    pub dry_run: bool,
    /// One JSON object on stdout instead of the prose summary.
    pub json: bool,
}

#[cfg(not(windows))]
pub fn run(_options: Options) -> anyhow::Result<()> {
    eprintln!("`ksx setup` identifies panels through Raw Input: Windows only.");
    std::process::exit(1);
}

#[cfg(windows)]
pub fn run(options: Options) -> anyhow::Result<()> {
    let root = ConfigRoot::discover()?;
    let store = Store::new(root);

    if let Some(title) = &options.profile {
        let games = store.load_games()?.value;
        if !games
            .games
            .iter()
            .any(|g| g.title.eq_ignore_ascii_case(title))
        {
            eprintln!("refused: no games.toml profile called \"{title}\".");
            eprintln!("         `ksx config export --what games` lists the ones there are.");
            std::process::exit(EXIT_REFUSED);
        }
    }

    let mut observer = RawInput;
    banner(&options);

    let mut runs: Vec<serde_json::Value> = Vec::new();
    let mut refused = false;
    let mut slot = options.slot;
    loop {
        match run_one(&mut observer, &store, &options, slot)? {
            Outcome::Done(value) => runs.push(value),
            // The user's own "no" is an answer, not a failure.
            Outcome::Discarded => break,
            Outcome::Refused => {
                refused = runs.is_empty();
                break;
            }
        }

        if slot >= MAX_SLOTS {
            eprintln!("\nThat is the last slot ksx can drive ({MAX_SLOTS}).");
            break;
        }
        if !ask(
            &format!("\nSet up the next player (slot {})?", slot + 1),
            false,
        ) {
            break;
        }
        slot += 1;
    }

    if options.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": !refused,
                "dry_run": options.dry_run,
                "runs": runs,
            })
        );
    } else if runs.is_empty() {
        println!("Nothing was written.");
    }
    if refused {
        std::process::exit(EXIT_REFUSED);
    }
    Ok(())
}

/// How one slot's run ended.
enum Outcome {
    /// Committed (or assembled, under `--dry-run`).
    Done(serde_json::Value),
    /// The user said no — an answer, not an error.
    Discarded,
    /// Nothing usable came out of the run.
    Refused,
}

/// One slot, start to finish.
fn run_one(
    observer: &mut dyn Observer,
    store: &Store,
    options: &Options,
    slot: u8,
) -> anyhow::Result<Outcome> {
    let mut wizard = Wizard::new(slot);
    let preset_name = preset_name_for(options, slot);

    eprintln!("\n=== Player {slot} =========================================");
    eprintln!("Hold a key on the panel you want player {slot} to use.");
    eprintln!("(Escape cancels. Nothing is written until you say so at the end.)\n");

    // Identify.
    loop {
        let input = observe_step(observer, IDENTIFY_SECS, &mut |remaining| {
            countdown("  listening for a press", remaining);
        });
        match wizard.feed(input) {
            Reaction::Identified { device } => {
                clear_line();
                eprintln!("  panel: {device}\n");
                break;
            }
            Reaction::StillListening { attempts_left } => {
                clear_line();
                eprintln!("  nothing heard. Press a key on the panel ({attempts_left} more try).");
            }
            Reaction::Cancelled(reason) => {
                clear_line();
                report_cancel(&reason);
                return Ok(match reason {
                    CancelReason::User => Outcome::Discarded,
                    _ => Outcome::Refused,
                });
            }
            _ => unreachable!("identify only yields identify reactions"),
        }
    }

    // Controls.
    eprintln!(
        "Press the control each prompt names. Press NOTHING for {}s to skip one;",
        options.step_secs
    );
    eprintln!("twice in a row ends the run and skips everything left.\n");
    while let Some(step) = wizard.current() {
        let (done, total) = wizard.progress();
        eprintln!("  [{}/{}] {}", done + 1, total, step.prompt);
        if !step.hint.is_empty() {
            eprintln!("        {}", step.hint);
        }
        let input = observe_step(observer, options.step_secs, &mut |remaining| {
            countdown("        waiting", remaining);
        });
        match wizard.feed(input) {
            Reaction::Bound { key, .. } => {
                clear_line();
                eprintln!("        ✓ {}", key.name());
            }
            Reaction::AlreadyTaken { key, taken_by } => {
                clear_line();
                eprintln!(
                    "        ALREADY TAKEN — {} is {}. Press a different one.",
                    key.name(),
                    taken_by.to_uppercase()
                );
            }
            Reaction::OtherDevice { device } => {
                clear_line();
                eprintln!("        that was a different device ({device}) — use the panel.");
            }
            Reaction::Skipped { .. } => {
                clear_line();
                eprintln!("        — skipped");
            }
            Reaction::FinishedEarly { skipped } => {
                clear_line();
                eprintln!("        — skipped ({skipped} more left unbound)\n");
            }
            Reaction::Complete => break,
            Reaction::Cancelled(reason) => {
                clear_line();
                report_cancel(&reason);
                return Ok(match reason {
                    CancelReason::User => Outcome::Discarded,
                    _ => Outcome::Refused,
                });
            }
            Reaction::Identified { .. } | Reaction::StillListening { .. } => {
                unreachable!("identify reactions cannot happen in the control phase")
            }
        }
    }

    // The control loop has exactly one other exit, and it returned above.
    debug_assert!(
        matches!(wizard.phase(), Phase::Review),
        "the control loop ended outside the review phase: {:?}",
        wizard.phase()
    );

    // Review — the transaction, still entirely in memory.
    let plan = Plan {
        slot: wizard.slot(),
        preset_name: preset_name.clone(),
        preset: wizard.preset(&preset_name),
        device_id: wizard.device().unwrap_or_default().to_owned(),
        device_alias: format!("P{slot} panel"),
        wiring: Wiring::None,
        bound: wizard
            .bound()
            .into_iter()
            .map(|(step, key)| (step.id, key))
            .collect(),
        skipped: wizard.skipped().into_iter().map(|(s, _)| s.id).collect(),
        audit: wizard.audit(),
    };
    print_review(&plan, &wizard);

    if !wizard.can_commit() {
        eprintln!("\nNothing was bound, so there is nothing to write.");
        return Ok(Outcome::Refused);
    }

    if options.dry_run {
        eprintln!("\n--dry-run: nothing written. It would have written:");
        eprintln!(
            "  preset  {}",
            store.canonical_preset_path(&preset_name)?.display()
        );
        match &options.profile {
            Some(title) => {
                eprintln!("  slot    {slot} of the \"{title}\" profile -> \"{preset_name}\"")
            }
            None => eprintln!("  slot    [[slot]] {slot} of config.toml -> \"{preset_name}\""),
        }
        return Ok(Outcome::Done(plan_json(&plan, &Written::default(), false)));
    }

    if !ask("\nWrite this preset?", false) {
        eprintln!("Discarded. Nothing was written.");
        return Ok(Outcome::Discarded);
    }

    let mut plan = plan;
    plan.wiring = match &options.profile {
        Some(title) => {
            if ask(
                &format!("Wire slot {slot} of the \"{title}\" profile to it?"),
                true,
            ) {
                Wiring::Profile(title.clone())
            } else {
                Wiring::None
            }
        }
        None => {
            if ask(&format!("Also make it slot {slot} in config.toml?"), true) {
                Wiring::Config
            } else {
                Wiring::None
            }
        }
    };

    let written = commit(store, &plan)?;
    if let Some(path) = &written.preset {
        eprintln!("  wrote {}", path.display());
    }
    if let Some(path) = &written.config {
        eprintln!("  wrote {}", path.display());
    }
    if let Some(path) = &written.games {
        eprintln!("  wrote {}", path.display());
    }
    Ok(Outcome::Done(plan_json(&plan, &written, true)))
}

fn preset_name_for(options: &Options, slot: u8) -> String {
    match (&options.preset, slot == options.slot) {
        (Some(name), true) => name.clone(),
        (Some(name), false) => format!("{name} P{slot}"),
        (None, _) => format!("Player {slot}"),
    }
}

fn plan_json(plan: &Plan, written: &Written, committed: bool) -> serde_json::Value {
    serde_json::json!({
        "slot": plan.slot,
        "preset": plan.preset_name,
        "device": plan.device_id,
        "committed": committed,
        "wiring": match &plan.wiring {
            Wiring::None => serde_json::json!("none"),
            Wiring::Config => serde_json::json!("config"),
            Wiring::Profile(title) => serde_json::json!({"profile": title}),
        },
        "bound": plan.bound.iter().map(|(step, key)| {
            let functions: Vec<String> = STEPS
                .iter()
                .find(|s| s.id == *step)
                .map(|s| s.bindings.iter().map(ksx_config::function_name).collect())
                .unwrap_or_default();
            serde_json::json!({"step": step, "key": key.name(), "functions": functions})
        }).collect::<Vec<_>>(),
        "skipped": plan.skipped,
        "audit": plan.audit.iter().map(|note| serde_json::json!({
            "level": note.level.word(),
            "code": note.code,
            "message": note.message,
        })).collect::<Vec<_>>(),
        "wrote": {
            "preset": written.preset.as_ref().map(|p| p.display().to_string()),
            "config": written.config.as_ref().map(|p| p.display().to_string()),
            "games": written.games.as_ref().map(|p| p.display().to_string()),
        },
    })
}

fn banner(options: &Options) {
    eprintln!("ksx setup — first contact");
    eprintln!();
    eprintln!("Stop emulation before running this: while `ksx run` has a panel");
    eprintln!("captured, its keystrokes are suppressed below win32k and this");
    eprintln!("wizard cannot hear them.");
    if options.dry_run {
        eprintln!();
        eprintln!("--dry-run: nothing will be written, whatever you answer.");
    }
}

fn print_review(plan: &Plan, wizard: &Wizard) {
    eprintln!("\n--- Player {} · \"{}\" ---", plan.slot, plan.preset_name);
    eprintln!("  panel: {}", plan.device_id);
    for (step_id, key) in &plan.bound {
        let functions: Vec<String> = wizard
            .steps
            .iter()
            .find(|s| s.id == *step_id)
            .map(|s| s.bindings.iter().map(ksx_config::function_name).collect())
            .unwrap_or_default();
        eprintln!(
            "  {:<20} {:<14} {}",
            step_id,
            key.name(),
            functions.join(" + ")
        );
    }
    if !plan.skipped.is_empty() {
        eprintln!("  unbound: {}", plan.skipped.join(", "));
    }
    for note in &plan.audit {
        eprintln!("  [{}] {}", note.level.word(), note.message);
    }
}

fn report_cancel(reason: &CancelReason) {
    match reason {
        CancelReason::User => eprintln!("Cancelled. Nothing was written."),
        CancelReason::NoDevice => {
            eprintln!("No key was pressed, so no panel was identified. Nothing was written.");
            eprintln!("If a session is running, stop it first — a captured panel is silent here.");
        }
        CancelReason::Failed(error) => {
            eprintln!("The key observer failed: {error}");
            eprintln!("Nothing was written. `ksx doctor` reports the capture stack.");
        }
    }
}

fn countdown(label: &str, remaining: u64) {
    eprint!("\r{label} … {remaining}s   ");
    let _ = std::io::stderr().flush();
}

fn clear_line() {
    eprint!("\r{:60}\r", "");
    let _ = std::io::stderr().flush();
}

/// Ask a yes/no question on the console.
///
/// The console input buffer is flushed first: the Raw Input observer does not
/// SUPPRESS anything, so every panel key pressed during the prompts also
/// landed in the terminal, and without the flush a stray "1" from the coin
/// button would answer this question instead of the user.
fn ask(question: &str, default_yes: bool) -> bool {
    flush_console_input();
    eprint!(
        "{question} {} ",
        if default_yes { "[Y/n]" } else { "[y/N]" }
    );
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        // EOF (no console at all): the safe answer is always "do not write".
        Ok(0) | Err(_) => false,
        Ok(_) => match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => default_yes,
        },
    }
}

#[cfg(windows)]
fn flush_console_input() {
    use windows_sys::Win32::System::Console::{
        FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE,
    };
    // SAFETY: both calls take a standard handle and touch nothing else; a
    // failure (no console) is ignored on purpose.
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        if !handle.is_null() {
            FlushConsoleInputBuffer(handle);
        }
    }
}

#[cfg(not(windows))]
fn flush_console_input() {}

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: &str = r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000";
    const DESK: &str = r"HID\VID_046D&PID_C31C&MI_00\7&1&0000";

    fn press(key: Key) -> Input {
        Input::Press {
            device: PANEL.to_owned(),
            key,
        }
    }

    /// Drive a wizard from the identify prompt through `keys` in prompt order.
    fn walk(keys: &[Key]) -> Wizard {
        let mut wizard = Wizard::new(1);
        wizard.feed(press(Key::One));
        for key in keys {
            wizard.feed(press(*key));
        }
        wizard
    }

    #[test]
    fn the_prompt_order_is_essentials_first() {
        let essentials: Vec<&str> = STEPS
            .iter()
            .take_while(|s| s.essential)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            essentials,
            vec!["up", "down", "left", "right", "south", "east", "west", "north", "start", "back"],
            "a run abandoned halfway must still leave a usable panel"
        );
        // ...and nothing essential hides in the tail.
        assert!(!STEPS[essentials.len()..].iter().any(|s| s.essential));
        // Every id is unique (the JSON and the audit key off them).
        let mut ids: Vec<&str> = STEPS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), STEPS.len());
    }

    #[test]
    fn a_direction_binds_the_hat_and_the_stick_from_one_press() {
        let step = STEPS.iter().find(|s| s.id == "up").unwrap();
        assert_eq!(step.bindings.len(), 2);
        let names: Vec<String> = step
            .bindings
            .iter()
            .map(ksx_config::function_name)
            .collect();
        assert_eq!(names, vec!["dpad.up".to_owned(), "ly.max".to_owned()]);
    }

    #[test]
    fn identify_takes_the_first_press_and_starts_the_controls() {
        let mut wizard = Wizard::new(2);
        assert_eq!(*wizard.phase(), Phase::Identify);
        assert!(wizard.current().is_none());
        let reaction = wizard.feed(press(Key::One));
        assert_eq!(
            reaction,
            Reaction::Identified {
                device: PANEL.to_owned()
            }
        );
        assert_eq!(wizard.device(), Some(PANEL));
        assert_eq!(wizard.current().map(|s| s.id), Some("up"));
        // The identifying press is NOT consumed as a binding — it only said
        // which panel this is.
        assert!(wizard.bound().is_empty());
    }

    #[test]
    fn a_silent_identify_gives_up_after_two_tries_and_writes_nothing() {
        let mut wizard = Wizard::new(1);
        assert_eq!(
            wizard.feed(Input::Timeout),
            Reaction::StillListening { attempts_left: 1 }
        );
        assert_eq!(
            wizard.feed(Input::Timeout),
            Reaction::Cancelled(CancelReason::NoDevice)
        );
        assert!(!wizard.can_commit());
        assert_eq!(*wizard.phase(), Phase::Cancelled(CancelReason::NoDevice));
    }

    #[test]
    fn presses_auto_advance_and_assemble_in_prompt_order() {
        let wizard = walk(&[Key::Up, Key::Down, Key::Left, Key::Right]);
        assert_eq!(wizard.current().map(|s| s.id), Some("south"));
        assert_eq!(
            wizard
                .bound()
                .iter()
                .map(|(s, k)| (s.id, k.name()))
                .collect::<Vec<_>>(),
            vec![
                ("up", "Up"),
                ("down", "Down"),
                ("left", "Left"),
                ("right", "Right")
            ]
        );
        // TRANSACTIONAL ASSEMBLY: one press per direction, two bindings each.
        let preset = wizard.preset("P1");
        assert_eq!(preset.name, "P1");
        assert!(!preset.protected);
        assert_eq!(preset.entries.len(), 8);
        assert_eq!(
            preset.entries[0],
            (Key::Up, Binding::Dpad(DpadDirection::Up))
        );
        assert_eq!(
            preset.entries[1],
            (
                Key::Up,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MAX
                }
            )
        );
    }

    #[test]
    fn an_already_taken_key_is_refused_inline_and_the_prompt_stays_put() {
        let mut wizard = walk(&[Key::Up]);
        assert_eq!(wizard.current().map(|s| s.id), Some("down"));
        assert_eq!(
            wizard.feed(press(Key::Up)),
            Reaction::AlreadyTaken {
                key: Key::Up,
                taken_by: "up"
            }
        );
        // Still on DOWN, and nothing was written over UP.
        assert_eq!(wizard.current().map(|s| s.id), Some("down"));
        assert_eq!(wizard.bound().len(), 1);
        // A different key moves on.
        assert_eq!(
            wizard.feed(press(Key::Down)),
            Reaction::Bound {
                step: "down",
                key: Key::Down
            }
        );
        assert_eq!(wizard.current().map(|s| s.id), Some("left"));
    }

    #[test]
    fn a_press_on_another_device_is_refused_without_advancing() {
        let mut wizard = walk(&[]);
        let reaction = wizard.feed(Input::Press {
            device: DESK.to_owned(),
            key: Key::G,
        });
        assert_eq!(
            reaction,
            Reaction::OtherDevice {
                device: DESK.to_owned()
            }
        );
        assert_eq!(wizard.current().map(|s| s.id), Some("up"));
        assert!(wizard.bound().is_empty());
    }

    #[test]
    fn one_silent_prompt_skips_that_control_and_advances() {
        let mut wizard = walk(&[Key::Up]);
        assert_eq!(
            wizard.feed(Input::Timeout),
            Reaction::Skipped {
                step: "down",
                reason: SkipReason::Idle
            }
        );
        assert_eq!(wizard.current().map(|s| s.id), Some("left"));
        // A press resets the streak, so a later single silence is still just
        // one skip.
        wizard.feed(press(Key::Left));
        assert_eq!(
            wizard.feed(Input::Timeout),
            Reaction::Skipped {
                step: "right",
                reason: SkipReason::Idle
            }
        );
        assert_eq!(wizard.current().map(|s| s.id), Some("south"));
    }

    #[test]
    fn two_silent_prompts_in_a_row_end_the_run_and_skip_the_rest() {
        let mut wizard = walk(&[Key::Up, Key::Down, Key::Left, Key::Right]);
        wizard.feed(Input::Timeout); // south
        let reaction = wizard.feed(Input::Timeout); // east -> done
        let remaining = STEPS.len() - 6;
        assert_eq!(reaction, Reaction::FinishedEarly { skipped: remaining });
        assert_eq!(*wizard.phase(), Phase::Review);
        assert!(wizard.current().is_none());
        assert_eq!(wizard.skipped().len(), STEPS.len() - 4);
        assert!(wizard
            .skipped()
            .iter()
            .any(|(s, r)| s.id == "north" && *r == SkipReason::NotAsked));
        assert!(wizard
            .skipped()
            .iter()
            .any(|(s, r)| s.id == "south" && *r == SkipReason::Idle));
        // What survives is exactly what was pressed.
        assert_eq!(wizard.preset("P1").entries.len(), 8);
    }

    #[test]
    fn escape_cancels_from_anywhere_and_commits_nothing() {
        let mut wizard = walk(&[Key::Up, Key::Down]);
        assert_eq!(
            wizard.feed(classify(PANEL.to_owned(), Key::Escape)),
            Reaction::Cancelled(CancelReason::User)
        );
        assert_eq!(*wizard.phase(), Phase::Cancelled(CancelReason::User));
        // A cancelled run keeps answering "cancelled" — no resurrection.
        assert_eq!(
            wizard.feed(press(Key::Left)),
            Reaction::Cancelled(CancelReason::User)
        );
        assert_eq!(wizard.bound().len(), 2);
    }

    #[test]
    fn an_observer_failure_cancels_with_the_reason() {
        let mut wizard = walk(&[]);
        let reaction = wizard.feed(Input::Failed("raw input sink exploded".into()));
        assert_eq!(
            reaction,
            Reaction::Cancelled(CancelReason::Failed("raw input sink exploded".into()))
        );
        assert!(!wizard.can_commit());
    }

    #[test]
    fn a_full_run_reaches_review_with_no_leftovers() {
        let keys: Vec<Key> = Key::ALL
            .iter()
            .copied()
            .filter(|k| *k != Key::None && *k != Key::Escape && *k != Key::Unknown)
            .take(STEPS.len())
            .collect();
        assert_eq!(keys.len(), STEPS.len());
        let wizard = walk(&keys);
        assert_eq!(*wizard.phase(), Phase::Review);
        assert!(wizard.skipped().is_empty());
        assert_eq!(wizard.bound().len(), STEPS.len());
        assert!(wizard.can_commit());
        let expected: usize = STEPS.iter().map(|s| s.bindings.len()).sum();
        assert_eq!(wizard.preset("Full").entries.len(), expected);
    }

    // --- the audit ---------------------------------------------------------

    fn codes(wizard: &Wizard) -> Vec<&'static str> {
        wizard.audit().iter().map(|n| n.code).collect()
    }

    #[test]
    fn the_audit_warns_when_the_panel_cannot_reach_start_or_back() {
        // Directions + all four faces, nothing else: no way out of a game.
        let wizard = walk(&[
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::A,
            Key::B,
            Key::C,
            Key::D,
        ]);
        assert!(codes(&wizard).contains(&"no-exit"), "{:?}", wizard.audit());
        assert!(wizard.audit().iter().any(|n| n.code == "no-exit"
            && n.level == AuditLevel::Warn
            && n.message.contains("exit keys")));
    }

    #[test]
    fn the_audit_names_which_exit_key_is_missing() {
        let mut wizard = walk(&[
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::A,
            Key::B,
            Key::C,
            Key::D,
        ]);
        wizard.feed(press(Key::One)); // start
        assert!(codes(&wizard).contains(&"no-back"));
        assert!(!codes(&wizard).contains(&"no-exit"));
        assert!(!codes(&wizard).contains(&"no-start"));

        let mut wizard = walk(&[
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::A,
            Key::B,
            Key::C,
            Key::D,
        ]);
        wizard.feed(Input::Timeout); // start skipped
        wizard.feed(press(Key::Five)); // back
        assert!(codes(&wizard).contains(&"no-start"));
    }

    #[test]
    fn the_audit_counts_directions_and_face_buttons() {
        let wizard = walk(&[Key::Up, Key::Down]);
        let codes = codes(&wizard);
        assert!(codes.contains(&"partial-directions"));
        assert!(codes.contains(&"few-buttons"));
        assert!(codes.contains(&"no-exit"));
        assert!(codes.contains(&"skipped"));
    }

    #[test]
    fn a_complete_panel_audits_clean_apart_from_the_skip_note() {
        let wizard = walk(&[
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::LeftControl,
            Key::LeftAlt,
            Key::Space,
            Key::LeftShift,
            Key::One,
            Key::Five,
        ]);
        let notes = wizard.audit();
        assert!(
            notes.iter().all(|n| n.level == AuditLevel::Info),
            "{notes:?}"
        );
        assert_eq!(codes(&wizard), vec!["skipped"]);
        assert!(wizard.can_commit());
    }

    #[test]
    fn a_run_with_nothing_bound_cannot_commit() {
        let mut wizard = walk(&[]);
        wizard.feed(Input::Timeout);
        wizard.feed(Input::Timeout);
        assert!(!wizard.can_commit());
        assert_eq!(codes(&wizard), vec!["nothing-bound"]);
    }

    // --- the countdown loop ------------------------------------------------

    /// A scripted observer: one queued outcome per slice.
    struct Scripted(Vec<Result<Option<(String, Key)>, String>>);

    impl Observer for Scripted {
        fn slice(&mut self, _slice: Duration) -> Result<Option<(String, Key)>, String> {
            if self.0.is_empty() {
                return Ok(None);
            }
            self.0.remove(0)
        }
    }

    #[test]
    fn the_countdown_ticks_once_per_second_and_stops_on_a_press() {
        let mut observer = Scripted(vec![
            Ok(None),
            Ok(None),
            Ok(Some((PANEL.to_owned(), Key::G))),
        ]);
        let mut ticks = Vec::new();
        let input = observe_step(&mut observer, 6, &mut |remaining| ticks.push(remaining));
        assert_eq!(
            input,
            Input::Press {
                device: PANEL.to_owned(),
                key: Key::G
            }
        );
        // Three slices ran, so three ticks were shown — and the countdown
        // counts DOWN from the whole prompt.
        assert_eq!(ticks, vec![6, 5, 4]);
    }

    #[test]
    fn a_silent_prompt_times_out_after_the_last_slice() {
        let mut observer = Scripted(Vec::new());
        let mut ticks = Vec::new();
        let input = observe_step(&mut observer, 3, &mut |remaining| ticks.push(remaining));
        assert_eq!(input, Input::Timeout);
        assert_eq!(ticks, vec![3, 2, 1]);
    }

    #[test]
    fn an_observer_error_becomes_a_failure_not_a_timeout() {
        let mut observer = Scripted(vec![Err("no raw input".into())]);
        let input = observe_step(&mut observer, 5, &mut |_| {});
        assert_eq!(input, Input::Failed("no raw input".into()));
    }

    #[test]
    fn escape_is_the_only_reserved_key() {
        assert_eq!(classify(PANEL.into(), Key::Escape), Input::Cancel);
        assert_eq!(
            classify(PANEL.into(), Key::One),
            Input::Press {
                device: PANEL.into(),
                key: Key::One
            }
        );
    }

    // --- the transaction ---------------------------------------------------

    fn plan_for(wizard: &Wizard, wiring: Wiring) -> Plan {
        Plan {
            slot: wizard.slot(),
            preset_name: "P1".to_owned(),
            preset: wizard.preset("P1"),
            device_id: wizard.device().unwrap_or_default().to_owned(),
            device_alias: "P1 panel".to_owned(),
            wiring,
            bound: wizard.bound().into_iter().map(|(s, k)| (s.id, k)).collect(),
            skipped: wizard.skipped().into_iter().map(|(s, _)| s.id).collect(),
            audit: wizard.audit(),
        }
    }

    #[test]
    fn a_config_wiring_names_the_device_once_and_points_the_slot_at_the_preset() {
        let wizard = walk(&[Key::Up, Key::Down]);
        let plan = plan_for(&wizard, Wiring::Config);
        let mut devices = Vec::new();
        let mut slots = Vec::new();
        let alias = upsert_device(&mut devices, &plan.device_id, &plan.device_alias).unwrap();
        upsert_slot(&mut slots, &plan, &alias);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].alias, "P1 panel");
        assert_eq!(devices[0].id.raw(), PANEL);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].number, 1);
        assert_eq!(slots[0].keyboard.as_deref(), Some("P1 panel"));
        assert_eq!(slots[0].preset, "P1");

        // Idempotent: a second run over the same id adds no second entry, does
        // not rename the one the user may have edited, and the slot follows
        // the name that is actually in the file.
        devices[0].alias = "Cabinet P1".to_owned();
        let alias = upsert_device(&mut devices, &plan.device_id, &plan.device_alias).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].alias, "Cabinet P1");
        assert_eq!(alias, "Cabinet P1");

        // ...and re-running the slot repoints it rather than duplicating it.
        upsert_slot(&mut slots, &plan, &alias);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].keyboard.as_deref(), Some("Cabinet P1"));
    }

    #[test]
    fn a_second_panel_never_steals_the_first_ones_alias() {
        let mut devices = Vec::new();
        assert_eq!(
            upsert_device(&mut devices, PANEL, "P1 panel").unwrap(),
            "P1 panel"
        );
        assert_eq!(
            upsert_device(&mut devices, DESK, "P1 panel").unwrap(),
            "P1 panel (2)"
        );
        assert_eq!(devices.len(), 2);
        assert_ne!(devices[0].alias, devices[1].alias);
    }

    /// The wizard commits what Raw Input reported, and a value that cannot name
    /// a device on Windows is refused HERE rather than written into a file that
    /// then refuses to load — which would leave the machine worse off than
    /// before the wizard ran.
    #[test]
    fn an_id_that_could_never_name_a_device_is_refused_rather_than_committed() {
        let mut devices = Vec::new();
        assert!(upsert_device(&mut devices, "not-a-device", "P1 panel").is_err());
        assert!(devices.is_empty());
    }

    #[test]
    fn a_profile_wiring_stores_the_instance_path_not_an_alias() {
        let wizard = walk(&[Key::Up]);
        let plan = plan_for(&wizard, Wiring::Profile("MAME".into()));
        let mut slots = Vec::new();
        upsert_game_slot(&mut slots, &plan);
        assert_eq!(slots[0].keyboard.as_deref(), Some(PANEL));
        assert_eq!(slots[0].preset, "P1");
        assert_eq!(slots[0].user_index, Some(1));
        upsert_game_slot(&mut slots, &plan);
        assert_eq!(slots.len(), 1, "re-running a slot repoints it");
    }

    #[test]
    fn the_assembled_preset_round_trips_through_the_file_schema() {
        let wizard = walk(&[Key::Up, Key::Down, Key::Left, Key::Right, Key::LeftControl]);
        let preset = wizard.preset("Cabinet P1");
        let file = PresetFile::from_core(&preset);
        assert_eq!(file.name, "Cabinet P1");
        let back = file.to_core().expect("round trip");
        let mut a = preset.entries.clone();
        let mut b = back.entries.clone();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        // The dpad+stick pair really is two rows the file keeps apart.
        assert!(file.bindings.contains_key("dpad.up"));
        assert!(file.bindings.contains_key("ly.max"));
    }
}
