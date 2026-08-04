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
//! Still to come (M3–M6): hotplug via `CM_Register_Notification`, driver
//! install orchestration (bundled signature-verified installers, pnputil),
//! Task Scheduler autostart, thread-priority helpers.

pub mod advice;
pub mod autostart;
pub mod installer;
pub mod parse;
pub mod process;
pub mod report;
pub mod sealed;
pub mod sha256;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::collect;

pub use advice::{summarize, Advice, Severity};
pub use report::{
    BusDriverReport, CiPolicyMode, CiPolicyReport, ClassFilterReport, CodeIntegrityReport,
    DriverFileReport, DriverReport, InterceptionReport, ServiceInfo, ServiceState, SignatureInfo,
    SignatureStatus, StartType, WhqlEvaluationReport,
};
