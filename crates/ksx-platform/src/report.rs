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
    /// Signing certificate `NotBefore`, RFC 3339 UTC.
    pub not_before_utc: Option<String>,
    /// Signing certificate `NotAfter`, RFC 3339 UTC.
    pub not_after_utc: Option<String>,
    pub cert_expired: Option<bool>,
    /// The countersignature that says *when* this file was signed, and whether
    /// that claim survives checking. `None` when the signature carries none.
    pub timestamp: Option<TimestampInfo>,
}

impl SignatureInfo {
    pub fn unknown() -> Self {
        Self {
            status: SignatureStatus::Unknown,
            signer: None,
            issuer: None,
            not_before_utc: None,
            not_after_utc: None,
            cert_expired: None,
            timestamp: None,
        }
    }
}

/// A timestamp countersignature, **and the result of checking it**.
///
/// A countersignature is a claim ("this file was signed at T"), not a fact. It
/// is worth something only once four separate things have been confirmed, and
/// each one is a field here rather than an assumption:
///
/// 1. it really is a *timestamp* countersignature — `SGNR_TYPE_TIMESTAMP`, the
///    class wintrust puts both RFC 3161 and legacy PKCS#9 countersigners in —
///    and not some other unauthenticated attribute ([`is_timestamp_signer`]);
/// 2. its own certificate chain verified, i.e. the timestamping authority is
///    someone Windows trusts ([`chain_ok`]);
/// 3. the authority's certificate was itself valid at the instant it claims to
///    have stamped ([`within_timestamp_cert_validity`]) — a TSA vouching for a
///    moment outside its own life is vouching for nothing;
/// 4. and the stamped instant falls inside the *signing* certificate's validity
///    window ([`within_signing_cert_validity`]) — the actual question.
///
/// [`TimestampInfo::problem`] is the single place those are turned into a
/// verdict, so "verified" can never drift from "checked".
///
/// [`is_timestamp_signer`]: Self::is_timestamp_signer
/// [`chain_ok`]: Self::chain_ok
/// [`within_timestamp_cert_validity`]: Self::within_timestamp_cert_validity
/// [`within_signing_cert_validity`]: Self::within_signing_cert_validity
#[derive(Debug, Clone, Serialize)]
pub struct TimestampInfo {
    /// The signing time the countersignature asserts, RFC 3339 UTC.
    pub signed_at_utc: Option<String>,
    /// The same instant in FILETIME ticks — 0 when it could not be read.
    pub signed_at_ticks: u64,
    /// Timestamping authority: the leaf CN of the countersignature's own chain.
    pub authority: Option<String>,
    /// wintrust classified this countersigner as `SGNR_TYPE_TIMESTAMP`.
    pub is_timestamp_signer: bool,
    /// The countersignature verified on its own terms (`dwError == 0`) and came
    /// with a certificate chain.
    pub chain_ok: bool,
    /// `signed_at` lies inside the **signing** certificate's validity window.
    pub within_signing_cert_validity: bool,
    /// `signed_at` lies inside the **timestamping** certificate's own window.
    pub within_timestamp_cert_validity: bool,
}

impl TimestampInfo {
    /// The first requirement this timestamp fails, or `None` if it proves what
    /// it claims. Ordered cheapest-and-most-fundamental first so the message a
    /// user sees names the real problem rather than a downstream symptom.
    ///
    /// Note what is deliberately *not* checked: "the signing time is not in the
    /// future". A timestamp is only ever consulted for a certificate that has
    /// already expired, so `signed_at <= NotAfter < now` follows from
    /// [`within_signing_cert_validity`](Self::within_signing_cert_validity) —
    /// adding a second, clock-sensitive comparison would buy nothing and could
    /// fail on a machine with a skewed RTC.
    pub fn problem(&self) -> Option<&'static str> {
        if !self.is_timestamp_signer {
            return Some(
                "the countersignature is not a timestamp countersignature \
                 (wintrust did not classify it as SGNR_TYPE_TIMESTAMP)",
            );
        }
        if !self.chain_ok {
            return Some(
                "the timestamp countersignature's own certificate chain did not verify, \
                 so the timestamping authority is not one Windows trusts",
            );
        }
        if self.signed_at_ticks == 0 || self.signed_at_utc.is_none() {
            return Some("the timestamp carries no readable signing time");
        }
        if !self.within_timestamp_cert_validity {
            return Some(
                "the timestamping authority's own certificate was not valid at the instant \
                 it claims to have countersigned",
            );
        }
        if !self.within_signing_cert_validity {
            return Some(
                "the timestamp's signing time falls outside the signing certificate's \
                 validity window, so it does not show the file was signed while the \
                 certificate was live",
            );
        }
        None
    }

    /// All four checks passed: this timestamp really does prove the file was
    /// signed while its certificate was valid.
    pub fn is_verified(&self) -> bool {
        self.problem().is_none()
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
