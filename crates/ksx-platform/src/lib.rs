//! ksx-platform — Windows-specific plumbing outside the input hot path.
//!
//! - device hotplug via `CM_Register_Notification` (not 1 Hz polling)
//! - driver health: ViGEmBus presence/version; Interception presence, signature
//!   status (`keyboard.sys` is 2012-cross-signed) and the `{784c4414-…}` CI-policy
//!   state — refuse-with-explanation beats the legacy "Slot is invalidated"
//! - driver install orchestration (bundled signature-verified installers, pnputil)
//! - Task Scheduler autostart, thread-priority helpers
//!
//! Implemented incrementally M3–M6.
