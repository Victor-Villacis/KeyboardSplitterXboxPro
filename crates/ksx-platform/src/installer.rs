//! `ksx install-drivers` — verify the bundled ViGEmBus installer, then plan.
//!
//! The rule this module exists to enforce: **nothing is executed before its
//! identity is proven.** The bundled installer is a kernel-driver setup running
//! elevated; if it were swapped, everything downstream is moot. So the order is
//! always
//!
//! 1. locate the bundled file,
//! 2. SHA-256 it against the value recorded in `docs/DRIVERS.md`,
//! 3. check its Authenticode chain *and* that the signer really is Nefarius,
//! 4. only then offer to run it — and only when explicitly asked to.
//!
//! Both checks are required. A hash alone cannot notice that we recorded the
//! hash of a *tampered* file; a signature alone cannot notice that someone
//! swapped ViGEmBus 1.22.0 for a differently-signed Nefarius binary. Two
//! independent pins is the point.
//!
//! Everything except [`verify`]'s Authenticode call and the execution step is
//! pure, so the plan, the verdicts and the rendering are unit-tested on any
//! platform with fixtures.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::report::{DriverReport, SignatureInfo, SignatureStatus};
use crate::sha256;

/// File name of the bundled ViGEmBus setup (the official v1.22.0 release asset).
pub const INSTALLER_FILE_NAME: &str = "ViGEmBus_1.22.0_x64_x86_arm64.exe";

/// SHA-256 recorded in `docs/DRIVERS.md`, re-verified 2026-08-04.
pub const EXPECTED_SHA256: &str =
    "89220A7865076B342892F98865F3499FB7C4CFD673159E89D352C360FD014C6A";

/// Authenticode subject fragment the signer name must contain.
///
/// A fragment, not the whole DN: `CertGetNameStringW`'s simple display name
/// returns the CN only ("Nefarius Software Solutions e.U."), while the full
/// subject in `docs/DRIVERS.md` also carries `L=Wels, C=AT`. Matching on the
/// organisation name is the part that is actually load-bearing.
pub const EXPECTED_SIGNER: &str = "Nefarius Software Solutions";

/// The bundle is a WiX Burn bootstrapper (confirmed by inspection of the
/// shipped binary), so these are its documented unattended switches.
pub const DEFAULT_INSTALL_ARGS: &[&str] = &["/quiet", "/norestart"];

// ---------------------------------------------------------------------------
// Locating the bundle
// ---------------------------------------------------------------------------

/// Where the installer was looked for, and whether it turned up.
#[derive(Clone, Debug, Serialize)]
pub struct Location {
    pub found: Option<PathBuf>,
    /// Every candidate tried, in order — printed when nothing was found so the
    /// user can see exactly where to drop the file.
    pub searched: Vec<PathBuf>,
}

/// Search for the bundled installer next to the running `ksx`, then in the
/// development tree, then in the working directory.
///
/// `exe_dir` is the directory holding `ksx.exe`; a release lays the file out as
/// `ksx.exe` + `drivers\<INSTALLER_FILE_NAME>`. During development `ksx.exe`
/// lives in `target\debug\`, so the repo-root `drivers\` directory is reached
/// by walking up.
pub fn locate(exe_dir: Option<&Path>, cwd: Option<&Path>) -> Location {
    let mut searched = Vec::new();
    let mut push = |dir: PathBuf| {
        let candidate = dir.join("drivers").join(INSTALLER_FILE_NAME);
        if !searched.contains(&candidate) {
            searched.push(candidate);
        }
    };
    if let Some(dir) = exe_dir {
        push(dir.to_path_buf());
        // target\debug\ksx.exe → repo root is three levels up in a workspace
        // build (target\debug, target, root) and two in a plain build.
        for ups in 1..=3 {
            let mut up = dir.to_path_buf();
            let mut ok = true;
            for _ in 0..ups {
                if !up.pop() {
                    ok = false;
                    break;
                }
            }
            if ok {
                push(up);
            }
        }
    }
    if let Some(dir) = cwd {
        push(dir.to_path_buf());
    }
    let found = searched.iter().find(|p| p.is_file()).cloned();
    Location { found, searched }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Result of pinning the installer's identity. Both checks always run so the
/// report can say *which* pin failed.
#[derive(Clone, Debug, Serialize)]
pub struct Verification {
    pub path: PathBuf,
    /// Computed digest, uppercase hex. `None` when the file could not be read.
    pub sha256: Option<String>,
    pub expected_sha256: &'static str,
    pub sha256_ok: bool,
    /// `None` off Windows, where there is no Authenticode to check.
    pub signature: Option<SignatureInfo>,
    pub expected_signer: &'static str,
    pub signature_ok: bool,
    /// Set when the file could not even be read.
    pub read_error: Option<String>,
}

impl Verification {
    /// Both pins must hold. There is deliberately no "warn and continue".
    pub fn is_trusted(&self) -> bool {
        self.sha256_ok && self.signature_ok
    }

    /// Every reason this file is not the file we shipped, most important first.
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(err) = &self.read_error {
            out.push(format!("the installer could not be read: {err}"));
        }
        if !self.sha256_ok {
            out.push(match &self.sha256 {
                Some(got) => format!(
                    "SHA-256 mismatch: expected {}, got {got}",
                    self.expected_sha256
                ),
                None => "SHA-256 could not be computed".to_owned(),
            });
        }
        if !self.signature_ok {
            out.push(match &self.signature {
                Some(sig) => match &sig.signer {
                    Some(signer) if sig.status != SignatureStatus::Valid => format!(
                        "Authenticode is {:?} (signer '{signer}'); a driver installer must \
                         verify as Valid",
                        sig.status
                    ),
                    Some(signer) => format!(
                        "Authenticode is valid but the signer is '{signer}', not '{}'",
                        self.expected_signer
                    ),
                    None => format!(
                        "Authenticode is {:?} and no signer could be read",
                        sig.status
                    ),
                },
                None => "Authenticode could not be checked on this platform".to_owned(),
            });
        }
        out
    }
}

/// Hash + Authenticode-check the file at `path`.
///
/// `signer` is injected so the pure verdict logic is testable without Windows;
/// [`verify`] wires in the real `WinVerifyTrust` path.
pub fn verify_with(
    path: &Path,
    signature: Option<SignatureInfo>,
    digest: Result<[u8; 32], String>,
) -> Verification {
    let (sha, read_error) = match digest {
        Ok(d) => (Some(sha256::hex_upper(&d)), None),
        Err(err) => (None, Some(err)),
    };
    let sha256_ok = sha
        .as_deref()
        .is_some_and(|got| got.eq_ignore_ascii_case(EXPECTED_SHA256));
    let signature_ok = signature.as_ref().is_some_and(|sig| {
        sig.status == SignatureStatus::Valid
            && sig
                .signer
                .as_deref()
                .is_some_and(|s| s.contains(EXPECTED_SIGNER))
    });
    Verification {
        path: path.to_path_buf(),
        sha256: sha,
        expected_sha256: EXPECTED_SHA256,
        sha256_ok,
        signature,
        expected_signer: EXPECTED_SIGNER,
        signature_ok,
        read_error,
    }
}

/// Live verification against the real file (Windows does the Authenticode part).
pub fn verify(path: &Path) -> Verification {
    let digest = sha256::hash_file(path).map_err(|e| e.to_string());
    #[cfg(windows)]
    let signature = Some(crate::win::signature::verify(&path.display().to_string()));
    #[cfg(not(windows))]
    let signature = None;
    verify_with(path, signature, digest)
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// What `ksx install-drivers` would do, and why.
#[derive(Clone, Debug, Serialize)]
pub struct InstallPlan {
    pub location: Location,
    /// `None` when the installer file was never found.
    pub verification: Option<Verification>,
    /// ViGEmBus as it is installed *right now*.
    pub installed: InstalledState,
    /// `None` when elevation could not be determined.
    pub elevated: Option<bool>,
    pub action: Action,
    /// The exact argv that would be executed, when there is one.
    pub command: Option<Vec<String>>,
}

/// What is already on the machine, distilled from [`crate::collect`].
#[derive(Clone, Debug, Default, Serialize)]
pub struct InstalledState {
    pub vigembus_installed: bool,
    pub vigembus_running: bool,
    pub vigembus_version: Option<String>,
    /// Interception's keyboard class filter — reported, never installed by ksx
    /// (its licence is non-commercial and its installer is not ours to bundle).
    pub interception_installed: bool,
}

impl InstalledState {
    pub fn from_report(report: &DriverReport) -> Self {
        Self {
            vigembus_installed: report.vigembus.installed,
            vigembus_running: matches!(
                report.vigembus.service.as_ref().map(|s| s.state),
                Some(crate::report::ServiceState::Running)
            ),
            vigembus_version: report
                .vigembus
                .driver_file
                .as_ref()
                .and_then(|f| f.file_version.clone()),
            interception_installed: report.interception.installed,
        }
    }
}

/// The single decision this command makes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Action {
    /// Installer is missing — nothing can be verified or run.
    InstallerMissing,
    /// Verification failed. This is a hard stop (exit 2).
    Refuse { reasons: Vec<String> },
    /// ViGEmBus is already installed and healthy; running setup again is a
    /// no-op the user probably did not mean. Repairing is still offered.
    AlreadyInstalled { version: Option<String> },
    /// Verified and ready, but this process is not elevated: a driver install
    /// needs an admin token and the WiX bundle would just bounce off UAC in a
    /// non-interactive session.
    NeedsElevation,
    /// Verified, elevated, ready to execute.
    Ready,
}

impl Action {
    pub fn code(&self) -> &'static str {
        match self {
            Action::InstallerMissing => "installer-missing",
            Action::Refuse { .. } => "verification-failed",
            Action::AlreadyInstalled { .. } => "already-installed",
            Action::NeedsElevation => "needs-elevation",
            Action::Ready => "ready",
        }
    }

    /// Only [`Action::Ready`] may ever execute. `AlreadyInstalled` is executable
    /// too, but only when the caller explicitly asks to repair.
    pub fn is_executable(&self) -> bool {
        matches!(self, Action::Ready)
    }
}

/// Assemble the plan. Pure: every input is passed in, so the whole decision
/// table is testable without a driver, a file, or an elevated token.
pub fn plan(
    location: Location,
    verification: Option<Verification>,
    installed: InstalledState,
    elevated: Option<bool>,
    args: &[String],
    repair: bool,
) -> InstallPlan {
    let action = match (&location.found, &verification) {
        (None, _) => Action::InstallerMissing,
        (Some(_), None) => Action::InstallerMissing,
        (Some(_), Some(v)) if !v.is_trusted() => Action::Refuse {
            reasons: v.failures(),
        },
        // Verification comes first on purpose: "already installed" must never
        // be able to mask a tampered bundle sitting in the release directory.
        (Some(_), Some(_)) if installed.vigembus_installed && !repair => Action::AlreadyInstalled {
            version: installed.vigembus_version.clone(),
        },
        (Some(_), Some(_)) if elevated != Some(true) => Action::NeedsElevation,
        (Some(_), Some(_)) => Action::Ready,
    };

    let command = location.found.as_ref().map(|path| {
        let mut argv = vec![path.display().to_string()];
        argv.extend(args.iter().cloned());
        argv
    });

    InstallPlan {
        location,
        verification,
        installed,
        elevated,
        action,
        command,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl InstallPlan {
    pub fn render_human(&self, dry_run: bool) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();

        let _ = writeln!(out, "installed right now:");
        let _ = writeln!(
            out,
            "  ViGEmBus      {}{}",
            if self.installed.vigembus_installed {
                "installed"
            } else {
                "NOT installed"
            },
            match (
                &self.installed.vigembus_version,
                self.installed.vigembus_running
            ) {
                (Some(v), true) => format!(" (v{v}, service running)"),
                (Some(v), false) => format!(" (v{v}, service NOT running)"),
                (None, _) => String::new(),
            }
        );
        let _ = writeln!(
            out,
            "  Interception  {} (ksx never installs this: LGPL/non-commercial, \
             not ours to bundle)",
            if self.installed.interception_installed {
                "installed"
            } else {
                "not installed"
            }
        );

        let _ = writeln!(out, "\nbundled installer:");
        match (&self.location.found, &self.verification) {
            (None, _) | (_, None) => {
                let _ = writeln!(out, "  [FAIL] not found. Looked in:");
                for candidate in &self.location.searched {
                    let _ = writeln!(out, "           {}", candidate.display());
                }
            }
            (Some(path), Some(v)) => {
                let _ = writeln!(out, "  {}", path.display());
                let _ = writeln!(
                    out,
                    "  sha256        {} {}",
                    if v.sha256_ok { "[OK]  " } else { "[FAIL]" },
                    v.sha256.as_deref().unwrap_or("<unreadable>")
                );
                let _ = writeln!(
                    out,
                    "  authenticode  {} {}",
                    if v.signature_ok { "[OK]  " } else { "[FAIL]" },
                    match &v.signature {
                        Some(sig) => format!(
                            "{:?}, signer {}",
                            sig.status,
                            sig.signer.as_deref().unwrap_or("<unknown>")
                        ),
                        None => "not checked (non-Windows)".to_owned(),
                    }
                );
            }
        }

        let _ = writeln!(out, "\nelevation: {}", describe_elevation(self.elevated));

        let _ = writeln!(out, "\nverdict: {}", self.verdict_line());
        if let Some(command) = &self.command {
            let _ = writeln!(out, "command:  {}", quote_argv(command));
        }
        if dry_run || !self.action.is_executable() {
            let _ = writeln!(
                out,
                "\nnothing was executed{}.",
                if dry_run { " (--dry-run)" } else { "" }
            );
        }
        out
    }

    /// One line that says what happens next — the part a user actually reads.
    pub fn verdict_line(&self) -> String {
        match &self.action {
            Action::InstallerMissing => format!(
                "the bundled installer is missing. Put {INSTALLER_FILE_NAME} in a `drivers` \
                 folder next to ksx.exe (official ViGEmBus v1.22.0 release asset, sha256 \
                 {EXPECTED_SHA256})"
            ),
            Action::Refuse { reasons } => format!(
                "REFUSING to run the installer — it is not the file ksx ships:\n  - {}\n\
                 Delete it and restore the official ViGEmBus 1.22.0 release asset. ksx never \
                 downloads at runtime, so this file is the only thing it can trust.",
                reasons.join("\n  - ")
            ),
            Action::AlreadyInstalled { version } => format!(
                "ViGEmBus {} is already installed; nothing to do. Re-run with --repair to run \
                 setup anyway.",
                version.as_deref().unwrap_or("(version unknown)")
            ),
            Action::NeedsElevation => "verified, but this process is not elevated. A kernel \
                 driver install needs an administrator token: re-run `ksx install-drivers --yes` \
                 from an elevated terminal (right-click Windows Terminal -> Run as \
                 administrator). ksx will not silently self-elevate."
                .to_owned(),
            Action::Ready => {
                "verified and elevated. Re-run with --yes to execute the installer.".to_owned()
            }
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "action": self.action.code(),
            "verdict": self.verdict_line(),
            "installer": {
                "path": self.location.found.as_ref().map(|p| p.display().to_string()),
                "searched": self.location.searched.iter()
                    .map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "expected_sha256": EXPECTED_SHA256,
                "expected_signer": EXPECTED_SIGNER,
                "sha256": self.verification.as_ref().and_then(|v| v.sha256.clone()),
                "sha256_ok": self.verification.as_ref().is_some_and(|v| v.sha256_ok),
                "signature_ok": self.verification.as_ref().is_some_and(|v| v.signature_ok),
                "signer": self.verification.as_ref()
                    .and_then(|v| v.signature.as_ref())
                    .and_then(|s| s.signer.clone()),
                "trusted": self.verification.as_ref().is_some_and(|v| v.is_trusted()),
                "failures": self.verification.as_ref().map(|v| v.failures()).unwrap_or_default(),
            },
            "installed": self.installed,
            "elevated": self.elevated,
            "command": self.command,
        })
    }
}

fn describe_elevation(elevated: Option<bool>) -> &'static str {
    match elevated {
        Some(true) => "elevated (administrator token)",
        Some(false) => "NOT elevated",
        None => "unknown (could not read the process token)",
    }
}

/// Render an argv the way a user could paste it back into a shell.
pub fn quote_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Execute the verified installer and wait for it.
///
/// Callable **only** with a plan whose action [`Action::is_executable`] (or an
/// explicit repair of an already-installed bus): the type system cannot express
/// that, so the check is re-asserted here rather than trusted from the caller.
/// Nothing in the test suite ever reaches this function.
pub fn execute(plan: &InstallPlan) -> Result<std::process::ExitStatus, InstallError> {
    let Some(verification) = &plan.verification else {
        return Err(InstallError::NotVerified);
    };
    if !verification.is_trusted() {
        return Err(InstallError::NotVerified);
    }
    let Some(argv) = &plan.command else {
        return Err(InstallError::NotVerified);
    };
    let (exe, args) = argv.split_first().ok_or(InstallError::NotVerified)?;
    std::process::Command::new(exe)
        .args(args)
        .status()
        .map_err(|err| InstallError::Spawn(err.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("refusing to execute an installer that did not pass hash + signature verification")]
    NotVerified,
    #[error("could not start the installer: {0}")]
    Spawn(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(status: SignatureStatus, signer: &str) -> SignatureInfo {
        SignatureInfo {
            status,
            signer: Some(signer.to_owned()),
            issuer: None,
            not_after_utc: None,
            cert_expired: Some(false),
        }
    }

    fn good_digest() -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = u8::from_str_radix(&EXPECTED_SHA256[i * 2..i * 2 + 1], 16).unwrap();
            let lo = u8::from_str_radix(&EXPECTED_SHA256[i * 2 + 1..i * 2 + 2], 16).unwrap();
            *byte = (hi << 4) | lo;
        }
        out
    }

    fn trusted() -> Verification {
        verify_with(
            Path::new("drivers/x.exe"),
            Some(sig(
                SignatureStatus::Valid,
                "Nefarius Software Solutions e.U.",
            )),
            Ok(good_digest()),
        )
    }

    fn located() -> Location {
        Location {
            found: Some(PathBuf::from("drivers/x.exe")),
            searched: vec![PathBuf::from("drivers/x.exe")],
        }
    }

    fn args() -> Vec<String> {
        DEFAULT_INSTALL_ARGS
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn the_recorded_hash_round_trips() {
        assert_eq!(sha256::hex_upper(&good_digest()), EXPECTED_SHA256);
        assert!(trusted().is_trusted());
        assert!(trusted().failures().is_empty());
    }

    /// Both pins are load-bearing: neither one alone may authorise execution.
    #[test]
    fn a_single_passing_pin_is_never_enough() {
        // Right signer, wrong bytes.
        let tampered = verify_with(
            Path::new("x"),
            Some(sig(
                SignatureStatus::Valid,
                "Nefarius Software Solutions e.U.",
            )),
            Ok([0u8; 32]),
        );
        assert!(!tampered.is_trusted());
        assert!(
            tampered.failures().iter().any(|f| f.contains("SHA-256")),
            "{:?}",
            tampered.failures()
        );

        // Right bytes, wrong signer.
        let impostor = verify_with(
            Path::new("x"),
            Some(sig(SignatureStatus::Valid, "Totally Legit Drivers Ltd")),
            Ok(good_digest()),
        );
        assert!(!impostor.is_trusted());
        assert!(
            impostor
                .failures()
                .iter()
                .any(|f| f.contains("Totally Legit")),
            "{:?}",
            impostor.failures()
        );
    }

    /// A cross-signed-style "valid chain, expired cert" is exactly the state
    /// this project exists to escape — it must not pass for a *new* install.
    #[test]
    fn expired_or_untrusted_signatures_are_refused() {
        for status in [
            SignatureStatus::ValidExpiredCert,
            SignatureStatus::Expired,
            SignatureStatus::Untrusted,
            SignatureStatus::Unsigned,
            SignatureStatus::Unknown,
        ] {
            let v = verify_with(
                Path::new("x"),
                Some(sig(status, "Nefarius Software Solutions e.U.")),
                Ok(good_digest()),
            );
            assert!(!v.is_trusted(), "{status:?} must not be trusted");
        }
    }

    #[test]
    fn unreadable_installer_reports_the_io_error() {
        let v = verify_with(Path::new("x"), None, Err("access denied".into()));
        assert!(!v.is_trusted());
        assert!(v.failures().iter().any(|f| f.contains("access denied")));
    }

    #[test]
    fn verification_failure_refuses_even_when_elevated_and_missing() {
        let bad = verify_with(Path::new("x"), None, Ok([0u8; 32]));
        let plan = plan(
            located(),
            Some(bad),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert!(matches!(plan.action, Action::Refuse { .. }));
        assert!(!plan.action.is_executable());
        assert!(plan.verdict_line().contains("REFUSING"));
    }

    /// Verification outranks "already installed": a tampered bundle sitting in
    /// the release directory must be reported even on a machine that needs no
    /// install at all.
    #[test]
    fn tampered_bundle_is_reported_even_when_vigembus_is_present() {
        let bad = verify_with(Path::new("x"), None, Ok([0u8; 32]));
        let installed = InstalledState {
            vigembus_installed: true,
            vigembus_running: true,
            vigembus_version: Some("1.22.0.0".into()),
            interception_installed: true,
        };
        let plan = plan(located(), Some(bad), installed, Some(true), &args(), false);
        assert!(matches!(plan.action, Action::Refuse { .. }));
    }

    #[test]
    fn already_installed_short_circuits_unless_repairing() {
        let installed = InstalledState {
            vigembus_installed: true,
            vigembus_running: true,
            vigembus_version: Some("1.22.0.0".into()),
            interception_installed: false,
        };
        let p = plan(
            located(),
            Some(trusted()),
            installed.clone(),
            Some(true),
            &args(),
            false,
        );
        assert_eq!(
            p.action,
            Action::AlreadyInstalled {
                version: Some("1.22.0.0".into())
            }
        );
        assert!(!p.action.is_executable());
        assert!(p.verdict_line().contains("--repair"));

        let repairing = plan(
            located(),
            Some(trusted()),
            installed,
            Some(true),
            &args(),
            true,
        );
        assert_eq!(repairing.action, Action::Ready);
    }

    #[test]
    fn a_non_elevated_process_is_told_what_to_do_instead_of_failing_silently() {
        for elevated in [Some(false), None] {
            let p = plan(
                located(),
                Some(trusted()),
                InstalledState::default(),
                elevated,
                &args(),
                false,
            );
            assert_eq!(p.action, Action::NeedsElevation, "{elevated:?}");
            assert!(!p.action.is_executable());
            let line = p.verdict_line();
            assert!(line.contains("administrator"), "{line}");
            assert!(
                line.contains("not silently self-elevate"),
                "elevation policy must be explicit: {line}"
            );
        }
    }

    #[test]
    fn ready_only_when_verified_elevated_and_absent() {
        let p = plan(
            located(),
            Some(trusted()),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert_eq!(p.action, Action::Ready);
        assert!(p.action.is_executable());
        assert_eq!(
            p.command.as_deref(),
            Some(
                [
                    "drivers/x.exe".to_owned(),
                    "/quiet".to_owned(),
                    "/norestart".to_owned()
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn missing_installer_lists_every_place_it_looked() {
        let location = Location {
            found: None,
            searched: vec![
                PathBuf::from("a/drivers/x.exe"),
                PathBuf::from("b/drivers/x.exe"),
            ],
        };
        let p = plan(
            location,
            None,
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert_eq!(p.action, Action::InstallerMissing);
        let text = p.render_human(false);
        assert!(text.contains("a/drivers/x.exe"), "{text}");
        assert!(text.contains("b/drivers/x.exe"), "{text}");
        assert!(p.command.is_none());
    }

    /// `execute` re-checks verification itself: a caller bug must not be able
    /// to run an unverified kernel-driver installer.
    #[test]
    fn execute_refuses_an_unverified_plan_without_spawning_anything() {
        let bad = verify_with(Path::new("x"), None, Ok([0u8; 32]));
        let p = plan(
            located(),
            Some(bad),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert!(matches!(execute(&p), Err(InstallError::NotVerified)));

        let missing = plan(
            Location {
                found: None,
                searched: vec![],
            },
            None,
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert!(matches!(execute(&missing), Err(InstallError::NotVerified)));
    }

    #[test]
    fn locate_prefers_the_directory_next_to_the_exe() {
        let loc = locate(Some(Path::new("C:/app")), Some(Path::new("C:/elsewhere")));
        assert_eq!(
            loc.searched.first().unwrap(),
            &PathBuf::from("C:/app")
                .join("drivers")
                .join(INSTALLER_FILE_NAME)
        );
        assert!(loc.searched.iter().any(|p| p.starts_with("C:/elsewhere")));
    }

    /// Development layout: `target\debug\ksx.exe` must still find the repo's
    /// `drivers\` directory, or `--dry-run` is untestable from a cargo build.
    #[test]
    fn locate_walks_up_out_of_the_cargo_target_directory() {
        let loc = locate(Some(Path::new("C:/repo/target/debug")), None);
        assert!(
            loc.searched.contains(
                &PathBuf::from("C:/repo")
                    .join("drivers")
                    .join(INSTALLER_FILE_NAME)
            ),
            "{:?}",
            loc.searched
        );
    }

    #[test]
    fn json_is_stable_and_carries_the_expected_pins() {
        let p = plan(
            located(),
            Some(trusted()),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        let v = p.to_json();
        assert_eq!(v.pointer("/action"), Some(&serde_json::json!("ready")));
        assert_eq!(
            v.pointer("/installer/expected_sha256"),
            Some(&serde_json::json!(EXPECTED_SHA256))
        );
        assert_eq!(
            v.pointer("/installer/trusted"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(v.pointer("/elevated"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn argv_quoting_survives_program_files() {
        assert_eq!(
            quote_argv(&[
                r"C:\Program Files\ksx\drivers\setup.exe".to_owned(),
                "/quiet".to_owned()
            ]),
            "\"C:\\Program Files\\ksx\\drivers\\setup.exe\" /quiet"
        );
    }
}
