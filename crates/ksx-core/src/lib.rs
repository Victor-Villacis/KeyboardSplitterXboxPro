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
//! - [`preset`] — [`Binding`], [`Chord`] (guarded bindings), [`Preset`], and
//!   the `default`/`empty` built-ins
//! - [`macros`] — [`Macro`]: timed sequences, the sampling floor, and the
//!   interruption policies; the scheduler that runs them lives in [`engine`]
//! - [`slot`] — [`SlotSpec`] and the 13-variant [`InvalidationReason`] taxonomy
//! - [`socd`] — [`Socd`]: SOCD cleaning, generated as chords rather than as a
//!   new engine rule
//! - [`persona`] — [`Persona`]: which controller a slot presents itself as
//! - [`engine`] — the [`Engine`]: precompiled dispatch, per-device key state, diffing
//! - [`escape`] — [`EscapeDetector`], emergency-escape detection (policy lives upstream)

pub mod device;
pub mod engine;
pub mod escape;
pub mod key;
pub mod macros;
pub mod pad;
pub mod persona;
pub mod preset;
pub mod slot;
pub mod socd;

pub use device::{DeviceId, KeyEvent};
pub use engine::{Deltas, Engine, EngineTables, PadDelta, ResolvedSlot};
pub use escape::{Escape, EscapeDetector};
pub use key::Key;
pub use macros::{
    Interrupt, Macro, MacroStep, MacroTrigger, OnRelease, Retrigger, StepDuration,
    UnknownInterrupt, UnknownOnRelease, UnknownRetrigger, MIN_STEP_MS,
};
pub use pad::{
    Axis, DpadDirection, PadState, Trigger, XButton, XButtons, AXIS_CENTER, AXIS_MAX, AXIS_MIN,
};
pub use persona::{Persona, UnknownPersona};
pub use preset::{Binding, Chord, Macros, Preset};
pub use slot::{InvalidSlotNumber, InvalidationReason, SlotSpec, MAX_SLOTS, MAX_XINPUT_SLOTS};
pub use socd::{OpposingPair, Socd, UnknownSocd};
