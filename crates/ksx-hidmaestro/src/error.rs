//! Error taxonomy for the HIDMaestro client.

use std::fmt;

/// What can go wrong talking to HIDMaestro.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HmError {
    /// HIDMaestro is not installed on this machine.
    ///
    /// The **only** error `ksx doctor` and the CLI need to special-case: every
    /// other variant means "installed but misbehaving", which is a bug report;
    /// this one means "install it", which is an action.
    #[error(
        "HIDMaestro is not installed: {probe}. \
         The DualSense / Switch Pro / Xbox Series personas require it \
         (https://github.com/hifihedgehog/HIDMaestro, MIT). \
         Xbox 360 and PlayStation personas do not — they run on ViGEmBus."
    )]
    NotInstalled { probe: ProbeSummary },

    /// ksx itself cannot create a HIDMaestro device — on any machine.
    ///
    /// The variant that exists so [`HmError::NotInstalled`] stops being said
    /// when it is not the reason. Everything above HIDMaestro's shared section
    /// is written and tested (seqlock, lifecycle order, axis routing,
    /// keepalive, decode table); the section's own byte layout has never been
    /// transcribed from HIDMaestro's sources, so [`crate::HmDriverApi`] has no
    /// production implementation that can create anything. Reporting that as
    /// "not installed" costs the user a driver install and leaves them exactly
    /// where they started.
    #[error(
        "creating a HIDMaestro device is not implemented in this build of ksx: {detail}. \
         The DualSense / Switch Pro / Xbox Series personas cannot plug on any machine, \
         installed or not. Xbox 360 and PlayStation are unaffected: they run on ViGEmBus."
    )]
    NotImplemented { detail: &'static str },

    /// The driver is installed but `InstallDriver` needs elevation we do not
    /// have. PadForge sidesteps this by running elevated always; ksx does not,
    /// so the error has to be nameable.
    #[error("HIDMaestro driver installation requires elevation (run ksx as administrator)")]
    ElevationRequired,

    /// The named shared section exists but is smaller than the latch layout
    /// needs — an SDK/driver version mismatch, not a transient failure.
    #[error("HIDMaestro shared section is {actual} bytes; the latch needs at least {needed}")]
    SectionTooSmall { actual: usize, needed: usize },

    /// A seqlock read never caught a stable snapshot. Means the writer is
    /// wedged mid-update (or the section is being torn down), never that the
    /// reader was merely unlucky — the retry budget is generous.
    #[error("seqlock read did not stabilize after {retries} attempts")]
    LatchTorn { retries: u32 },

    /// The profile carries no captured HID descriptor, so no device can be
    /// created from it. HIDMaestro's own catalog marks these `IsDeployable =
    /// false`; creating one throws.
    #[error("profile '{0}' has no HID descriptor and cannot be deployed")]
    ProfileNotDeployable(String),

    /// No profile with that slug is in the catalog.
    #[error("no HIDMaestro profile named '{0}'")]
    UnknownProfile(String),

    /// The slot has no live controller (never created, or already destroyed).
    #[error("HIDMaestro slot {0} is not live")]
    DeadSlot(u32),

    /// A Win32 call failed.
    #[error("HIDMaestro {op} failed (win32 error {code})")]
    Win32 { op: &'static str, code: u32 },
}

impl HmError {
    /// True when the root cause is "HIDMaestro is not installed" — the CLI uses
    /// this to pick an exit code and print the install hint, exactly as
    /// [`ksx_output`-style](https://docs.rs) `is_bus_missing` does for ViGEmBus.
    pub fn is_not_installed(&self) -> bool {
        matches!(self, HmError::NotInstalled { .. })
    }

    /// True when the root cause is "ksx cannot do this yet".
    ///
    /// Kept apart from [`HmError::is_not_installed`] because the two lead to
    /// opposite advice: one is an action the user can take, the other is one
    /// they must not be sent on.
    pub fn is_not_implemented(&self) -> bool {
        matches!(self, HmError::NotImplemented { .. })
    }
}

/// What the availability probe looked for, and what it found.
///
/// Carried inside [`HmError::NotInstalled`] so "not installed" is *evidence*
/// rather than an assertion: the user (or a bug report) can see which paths and
/// which section name were checked and disagree with them concretely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSummary {
    /// Human-readable targets, in probe order.
    pub looked_for: Vec<String>,
    /// Any target that did exist (a partial install is worth naming).
    pub found: Vec<String>,
}

impl fmt::Display for ProbeSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.found.is_empty() {
            write!(
                f,
                "looked for {} and found none",
                self.looked_for.join(", ")
            )
        } else {
            write!(
                f,
                "looked for {} and found only {} (partial install)",
                self.looked_for.join(", "),
                self.found.join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> ProbeSummary {
        ProbeSummary {
            looked_for: vec!["a".into(), "b".into()],
            found: vec![],
        }
    }

    #[test]
    fn not_installed_says_what_it_costs_and_what_it_does_not() {
        let msg = HmError::NotInstalled { probe: summary() }.to_string();
        assert!(msg.contains("not installed"), "{msg}");
        // It must name the personas that need it...
        for persona in ["DualSense", "Switch Pro", "Xbox Series"] {
            assert!(msg.contains(persona), "{msg} should mention {persona}");
        }
        // ...and reassure that the cabinet's own personas are unaffected, so
        // nobody installs a second driver out of fear.
        assert!(msg.contains("ViGEmBus"), "{msg}");
        assert!(msg.contains("looked for a, b"), "{msg}");
    }

    #[test]
    fn a_partial_install_is_reported_as_partial() {
        let probe = ProbeSummary {
            looked_for: vec!["svc".into(), "dll".into()],
            found: vec!["svc".into()],
        };
        let msg = probe.to_string();
        assert!(msg.contains("partial install"), "{msg}");
        assert!(msg.contains("found only svc"), "{msg}");
    }

    #[test]
    fn only_not_installed_is_actionable_as_an_install() {
        assert!(HmError::NotInstalled { probe: summary() }.is_not_installed());
        assert!(!HmError::ElevationRequired.is_not_installed());
        assert!(!HmError::DeadSlot(3).is_not_installed());
        assert!(!HmError::LatchTorn { retries: 64 }.is_not_installed());
    }

    #[test]
    fn not_implemented_is_never_reported_as_not_installed() {
        // The confusion this variant exists to end. Folding the two together
        // is what sends a user off to install a driver that cannot help.
        let err = HmError::NotImplemented {
            detail: "no section mapper",
        };
        assert!(err.is_not_implemented());
        assert!(!err.is_not_installed());
        assert!(!HmError::NotInstalled { probe: summary() }.is_not_implemented());
    }

    #[test]
    fn not_implemented_says_installing_will_not_help() {
        let msg = HmError::NotImplemented {
            detail: "no section mapper",
        }
        .to_string();
        assert!(msg.contains("not implemented in this build"), "{msg}");
        assert!(msg.contains("no section mapper"), "{msg}");
        // The load-bearing clause: without it the message still reads as an
        // install problem, which is the whole failure mode.
        assert!(msg.contains("on any machine, installed or not"), "{msg}");
        // ...and it must still say what keeps working, so nobody panics.
        assert!(msg.contains("ViGEmBus"), "{msg}");
    }
}
