//! ksx-capture — per-device keyboard capture + OS-input blocking.
//!
//! Three backends behind one trait (see `docs/research/keyboard-capture-2026.md`):
//! - `interception` (M3, day-1 default): via `kanata-interception`; ships with a
//!   loud EOL warning (2012 cross-signed cert + the 2026 CI-policy cliff) and a
//!   10-slot ID-drift exhaustion detector.
//! - `winusb` (M6, strategic primary): claims the I-PAC's `MI_00` interface via
//!   in-box winusb.sys + `nusb` — blocking and identity become structural.
//! - `rawinput_identify` (M3): identify-only device picker; NEVER a blocking
//!   backend (the RawInput+LLHOOK correlation hack is rejected by design).
//!
//! Hot-path rule: the capture thread only receives, decides pass/suppress from an
//! arc-swap snapshot, and pushes into a bounded channel. Zero alloc, zero locks.
