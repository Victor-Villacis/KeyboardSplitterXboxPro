//! `ksx install-drivers` — verify the bundled ViGEmBus installer, then plan.
//!
//! The rule this module exists to enforce: **nothing is executed before its
//! identity is proven, and nothing can change between the proof and the use.**
//! The bundled installer is a kernel-driver setup running elevated; if it were
//! swapped, everything downstream is moot. So the order is always
//!
//! 1. locate the bundled file — in a directory a standard user cannot write,
//! 2. open it **once**, denying writers and deleters ([`crate::sealed`]),
//! 3. SHA-256 it *through that handle* against the value in `docs/DRIVERS.md`,
//! 4. check its Authenticode chain *through that same handle*, and that the
//!    signer really is Nefarius,
//! 5. only then offer to run it — still holding the handle, and targeting the
//!    path the handle itself resolves to.
//!
//! Both pins are required. A hash alone cannot notice that we recorded the hash
//! of a *tampered* file; a signature alone cannot notice that someone swapped
//! ViGEmBus 1.22.0 for a differently-signed Nefarius binary. Two independent
//! pins is the point.
//!
//! # The signature policy, and why an expired certificate can still pass
//!
//! Step 4 is not "the certificate is valid today". Code-signing certificates
//! are issued for a year or two; the binaries they sign outlive them by design,
//! and that is what the timestamp countersignature is *for*. Windows itself
//! resolves this by asking whether the file was signed **while** the
//! certificate was live, and accepts it forever after if so. ksx does the same
//! — see [`SignatureVerdict`] — with one addition: the timestamp is verified
//! rather than assumed. A countersignature nobody checked is decoration.
//!
//! So there are three distinct outcomes for a correctly-signed Nefarius binary,
//! and they are reported as three states with three codes, never as one bool:
//!
//! | state | code | run it? |
//! |---|---|---|
//! | certificate still inside its window | `valid` | yes |
//! | certificate expired, verified timestamp covers the signing time | `expired-timestamp-verified` | yes |
//! | certificate expired, no timestamp survives checking | `expired-no-valid-timestamp` | **no** |
//!
//! Everything else — wrong signer, untrusted chain, unsigned, bad hash — is
//! refused exactly as before.
//!
//! # Threat model (why steps 1 and 2 exist at all)
//!
//! `ksx install-drivers` is the one ksx command that runs with an administrator
//! token. Everything it executes is therefore a privilege boundary, and the two
//! classic ways across it are:
//!
//! - **A user-writable search path.** If the search included, say, a `drivers\`
//!   folder next to a development build under `C:\Users\…`, any standard user
//!   (or any code running as them) could drop a file there and have an admin
//!   run it. [`locate`] refuses candidate directories a standard user can write
//!   **whenever this process is elevated** — which is the only case where the
//!   refusal matters, because a non-elevated process can never reach
//!   [`Action::Ready`] anyway. Net effect: execution is only ever possible from
//!   a protected directory.
//! - **Time-of-check to time-of-use.** Verifying by path and then executing by
//!   path means two `open()`s with a window in between. Closed by
//!   [`crate::sealed::SealedFile`]; the exact guarantee is documented there.
//!
//! Everything except the Authenticode call and the execution step is pure, so
//! the plan, the verdicts and the rendering are unit-tested on any platform
//! with fixtures.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::report::{DriverReport, SignatureInfo, SignatureStatus};
pub use crate::sealed::SealedFile;
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

/// Directory roots a standard user cannot write to on a default Windows
/// installation. Used as a *policy*, not as a security oracle: see
/// [`SearchPolicy::is_protected`].
const PROTECTED_ROOT_VARS: &[&str] = &[
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "SystemRoot",
];

/// Which directories this process is willing to run an installer out of.
#[derive(Clone, Debug)]
pub struct SearchPolicy {
    /// `Some(true)` when this process holds an administrator token. The
    /// restriction only binds when elevated, because that is the only state in
    /// which a user-writable installer could be run *with more privilege than
    /// the person who wrote it*. `None` (unknown) is treated as elevated: when
    /// we cannot tell, assume the dangerous case.
    pub elevated: Option<bool>,
    /// Absolute directory prefixes considered non-user-writable.
    pub protected_roots: Vec<PathBuf>,
}

impl SearchPolicy {
    /// Build the policy from the process environment.
    pub fn from_env(elevated: Option<bool>) -> Self {
        let protected_roots = PROTECTED_ROOT_VARS
            .iter()
            .filter_map(|var| std::env::var(var).ok())
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .collect();
        Self {
            elevated,
            protected_roots,
        }
    }

    /// A policy that trusts nothing — used by tests and by any caller that
    /// wants the strictest reading.
    pub fn with_roots(elevated: Option<bool>, roots: &[&str]) -> Self {
        Self {
            elevated,
            protected_roots: roots.iter().map(PathBuf::from).collect(),
        }
    }

    /// Is the restriction in force? Only when elevated (or unknown).
    pub fn restricted(&self) -> bool {
        self.elevated != Some(false)
    }

    /// Does `dir` sit under a root a standard user cannot write?
    ///
    /// This is a **prefix policy over the well-known protected roots**
    /// (`%ProgramFiles%`, `%ProgramFiles(x86)%`, `%ProgramW6432%`,
    /// `%SystemRoot%`), not a live ACL evaluation. Deliberately:
    ///
    /// - An `AuthzAccessCheck` on the *directory* answers the wrong question
    ///   anyway (what matters is whether anyone can create/replace the file at
    ///   that exact name, including via an inherited ACE on a parent), and a
    ///   half-right ACL check reads as authority it does not have.
    /// - The prefix policy fails *closed*: an administrator who has loosened
    ///   the ACL on `C:\Program Files\ksx` gets no warning from us, but the
    ///   normal case — a build tree under `C:\Users\…`, a download folder, a
    ///   USB stick, `C:\drivers\` — is refused, and that is the case the
    ///   escalation chain actually needs.
    /// - `%ProgramData%` is deliberately **not** protected: its default ACL
    ///   grants `Users` create-file rights, so it is user-writable in exactly
    ///   the way that matters here.
    ///
    /// Documented in `docs/DRIVERS.md` so the guarantee is not folklore.
    pub fn is_protected(&self, dir: &Path) -> bool {
        // `C:\Program Files\..\Users\v\evil` starts with `C:\Program Files`
        // component-wise and is *not* protected. Today's callers pass paths
        // from `current_exe`/`current_dir`, which Windows has already
        // normalised, so this is a latent hole rather than a live one — but
        // this function is `pub`, the prefix test is the whole guarantee, and
        // "fails closed" has to mean it. Anything unnormalised is refused
        // outright instead of being reasoned about.
        if dir
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }
        self.protected_roots
            .iter()
            .any(|root| starts_with_ci(dir, root))
    }
}

/// Case-insensitive, component-wise path prefix test (Windows paths are
/// case-insensitive; `C:\Program Files\x` must match `C:\PROGRAM FILES`).
fn starts_with_ci(path: &Path, prefix: &Path) -> bool {
    let mut p = path.components();
    for want in prefix.components() {
        match p.next() {
            Some(got)
                if got
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&want.as_os_str().to_string_lossy()) => {}
            _ => return false,
        }
    }
    true
}

/// A candidate the search would have tried but refused.
#[derive(Clone, Debug, Serialize)]
pub struct RejectedCandidate {
    pub path: PathBuf,
    pub reason: String,
}

/// Where the installer was looked for, and whether it turned up.
#[derive(Clone, Debug, Serialize)]
pub struct Location {
    pub found: Option<PathBuf>,
    /// Every candidate actually tried, in order — printed when nothing was
    /// found so the user can see exactly where to drop the file.
    pub searched: Vec<PathBuf>,
    /// Candidates the policy refused to even look at, with the reason. Printed
    /// too: "it is right there and ksx ignored it" must never be a mystery.
    pub rejected: Vec<RejectedCandidate>,
    /// Whether the user-writable-directory restriction was in force.
    pub restricted: bool,
}

/// Search for the bundled installer, honouring the process's own elevation.
///
/// `exe_dir` is the directory holding `ksx.exe`; a release lays the file out as
/// `ksx.exe` + `drivers\<INSTALLER_FILE_NAME>`. During development `ksx.exe`
/// lives in `target\debug\`, so the repo-root `drivers\` directory is reached
/// by walking up — but those development locations are under a user-writable
/// tree, so they are only searched when this process is **not** elevated (where
/// the plan can never reach [`Action::Ready`] anyway).
pub fn locate(exe_dir: Option<&Path>, cwd: Option<&Path>) -> Location {
    locate_with(
        &SearchPolicy::from_env(crate::process::is_elevated()),
        exe_dir,
        cwd,
    )
}

/// [`locate`] with the policy injected, so the refusal table is unit-testable
/// without an administrator token or a particular machine layout.
pub fn locate_with(policy: &SearchPolicy, exe_dir: Option<&Path>, cwd: Option<&Path>) -> Location {
    let mut searched: Vec<PathBuf> = Vec::new();
    let mut rejected: Vec<RejectedCandidate> = Vec::new();

    let mut consider = |dir: PathBuf, what: &str| {
        let candidate = dir.join("drivers").join(INSTALLER_FILE_NAME);
        if searched.contains(&candidate) || rejected.iter().any(|r| r.path == candidate) {
            return;
        }
        if policy.restricted() && !policy.is_protected(&dir) {
            rejected.push(RejectedCandidate {
                path: candidate,
                reason: format!(
                    "{what} ({}) is not under a directory a standard user is unable to write, \
                     and this process is elevated: running an installer from there would let \
                     anyone who can write that folder execute code as administrator",
                    dir.display()
                ),
            });
            return;
        }
        searched.push(candidate);
    };

    if let Some(dir) = exe_dir {
        consider(dir.to_path_buf(), "the ksx.exe directory");
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
                consider(up, "a parent of the ksx.exe directory");
            }
        }
    }
    if let Some(dir) = cwd {
        consider(dir.to_path_buf(), "the working directory");
    }

    let found = searched.iter().find(|p| p.is_file()).cloned();
    Location {
        found,
        searched,
        rejected,
        restricted: policy.restricted(),
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// The Authenticode pin's outcome — one variant per state that can actually
/// occur, each with a stable `code` for `--json`.
///
/// This exists instead of a boolean because the three interesting outcomes are
/// genuinely different things to tell a user: "signed and current", "signed
/// years ago and the paperwork proves it", and "signed by someone whose
/// certificate ran out with nothing to show when". Collapsing the middle one
/// into either neighbour is a lie — into `valid` it hides that the certificate
/// is dead, into `refused` it strands a legitimately-signed bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "kebab-case")]
pub enum SignatureVerdict {
    /// Chain verifies, signer matches, certificate still inside its window.
    Valid,
    /// Chain verifies, signer matches, certificate has **expired** — and a
    /// timestamp countersignature that passed every check in
    /// [`crate::report::TimestampInfo`] places the signing time inside the
    /// certificate's validity window. Accepted: this is what Windows does, and
    /// it is the entire purpose of timestamping.
    ExpiredTimestampVerified {
        signed_at_utc: String,
        not_after_utc: Option<String>,
        authority: Option<String>,
    },
    /// Chain verifies, signer matches, certificate has expired, and no
    /// countersignature proves it was signed in time. Refused.
    ExpiredNoValidTimestamp {
        not_after_utc: Option<String>,
        /// Which requirement the countersignature failed, or that there was
        /// none at all.
        problem: String,
    },
    /// The chain verifies, but this is somebody else's binary.
    WrongSigner { signer: Option<String> },
    /// The chain itself does not verify (revoked, tampered, self-signed, …).
    ChainNotTrusted { status: SignatureStatus },
    /// No Authenticode signature at all.
    Unsigned,
    /// Off Windows: there is no Authenticode to ask about, so nothing is ever
    /// trusted here.
    NotChecked,
}

impl SignatureVerdict {
    /// Stable identifier for `--json` consumers and provisioning scripts.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::ExpiredTimestampVerified { .. } => "expired-timestamp-verified",
            Self::ExpiredNoValidTimestamp { .. } => "expired-no-valid-timestamp",
            Self::WrongSigner { .. } => "wrong-signer",
            Self::ChainNotTrusted { .. } => "chain-not-trusted",
            Self::Unsigned => "unsigned",
            Self::NotChecked => "not-checked",
        }
    }

    /// The only two states that may authorise running a kernel-driver installer
    /// with an administrator token.
    pub fn accepted(&self) -> bool {
        matches!(self, Self::Valid | Self::ExpiredTimestampVerified { .. })
    }

    /// One sentence a human can act on — for accepted states too, because
    /// "accepted an expired certificate" is a thing the user is entitled to
    /// read rather than have quietly waved through.
    pub fn explain(&self) -> String {
        match self {
            Self::Valid => {
                "Authenticode verifies and the signing certificate is current".to_owned()
            }
            Self::ExpiredTimestampVerified {
                signed_at_utc,
                not_after_utc,
                authority,
            } => format!(
                "the signing certificate expired{}, but a verified timestamp countersignature{} \
                 places the signing time at {signed_at_utc}, inside the certificate's validity \
                 window. Windows accepts a signature on those terms and so does ksx — that is \
                 what timestamping is for. The certificate being expired *now* is not evidence \
                 of anything.",
                match not_after_utc {
                    Some(when) => format!(" on {when}"),
                    None => String::new(),
                },
                match authority {
                    Some(who) => format!(" from '{who}'"),
                    None => String::new(),
                }
            ),
            Self::ExpiredNoValidTimestamp {
                not_after_utc,
                problem,
            } => format!(
                "Authenticode chains correctly and the signer is right, but the signing \
                 certificate expired{} and {problem}. Without a timestamp there is nothing to \
                 show the file was signed while the certificate was live, so ksx refuses to run \
                 it elevated. See docs/DRIVERS.md ('expired signing certificate').",
                match not_after_utc {
                    Some(when) => format!(" on {when}"),
                    None => String::new(),
                }
            ),
            Self::WrongSigner { signer: Some(got) } => {
                format!("Authenticode is valid but the signer is '{got}', not '{EXPECTED_SIGNER}'")
            }
            Self::WrongSigner { signer: None } => {
                "Authenticode verified but no signer name could be read from the certificate, \
                 so the signer pin cannot be checked"
                    .to_owned()
            }
            Self::ChainNotTrusted { status } => format!(
                "Authenticode is {status:?}; a driver installer must present a chain that verifies"
            ),
            Self::Unsigned => "the file carries no Authenticode signature at all".to_owned(),
            Self::NotChecked => "Authenticode could not be checked on this platform".to_owned(),
        }
    }
}

/// Apply the signature policy to what `WinVerifyTrust` reported. Pure, so every
/// state below is exercised by fixtures on any platform.
///
/// Order matters. The signer pin is checked *before* anything about validity
/// windows: "expired certificate" is only an interesting question once the
/// certificate is the one we expect, and a stranger's currently-valid
/// certificate must never read as a milder problem than Nefarius's expired one.
pub fn judge_signature(signature: Option<&SignatureInfo>) -> SignatureVerdict {
    let Some(sig) = signature else {
        return SignatureVerdict::NotChecked;
    };
    match sig.status {
        SignatureStatus::Unsigned => return SignatureVerdict::Unsigned,
        // `Expired` is WinVerifyTrust's own CERT_E_EXPIRED: Windows looked for a
        // timestamp covering the signing time and did not find an acceptable
        // one. Reported as the expiry state it is, not as a generic untrusted
        // chain — but never accepted, so ksx can never be more permissive than
        // the platform whose behaviour it is matching.
        SignatureStatus::Valid | SignatureStatus::ValidExpiredCert | SignatureStatus::Expired => {}
        status => return SignatureVerdict::ChainNotTrusted { status },
    }

    // Anchored, not a substring search. `contains` on a security boundary
    // accepts "Evil Nefarius Software Solutions Ltd" — the SHA-256 pin makes
    // that unexploitable here, but a signer check that can be satisfied by a
    // name someone else chose is not a check. The trailing tail is allowed
    // because `CertGetNameStringW` returns the CN ("Nefarius Software
    // Solutions e.U.") while the full DN carries `L=Wels, C=AT`.
    let signer_ok = sig.signer.as_deref().is_some_and(|s| {
        let s = s.trim();
        s == EXPECTED_SIGNER
            || s.strip_prefix(EXPECTED_SIGNER)
                .is_some_and(|tail| tail.starts_with([' ', ',']))
    });
    if !signer_ok {
        return SignatureVerdict::WrongSigner {
            signer: sig.signer.clone(),
        };
    }

    if sig.status == SignatureStatus::Valid && sig.cert_expired == Some(false) {
        return SignatureVerdict::Valid;
    }

    // Expired — or a certificate whose window could not be read, which is not
    // the same as a valid one and is treated as the dangerous case. The
    // timestamp is now the only thing that can carry it.
    let not_after_utc = sig.not_after_utc.clone();
    if sig.status == SignatureStatus::Expired {
        return SignatureVerdict::ExpiredNoValidTimestamp {
            not_after_utc,
            problem: "Windows itself rejected the chain with CERT_E_EXPIRED, meaning no \
                      timestamp countersignature it would accept covers the signing time"
                .to_owned(),
        };
    }
    match sig.timestamp.as_ref() {
        Some(ts) if ts.is_verified() => SignatureVerdict::ExpiredTimestampVerified {
            // `is_verified()` guarantees this is `Some`; the fallback keeps the
            // guarantee from being a `.unwrap()` in an elevated code path.
            signed_at_utc: ts
                .signed_at_utc
                .clone()
                .unwrap_or_else(|| ts.signed_at_ticks.to_string()),
            not_after_utc,
            authority: ts.authority.clone(),
        },
        Some(ts) => SignatureVerdict::ExpiredNoValidTimestamp {
            not_after_utc,
            problem: ts
                .problem()
                .unwrap_or("the timestamp countersignature did not verify")
                .to_owned(),
        },
        None => SignatureVerdict::ExpiredNoValidTimestamp {
            not_after_utc,
            problem: "the signature carries no timestamp countersignature at all".to_owned(),
        },
    }
}

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
    /// The policy's reading of `signature` — the thing that actually decides.
    pub signature_verdict: SignatureVerdict,
    /// `signature_verdict.accepted()`, kept as a field so serialised plans stay
    /// readable at a glance. Never computed independently of the verdict.
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
            out.push(self.signature_verdict.explain());
        }
        out
    }

    /// The one-line signature note shown even when the file is accepted — so
    /// "ksx ran an installer whose certificate expired last year" is something
    /// the user was told, not something they find out later.
    pub fn signature_note(&self) -> Option<String> {
        matches!(
            self.signature_verdict,
            SignatureVerdict::ExpiredTimestampVerified { .. }
        )
        .then(|| self.signature_verdict.explain())
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
    let signature_verdict = judge_signature(signature.as_ref());
    Verification {
        path: path.to_path_buf(),
        sha256: sha,
        expected_sha256: EXPECTED_SHA256,
        sha256_ok,
        signature,
        expected_signer: EXPECTED_SIGNER,
        signature_ok: signature_verdict.accepted(),
        signature_verdict,
        read_error,
    }
}

/// Verify a **sealed** file: both pins are read through the one open handle
/// that is holding writers out, and that handle stays alive in the caller's
/// hands afterwards. This is the only verification path `install-drivers` uses.
pub fn verify_sealed(sealed: &mut SealedFile) -> Verification {
    let digest = sealed.digest().map_err(|e| e.to_string());
    let signature = sealed.signature();
    verify_with(sealed.path(), signature, digest)
}

/// Seal and verify in one step. `Err` means the file could not even be opened
/// with writers locked out — including "somebody else has it open for writing",
/// which is itself a refusal, not a retry.
pub fn seal_and_verify(path: &Path) -> Result<(SealedFile, Verification), std::io::Error> {
    let mut sealed = SealedFile::open(path)?;
    let verification = verify_sealed(&mut sealed);
    Ok((sealed, verification))
}

/// Verification by path, used only where no execution can follow (diagnostics).
///
/// Kept deliberately separate from [`verify_sealed`] and **never** wired into
/// the execute path: hashing by path is what created the TOCTOU hole in the
/// first place, so the two live under different names and only one of them can
/// authorise running anything.
pub fn verify_for_report(path: &Path) -> Verification {
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

    // The argv is shown ONLY for a file that passed both pins.
    //
    // A `Refuse` verdict used to print a fully-quoted, pasteable command line
    // for the tampered binary — one copy-paste away from "ksx wouldn't run it,
    // so I ran it myself", from an elevated prompt, which is the exact outcome
    // the refusal exists to prevent. Refusing and then handing over the
    // instructions is not refusing.
    let command = match (&location.found, &verification) {
        (Some(path), Some(v)) if v.is_trusted() => {
            let mut argv = vec![path.display().to_string()];
            argv.extend(args.iter().cloned());
            Some(argv)
        }
        _ => None,
    };

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
                if self.location.searched.is_empty() {
                    let _ = writeln!(out, "           (nothing — every candidate was refused)");
                }
                for refused in &self.location.rejected {
                    let _ = writeln!(out, "  [SKIP] {}", refused.path.display());
                    let _ = writeln!(out, "           {}", refused.reason);
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
                    "  authenticode  {} {}, signer {}",
                    if v.signature_ok { "[OK]  " } else { "[FAIL]" },
                    v.signature_verdict.code(),
                    v.signature
                        .as_ref()
                        .and_then(|s| s.signer.as_deref())
                        .unwrap_or("<unknown>")
                );
                // The certificate window and the timestamp are printed for every
                // signed file, passing or failing. They are the evidence behind
                // the verdict on the line above, and a verdict whose evidence is
                // hidden is a verdict the user has to take on faith.
                if let Some(sig) = &v.signature {
                    if sig.not_after_utc.is_some() {
                        // No `[OK]`/`[FAIL]` marker: a validity window is
                        // evidence, not a verdict. It is indented past the
                        // marker column so its text still lines up with the
                        // lines above and below it.
                        let _ = writeln!(
                            out,
                            "  certificate          valid {} .. {}{}",
                            sig.not_before_utc.as_deref().unwrap_or("?"),
                            sig.not_after_utc.as_deref().unwrap_or("?"),
                            match sig.cert_expired {
                                Some(true) => "  (EXPIRED)",
                                Some(false) => "  (current)",
                                None => "",
                            }
                        );
                    }
                    match &sig.timestamp {
                        Some(ts) => {
                            let _ = writeln!(
                                out,
                                "  timestamp     {} signed {}{}",
                                if ts.is_verified() { "[OK]  " } else { "[FAIL]" },
                                ts.signed_at_utc.as_deref().unwrap_or("<unreadable>"),
                                match &ts.authority {
                                    Some(who) => format!(", countersigned by {who}"),
                                    None => String::new(),
                                }
                            );
                            if let Some(problem) = ts.problem() {
                                let _ = writeln!(out, "                       {problem}");
                            }
                        }
                        None if sig.cert_expired == Some(true) => {
                            let _ = writeln!(out, "  timestamp     [FAIL] none present");
                        }
                        None => {}
                    }
                }
                if let Some(note) = v.signature_note() {
                    let _ = writeln!(out, "\n  note: {note}");
                }
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
                 Delete it and restore the official ViGEmBus 1.22.0 release asset from \
                 https://github.com/nefarius/ViGEmBus/releases (sha256 {EXPECTED_SHA256}). ksx \
                 never downloads at runtime, so this file is the only thing it can trust — and \
                 it will not tell you how to run it anyway: a file that fails both-pin \
                 verification is not one to execute by hand from an elevated prompt.",
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
                "rejected": self.location.rejected.iter().map(|r| serde_json::json!({
                    "path": r.path.display().to_string(),
                    "reason": r.reason,
                })).collect::<Vec<_>>(),
                "search_restricted": self.location.restricted,
                "expected_sha256": EXPECTED_SHA256,
                "expected_signer": EXPECTED_SIGNER,
                "sha256": self.verification.as_ref().and_then(|v| v.sha256.clone()),
                "sha256_ok": self.verification.as_ref().is_some_and(|v| v.sha256_ok),
                "signature_ok": self.verification.as_ref().is_some_and(|v| v.signature_ok),
                // The signature policy's own state, with a stable code:
                // "valid" | "expired-timestamp-verified" |
                // "expired-no-valid-timestamp" | "wrong-signer" |
                // "chain-not-trusted" | "unsigned" | "not-checked".
                "signature_code": self.verification.as_ref()
                    .map(|v| v.signature_verdict.code()),
                "signature_verdict": self.verification.as_ref()
                    .map(|v| serde_json::json!({
                        "accepted": v.signature_verdict.accepted(),
                        "explanation": v.signature_verdict.explain(),
                        "detail": v.signature_verdict,
                    })),
                // The raw evidence, so a script can re-derive the verdict rather
                // than trust it: chain status, certificate window, timestamp and
                // every individual check that was run on it.
                "signature": self.verification.as_ref().and_then(|v| v.signature.clone()),
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

/// Execute the verified installer and wait for it — **from the sealed handle**.
///
/// The caller must still be holding the [`SealedFile`] the plan was built from.
/// That is the whole contract: while it lives, nobody can rewrite or delete
/// those bytes, so what was hashed is what runs.
///
/// Three things are re-asserted here rather than trusted from the caller,
/// because a caller bug must not be able to run an unverified kernel-driver
/// installer with an administrator token:
///
/// 1. the plan's action is executable,
/// 2. the sealed file **re-hashes and re-verifies** to the pinned identity,
///    right now, through the same handle,
/// 3. the process is spawned against [`SealedFile::exec_path`] — the path the
///    open handle resolves to — not the path string the search produced, so a
///    junction swapped under the search path cannot redirect the launch.
///
/// Nothing in the test suite ever reaches the spawn.
pub fn execute(
    plan: &InstallPlan,
    sealed: &mut SealedFile,
    args: &[String],
) -> Result<std::process::ExitStatus, InstallError> {
    if !plan.action.is_executable() {
        return Err(InstallError::NotExecutable(plan.action.code()));
    }
    // Re-verification, not a cached verdict. The plan's `verification` is a
    // report; this is the decision.
    let now = verify_sealed(sealed);
    if !now.is_trusted() {
        return Err(InstallError::NotVerified);
    }
    // ...and it must be the same file the plan was built from.
    match (&plan.location.found, &plan.verification) {
        (Some(planned), Some(before))
            if planned == sealed.path() && before.sha256 == now.sha256 => {}
        _ => return Err(InstallError::NotVerified),
    }

    std::process::Command::new(sealed.exec_path())
        .args(args)
        .status()
        .map_err(|err| InstallError::Spawn(err.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("refusing to execute an installer that did not pass hash + signature verification")]
    NotVerified,
    #[error("refusing to execute: the plan's verdict is '{0}', not 'ready'")]
    NotExecutable(&'static str),
    #[error("the bundled installer could not be opened with writers locked out: {0}")]
    NotSealed(String),
    #[error("could not start the installer: {0}")]
    Spawn(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::report::TimestampInfo;

    const NEFARIUS: &str = "Nefarius Software Solutions e.U.";

    fn sig(status: SignatureStatus, signer: &str) -> SignatureInfo {
        SignatureInfo {
            status,
            signer: Some(signer.to_owned()),
            cert_expired: Some(false),
            ..SignatureInfo::unknown()
        }
    }

    /// The live bundle's shape: chain verifies, signer is Nefarius, certificate
    /// ran out in Feb 2025, RFC-3161 countersignature from Nov 2023.
    /// Ticks/dates mirror what wintrust reports for
    /// `ViGEmBus_1.22.0_x64_x86_arm64.exe` on this machine.
    fn expired_cert_sig(timestamp: Option<TimestampInfo>) -> SignatureInfo {
        SignatureInfo {
            status: SignatureStatus::ValidExpiredCert,
            signer: Some(NEFARIUS.to_owned()),
            issuer: Some("DigiCert Trusted G4 Code Signing RSA4096 SHA384 2021 CA1".to_owned()),
            not_before_utc: Some("2023-03-13T00:00:00Z".into()),
            not_after_utc: Some("2025-02-17T00:59:59Z".into()),
            cert_expired: Some(true),
            timestamp,
        }
    }

    /// A countersignature that passes every check.
    fn good_timestamp() -> TimestampInfo {
        TimestampInfo {
            signed_at_utc: Some("2023-11-02T15:12:03Z".into()),
            signed_at_ticks: 133_434_163_230_000_000,
            authority: Some("DigiCert Timestamp 2023".into()),
            is_timestamp_signer: true,
            chain_ok: true,
            within_signing_cert_validity: true,
            within_timestamp_cert_validity: true,
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
            rejected: Vec::new(),
            restricted: true,
        }
    }

    /// The policy used by the search tests: only `C:\Program Files` and
    /// `C:\Windows` count as protected, so the refusal table is deterministic
    /// on any machine (and on a Linux CI runner).
    fn policy(elevated: Option<bool>) -> SearchPolicy {
        SearchPolicy::with_roots(elevated, &[r"C:\Program Files", r"C:\Windows"])
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

    // -----------------------------------------------------------------------
    // The signature policy: three accepted/refused states, one code each.
    // -----------------------------------------------------------------------

    /// **The decision this policy exists for.** An expired certificate plus a
    /// timestamp that survives every check is accepted, because the timestamp
    /// proves the file was signed while the certificate was live — which is
    /// exactly what Windows concludes and the entire purpose of timestamping.
    #[test]
    fn an_expired_certificate_with_a_verified_timestamp_is_accepted() {
        let v = verify_with(
            Path::new("x"),
            Some(expired_cert_sig(Some(good_timestamp()))),
            Ok(good_digest()),
        );
        assert!(v.is_trusted(), "{:?}", v.signature_verdict);
        assert_eq!(v.signature_verdict.code(), "expired-timestamp-verified");
        assert!(v.failures().is_empty(), "{:?}", v.failures());

        // ...and it says so out loud rather than quietly waving it through.
        let note = v.signature_note().expect("an accepted expiry is announced");
        assert!(note.contains("2023-11-02T15:12:03Z"), "{note}");
        assert!(note.contains("2025-02-17T00:59:59Z"), "{note}");
        assert!(note.contains("DigiCert Timestamp 2023"), "{note}");

        let p = plan(
            located(),
            Some(v),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert_eq!(p.action, Action::Ready);
    }

    /// The other half of the same decision: **absent** a timestamp, an expired
    /// certificate is still refused. The relaxation is "a proof was presented
    /// and checked", never "expiry is fine".
    #[test]
    fn an_expired_certificate_without_a_timestamp_is_refused() {
        let v = verify_with(
            Path::new("x"),
            Some(expired_cert_sig(None)),
            Ok(good_digest()),
        );
        assert!(!v.is_trusted());
        assert_eq!(v.signature_verdict.code(), "expired-no-valid-timestamp");
        assert!(v.signature_note().is_none());
        let failure = v.failures().join(" ");
        assert!(
            failure.contains("no timestamp countersignature"),
            "{failure}"
        );
        assert!(failure.contains("2025-02-17"), "{failure}");
        assert!(failure.contains("docs/DRIVERS.md"), "{failure}");
        assert!(
            !failure.contains("SHA-256"),
            "the hash pinned fine; do not muddy the message: {failure}"
        );
    }

    /// **The check is a check, not a courtesy.** Each way a countersignature
    /// can fail to prove what it claims is refused on its own, and the refusal
    /// names that specific failure — otherwise "we verify the timestamp" is a
    /// sentence in a doc rather than a property of the code.
    #[test]
    fn every_broken_timestamp_is_refused_and_named() {
        let cases: [(TimestampInfo, &str); 5] = [
            (
                // Some other unauthenticated countersignature wearing the hat.
                TimestampInfo {
                    is_timestamp_signer: false,
                    ..good_timestamp()
                },
                "not a timestamp countersignature",
            ),
            (
                // A timestamping authority Windows does not trust.
                TimestampInfo {
                    chain_ok: false,
                    ..good_timestamp()
                },
                "own certificate chain did not verify",
            ),
            (
                // Present, well-formed, and says nothing.
                TimestampInfo {
                    signed_at_utc: None,
                    signed_at_ticks: 0,
                    ..good_timestamp()
                },
                "no readable signing time",
            ),
            (
                // A TSA vouching for an instant outside its own certificate's
                // life is vouching for nothing.
                TimestampInfo {
                    within_timestamp_cert_validity: false,
                    ..good_timestamp()
                },
                "was not valid at the instant",
            ),
            (
                // The one that matters most: a real, trusted timestamp whose
                // instant is simply not inside the signing certificate's
                // window. Trusting this would be trusting an unchecked claim.
                TimestampInfo {
                    within_signing_cert_validity: false,
                    ..good_timestamp()
                },
                "outside the signing certificate's validity window",
            ),
        ];
        for (timestamp, expected) in cases {
            let v = verify_with(
                Path::new("x"),
                Some(expired_cert_sig(Some(timestamp.clone()))),
                Ok(good_digest()),
            );
            assert!(!v.is_trusted(), "{timestamp:?} must not be trusted");
            assert_eq!(
                v.signature_verdict.code(),
                "expired-no-valid-timestamp",
                "{timestamp:?}"
            );
            let failure = v.failures().join(" ");
            assert!(failure.contains(expected), "{failure}");
        }
    }

    /// A timestamp cannot rescue a signature Windows itself refused. If
    /// `WinVerifyTrust` returned `CERT_E_EXPIRED` it already looked for an
    /// acceptable countersignature and did not find one; accepting on our own
    /// evidence there would make ksx *more* permissive than the platform whose
    /// behaviour it is matching.
    #[test]
    fn a_timestamp_cannot_override_win_verify_trusts_own_expiry_refusal() {
        let mut signature = expired_cert_sig(Some(good_timestamp()));
        signature.status = SignatureStatus::Expired;
        let v = verify_with(Path::new("x"), Some(signature), Ok(good_digest()));
        assert!(!v.is_trusted());
        assert_eq!(v.signature_verdict.code(), "expired-no-valid-timestamp");
        assert!(v.failures().join(" ").contains("CERT_E_EXPIRED"));
    }

    /// Everything the relaxation does **not** touch. A broken chain, a stranger,
    /// or no signature at all is refused whatever timestamp it carries.
    #[test]
    fn a_verified_timestamp_never_rescues_anything_else() {
        for status in [
            SignatureStatus::Untrusted,
            SignatureStatus::Unsigned,
            SignatureStatus::Unknown,
        ] {
            let mut signature = expired_cert_sig(Some(good_timestamp()));
            signature.status = status;
            let v = verify_with(Path::new("x"), Some(signature), Ok(good_digest()));
            assert!(!v.is_trusted(), "{status:?} must not be trusted");
            assert!(
                matches!(
                    v.signature_verdict,
                    SignatureVerdict::ChainNotTrusted { .. } | SignatureVerdict::Unsigned
                ),
                "{status:?} → {:?}",
                v.signature_verdict
            );
        }

        // A stranger's binary, perfectly timestamped, right bytes claimed.
        let mut impostor = expired_cert_sig(Some(good_timestamp()));
        impostor.signer = Some("Totally Legit Drivers Ltd".into());
        let v = verify_with(Path::new("x"), Some(impostor), Ok(good_digest()));
        assert!(!v.is_trusted());
        assert_eq!(v.signature_verdict.code(), "wrong-signer");

        // ...and the timestamp never touches the hash pin.
        let v = verify_with(
            Path::new("x"),
            Some(expired_cert_sig(Some(good_timestamp()))),
            Ok([0u8; 32]),
        );
        assert!(v.signature_ok, "the signature really is fine here");
        assert!(!v.is_trusted(), "but the bytes are not the bytes we ship");
        assert!(v.failures().iter().any(|f| f.contains("SHA-256")));
    }

    /// A certificate whose window could not be read is not a valid one. The
    /// unknown case falls to the timestamp path and is refused there, rather
    /// than being optimistically treated as current.
    #[test]
    fn an_unreadable_certificate_window_fails_closed() {
        let signature = SignatureInfo {
            status: SignatureStatus::Valid,
            signer: Some(NEFARIUS.to_owned()),
            cert_expired: None,
            ..SignatureInfo::unknown()
        };
        let v = verify_with(Path::new("x"), Some(signature), Ok(good_digest()));
        assert!(!v.is_trusted());
        assert_eq!(v.signature_verdict.code(), "expired-no-valid-timestamp");
    }

    /// The three states a user can hit with a genuine Nefarius bundle must be
    /// three distinguishable codes in `--json`, never one boolean.
    #[test]
    fn the_three_certificate_states_have_distinct_json_codes() {
        let states = [
            (
                verify_with(
                    Path::new("x"),
                    Some(sig(SignatureStatus::Valid, NEFARIUS)),
                    Ok(good_digest()),
                ),
                "valid",
                true,
            ),
            (
                verify_with(
                    Path::new("x"),
                    Some(expired_cert_sig(Some(good_timestamp()))),
                    Ok(good_digest()),
                ),
                "expired-timestamp-verified",
                true,
            ),
            (
                verify_with(
                    Path::new("x"),
                    Some(expired_cert_sig(None)),
                    Ok(good_digest()),
                ),
                "expired-no-valid-timestamp",
                false,
            ),
        ];
        let mut seen: Vec<&str> = Vec::new();
        for (verification, code, accepted) in states {
            assert_eq!(verification.signature_ok, accepted, "{code}");
            let p = plan(
                located(),
                Some(verification),
                InstalledState::default(),
                Some(true),
                &args(),
                false,
            );
            let json = p.to_json();
            assert_eq!(
                json.pointer("/installer/signature_code"),
                Some(&serde_json::json!(code))
            );
            assert_eq!(
                json.pointer("/installer/signature_verdict/accepted"),
                Some(&serde_json::json!(accepted)),
                "{code}"
            );
            assert_eq!(
                json.pointer("/installer/signature_verdict/detail/code"),
                Some(&serde_json::json!(code)),
                "the tagged detail must agree with the flat code"
            );
            assert_eq!(
                json.pointer("/action").unwrap().as_str().unwrap() == "ready",
                accepted
            );
            seen.push(code);
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            3,
            "the three states must not collapse: {seen:?}"
        );
    }

    /// The accepted-with-timestamp case prints its evidence: a user must be
    /// able to see *which* expired certificate was accepted and *what* dated it.
    #[test]
    fn the_human_report_shows_the_certificate_window_and_the_timestamp() {
        let p = plan(
            located(),
            Some(verify_with(
                Path::new("drivers/x.exe"),
                Some(expired_cert_sig(Some(good_timestamp()))),
                Ok(good_digest()),
            )),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        let text = p.render_human(true);
        assert!(text.contains("authenticode  [OK]"), "{text}");
        assert!(text.contains("expired-timestamp-verified"), "{text}");
        assert!(text.contains("(EXPIRED)"), "{text}");
        assert!(text.contains("2023-03-13T00:00:00Z"), "{text}");
        assert!(text.contains("timestamp     [OK]"), "{text}");
        assert!(text.contains("2023-11-02T15:12:03Z"), "{text}");
        assert!(text.contains("DigiCert Timestamp 2023"), "{text}");
        assert!(text.contains("note:"), "{text}");

        // Four evidence lines, one text column. Hand-padded label widths drift
        // the moment somebody adds a fifth, and a misaligned report is the
        // first thing a reader stops trusting.
        let column = |needle: &str| {
            text.find(needle)
                .map(|at| at - text[..at].rfind('\n').unwrap())
        };
        assert_eq!(column("89220A"), column("expired-timestamp-verified"));
        assert_eq!(column("89220A"), column("valid 2023-03-13T00:00:00Z"));
        assert_eq!(column("89220A"), column("signed 2023-11-02T15:12:03Z"));

        // ...and the refusing case shows the missing evidence just as plainly.
        let refused = plan(
            located(),
            Some(verify_with(
                Path::new("drivers/x.exe"),
                Some(expired_cert_sig(None)),
                Ok(good_digest()),
            )),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        let text = refused.render_human(true);
        assert!(text.contains("authenticode  [FAIL]"), "{text}");
        assert!(text.contains("expired-no-valid-timestamp"), "{text}");
        assert!(text.contains("timestamp     [FAIL] none present"), "{text}");
        assert!(!text.contains("note:"), "{text}");
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
            rejected: Vec::new(),
            restricted: true,
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
    /// to run an unverified kernel-driver installer. Nothing here spawns —
    /// every case is refused before `Command` is built.
    #[test]
    fn execute_refuses_an_unverified_plan_without_spawning_anything() {
        let scratch = std::env::temp_dir().join(format!("ksx-exec-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let file = scratch.join("not-the-installer.exe");
        std::fs::write(&file, b"definitely not ViGEmBus").unwrap();
        let mut sealed = SealedFile::open(&file).unwrap();

        // A plan whose *own* verdict is Refuse.
        let bad = verify_with(&file, None, Ok([0u8; 32]));
        let refused = plan(
            located(),
            Some(bad),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert!(matches!(
            execute(&refused, &mut sealed, &args()),
            Err(InstallError::NotExecutable("verification-failed"))
        ));

        // ...and a plan that *claims* Ready over a file that is not the pinned
        // one: re-verification through the handle is what stops it, not the
        // cached verdict.
        let lying = InstallPlan {
            action: Action::Ready,
            ..plan(
                located(),
                Some(trusted()),
                InstalledState::default(),
                Some(true),
                &args(),
                false,
            )
        };
        assert!(matches!(
            execute(&lying, &mut sealed, &args()),
            Err(InstallError::NotVerified)
        ));

        drop(sealed);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn locate_prefers_the_directory_next_to_the_exe() {
        let loc = locate_with(
            &policy(Some(false)),
            Some(Path::new("C:/app")),
            Some(Path::new("C:/elsewhere")),
        );
        assert_eq!(
            loc.searched.first().unwrap(),
            &PathBuf::from("C:/app")
                .join("drivers")
                .join(INSTALLER_FILE_NAME)
        );
        assert!(loc.searched.iter().any(|p| p.starts_with("C:/elsewhere")));
    }

    /// Development layout: `target\debug\ksx.exe` must still find the repo's
    /// `drivers\` directory, or `--dry-run` is untestable from a cargo build —
    /// but only when the process is NOT elevated.
    #[test]
    fn locate_walks_up_out_of_the_cargo_target_directory_when_unelevated() {
        let loc = locate_with(
            &policy(Some(false)),
            Some(Path::new("C:/repo/target/debug")),
            None,
        );
        assert!(
            loc.searched.contains(
                &PathBuf::from("C:/repo")
                    .join("drivers")
                    .join(INSTALLER_FILE_NAME)
            ),
            "{:?}",
            loc.searched
        );
        assert!(loc.rejected.is_empty(), "{:?}", loc.rejected);
        assert!(!loc.restricted);
    }

    /// **The privilege-escalation fix.** Elevated, a user-writable directory is
    /// not a place an installer may come from: anyone who can write there would
    /// get their bytes executed with an administrator token.
    #[test]
    fn an_elevated_process_refuses_user_writable_search_directories() {
        for elevated in [Some(true), None] {
            let loc = locate_with(
                &policy(elevated),
                Some(Path::new(r"C:\Users\victor\ksx\target\debug")),
                Some(Path::new(r"C:\drivers")),
            );
            assert!(
                loc.searched.is_empty(),
                "elevated={elevated:?} searched user-writable paths: {:?}",
                loc.searched
            );
            assert!(loc.found.is_none());
            assert!(
                loc.rejected.iter().any(|r| r.reason.contains("elevated")),
                "the refusal must say why: {:?}",
                loc.rejected
            );
            // C:\drivers is the archetype: writable by anyone, next to nothing.
            assert!(
                loc.rejected
                    .iter()
                    .any(|r| r.path.starts_with(r"C:\drivers")),
                "{:?}",
                loc.rejected
            );
        }
    }

    /// The installed layout still works elevated — that is the whole point of
    /// restricting rather than forbidding.
    #[test]
    fn an_elevated_process_still_searches_program_files() {
        let loc = locate_with(
            &policy(Some(true)),
            Some(Path::new(r"C:\Program Files\ksx")),
            Some(Path::new(r"C:\Users\victor")),
        );
        assert_eq!(
            loc.searched.first(),
            Some(
                &PathBuf::from(r"C:\Program Files\ksx")
                    .join("drivers")
                    .join(INSTALLER_FILE_NAME)
            ),
            "the exe's own directory is searched first: {loc:?}"
        );
        assert!(
            loc.searched.iter().all(
                |p| policy(Some(true)).is_protected(p.parent().and_then(Path::parent).unwrap())
            ),
            "every searched candidate must sit under a protected root: {:?}",
            loc.searched
        );
        // `C:\` (a parent of the exe dir) and the user's home (the cwd) are both
        // user-writable, so both are refused.
        assert!(
            loc.rejected.iter().any(|r| r.path
                == PathBuf::from(r"C:\")
                    .join("drivers")
                    .join(INSTALLER_FILE_NAME)),
            "{:?}",
            loc.rejected
        );
        assert!(
            loc.rejected
                .iter()
                .any(|r| r.path.starts_with(r"C:\Users\victor")),
            "{:?}",
            loc.rejected
        );
    }

    #[test]
    fn protected_root_matching_is_case_insensitive_and_component_wise() {
        let p = policy(Some(true));
        assert!(p.is_protected(Path::new(r"C:\PROGRAM FILES\ksx")));
        assert!(p.is_protected(Path::new(r"c:\program files\ksx\drivers")));
        assert!(!p.is_protected(Path::new(r"C:\Program Files Evil\ksx")));
        assert!(!p.is_protected(Path::new(r"C:\Users\victor\Program Files")));
        assert!(!p.is_protected(Path::new(r"C:\ProgramData\ksx")));
    }

    /// A component-wise prefix test says `C:\Program Files\..\Users\v\evil`
    /// starts with `C:\Program Files`. It does, and it is still a folder the
    /// user owns — so an unnormalised path must be refused, not matched.
    #[test]
    fn a_parent_dir_traversal_out_of_a_protected_root_is_not_protected() {
        let p = policy(Some(true));
        assert!(
            !p.is_protected(Path::new(r"C:\Program Files\..\Users\victor\evil")),
            "traversal out of a protected root must not count as protected"
        );
        assert!(!p.is_protected(Path::new(r"C:\Program Files\ksx\..\..\Users\victor")));
        // The same walk without traversal is still fine.
        assert!(p.is_protected(Path::new(r"C:\Program Files\ksx\drivers")));
    }

    /// A `Refuse` verdict must not hand the user a ready-to-paste command line
    /// for the tampered binary. Refusing and then printing the instructions is
    /// not refusing.
    #[test]
    fn a_refused_plan_never_prints_a_runnable_command() {
        let bad = verify_with(Path::new("drivers/x.exe"), None, Ok([0u8; 32]));
        let p = plan(
            located(),
            Some(bad),
            InstalledState::default(),
            Some(true),
            &args(),
            false,
        );
        assert!(matches!(p.action, Action::Refuse { .. }));
        assert_eq!(
            p.command, None,
            "no argv may be offered for a tampered file"
        );
        let text = p.render_human(false);
        assert!(!text.contains("command:"), "{text}");
        assert!(!text.contains("/quiet"), "{text}");
        assert!(text.contains("REFUSING"), "{text}");
        assert_eq!(
            p.to_json().pointer("/command"),
            Some(&serde_json::Value::Null),
            "--json must not carry it either"
        );
    }

    /// End-to-end over a real file: seal it, verify it, and prove the bytes
    /// cannot move under us while the seal is held.
    #[test]
    fn sealing_a_real_file_reports_the_mismatch_and_locks_it() {
        let scratch = std::env::temp_dir().join(format!("ksx-seal-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let file = scratch.join("bundle.exe");
        std::fs::write(&file, b"not ViGEmBus").unwrap();

        let (sealed, verification) = seal_and_verify(&file).unwrap();
        assert!(!verification.is_trusted());
        assert!(verification
            .failures()
            .iter()
            .any(|f| f.contains("SHA-256")));
        #[cfg(windows)]
        assert!(
            std::fs::write(&file, b"swapped!").is_err(),
            "the seal must outlive verification — that is the TOCTOU fix"
        );
        drop(sealed);
        let _ = std::fs::remove_file(&file);
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
