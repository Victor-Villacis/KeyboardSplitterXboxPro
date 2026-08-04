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
//! Implemented in M1.
