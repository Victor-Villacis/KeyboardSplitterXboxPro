//! Availability probe, and the driver implementation for a machine that does
//! not have HIDMaestro.
//!
//! **Status on Victor's cabinet as of 2026-08-06: NOT INSTALLED.** Probed live:
//! no `HIDMaestro` service, no driver package in `pnputil /enum-drivers`, no
//! `ROOT\HIDMAESTRO` device node, nothing under `Program Files`. The only
//! artifact anywhere on the machine is a bundled managed assembly inside an
//! unrelated clone (`HIDMaestro.Core.dll` 1.3.22, in `controller-project`),
//! which is a .NET library, not an installed driver.
//!
//! So this module contains no fake. [`UnavailableDriver`] answers the sweep
//! honestly (nothing to sweep, because nothing can exist) and refuses at
//! `install_driver` with the probe evidence attached. Everything upstream of it
//! — the seqlock, the lifecycle order, the axis routing, the keepalive, the
//! decode table — is real code that will run unchanged the day a real driver
//! implementation is dropped in behind the same trait.
//!
//! ## What a real implementation still needs
//!
//! The one thing the protocol map does **not** contain is the byte layout and
//! name of HIDMaestro's shared section — the audit documents the *discipline*
//! (seqlocked latch, consumer-driven cadence) and the *field set*, because
//! PadForge consumes the SDK in-process as a managed library and never sees the
//! section directly. Two routes, both open:
//!
//! 1. Read `HIDMaestro`'s own MIT sources (`driver.c` / `companion.c` are cited
//!    by name in PadForge's comments) and implement the section natively.
//! 2. Host the MIT `HIDMaestro.Core.dll` and call it — costs a CLR in-process.
//!
//! Route 1 is the right one for ksx (no CLR in a 1 kHz daemon), and it is a
//! *transcription* job on top of what is already here, not a redesign.

use crate::error::{HmError, ProbeSummary};

/// Where a real install puts things. Checked in order; any hit is reported so a
/// partial install is distinguishable from no install.
///
/// `ksx-platform`'s doctor collector probes the same targets through its own
/// registry helpers (it deliberately depends on nothing); if this list changes,
/// change `ksx-platform/src/win/mod.rs::hidmaestro` with it.
pub const PROBE_TARGETS: &[&str] = &[
    r"HKLM\SYSTEM\CurrentControlSet\Services\HIDMaestro",
    r"%SystemRoot%\System32\drivers\UMDF\HIDMaestro.dll",
    r"%ProgramFiles%\HIDMaestro",
];

/// Is HIDMaestro installed?
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    pub installed: bool,
    pub probe: ProbeSummary,
}

impl Availability {
    /// Turns a probe into the error the CLI shows, or `Ok` if installed.
    pub fn require(&self) -> Result<(), HmError> {
        if self.installed {
            Ok(())
        } else {
            Err(HmError::NotInstalled {
                probe: self.probe.clone(),
            })
        }
    }
}

/// Decides installed-vs-not from raw probe hits.
///
/// Pure, so the *policy* ("a service key alone is not an install") is testable
/// without touching a registry. The policy: HIDMaestro counts as installed only
/// when the UMDF driver binary is actually on disk. A leftover service key from
/// an uninstall would otherwise let ksx promise personas it cannot deliver, and
/// fail later at create time instead of at config-validation time.
pub fn evaluate(hits: &[bool]) -> Availability {
    assert_eq!(hits.len(), PROBE_TARGETS.len(), "one hit per probe target");
    let found: Vec<String> = PROBE_TARGETS
        .iter()
        .zip(hits)
        .filter(|(_, hit)| **hit)
        .map(|(t, _)| (*t).to_string())
        .collect();
    // Index 1 is the UMDF driver binary — the load-bearing artifact.
    let installed = hits[1];
    Availability {
        installed,
        probe: ProbeSummary {
            looked_for: PROBE_TARGETS.iter().map(|t| (*t).to_string()).collect(),
            found,
        },
    }
}

/// Probes the live machine.
#[cfg(windows)]
pub fn probe() -> Availability {
    fn expand(path: &str) -> std::path::PathBuf {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string());
        std::path::PathBuf::from(
            path.replace("%SystemRoot%", &system_root)
                .replace("%ProgramFiles%", &program_files),
        )
    }
    // The service key is a registry path; checking it needs no registry API
    // here — its presence is reported by `ksx doctor` (ksx-platform already
    // owns registry access). This crate's own decision rests on the driver
    // binary, so a missing registry answer cannot change the verdict.
    let hits = [
        false,
        expand(PROBE_TARGETS[1]).is_file(),
        expand(PROBE_TARGETS[2]).is_dir(),
    ];
    evaluate(&hits)
}

#[cfg(not(windows))]
pub fn probe() -> Availability {
    // HIDMaestro is a Windows UMDF2 driver; there is nothing to find elsewhere.
    evaluate(&[false, false, false])
}

/// The [`crate::context::HmDriverApi`] implementation for a machine without
/// HIDMaestro.
///
/// Deliberately **not** a mock or a stub: it does not pretend to create
/// devices, and it does not silently succeed. It refuses at the first step that
/// would require the driver to exist, with the probe evidence attached.
pub struct UnavailableDriver {
    availability: Availability,
}

impl UnavailableDriver {
    pub fn new() -> Self {
        Self {
            availability: probe(),
        }
    }

    pub fn availability(&self) -> &Availability {
        &self.availability
    }
}

impl Default for UnavailableDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::context::HmDriverApi for UnavailableDriver {
    /// Never produced — `create_controller` always refuses — but the trait
    /// needs a type, and naming the heap one keeps this file free of Windows
    /// types it does not use.
    type Storage = crate::seqlock::HeapStorage;

    fn remove_all_virtual_controllers(&mut self) -> Result<usize, HmError> {
        // Truthful: with no driver installed, no virtual controllers can exist,
        // so a sweep really did remove zero. This is the one step that is
        // meaningfully answerable without the driver, and answering it keeps
        // the lifecycle order under test in production too.
        Ok(0)
    }

    fn load_default_profiles(&mut self) -> Result<usize, HmError> {
        self.availability.require()?;
        Err(HmError::NotInstalled {
            probe: self.availability.probe.clone(),
        })
    }

    fn install_driver(&mut self) -> Result<(), HmError> {
        self.availability.require()?;
        Err(HmError::NotInstalled {
            probe: self.availability.probe.clone(),
        })
    }

    fn publish_pid_pool(
        &mut self,
        _slot: crate::context::SlotId,
        _profile: &crate::profile::HmProfile,
    ) -> Result<(), HmError> {
        Err(HmError::NotInstalled {
            probe: self.availability.probe.clone(),
        })
    }

    fn create_controller(
        &mut self,
        _slot: crate::context::SlotId,
        _profile: &crate::profile::HmProfile,
    ) -> Result<crate::seqlock::Latch<Self::Storage>, HmError> {
        Err(HmError::NotInstalled {
            probe: self.availability.probe.clone(),
        })
    }

    fn park_feedback_index(
        &mut self,
        _slot: crate::context::SlotId,
        _index: i32,
    ) -> Result<(), HmError> {
        Ok(())
    }

    fn dispose_dispatchers(&mut self, _slot: crate::context::SlotId) -> Result<(), HmError> {
        Ok(())
    }

    fn destroy_controller(&mut self, _slot: crate::context::SlotId) -> Result<(), HmError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{HmContext, HmDriverApi};

    #[test]
    fn a_leftover_service_key_alone_is_not_an_install() {
        let a = evaluate(&[true, false, false]);
        assert!(!a.installed);
        let msg = a.probe.to_string();
        assert!(msg.contains("partial install"), "{msg}");
        assert!(a.require().is_err());
    }

    #[test]
    fn the_umdf_driver_binary_is_what_counts() {
        let a = evaluate(&[false, true, false]);
        assert!(a.installed);
        assert!(a.require().is_ok());
    }

    #[test]
    fn a_clean_machine_reports_what_it_looked_for() {
        let a = evaluate(&[false, false, false]);
        assert!(!a.installed);
        assert!(a.probe.found.is_empty());
        assert_eq!(a.probe.looked_for.len(), PROBE_TARGETS.len());
        let msg = a.require().unwrap_err().to_string();
        for target in PROBE_TARGETS {
            assert!(msg.contains(target), "{msg} should name {target}");
        }
    }

    /// The honesty contract: on a machine without HIDMaestro, nothing pretends.
    #[test]
    fn the_unavailable_driver_refuses_instead_of_faking_a_device() {
        let mut driver = UnavailableDriver::new();
        if driver.availability().installed {
            // Someone installed it — this test has nothing to say.
            return;
        }
        // The sweep is answerable and answered.
        assert_eq!(driver.remove_all_virtual_controllers().unwrap(), 0);
        // Everything that would need a driver refuses, and says so.
        let err = driver.install_driver().unwrap_err();
        assert!(err.is_not_installed(), "{err}");
        assert!(driver.load_default_profiles().is_err());

        // ...and through the context, `start` fails with the same error rather
        // than leaving a half-started session.
        let mut ctx = HmContext::new(UnavailableDriver::new());
        let err = ctx.start().unwrap_err();
        assert!(err.is_not_installed(), "{err}");
        assert!(ctx.live_slots().is_empty());
    }
}
