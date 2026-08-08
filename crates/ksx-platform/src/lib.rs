//! ksx-platform — Windows-specific plumbing outside the input hot path.
//!
//! Implemented now (feeds `ksx doctor`):
//! - [`collect`] (Windows): read-only driver-health snapshot — ViGEmBus,
//!   legacy ScpVBus, Interception class filters + Authenticode state, and the
//!   `{784C4414-…}` cross-signed-trust-removal CI policy (audit vs enforce,
//!   `CI\WhqlOnlyEvaluation` counters). Never elevates, never errors: broken
//!   machines produce a complete report with `Unknown`/`None` fields.
//! - [`summarize`]: pure report → [`Advice`] verdicts with stable codes for
//!   scripting (`--json`).
//!
//! - [`installer`] / [`autostart`] (M5): bundled signature-verified driver
//!   install orchestration and per-user Task Scheduler registration.
//! - [`inject`] (M6): `SendInput` keystroke synthesis, so a WinUSB-claimed
//!   arcade panel still types when ksx is not emulating.
//! - [`winusb`] (M6): the WinUSB rebind lifecycle — a read-only survey of the
//!   device tree, INF generation, and the exact `pnputil` command lines for
//!   claim and release. **Nothing in this module mutates a driver binding**;
//!   `apply` shells out to `pnputil`, and only the caller decides to run it.
//!
//! Still to come: hotplug via `CM_Register_Notification`, thread-priority
//! helpers.

pub mod advice;
pub mod app_paths;
pub mod autostart;
pub mod inject;
pub mod installer;
pub mod parse;
pub mod process;
pub mod report;
pub mod sealed;
pub mod sha256;
pub mod virtual_pads;
pub mod winusb;
pub mod xinput;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{collect, collect_vigembus, collect_virtual_pads};

pub use advice::{summarize, Advice, Severity};
pub use report::{
    BusDriverReport, CiPolicyMode, CiPolicyReport, ClassFilterReport, CodeIntegrityReport,
    DriverFileReport, DriverReport, HidMaestroReport, InterceptionReport, ServiceInfo,
    ServiceState, SignatureInfo, SignatureStatus, StartType, WhqlEvaluationReport,
};
pub use virtual_pads::{OwnerProcess, PersonaGuess, VirtualPadInfo, VirtualPadReport};
