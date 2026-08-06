//! Macros — one key, a timed SEQUENCE of pad states
//! (docs/INPUT-TRANSFORMS.md §1c).
//!
//! Hadouken is ↓, ↘, → + punch **over time**. Everything else ksx does is a
//! function of the currently-held key SET (§0.1) and needs no clock; a macro is
//! the one shape that genuinely does, so this module carries the model and
//! [`crate::engine`] carries the scheduler.
//!
//! # The sampling rule is a hard constraint
//!
//! §0.2: a game polling at 60 Hz sees state every ~16.7 ms, so a step held for
//! 5 ms is not unreliable — it is *invisible*. Every step must survive being
//! sampled at least twice, which is [`MIN_STEP_MS`] (33 ms). ksx therefore
//! **raises** a shorter step to the minimum unless the author explicitly says
//! [`MacroStep::allow_short`], and validation warns about both cases. There is
//! no configuration in which ksx silently emits a step a poller cannot see.
//!
//! # Determinism
//!
//! A macro's behavior is a pure function of `(events, clock)`: the engine is
//! handed the current time and never reads one, so the whole feature is
//! testable with a fake clock and replayable from the M3 corpus. Nothing here
//! allocates after [`crate::EngineTables::build`].

use std::fmt;
use std::str::FromStr;

use crate::key::Key;
use crate::preset::Binding;

/// The shortest step a 60 Hz poller can be relied on to see, in milliseconds
/// (§0.2: ~16.7 ms per sample, so ~33 ms is two samples).
///
/// This is a floor, not a default duration: a step that asks for less is
/// **raised** to it unless [`MacroStep::allow_short`] says the author knows.
pub const MIN_STEP_MS: u32 = 33;

/// One 60 Hz frame in milliseconds, as a rational — 1000/60 ≈ 16.667 ms.
///
/// Kept rational and rounded once, at conversion, so `frames = 3` is 50 ms and
/// not 3 × 17 = 51: a fighting-game sequence written in frames must not drift
/// because of the unit it was written in.
const FRAME_MS_NUM: u32 = 1000;
const FRAME_MS_DEN: u32 = 60;

/// How a step's duration was AUTHORED.
///
/// # `frames` is an ergonomic unit, not a frame lock
///
/// Fighting-game players think in frames, and `frames = 3` is a much better way
/// to say "three frames of down-forward" than `ms = 50`. So ksx accepts it, and
/// converts it once at 60 Hz.
///
/// **It buys readability and nothing else.** ksx cannot frame-lock to the game:
/// it publishes STATE and the game samples it on its own schedule (§0.0), at a
/// rate ksx does not know and a phase that drifts against ksx's clock every
/// second the two run. So `frames = 3` means "held for the wall-clock duration
/// of three 60 Hz frames" — approximately 50 ms — and NOT "the game will read
/// this on exactly three of its polls". It may read it on two, or on four, and
/// on a 120 Hz or vsync-coupled emulator it will read it on some other number
/// entirely. Anything that needed the stronger promise would need the game to
/// tell us when it polls, which no game does.
///
/// What the unit *does* guarantee is what [`MIN_STEP_MS`] guarantees: two
/// frames is the floor, and below it the step is raised (or explicitly opted
/// out of) rather than silently becoming invisible.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum StepDuration {
    /// Milliseconds, as written.
    Ms(u32),
    /// 60 Hz frames. See the type docs for exactly how weak this promise is.
    Frames(u32),
}

impl StepDuration {
    /// The duration in milliseconds, rounded to nearest once.
    pub const fn ms(self) -> u32 {
        match self {
            StepDuration::Ms(ms) => ms,
            // +DEN/2 is round-to-nearest: 1 frame → 17, 2 → 33, 3 → 50, 4 → 67.
            // (Two frames landing exactly on MIN_STEP_MS is not a coincidence —
            // the floor *is* two samples.)
            StepDuration::Frames(frames) => {
                (frames * FRAME_MS_NUM + FRAME_MS_DEN / 2) / FRAME_MS_DEN
            }
        }
    }
}

impl fmt::Display for StepDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepDuration::Ms(ms) => write!(f, "{ms} ms"),
            StepDuration::Frames(1) => write!(f, "1 frame ({} ms)", self.ms()),
            StepDuration::Frames(n) => write!(f, "{n} frames ({} ms)", self.ms()),
        }
    }
}

/// One step of a macro: a set of bindings to HOLD, and how long to hold them.
///
/// `hold` is a set, not a sequence — everything simultaneous is free (§0.1), so
/// the diagonal ↘ is simply `["dpad.down", "dpad.right"]` in one step. An empty
/// `hold` is legal and useful: it is a deliberate neutral gap, which is how a
/// macro says "let go, then press again" (two presses of the same button need a
/// released frame between them or the game sees one long hold).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroStep {
    /// Bindings held for the whole step. Order is irrelevant; duplicates are
    /// harmless (the engine presses an endpoint once).
    pub hold: Vec<Binding>,
    /// Requested hold, in the unit the file used. See
    /// [`MacroStep::effective_ms`] — this is what the file says, not necessarily
    /// what the engine runs.
    pub duration: StepDuration,
    /// "I know this is shorter than [`MIN_STEP_MS`] and may be missed."
    ///
    /// The opt-out exists because there are real reasons to want one (a 1-frame
    /// input on a 120 Hz-polled emulator, a step whose only job is to *release*
    /// something before the next press). It is never the default, and
    /// validation warns every time it is used.
    pub allow_short: bool,
}

impl MacroStep {
    /// A step at the honest duration: shorter than [`MIN_STEP_MS`] is raised.
    pub fn new(hold: Vec<Binding>, ms: u32) -> Self {
        Self {
            hold,
            duration: StepDuration::Ms(ms),
            allow_short: false,
        }
    }

    /// The same, in 60 Hz frames — the fighting-game unit
    /// (see [`StepDuration::Frames`] for what it does and does not promise).
    pub fn frames(hold: Vec<Binding>, frames: u32) -> Self {
        Self {
            hold,
            duration: StepDuration::Frames(frames),
            allow_short: false,
        }
    }

    /// A step that opts out of the sampling floor. See [`MacroStep::allow_short`].
    pub fn short(hold: Vec<Binding>, ms: u32) -> Self {
        Self {
            hold,
            duration: StepDuration::Ms(ms),
            allow_short: true,
        }
    }

    /// The duration as written, in milliseconds — before the sampling floor.
    pub const fn requested_ms(&self) -> u32 {
        self.duration.ms()
    }

    /// What the engine actually holds this step for.
    ///
    /// This is the single place the sampling rule is enforced, so the engine,
    /// the plan printer and any duration estimate all agree by construction.
    pub const fn effective_ms(&self) -> u32 {
        let ms = self.requested_ms();
        if self.allow_short || ms >= MIN_STEP_MS {
            ms
        } else {
            MIN_STEP_MS
        }
    }

    /// The shortest window this step may EVER be given, however late the
    /// scheduler is running.
    ///
    /// This is what makes a late wake stretch the schedule rather than swallow a
    /// step: a step whose absolute window has already passed is still published
    /// for this long, because a skipped step is an invisible input and §0.2
    /// forbids those. Honoring `allow_short` here too — the author asked.
    pub const fn min_visible_ms(&self) -> u32 {
        if self.allow_short {
            self.requested_ms()
        } else {
            MIN_STEP_MS
        }
    }

    /// Does this step ask for less than a 60 Hz poller can see?
    pub const fn is_short(&self) -> bool {
        self.requested_ms() < MIN_STEP_MS
    }

    /// Was this step's requested duration raised to [`MIN_STEP_MS`]? Validation
    /// reports it so the change is never silent.
    pub const fn was_raised(&self) -> bool {
        self.is_short() && !self.allow_short
    }
}

/// What happens when the trigger key is released mid-run.
///
/// [`OnRelease::Finish`] is the default because it is the fighting-game
/// expectation: you tap the macro button and the quarter-circle comes out
/// whole. A "hold to auto-fire" macro wants the other one.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum OnRelease {
    /// Run every remaining step, then stop. Releasing the key does nothing.
    #[default]
    Finish,
    /// Stop now: cancel pending steps and release everything the macro held,
    /// in one delta batch.
    Abort,
}

impl OnRelease {
    pub const ALL: &'static [OnRelease] = &[OnRelease::Finish, OnRelease::Abort];

    /// Canonical name — what a config file stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            OnRelease::Finish => "finish",
            OnRelease::Abort => "abort",
        }
    }
}

impl fmt::Display for OnRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown on_release policy '{0}' (expected one of: finish, abort)")]
pub struct UnknownOnRelease(pub String);

impl FromStr for OnRelease {
    type Err = UnknownOnRelease;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize(s).as_str() {
            "finish" | "complete" | "run" => Ok(OnRelease::Finish),
            "abort" | "cancel" | "stop" => Ok(OnRelease::Abort),
            _ => Err(UnknownOnRelease(s.to_owned())),
        }
    }
}

/// What happens when the trigger fires again while the macro is still running.
///
/// [`Retrigger::Ignore`] is the default: mashing a macro button should not
/// stutter the sequence back to its first step forever, which is exactly what
/// `restart` does on a panel with any bounce at all.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Retrigger {
    /// The press is swallowed; the run in flight continues undisturbed.
    #[default]
    Ignore,
    /// Start again from step 0. State from the old run is released and the new
    /// step pressed in the SAME delta batch, so nothing is stranded.
    Restart,
}

impl Retrigger {
    pub const ALL: &'static [Retrigger] = &[Retrigger::Ignore, Retrigger::Restart];

    pub const fn as_str(self) -> &'static str {
        match self {
            Retrigger::Ignore => "ignore",
            Retrigger::Restart => "restart",
        }
    }
}

impl fmt::Display for Retrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown retrigger policy '{0}' (expected one of: ignore, restart)")]
pub struct UnknownRetrigger(pub String);

impl FromStr for Retrigger {
    type Err = UnknownRetrigger;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize(s).as_str() {
            "ignore" | "keep" | "continue" => Ok(Retrigger::Ignore),
            "restart" | "retrigger" | "reset" => Ok(Retrigger::Restart),
            _ => Err(UnknownRetrigger(s.to_owned())),
        }
    }
}

/// What OTHER input does to a macro that is running.
///
/// [`OnRelease`] answers "what happens when you let go of the macro button";
/// this answers "what happens when you do something *else*". They compose: a
/// macro can finish on release and still be abandoned the moment the player
/// starts fighting it.
///
/// Every abort takes the same path as every other exit — pending steps
/// cancelled, everything the macro held released, one delta batch.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Interrupt {
    /// Nothing but the macro's own trigger affects it. The default, and the
    /// only behavior that never surprises: a macro you started is a macro that
    /// runs.
    #[default]
    None,
    /// ANY other bound key going down on this slot aborts it. The "get out of
    /// my way" setting: touch the panel and the sequence stops. A key that only
    /// triggers this same macro does not count — that is a retrigger, and
    /// [`Retrigger`] decides it.
    AnyInput,
    /// Abort only on input that CONTRADICTS the macro. Deliberately narrow, and
    /// exactly two rules, because a fuzzy version of this would be impossible
    /// to predict on a cabinet:
    ///
    /// 1. a key that drives a direction OPPOSING one the macro is holding right
    ///    now (holding → while the step holds ←; the dpad and both sticks, and
    ///    "opposing" is the same relation SOCD cleans, [`crate::socd`]);
    /// 2. a key that triggers a DIFFERENT macro on this slot — asking for
    ///    another sequence is asking to stop this one.
    ///
    /// A press that agrees with the macro, or is unrelated to it (a punch
    /// during a motion), passes through and does nothing to the run.
    Opposing,
}

impl Interrupt {
    pub const ALL: &'static [Interrupt] =
        &[Interrupt::None, Interrupt::AnyInput, Interrupt::Opposing];

    pub const fn as_str(self) -> &'static str {
        match self {
            Interrupt::None => "none",
            Interrupt::AnyInput => "any-input",
            Interrupt::Opposing => "opposing",
        }
    }

    /// Does this policy look at other keys at all? `false` is the zero-cost path.
    pub const fn is_active(self) -> bool {
        !matches!(self, Interrupt::None)
    }
}

impl fmt::Display for Interrupt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown interrupt policy '{0}' (expected one of: none, any-input, opposing)")]
pub struct UnknownInterrupt(pub String);

impl FromStr for Interrupt {
    type Err = UnknownInterrupt;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize(s).as_str() {
            "none" | "never" | "off" => Ok(Interrupt::None),
            "anyinput" | "any" | "anykey" => Ok(Interrupt::AnyInput),
            "opposing" | "opposite" | "conflicting" => Ok(Interrupt::Opposing),
            _ => Err(UnknownInterrupt(s.to_owned())),
        }
    }
}

/// Case- and separator-insensitive, like [`crate::Persona`] and [`crate::Socd`]:
/// config files are hand-edited, so `On_Release = "Abort"` must work.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// A named timed sequence.
///
/// One-shot by design: the last step ends the run even if the trigger is still
/// held. "Hold to repeat" is turbo (§2.3), a different feature with a different
/// aliasing problem, and folding the two would make both harder to explain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Macro {
    /// Unique within a preset, compared case-insensitively (function names are).
    pub name: String,
    /// Ordered steps. An empty list is a config error, reported by validation
    /// and inert in the engine.
    pub steps: Vec<MacroStep>,
    pub on_release: OnRelease,
    pub retrigger: Retrigger,
    /// What OTHER input does to a run in flight. Composes with `on_release`.
    pub interrupt: Interrupt,
}

impl Macro {
    /// A macro with the default policies: finish on release, ignore retrigger,
    /// interrupt on nothing.
    pub fn new(name: impl Into<String>, steps: Vec<MacroStep>) -> Self {
        Self {
            name: name.into(),
            steps,
            on_release: OnRelease::default(),
            retrigger: Retrigger::default(),
            interrupt: Interrupt::default(),
        }
    }

    /// Absolute offsets from the macro's start, in milliseconds: `deadlines[i]`
    /// is when step `i` ENDS.
    ///
    /// The whole drift story is this function. A macro's timeline is fixed the
    /// moment it starts; the scheduler asks "when should step *i* end", never
    /// "how long after the last wake" — so four 50 ms steps end at 50/100/150/200
    /// whatever the wake jitter was, instead of accumulating it four times.
    pub fn deadlines(&self) -> impl Iterator<Item = u32> + '_ {
        self.steps.iter().scan(0u32, |sum, step| {
            *sum = sum.saturating_add(step.effective_ms());
            Some(*sum)
        })
    }

    /// How long a full run takes, in milliseconds, at the durations the engine
    /// will actually use ([`MacroStep::effective_ms`]).
    pub fn total_ms(&self) -> u64 {
        self.steps.iter().map(|s| u64::from(s.effective_ms())).sum()
    }

    /// A macro with no steps does nothing at all. Kept representable (rather
    /// than refused at parse time) so validation can *name* it — loading is
    /// lenient, validation is where problems become actionable.
    pub fn is_inert(&self) -> bool {
        self.steps.is_empty()
    }
}

/// A key that starts a macro: the file row `macro.<name> = "<key>"`.
///
/// Triggers are a separate list from [`crate::Preset::entries`] for the same
/// reason chords are: "no macros ⇒ nothing changed" stays checkable rather than
/// claimed. Many keys → one macro is native (multi-bind), and so is one key →
/// several macros; nothing here is unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacroTrigger {
    /// [`Key::None`] makes the row an inert placeholder, like an unbound entry.
    pub key: Key,
    /// Index into [`crate::Preset::macros`]. Resolved from the file's macro
    /// *name* by `ksx-config`, so the core model cannot carry a dangling name;
    /// an out-of-range index is ignored by the engine and reported by
    /// validation.
    pub index: u16,
}

impl MacroTrigger {
    pub const fn new(key: Key, index: u16) -> Self {
        Self { key, index }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pad::{DpadDirection, XButton};

    fn hadouken() -> Macro {
        Macro::new(
            "hadouken",
            vec![
                MacroStep::new(vec![Binding::Dpad(DpadDirection::Down)], 50),
                MacroStep::new(
                    vec![
                        Binding::Dpad(DpadDirection::Down),
                        Binding::Dpad(DpadDirection::Right),
                    ],
                    50,
                ),
                MacroStep::new(vec![Binding::Dpad(DpadDirection::Right)], 50),
                MacroStep::new(vec![Binding::Button(XButton::A)], 50),
            ],
        )
    }

    #[test]
    fn the_defaults_are_the_fighting_game_expectation() {
        let m = hadouken();
        assert_eq!(m.on_release, OnRelease::Finish);
        assert_eq!(m.retrigger, Retrigger::Ignore);
        assert_eq!(m.total_ms(), 200);
        assert!(!m.is_inert());
    }

    /// The sampling rule (§0.2), as code: a step nobody could see is raised.
    #[test]
    fn a_short_step_is_raised_unless_the_author_opts_out() {
        let raised = MacroStep::new(vec![], 5);
        assert!(raised.is_short());
        assert!(raised.was_raised());
        assert_eq!(raised.effective_ms(), MIN_STEP_MS);

        let opted_out = MacroStep::short(vec![], 5);
        assert!(opted_out.is_short());
        assert!(!opted_out.was_raised());
        assert_eq!(opted_out.effective_ms(), 5);

        // At or above the floor nothing is touched, opt-out or not.
        assert_eq!(MacroStep::new(vec![], 33).effective_ms(), 33);
        assert_eq!(MacroStep::new(vec![], 500).effective_ms(), 500);
        assert!(!MacroStep::new(vec![], 33).is_short());

        // Two 60 Hz samples is where the number comes from.
        assert_eq!(MIN_STEP_MS, 33);
    }

    #[test]
    fn total_ms_counts_the_duration_the_engine_will_run() {
        let m = Macro::new(
            "raised",
            vec![MacroStep::new(vec![], 5), MacroStep::short(vec![], 5)],
        );
        assert_eq!(m.total_ms(), u64::from(MIN_STEP_MS) + 5);
    }

    #[test]
    fn policies_round_trip_and_take_the_spellings_people_type() {
        for &policy in OnRelease::ALL {
            assert_eq!(policy.as_str().parse::<OnRelease>(), Ok(policy));
            assert_eq!(policy.to_string(), policy.as_str());
        }
        for &policy in Retrigger::ALL {
            assert_eq!(policy.as_str().parse::<Retrigger>(), Ok(policy));
            assert_eq!(policy.to_string(), policy.as_str());
        }
        assert_eq!("Abort".parse(), Ok(OnRelease::Abort));
        assert_eq!("re-start".parse(), Ok(Retrigger::Restart));
        assert_eq!(OnRelease::default(), OnRelease::Finish);
        assert_eq!(Retrigger::default(), Retrigger::Ignore);

        let err = "maybe".parse::<OnRelease>().unwrap_err();
        assert!(err.to_string().contains("abort"), "{err}");
        let err = "maybe".parse::<Retrigger>().unwrap_err();
        assert!(err.to_string().contains("restart"), "{err}");
    }

    /// `frames` is a readable unit for the same schedule, rounded ONCE so a
    /// sequence written in frames does not drift against one written in ms.
    #[test]
    fn frames_convert_at_60_hz_and_round_once() {
        assert_eq!(StepDuration::Frames(1).ms(), 17);
        assert_eq!(StepDuration::Frames(2).ms(), MIN_STEP_MS); // the floor IS 2 frames
        assert_eq!(StepDuration::Frames(3).ms(), 50);
        assert_eq!(StepDuration::Frames(4).ms(), 67);
        assert_eq!(StepDuration::Frames(60).ms(), 1_000);
        // Rounded once, not per frame: 3 frames is 50 ms, not 3 x 17 = 51.
        assert_ne!(
            StepDuration::Frames(3).ms(),
            3 * StepDuration::Frames(1).ms()
        );
        assert_eq!(StepDuration::Ms(50).ms(), 50);

        let step = MacroStep::frames(vec![], 3);
        assert_eq!(step.requested_ms(), 50);
        assert!(!step.is_short());
        // A single frame is below the floor and is raised like any short step.
        assert!(MacroStep::frames(vec![], 1).was_raised());

        assert_eq!(StepDuration::Frames(1).to_string(), "1 frame (17 ms)");
        assert_eq!(StepDuration::Frames(3).to_string(), "3 frames (50 ms)");
        assert_eq!(StepDuration::Ms(50).to_string(), "50 ms");
    }

    /// The anti-drift contract, stated as data: deadlines are ABSOLUTE offsets
    /// from the start of the run, so the scheduler never accumulates jitter.
    #[test]
    fn deadlines_are_cumulative_offsets_from_the_start() {
        assert_eq!(
            hadouken().deadlines().collect::<Vec<_>>(),
            vec![50, 100, 150, 200]
        );
        // ...and they are built from the EFFECTIVE durations, so a raised step
        // moves everything after it rather than being quietly swallowed.
        let raised = Macro::new(
            "raised",
            vec![MacroStep::new(vec![], 5), MacroStep::new(vec![], 50)],
        );
        assert_eq!(
            raised.deadlines().collect::<Vec<_>>(),
            vec![MIN_STEP_MS, MIN_STEP_MS + 50]
        );
    }

    /// However late the scheduler runs, a step is published for at least this
    /// long — because a skipped step is an input the game never sampled.
    #[test]
    fn every_step_has_a_minimum_visible_window() {
        assert_eq!(MacroStep::new(vec![], 5).min_visible_ms(), MIN_STEP_MS);
        assert_eq!(MacroStep::new(vec![], 500).min_visible_ms(), MIN_STEP_MS);
        // The opt-out is honored here too: the author asked for 5 ms.
        assert_eq!(MacroStep::short(vec![], 5).min_visible_ms(), 5);
    }

    #[test]
    fn interrupt_policies_round_trip_and_default_to_none() {
        for &policy in Interrupt::ALL {
            assert_eq!(policy.as_str().parse::<Interrupt>(), Ok(policy));
            assert_eq!(policy.to_string(), policy.as_str());
        }
        assert_eq!(Interrupt::default(), Interrupt::None);
        assert!(!Interrupt::None.is_active());
        assert!(Interrupt::AnyInput.is_active() && Interrupt::Opposing.is_active());
        assert_eq!("any_input".parse(), Ok(Interrupt::AnyInput));
        assert_eq!("AnyInput".parse(), Ok(Interrupt::AnyInput));
        let err = "sometimes".parse::<Interrupt>().unwrap_err();
        assert!(err.to_string().contains("opposing"), "{err}");
    }

    #[test]
    fn an_empty_step_list_is_representable_and_inert() {
        // Loading is lenient; validation is where this becomes actionable.
        assert!(Macro::new("nothing", vec![]).is_inert());
        assert_eq!(Macro::new("nothing", vec![]).total_ms(), 0);
    }
}
