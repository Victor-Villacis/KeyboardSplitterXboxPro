//! `ksx install-drivers` — the one command that runs with an administrator
//! token, and therefore the one with the strictest rules.
//!
//! The decision table, the search-path policy, the two identity pins and the
//! sealed-handle execution all live in `ksx_platform::installer`. This file is
//! the command around them: it reports what is on the machine, seals and
//! verifies the bundle, prints the verdict, and — only with an explicit
//! `--yes` — executes.
//!
//! # What it will not do
//!
//! - **Self-elevate.** A process that quietly relaunches itself as
//!   administrator teaches users to click through UAC prompts they did not ask
//!   for. If elevation is needed, ksx says so and stops.
//! - **Download anything.** Ever. The bundled file is the only artefact, and
//!   its SHA-256 and Authenticode signer are pinned in `docs/DRIVERS.md`.
//! - **Run a file that failed verification**, or tell you how to run it
//!   yourself.
//! - **Install Interception.** Its licence is non-commercial and its installer
//!   is not ours to redistribute; ksx reports its state and points at
//!   `docs/DRIVERS.md`.
//!
//! # Exit codes
//!
//! | 0 | nothing to do, or the installer ran and succeeded |
//! | 1 | unexpected error |
//! | 2 | refused: verification failed, the bundle is missing, or elevation is required |
//! | 3 | the installer ran and returned a non-zero exit code |

use ksx_platform::installer::{self, Action, InstallPlan, SealedFile};

/// Refused — nothing was executed.
pub const EXIT_REFUSED: i32 = 2;
/// The installer ran and failed.
pub const EXIT_INSTALLER_FAILED: i32 = 3;

pub struct Options {
    pub dry_run: bool,
    pub json: bool,
    /// Actually execute. Without it the command is a report, which is what an
    /// unattended caller should be using anyway.
    pub yes: bool,
    /// Run setup again even when ViGEmBus is already installed.
    pub repair: bool,
}

pub fn run(opts: Options) -> anyhow::Result<()> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let cwd = std::env::current_dir().ok();
    let location = installer::locate(exe_dir.as_deref(), cwd.as_deref());

    // Seal before verifying: the handle that proves the identity is the same
    // handle that will be executed from, and it stays open until this function
    // returns. See `ksx_platform::sealed` for the exact guarantee.
    let (mut sealed, verification) = match &location.found {
        Some(path) => match installer::seal_and_verify(path) {
            Ok((sealed, verification)) => (Some(sealed), Some(verification)),
            Err(err) => {
                // Could not even open it with writers locked out — including
                // "somebody else has it open for writing", which is its own
                // reason not to run a driver installer.
                let message = format!(
                    "the bundled installer at {} could not be opened with writers locked \
                     out: {err}",
                    path.display()
                );
                refuse(opts.json, "installer-not-sealable", &message);
            }
        },
        None => (None, None),
    };

    let installed = installer::InstalledState::from_report(&driver_report());
    let elevated = ksx_platform::process::is_elevated();
    let args: Vec<String> = installer::DEFAULT_INSTALL_ARGS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let plan = installer::plan(
        location,
        verification,
        installed,
        elevated,
        &args,
        opts.repair,
    );

    let will_execute = opts.yes && !opts.dry_run && plan.action.is_executable();
    report(&plan, &opts, will_execute)?;

    if !will_execute {
        // Everything that is not "ready and asked for" is a report. Only the
        // states that mean "this cannot work" get a non-zero code.
        let code = match &plan.action {
            Action::Refuse { .. } | Action::InstallerMissing | Action::NeedsElevation => {
                EXIT_REFUSED
            }
            Action::AlreadyInstalled { .. } | Action::Ready => 0,
        };
        if code != 0 {
            std::process::exit(code);
        }
        return Ok(());
    }

    let sealed = sealed
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("internal: a ready plan with no sealed installer"))?;
    execute(&plan, sealed, &args, opts.json)
}

/// Run it. Reached only with `--yes`, an executable verdict, and the file still
/// sealed — and `execute` re-asserts all of that itself.
fn execute(
    plan: &InstallPlan,
    sealed: &mut SealedFile,
    args: &[String],
    json: bool,
) -> anyhow::Result<()> {
    if !json {
        println!("\nrunning the installer (this needs a moment, and may ask to reboot)...");
    }
    let status = installer::execute(plan, sealed, args)?;
    let code = status.code().unwrap_or(-1);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": "executed",
                "exit_code": code,
                "succeeded": status.success(),
            }))?
        );
    } else if status.success() {
        println!("installer finished (exit {code}). Run `ksx doctor` to confirm the bus is up.");
    } else {
        println!(
            "installer FAILED (exit {code}). Run `ksx doctor` for the machine's current state; \
             a WiX bundle logs to %TEMP%\\ViGEmBus*.log."
        );
    }
    if !status.success() {
        std::process::exit(EXIT_INSTALLER_FAILED);
    }
    Ok(())
}

fn report(plan: &InstallPlan, opts: &Options, will_execute: bool) -> anyhow::Result<()> {
    if opts.json {
        let mut value = plan.to_json();
        value["will_execute"] = serde_json::json!(will_execute);
        value["dry_run"] = serde_json::json!(opts.dry_run);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    print!("{}", plan.render_human(opts.dry_run || !will_execute));
    if plan.action.is_executable() && !opts.yes {
        println!("\nRe-run with --yes to execute it. Nothing was run.");
    }
    Ok(())
}

#[cfg(windows)]
fn driver_report() -> ksx_platform::DriverReport {
    ksx_platform::collect()
}

#[cfg(not(windows))]
fn driver_report() -> ksx_platform::DriverReport {
    // Off Windows there is nothing to collect; the plan then reports "not
    // installed" and refuses on the (absent) Authenticode pin anyway.
    ksx_platform::DriverReport::default()
}

fn refuse(json: bool, code: &str, message: &str) -> ! {
    if json {
        println!("{}", crate::pads::error_json(code, message));
    } else {
        eprintln!("error: {message}");
    }
    std::process::exit(EXIT_REFUSED);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exit codes are the contract a provisioning script reads.
    #[test]
    fn refusal_matches_every_other_cannot_start() {
        assert_eq!(EXIT_REFUSED, crate::run::EXIT_CANNOT_START);
        assert_eq!(EXIT_INSTALLER_FAILED, 3);
    }

    fn pinned_digest() -> [u8; 32] {
        let mut digest = [0u8; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            let hex = &installer::EXPECTED_SHA256[i * 2..i * 2 + 2];
            *byte = u8::from_str_radix(hex, 16).unwrap();
        }
        digest
    }

    fn plan_for(signature: ksx_platform::SignatureInfo) -> ksx_platform::installer::InstallPlan {
        use ksx_platform::installer::{plan, InstalledState, Location};
        plan(
            Location {
                found: Some("x".into()),
                searched: vec!["x".into()],
                rejected: Vec::new(),
                restricted: true,
            },
            Some(ksx_platform::installer::verify_with(
                std::path::Path::new("x"),
                Some(signature),
                Ok(pinned_digest()),
            )),
            InstalledState::default(),
            Some(true),
            &["/quiet".to_owned()],
            false,
        )
    }

    /// An expired certificate with **nothing to date the signature** must never
    /// authorise a new install — asserted here as well as in the platform
    /// crate, because this is the layer that decides whether to run anything.
    #[test]
    fn an_undated_expired_cert_never_reaches_an_executable_verdict() {
        use ksx_platform::{SignatureInfo, SignatureStatus};

        let p = plan_for(SignatureInfo {
            status: SignatureStatus::ValidExpiredCert,
            signer: Some("Nefarius Software Solutions e.U.".into()),
            not_after_utc: Some("2025-02-17T00:59:59Z".into()),
            cert_expired: Some(true),
            // No countersignature: nothing shows when this was signed.
            timestamp: None,
            ..SignatureInfo::unknown()
        });
        assert!(matches!(p.action, Action::Refuse { .. }), "{:?}", p.action);
        assert!(!p.action.is_executable());
        assert_eq!(p.command, None);
        assert!(
            p.verdict_line().contains("REFUSING"),
            "{}",
            p.verdict_line()
        );
        assert_eq!(
            p.to_json().pointer("/installer/signature_code"),
            Some(&serde_json::json!("expired-no-valid-timestamp"))
        );
    }

    /// ...and the same certificate **with** a verified timestamp does reach an
    /// executable verdict, because the timestamp proves the file was signed
    /// while the certificate was live. Both halves are asserted at this layer:
    /// the whole point of the policy is that these two cases diverge here.
    #[test]
    fn a_verified_timestamp_makes_the_same_expired_cert_installable() {
        use ksx_platform::report::TimestampInfo;
        use ksx_platform::{SignatureInfo, SignatureStatus};

        let p = plan_for(SignatureInfo {
            status: SignatureStatus::ValidExpiredCert,
            signer: Some("Nefarius Software Solutions e.U.".into()),
            not_before_utc: Some("2023-03-13T00:00:00Z".into()),
            not_after_utc: Some("2025-02-17T00:59:59Z".into()),
            cert_expired: Some(true),
            timestamp: Some(TimestampInfo {
                signed_at_utc: Some("2023-11-02T15:12:03Z".into()),
                signed_at_ticks: 133_434_163_230_000_000,
                authority: Some("DigiCert Timestamp 2023".into()),
                is_timestamp_signer: true,
                chain_ok: true,
                within_signing_cert_validity: true,
                within_timestamp_cert_validity: true,
            }),
            ..SignatureInfo::unknown()
        });
        assert_eq!(p.action, Action::Ready);
        assert!(p.action.is_executable());
        assert_eq!(
            p.to_json().pointer("/installer/signature_code"),
            Some(&serde_json::json!("expired-timestamp-verified"))
        );
        // Accepted, but never silently: the report says which expired
        // certificate was accepted and what dated it.
        let text = p.render_human(true);
        assert!(text.contains("2023-11-02T15:12:03Z"), "{text}");
        assert!(text.contains("(EXPIRED)"), "{text}");
    }

    /// WiX Burn's real unattended switches — getting these wrong means an
    /// unattended install pops a GUI on a cabinet at boot.
    #[test]
    fn the_unattended_switches_are_the_burn_bootstrapper_ones() {
        assert_eq!(installer::DEFAULT_INSTALL_ARGS, &["/quiet", "/norestart"]);
    }
}
