//! ksx-output — virtual Xbox 360 pads.
//!
//! Primary (only, for now) backend: ViGEmBus 1.22.0 through the vendored
//! `vigem-client` (pure-Rust IOCTL, no C DLL). `request_notification` is used from
//! day one: the LED/player-index callback is the authoritative XInput slot mapping
//! and replaces the legacy 30 Hz XInput poller + slot-guessing heuristics.
//!
//! `PadState` stays in XInput wire shape so plan-B backends (HIDMaestro,
//! libvirtualhid — see `docs/research/virtual-gamepad-2026.md`) need no translation
//! layer. Implemented in M2.
