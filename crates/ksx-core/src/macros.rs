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

/// The fastest turbo ksx will pretend to deliver, in hertz.
///
/// §0.2 again, and it is arithmetic rather than policy: one turbo cycle is a
/// press AND a release, so at a 60 Hz poll the very best case is one sample
/// pressed and one sample released — 30 cycles a second. Anything faster is a
/// number that cannot survive contact with the game, so a file asking for it is
/// CLAMPED to this and told why.
///
/// The honest ceiling is usually lower still: ksx's own step floor
/// ([`MIN_STEP_MS`]) makes the shortest visible press 33 ms and the shortest
/// visible gap another 33, so a one-step macro tops out near 15 Hz. That is not
/// a second cap — it falls out of [`Macro::turbo_gap_ms`] — but it is why the
/// *effective* rate a preset gets is reported rather than assumed.
pub const TURBO_MAX_HZ: u32 = 30;

/// How a turbo macro's REPEAT RATE was authored.
///
/// Two spellings of one number, exactly like [`StepDuration`], and for the same
/// reason: `turbo_hz = 10` is how a player thinks about an auto-fire button and
/// `gap_ms = 50` is how a frame-counter thinks about the released window
/// between two presses. Giving both is refused by the config layer — two units
/// for one number would make "which wins" something a reader has to remember.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum TurboRate {
    /// Full cycles (run + gap) per second, clamped to [`TURBO_MAX_HZ`].
    Hz(u32),
    /// The neutral gap BETWEEN runs, in milliseconds. Raised to [`MIN_STEP_MS`]
    /// for the same reason a step is: a gap the game never samples is not a
    /// gap, it is one long hold.
    GapMs(u32),
}

/// What a macro does when its last step ends and the trigger is STILL down.
///
/// [`Repeat::Once`] is the default and is the fighting-game expectation: you
/// press the button, the quarter-circle comes out, and holding the button does
/// not turn a special move into a machine gun. The other two exist because
/// auto-fire is a real thing people want on a cabinet, and because wanting it
/// should be something you *say* rather than something a stuck key does to you.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Repeat {
    /// One run per press. Holding the trigger changes nothing.
    #[default]
    Once,
    /// Re-run from step 0 as soon as the last step ends, for as long as the
    /// trigger is still down. The run in flight is never cut short — the
    /// decision is taken at the END of a run, which is what makes this compose
    /// with [`OnRelease::Finish`] instead of fighting it.
    WhileHeld,
    /// [`Repeat::WhileHeld`] with a deliberate neutral GAP between runs, so the
    /// game sees a released frame and reads two presses instead of one hold.
    /// The gap is [`Macro::turbo_gap_ms`].
    Turbo,
}

impl Repeat {
    pub const ALL: &'static [Repeat] = &[Repeat::Once, Repeat::WhileHeld, Repeat::Turbo];

    pub const fn as_str(self) -> &'static str {
        match self {
            Repeat::Once => "once",
            Repeat::WhileHeld => "while-held",
            Repeat::Turbo => "turbo",
        }
    }

    /// Does this policy re-run at all? `false` is the zero-cost path, and the
    /// behavior every preset written before this feature keeps.
    pub const fn repeats(self) -> bool {
        !matches!(self, Repeat::Once)
    }

    /// Does this policy put a released window between runs?
    pub const fn wants_gap(self) -> bool {
        matches!(self, Repeat::Turbo)
    }
}

impl fmt::Display for Repeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown repeat policy '{0}' (expected one of: once, while-held, turbo)")]
pub struct UnknownRepeat(pub String);

impl FromStr for Repeat {
    type Err = UnknownRepeat;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize(s).as_str() {
            "once" | "oneshot" | "single" => Ok(Repeat::Once),
            "whileheld" | "held" | "hold" | "repeat" => Ok(Repeat::WhileHeld),
            "turbo" | "autofire" | "rapid" => Ok(Repeat::Turbo),
            _ => Err(UnknownRepeat(s.to_owned())),
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
/// One-shot BY DEFAULT: the last step ends the run even if the trigger is still
/// held ([`Repeat::Once`]). Holding a key must never turn a special move into a
/// machine gun by accident — the two ways to ask for that on purpose are
/// [`Repeat::WhileHeld`] and [`Repeat::Turbo`].
///
/// # Precedence, when every policy has an opinion
///
/// The four settings answer four different questions and are evaluated in this
/// order, which is the order the events happen in:
///
/// 1. [`Interrupt`] — *other* input arrived. An abort is an EXIT: the run stops,
///    everything releases, and no repeat follows it. Interrupt beats everything.
/// 2. [`OnRelease`] — the trigger came up. `abort` stops now (and therefore
///    never repeats); `finish` lets the run in flight complete.
/// 3. [`Repeat`] — the last step just ended. `once` stops. `while-held` and
///    `turbo` re-run **only if the trigger is still down at that instant**,
///    which is exactly why `finish` + `while-held` means "let go and it stops
///    after this run" rather than a contradiction.
/// 4. [`Retrigger`] — a NEW press arrived while a run was in flight. Unchanged
///    by any of the above, and unrelated to repeat: repeat re-runs on its own
///    schedule and never counts as a press.
///
/// Every exit path is the same one: pending steps cancelled, everything the
/// macro held released, in one delta batch.
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
    /// What the END of a run does while the trigger is still held.
    pub repeat: Repeat,
    /// The turbo rate, as authored. Only [`Repeat::Turbo`] reads it; kept for
    /// every policy so a file that experiments with the setting does not lose
    /// the number when it flips back to `once`.
    pub turbo: Option<TurboRate>,
    /// Does this macro run at all? `true` by default, and omitted from the file
    /// when it is, so every preset written before the setting existed is byte
    /// identical.
    ///
    /// A disabled macro keeps EVERYTHING — its steps, its policies, its
    /// `macro.<name>` trigger row — and simply never starts. Deleting it would
    /// lose the work and unbind the key; this loses neither. There are exactly
    /// two reasons to want it, and both are why it is a flag rather than a
    /// comment-out:
    ///
    /// - **to TEST.** Isolating one macro means silencing its neighbours, and
    ///   the thing you silence has to come back unchanged afterwards — a
    ///   half-remembered retyping of a step list is a new bug hunting the old
    ///   one.
    /// - **to COMPETE.** A cabinet in a tournament wants macros OFF, not
    ///   deleted: the panel goes back to being a panel for an evening and the
    ///   sequences are still there on Monday. (For a whole slot at once, that
    ///   is [`MacroSwitch`], and it overrides this.)
    ///
    /// Disabling a macro that is RUNNING is an exit like any other: pending
    /// steps cancelled, everything it held released, one delta batch.
    pub enabled: bool,
}

impl Macro {
    /// A macro with the default policies: finish on release, ignore retrigger,
    /// interrupt on nothing, run once.
    pub fn new(name: impl Into<String>, steps: Vec<MacroStep>) -> Self {
        Self {
            name: name.into(),
            steps,
            on_release: OnRelease::default(),
            retrigger: Retrigger::default(),
            interrupt: Interrupt::default(),
            repeat: Repeat::default(),
            turbo: None,
            enabled: true,
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

    /// The neutral window the engine leaves BETWEEN two runs, in milliseconds.
    ///
    /// Zero for everything but [`Repeat::Turbo`]: `while-held` deliberately has
    /// no gap (it re-runs a *motion*, and a motion whose first step repeats its
    /// own last step must not blink), while turbo's entire job is the gap.
    ///
    /// The rate is resolved here, once, so the engine, validation and any plan
    /// printer agree by construction:
    ///
    /// - `turbo_hz` is clamped to [`TURBO_MAX_HZ`] and turned into a cycle
    ///   length; the gap is whatever the cycle has left after the run;
    /// - `gap_ms` is taken as written;
    /// - either way the result is RAISED to [`MIN_STEP_MS`], because a gap a
    ///   60 Hz poller cannot sample is not a gap — the game reads one long hold
    ///   and the turbo silently does nothing (§0.2).
    ///
    /// A turbo with no rate at all falls back to the floor rather than guessing
    /// a rate; the config layer refuses that file, and this keeps the model
    /// total.
    pub fn turbo_gap_ms(&self) -> u32 {
        if !self.repeat.wants_gap() {
            return 0;
        }
        let asked = match self.turbo {
            None => MIN_STEP_MS,
            Some(TurboRate::GapMs(ms)) => ms,
            Some(TurboRate::Hz(hz)) => {
                let cycle = cycle_ms(hz);
                let run = u32::try_from(self.total_ms()).unwrap_or(u32::MAX);
                cycle.saturating_sub(run)
            }
        };
        asked.max(MIN_STEP_MS)
    }

    /// One full turbo cycle — a run plus its gap — in milliseconds.
    pub fn turbo_cycle_ms(&self) -> u64 {
        self.total_ms() + u64::from(self.turbo_gap_ms())
    }

    /// The rate a turbo macro ACTUALLY delivers, in hertz, rounded to nearest.
    ///
    /// This is the number worth printing: `turbo_hz = 30` on a 50 ms macro is
    /// not 30 Hz and never could be, and saying so is the whole point of
    /// resolving the rate in one place.
    pub fn effective_turbo_hz(&self) -> u32 {
        let cycle = self.turbo_cycle_ms();
        if cycle == 0 {
            return 0;
        }
        u32::try_from((1_000 + cycle / 2) / cycle).unwrap_or(u32::MAX)
    }

    /// Was the authored turbo rate not deliverable as written? Returns the
    /// `(asked, effective)` hertz so validation can say both numbers.
    ///
    /// `None` when the macro is not a turbo, was authored as a `gap_ms` that
    /// survived the floor, or asked for a rate it actually gets.
    pub fn turbo_rate_clamped(&self) -> Option<(u32, u32)> {
        let Some(TurboRate::Hz(asked)) = self.turbo else {
            return None;
        };
        if !self.repeat.wants_gap() {
            return None;
        }
        let effective = self.effective_turbo_hz();
        (effective != asked).then_some((asked, effective))
    }

    /// Was an explicit `gap_ms` raised to the sampling floor?
    pub fn turbo_gap_raised(&self) -> Option<(u32, u32)> {
        let Some(TurboRate::GapMs(ms)) = self.turbo else {
            return None;
        };
        (self.repeat.wants_gap() && ms < MIN_STEP_MS).then_some((ms, MIN_STEP_MS))
    }
}

/// Milliseconds in one cycle at `hz`, with `hz` clamped to the sampling ceiling
/// and to at least 1 (a 0 Hz turbo is a file that meant "off", not a division by
/// zero).
fn cycle_ms(hz: u32) -> u32 {
    let hz = hz.clamp(1, TURBO_MAX_HZ);
    (1_000 + hz / 2) / hz
}

/// A slot-wide master switch for macros — the "tournament mode" setting.
///
/// [`Macro::enabled`] silences ONE macro; this silences a whole slot in one
/// edit, which is the shape the request actually comes in: a cabinet entering a
/// tournament wants every sequence off on that panel, not a hunt through a
/// preset flipping flags it will have to flip back. It is a SLOT property
/// rather than a preset one for the same reason [`crate::Socd`] is — the panel
/// and the occasion decide it, and the preset (which is shared, and which M7
/// lets people hand around) must not have to change to suit them.
///
/// [`MacroSwitch::Off`] beats every per-macro `enabled = true`: a master switch
/// that individual settings could out-vote would not be a master switch.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum MacroSwitch {
    /// Macros run, subject to their own [`Macro::enabled`]. The default, and
    /// what every configuration written before this setting existed means.
    #[default]
    On,
    /// No macro on this slot runs, whatever its own flag says. Definitions and
    /// trigger rows are untouched — this is a mute, not a delete.
    Off,
}

impl MacroSwitch {
    pub const ALL: &'static [MacroSwitch] = &[MacroSwitch::On, MacroSwitch::Off];

    /// Canonical name — what a config file stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            MacroSwitch::On => "on",
            MacroSwitch::Off => "off",
        }
    }

    /// Does this slot run macros at all? `false` is the tournament setting.
    pub const fn is_on(self) -> bool {
        matches!(self, MacroSwitch::On)
    }
}

impl fmt::Display for MacroSwitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown macros switch '{0}' (expected one of: on, off)")]
pub struct UnknownMacroSwitch(pub String);

impl FromStr for MacroSwitch {
    type Err = UnknownMacroSwitch;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match normalize(s).as_str() {
            "on" | "true" | "yes" | "enabled" => Ok(MacroSwitch::On),
            "off" | "false" | "no" | "disabled" | "none" => Ok(MacroSwitch::Off),
            _ => Err(UnknownMacroSwitch(s.to_owned())),
        }
    }
}

/// A key that starts a macro: the file row `macro.<name> = "<key>"`.
///
/// Triggers are a separate list from [`crate::Preset::entries`] for the same
/// reason chords are: "no macros ⇒ nothing changed" stays checkable rather than
/// claimed. Many keys → one macro is native (multi-bind).
///
/// # One macro per key, per preset
///
/// One key starting SEVERAL macros of the same preset is refused by the writers
/// (`ksx map`, the pipe, Studio) unless the caller forces it, and the reason is
/// worth stating where the model lives, because the rule that is right for
/// bindings is wrong here:
///
/// - an ordinary BINDING is declarative STATE. Two keys setting the same bit is
///   well-defined, one key setting two bits is well-defined, and fan-out is the
///   product rather than a hazard — which is why nothing about
///   [`crate::Preset::entries`] is unique.
/// - a MACRO is an imperative TIMELINE. Two of them started by one key do not
///   compose into a third timeline; they run at once and the game reads their
///   SUPERPOSITION — a state no single step list contains, repeating for as long
///   as the loudest `repeat` policy among them says. Nobody asked for that
///   sequence, and it is invisible in the macro you are debugging.
///
/// So the model still ALLOWS it (hand-edited files exist, and validation warns
/// about them rather than refusing to load), and the write path is what says no.
/// Cross-slot and cross-preset sharing are untouched: that is fan-out again —
/// two players, two timelines, one key — and it stays legal.
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
    fn repeat_policies_round_trip_and_default_to_once() {
        for &policy in Repeat::ALL {
            assert_eq!(policy.as_str().parse::<Repeat>(), Ok(policy));
            assert_eq!(policy.to_string(), policy.as_str());
        }
        assert_eq!(Repeat::default(), Repeat::Once);
        assert!(!Repeat::Once.repeats());
        assert!(Repeat::WhileHeld.repeats() && Repeat::Turbo.repeats());
        // Only turbo wants a released window between runs.
        assert!(!Repeat::WhileHeld.wants_gap() && Repeat::Turbo.wants_gap());
        assert_eq!("while_held".parse(), Ok(Repeat::WhileHeld));
        assert_eq!("WhileHeld".parse(), Ok(Repeat::WhileHeld));
        assert_eq!("auto-fire".parse(), Ok(Repeat::Turbo));
        let err = "sometimes".parse::<Repeat>().unwrap_err();
        assert!(err.to_string().contains("turbo"), "{err}");
    }

    /// The turbo arithmetic, and the two ceilings it runs into.
    #[test]
    fn a_turbo_rate_resolves_to_a_gap_the_game_can_see() {
        let turbo = |rate: TurboRate| {
            let mut m = Macro::new("t", vec![MacroStep::new(vec![], 50)]);
            m.repeat = Repeat::Turbo;
            m.turbo = Some(rate);
            m
        };

        // 10 Hz on a 50 ms macro: a 100 ms cycle, so a 50 ms gap.
        let m = turbo(TurboRate::Hz(10));
        assert_eq!(m.turbo_gap_ms(), 50);
        assert_eq!(m.turbo_cycle_ms(), 100);
        assert_eq!(m.effective_turbo_hz(), 10);
        assert_eq!(m.turbo_rate_clamped(), None);

        // The gap spelling means exactly the same thing.
        assert_eq!(turbo(TurboRate::GapMs(50)).turbo_gap_ms(), 50);

        // Above the sampling ceiling the rate is CLAMPED, not refused — and
        // what comes out is reported honestly (a 50 ms macro cannot do 30 Hz:
        // one run alone is longer than a 33 ms cycle).
        let fast = turbo(TurboRate::Hz(120));
        assert_eq!(fast.turbo_gap_ms(), MIN_STEP_MS);
        assert_eq!(fast.turbo_cycle_ms(), 50 + u64::from(MIN_STEP_MS));
        assert_eq!(fast.turbo_rate_clamped(), Some((120, 12)));

        // A gap below the floor is raised for the same reason a step is: an
        // unsampled release is not a release, it is one long hold.
        let tight = turbo(TurboRate::GapMs(5));
        assert_eq!(tight.turbo_gap_ms(), MIN_STEP_MS);
        assert_eq!(tight.turbo_gap_raised(), Some((5, MIN_STEP_MS)));

        // The ceiling itself is 30 Hz, and it is arithmetic: a press plus a
        // release is two samples at 60 Hz.
        assert_eq!(TURBO_MAX_HZ, 30);
        assert_eq!(cycle_ms(30), 33);
        assert_eq!(cycle_ms(0), 1_000); // "0 Hz" means "off", never a panic
    }

    /// Every other policy has no gap at all — `while-held` re-runs a MOTION,
    /// and a motion whose first step repeats its own last step must not blink.
    #[test]
    fn only_turbo_has_a_gap() {
        let mut m = Macro::new("t", vec![MacroStep::new(vec![], 50)]);
        assert_eq!(m.turbo_gap_ms(), 0);
        m.repeat = Repeat::WhileHeld;
        assert_eq!(m.turbo_gap_ms(), 0);
        // A rate carried on a non-turbo macro changes nothing (validation says
        // so out loud); flipping the policy is what turns it on.
        m.turbo = Some(TurboRate::Hz(10));
        assert_eq!(m.turbo_gap_ms(), 0);
        assert_eq!(m.turbo_rate_clamped(), None);
        m.repeat = Repeat::Turbo;
        assert_eq!(m.turbo_gap_ms(), 50);
    }

    /// `enabled` is the one policy whose default is `true`, and the whole point
    /// of the flag is that turning it off loses nothing.
    #[test]
    fn a_macro_is_enabled_until_it_is_told_otherwise() {
        let mut m = hadouken();
        assert!(m.enabled);
        m.enabled = false;
        // Everything that makes it a macro survives being switched off — that
        // is the difference between disabling and deleting.
        assert_eq!(m.steps.len(), 4);
        assert_eq!(m.total_ms(), 200);
        assert!(!m.is_inert(), "a disabled macro is silenced, not emptied");
    }

    #[test]
    fn the_slot_switch_defaults_to_on_and_takes_the_spellings_people_type() {
        for &switch in MacroSwitch::ALL {
            assert_eq!(switch.as_str().parse::<MacroSwitch>(), Ok(switch));
            assert_eq!(switch.to_string(), switch.as_str());
        }
        assert_eq!(MacroSwitch::default(), MacroSwitch::On);
        assert!(MacroSwitch::On.is_on() && !MacroSwitch::Off.is_on());
        assert_eq!("Off".parse(), Ok(MacroSwitch::Off));
        assert_eq!("disabled".parse(), Ok(MacroSwitch::Off));
        assert_eq!("true".parse(), Ok(MacroSwitch::On));
        let err = "sometimes".parse::<MacroSwitch>().unwrap_err();
        assert!(err.to_string().contains("off"), "{err}");
    }

    #[test]
    fn an_empty_step_list_is_representable_and_inert() {
        // Loading is lenient; validation is where this becomes actionable.
        assert!(Macro::new("nothing", vec![]).is_inert());
        assert_eq!(Macro::new("nothing", vec![]).total_ms(), 0);
    }
}
