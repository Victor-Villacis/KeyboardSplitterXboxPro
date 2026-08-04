//! Driver-health report types. Plain serializable data — collected on Windows by
//! [`crate::collect`], but constructible anywhere so verdict logic and JSON shape
//! are testable off-cabinet.

use serde::Serialize;

/// Everything `ksx doctor` knows about the driver stack. All fields are
/// best-effort: a query failure yields `None` / `Unknown`, never an error —
/// the report must come back even (especially) on a broken machine.
#[derive(Debug, Clone, Serialize)]
pub struct DriverReport {
    /// ViGEmBus — the virtual-pad bus ksx outputs through.
    pub vigembus: BusDriverReport,
    /// ScpVBus — the legacy C# app's bus. Coexistence info only; ksx never uses it.
    pub scpvbus: BusDriverReport,
    /// Interception keyboard/mouse class upper filters (M3 capture backend).
    pub interception: InterceptionReport,
    /// Code-integrity policy state relevant to the 2026 cross-signed-trust removal.
    pub code_integrity: CodeIntegrityReport,
}

/// A kernel bus driver registered as a service.
#[derive(Debug, Clone, Serialize)]
pub struct BusDriverReport {
    /// Service key exists under `HKLM\SYSTEM\CurrentControlSet\Services`.
    pub installed: bool,
    pub service: Option<ServiceInfo>,
    /// `None` when the driver file is absent from `System32\drivers`.
    pub driver_file: Option<DriverFileReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub start_type: StartType,
    pub image_path: Option<String>,
    pub display_name: Option<String>,
    /// Live state from the Service Control Manager (not the registry).
    pub state: ServiceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartType {
    Boot,
    System,
    Auto,
    Demand,
    Disabled,
    Unknown,
}

impl StartType {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            0 => Self::Boot,
            1 => Self::System,
            2 => Self::Auto,
            3 => Self::Demand,
            4 => Self::Disabled,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopped,
    StartPending,
    StopPending,
    Paused,
    PausePending,
    ContinuePending,
    /// Registry key exists but the SCM has no such service.
    NotRegistered,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriverFileReport {
    pub path: String,
    /// Canonical `a.b.c.d` from `VS_FIXEDFILEINFO`.
    pub file_version: Option<String>,
    /// `StringFileInfo\…\FileVersion` as tools display it (e.g. "1.00 built by: WinDDK").
    pub file_version_string: Option<String>,
    pub company: Option<String>,
    pub description: Option<String>,
    pub signature: Option<SignatureInfo>,
}

/// Authenticode summary. `status` is the WinVerifyTrust outcome refined with the
/// signing cert's own validity window: a 2012-expired cross-signed cert still
/// verifies `Valid` today via its timestamp countersignature, which is exactly
/// the "borrowed time" state worth flagging.
#[derive(Debug, Clone, Serialize)]
pub struct SignatureInfo {
    pub status: SignatureStatus,
    pub signer: Option<String>,
    pub issuer: Option<String>,
    /// Signing certificate `NotAfter`, RFC 3339 UTC.
    pub not_after_utc: Option<String>,
    pub cert_expired: Option<bool>,
}

impl SignatureInfo {
    pub fn unknown() -> Self {
        Self {
            status: SignatureStatus::Unknown,
            signer: None,
            issuer: None,
            not_after_utc: None,
            cert_expired: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureStatus {
    /// Trust chain verifies and the signing cert is within its validity window.
    Valid,
    /// Trust chain verifies (timestamp countersignature) but the signing cert
    /// has expired — the legacy cross-signed state.
    ValidExpiredCert,
    /// WinVerifyTrust returned CERT_E_EXPIRED.
    Expired,
    Untrusted,
    Unsigned,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct InterceptionReport {
    /// Filter live in the class stack AND keyboard.sys on disk.
    pub installed: bool,
    pub keyboard: ClassFilterReport,
    pub mouse: ClassFilterReport,
}

/// One device-class filter stack (keyboard or mouse).
#[derive(Debug, Clone, Serialize)]
pub struct ClassFilterReport {
    /// Raw `UpperFilters` REG_MULTI_SZ; empty when absent/unreadable.
    pub upper_filters: Vec<String>,
    /// Interception's filter service name present in `upper_filters`.
    pub filter_active: bool,
    /// `System32\drivers\keyboard.sys` / `mouse.sys`.
    pub driver_file: Option<DriverFileReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeIntegrityReport {
    /// The `{784C4414-79F4-4C32-A6A5-F0FB42A51D0D}` "Microsoft Windows Cross
    /// Certificates for Code Integrity Exceptions Audit Policy" — the 2026
    /// cross-signed-trust-removal rollout. `None` = not deployed on this machine.
    pub cross_cert_policy: Option<CiPolicyReport>,
    /// Total `.cip` files in `System32\CodeIntegrity\CiPolicies\Active`;
    /// `None` when the store is unreadable.
    pub active_policy_count: Option<usize>,
    /// `HKLM\SYSTEM\CurrentControlSet\Control\CI\WhqlOnlyEvaluation` — the
    /// evaluation-mode boot/uptime counters Windows accumulates before flipping
    /// cross-signed-trust removal to enforcement. Presence = still evaluating.
    pub whql_evaluation: Option<WhqlEvaluationReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CiPolicyReport {
    pub guid: String,
    pub file_path: String,
    /// Friendly name embedded in the policy binary's Settings section.
    pub name: Option<String>,
    /// `PolicyInfo/Information/Id`, e.g. "10.29611.0.0".
    pub policy_id: Option<String>,
    pub mode: CiPolicyMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiPolicyMode {
    Audit,
    Enforce,
    /// Option flags live inside a PKCS#7-wrapped binary policy and `CiTool
    /// --list-policies` needs elevation; without either signal we don't guess.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhqlEvaluationReport {
    pub num_boot_sessions: Option<u32>,
    pub latest_boot_id: Option<u32>,
    /// Last policy status event, RFC 3339 UTC (registry FILETIME).
    pub status_event_time_utc: Option<String>,
    /// Accumulated evaluation uptime, seconds.
    pub system_uptime_secs: Option<u64>,
}
