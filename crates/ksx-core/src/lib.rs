//! ksx-core — the pure translation engine.
//!
//! `(DeviceId, KeyEvent)` streams in, `PadState` deltas come out. This crate is
//! deliberately free of Windows dependencies, I/O, threads, and allocation on the
//! hot path so the entire mapping semantics are testable in CI (proptest lives here).
//!
//! Legacy semantics this crate is contractually bound to preserve (see
//! `docs/research/design-risk-review.md` §3):
//! - one keyboard → many slots fan-out (the I-PAC4 case)
//! - all-keys-up release rule incl. cross-category custom-function aggregation
//! - opposite-axis snap (to the opposite *binding's* value — deliberate fix)
//! - state-diff before submit (only genuine transitions leave the engine)
//!
//! Module map:
//! - [`key`] — the single key vocabulary (legacy `InterceptionKey`, exact names/values)
//! - [`pad`] — XInput wire-shape [`PadState`] + legacy `VirtualXbox` ID tables
//! - [`device`] — [`DeviceId`] (instance-path identity) and [`KeyEvent`]
//! - [`selector`] — [`DeviceSelector`]: what a `[[device]] id` MEANS, so a
//!   board keeps working after it is replugged into a different USB socket
//!   (`docs/DEVICE-IDENTITY.md`)
//! - [`preset`] — [`Binding`], [`Chord`] (guarded bindings), [`Preset`], and
//!   the `default`/`empty` built-ins
//! - [`macros`] — [`Macro`]: timed sequences, the sampling floor, and the
//!   interruption policies; the scheduler that runs them lives in [`engine`]
//! - [`slot`] — [`SlotSpec`] and the 13-variant [`InvalidationReason`] taxonomy
//! - [`socd`] — [`Socd`]: SOCD cleaning, generated as chords rather than as a
//!   new engine rule; also [`socd::pointing`], the ONE definition of "where
//!   does this binding point" that opposition and diagonals are both built on
//! - [`diagonal`] — [`Diag`]: diagonals as a PRESENTATION over the stored pair.
//!   Nothing in the engine calls it; the stored model is unchanged
//! - [`templates`] — in-box preset [`Template`]s for standard panels: the
//!   zero-mapping out-of-box experience (`docs/MAPPER-UX.md` commandment 9)
//! - [`persona`] — [`Persona`]: which controller a slot presents itself as
//! - [`engine`] — the [`Engine`]: precompiled dispatch, per-device key state, diffing
//! - [`escape`] — [`EscapeDetector`], emergency-escape detection (policy lives upstream)

pub mod device;
pub mod diagonal;
pub mod engine;
pub mod escape;
pub mod key;
pub mod macros;
pub mod pad;
pub mod persona;
pub mod preset;
pub mod selector;
pub mod slot;
pub mod socd;
pub mod templates;

pub use device::{DeviceId, KeyEvent};
pub use diagonal::{Diag, Held};
pub use engine::{Deltas, Engine, EngineTables, PadDelta, ResolvedSlot};
pub use escape::{Escape, EscapeDetector};
pub use key::Key;
pub use macros::{
    Interrupt, Macro, MacroStep, MacroSwitch, MacroTrigger, OnRelease, Repeat, Retrigger,
    StepDuration, TurboRate, UnknownInterrupt, UnknownMacroSwitch, UnknownOnRelease, UnknownRepeat,
    UnknownRetrigger, MIN_STEP_MS, TURBO_MAX_HZ,
};
pub use pad::{
    safe_axis, Axis, DpadDirection, PadState, Trigger, XButton, XButtons, AXIS_CENTER, AXIS_MAX,
    AXIS_MIN,
};
pub use persona::{PadBackend, Persona, UnknownPersona};
pub use preset::{Binding, Chord, Macros, Preset, TurboBinding};
pub use selector::{DeviceFacts, DeviceSelector, Match, Qualifier, SelectorParseError};
pub use slot::{InvalidSlotNumber, InvalidationReason, SlotSpec, MAX_SLOTS, MAX_XINPUT_SLOTS};
pub use socd::{DirMechanism, OpposingPair, Pointing, Socd, UnknownSocd};
pub use templates::{Template, TemplateError, MAX_TEMPLATE_PLAYERS, TEMPLATES};
