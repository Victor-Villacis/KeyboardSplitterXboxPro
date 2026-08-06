//! Output-layer error taxonomy.

use std::time::Duration;

use crate::backend::PadHandle;

/// Errors from a [`crate::VirtualPadBackend`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OutputError {
    /// ViGEmBus is not installed (the ViGEm bus device interface is absent).
    ///
    /// Maps exactly `vigem_client::Error::BusNotFound` from `Client::connect()`.
    #[error(
        "ViGEmBus is not installed: no ViGEm bus device interface was found. \
         Install it with `ksx install-drivers` \
         (bundled installer: drivers/ViGEmBus_1.22.0_x64_x86_arm64.exe)"
    )]
    BusNotFound,

    /// The pad was plugged but did not become ready within the deadline.
    #[error("virtual pad did not become ready within {0:?}")]
    PlugTimeout(Duration),

    /// The handle does not name a currently plugged pad (never plugged, already
    /// unplugged, or from another backend instance).
    #[error("unknown or unplugged pad handle {0:?}")]
    UnknownHandle(PadHandle),

    /// This backend cannot emulate the requested controller.
    ///
    /// Never downgraded to a working pad of a different kind: see
    /// [`VirtualPadBackend::plug_persona`](crate::VirtualPadBackend::plug_persona).
    #[error("this backend cannot emulate a '{0}' controller")]
    PersonaUnsupported(ksx_core::Persona),

    /// No backend is configured that could materialize this persona.
    ///
    /// Distinct from [`OutputError::PersonaUnsupported`]: that one means "this
    /// backend cannot do it", this one means "the backend that *can* do it was
    /// never wired up" — a build/config problem, not a driver problem.
    #[error(
        "no '{0}' backend is available in this build — the persona is valid but \
         nothing is wired up to plug it"
    )]
    BackendUnavailable(ksx_core::PadBackend),

    /// HIDMaestro is not installed. The M8 analogue of
    /// [`OutputError::BusNotFound`], and the only outcome available on a
    /// machine without it.
    ///
    /// Carries the probe summary verbatim so "not installed" is evidence the
    /// user can check, rather than an assertion they have to trust.
    #[error(
        "HIDMaestro is not installed ({probe}) — the DualSense, Switch Pro and \
         Xbox Series personas require it. Xbox 360 and PlayStation do not: \
         they run on ViGEmBus."
    )]
    HidMaestroMissing { probe: String },

    /// Any other HIDMaestro client failure.
    #[error("HIDMaestro operation failed")]
    HidMaestro(#[source] ksx_hidmaestro::HmError),

    /// The underlying driver client reported an error.
    #[cfg(windows)]
    #[error("ViGEmBus target operation failed")]
    TargetError(#[source] vigem_client::Error),
}

impl OutputError {
    /// True when the root cause is "ViGEmBus is not installed" — the CLI uses
    /// this to print the install hint and pick a stable exit code.
    ///
    /// Deliberately **not** true for [`OutputError::HidMaestroMissing`]: the two
    /// have different fixes, different affected personas, and a missing
    /// HIDMaestro leaves the cabinet fully working, so folding them together
    /// would tell a user to reinstall the driver that is not the problem.
    pub fn is_bus_missing(&self) -> bool {
        matches!(self, OutputError::BusNotFound)
    }

    /// True when the root cause is "HIDMaestro is not installed".
    pub fn is_hidmaestro_missing(&self) -> bool {
        match self {
            OutputError::HidMaestroMissing { .. } => true,
            OutputError::HidMaestro(err) => err.is_not_installed(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_not_found_message_is_actionable() {
        let msg = OutputError::BusNotFound.to_string();
        assert!(msg.contains("ViGEmBus is not installed"), "{msg}");
        assert!(msg.contains("ksx install-drivers"), "{msg}");
        assert!(
            msg.contains("drivers/ViGEmBus_1.22.0_x64_x86_arm64.exe"),
            "{msg}"
        );
        assert!(OutputError::BusNotFound.is_bus_missing());
    }

    #[test]
    fn only_bus_not_found_is_bus_missing() {
        assert!(!OutputError::PlugTimeout(Duration::from_secs(5)).is_bus_missing());
        assert!(!OutputError::UnknownHandle(PadHandle(7)).is_bus_missing());
        assert!(!OutputError::PersonaUnsupported(ksx_core::Persona::PlayStation).is_bus_missing());
        #[cfg(windows)]
        assert!(!OutputError::TargetError(vigem_client::Error::NoFreeSlot).is_bus_missing());
    }

    #[test]
    fn the_two_missing_driver_errors_are_never_confused() {
        // They have different fixes and different blast radii: a missing
        // HIDMaestro costs three personas, a missing ViGEmBus costs the whole
        // cabinet. Nothing may collapse them into one flag.
        let hm = OutputError::HidMaestroMissing {
            probe: "looked for a, b and found none".into(),
        };
        assert!(hm.is_hidmaestro_missing());
        assert!(!hm.is_bus_missing());
        assert!(!OutputError::BusNotFound.is_hidmaestro_missing());

        let msg = hm.to_string();
        assert!(msg.contains("HIDMaestro is not installed"), "{msg}");
        assert!(msg.contains("looked for a, b"), "{msg} must carry evidence");
        // And it must say what still works, so nobody panics about the cabinet.
        assert!(msg.contains("ViGEmBus"), "{msg}");
    }

    #[test]
    fn an_unavailable_backend_names_which_one() {
        let err = OutputError::BackendUnavailable(ksx_core::PadBackend::HidMaestro);
        assert!(err.to_string().contains("hidmaestro"), "{err}");
        assert!(!err.is_bus_missing());
        assert!(!err.is_hidmaestro_missing());
    }

    #[test]
    fn unsupported_persona_names_the_persona() {
        // The message has to say which controller was refused; "unsupported
        // persona" alone leaves the user guessing which slot to edit.
        let msg = OutputError::PersonaUnsupported(ksx_core::Persona::PlayStation).to_string();
        assert!(msg.contains("playstation"), "{msg}");
    }
}
