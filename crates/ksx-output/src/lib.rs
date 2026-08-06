//! ksx-output — virtual game pads.
//!
//! Primary (only, for now) backend: ViGEmBus 1.22.0 through the vendored
//! `vigem-client` (pure-Rust IOCTL, no C DLL). Slot identity comes from **active
//! correlation** — pulse LT below the game-visible threshold and watch which
//! XInput slot echoes it. Measured on real hardware, both `get_user_index()` and
//! the LED notification channel report wrong or missing slots
//! (docs/research/m2-xinput-findings.md); the echo is the only ground truth.
//! Notifications still feed rumble/LED *feedback* — they're just not trusted
//! for slot identity.
//!
//! `PadState` stays in XInput wire shape so plan-B backends (HIDMaestro,
//! libvirtualhid — see `docs/research/virtual-gamepad-2026.md`) need no translation
//! layer.
//!
//! Layout:
//! - [`VirtualPadBackend`] — the object-safe trait (design §3.2), the plan-B
//!   insurance policy.
//! - [`VigemBackend`] (Windows) — ViGEmBus implementation. Fails fast with
//!   [`OutputError::BusNotFound`] when the driver is absent; dropping it unplugs
//!   every pad (crash safety — a clean process exit never leaves ghost pads).
//! - [`MockBackend`] — driverless implementation for engine-to-output tests.
//! - `ds4` (Windows) — `PadState` → DualShock 4 report for the PlayStation
//!   persona. Not a field copy like the Xbox path: Y axes invert, the D-pad
//!   collapses to a 4-bit hat, and triggers are analog *and* digital.
//! - [`HidMaestroBackend`] (M8) — HIDMaestro implementation, for the personas
//!   ViGEmBus cannot express (DualSense, Switch Pro, Xbox Series). The protocol
//!   itself lives in `ksx-hidmaestro`; this crate holds only the adapter.
//!   **HIDMaestro is not installed on this cabinet**, so `connect()` currently
//!   returns [`OutputError::HidMaestroMissing`] — see that crate's docs for
//!   what is verified vs written-to-spec.
//! - [`RoutedBackend`] — persona → backend selection. X360/DS4 stay on ViGEm
//!   (slots 1–4 never migrate to HIDMaestro's XInput synthesis layer); the new
//!   personas select HIDMaestro, lazily, so a cabinet that uses none of them
//!   never even probes for the driver.
//! - `tests/loopback.rs` — cabinet-only XInput round-trip behind the `cab-tests`
//!   feature; never runs in CI or on machines without ViGEmBus.

mod backend;
#[cfg(windows)]
mod ds4;
mod error;
mod hidmaestro;
pub mod mock;
pub mod router;
#[cfg(windows)]
mod vigem;

pub use backend::{Feedback, PadHandle, VirtualPadBackend};
pub use error::OutputError;
pub use hidmaestro::HidMaestroBackend;
pub use mock::MockBackend;
pub use router::{HidMaestroFactory, RoutedBackend};
#[cfg(windows)]
pub use vigem::VigemBackend;
