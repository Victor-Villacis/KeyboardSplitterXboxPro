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

    /// The underlying driver client reported an error.
    #[cfg(windows)]
    #[error("ViGEmBus target operation failed")]
    TargetError(#[source] vigem_client::Error),
}

impl OutputError {
    /// True when the root cause is "ViGEmBus is not installed" — the CLI uses
    /// this to print the install hint and pick a stable exit code.
    pub fn is_bus_missing(&self) -> bool {
        matches!(self, OutputError::BusNotFound)
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
    fn unsupported_persona_names_the_persona() {
        // The message has to say which controller was refused; "unsupported
        // persona" alone leaves the user guessing which slot to edit.
        let msg = OutputError::PersonaUnsupported(ksx_core::Persona::PlayStation).to_string();
        assert!(msg.contains("playstation"), "{msg}");
    }
}
