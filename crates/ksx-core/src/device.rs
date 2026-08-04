//! Device identity and raw input events.

use crate::key::Key;

/// Stable device identity: the PnP *device instance path*, e.g.
/// `HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000`.
///
/// Never a positional index — the legacy `Keyboard_01..10` strong names were
/// Interception slot indices and drifted on every replug. Instance paths are
/// port-topology-derived and stable while a board stays on its physical port
/// (a documented cabinet invariant, not an accident).
///
/// Comparison is exact and case-sensitive: the capture layer must canonicalize
/// before constructing one (Windows returns uppercase from `CM_Get_Device_ID`;
/// store that verbatim). ksx-core never inspects the contents.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DeviceId {
    fn from(path: &str) -> Self {
        Self(path.to_owned())
    }
}

impl From<String> for DeviceId {
    fn from(path: String) -> Self {
        Self(path)
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One key transition on one device, as delivered by a capture backend.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct KeyEvent {
    pub device: DeviceId,
    pub key: Key,
    /// `true` = press, `false` = release.
    pub down: bool,
    /// Monotonic tick at capture time. Unit is backend-defined for now (QPC
    /// ticks on Windows); ksx-core only ever compares or subtracts it, so the
    /// placeholder stays OS-free.
    pub t: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_exact_and_case_sensitive() {
        let a = DeviceId::new(r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000");
        let b = DeviceId::from(r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000");
        let c = DeviceId::new(r"hid\vid_d209&pid_0430&mi_00\8&2a0d0500&0&0000");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.as_str(), r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000");
        assert_eq!(a.to_string(), a.as_str());
    }

    #[test]
    fn key_event_equality() {
        let ev = |down: bool| KeyEvent {
            device: DeviceId::new("X"),
            key: Key::A,
            down,
            t: 42,
        };
        assert_eq!(ev(true), ev(true));
        assert_ne!(ev(true), ev(false));
    }
}
