//! The pure pass/suppress decision core, shared by the real Interception
//! backend and the mock. This is the logic that gets unit-tested heavily; the
//! backends are thin shells that feed it strokes and obey its verdicts.

use ksx_core::{DeviceId, KeyEvent};

use crate::keymap::{corrected_key, is_down};

/// The mode a capture thread is in, swapped atomically via `arc-swap` in the
/// real backend (the hot loop reads a snapshot, the ctl handler publishes new
/// ones — no locks).
#[derive(Clone, Debug)]
pub struct CaptureSet {
    /// Observe-only: report AND re-send everything, suppress nothing.
    pub passthrough: bool,
    /// Devices whose strokes are suppressed from the OS while capturing.
    pub captured: Vec<DeviceId>,
}

impl CaptureSet {
    /// The startup mode: passthrough, nothing captured. Safe by construction —
    /// a freshly started backend can never eat a keystroke.
    pub fn passthrough() -> Self {
        Self {
            passthrough: true,
            captured: Vec::new(),
        }
    }

    /// Capturing mode with the given suppressed set.
    pub fn capturing(ids: Vec<DeviceId>) -> Self {
        Self {
            passthrough: false,
            captured: ids,
        }
    }

    /// Is this device's input suppressed from the OS right now?
    pub fn is_captured(&self, id: &DeviceId) -> bool {
        !self.passthrough && self.captured.iter().any(|c| c == id)
    }
}

/// Verdict for one keyboard stroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrokeOutcome {
    /// Re-send the stroke to the OS, byte-for-byte as received. `false` only
    /// when capturing and the source device is captured (swallowed).
    pub resend: bool,
    /// The event to report to the engine. Always produced for keyboard strokes:
    /// the engine sees *all* devices (emergency escapes fire on any keyboard,
    /// risk review §3 item 2; `ksx monitor` wants unassigned traffic too).
    pub event: KeyEvent,
}

/// Decide one keyboard stroke. Pure: no clock, no OS, no channel.
///
/// `is_captured` is precomputed by the caller (slot-table lookup in the real
/// backend, set lookup in the mock) so this function stays allocation-free
/// except for the `DeviceId` clone the owned `KeyEvent` contract requires.
pub fn process_keyboard_stroke(
    passthrough: bool,
    is_captured: bool,
    device: &DeviceId,
    code: u16,
    state: u16,
    t: u64,
) -> StrokeOutcome {
    StrokeOutcome {
        resend: passthrough || !is_captured,
        event: KeyEvent {
            device: device.clone(),
            key: corrected_key(code, state),
            down: is_down(state),
            t,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{KEY_DOWN, KEY_E0, KEY_UP};
    use ksx_core::Key;

    fn dev(s: &str) -> DeviceId {
        DeviceId::from(s)
    }

    #[test]
    fn passthrough_resends_and_reports_everything() {
        let set = CaptureSet::passthrough();
        let d = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let out =
            process_keyboard_stroke(set.passthrough, set.is_captured(&d), &d, 30, KEY_DOWN, 7);
        assert!(out.resend);
        assert_eq!(out.event.key, Key::A);
        assert!(out.event.down);
        assert_eq!(out.event.t, 7);
        assert_eq!(out.event.device, d);
    }

    #[test]
    fn captured_device_is_swallowed_but_still_reported() {
        let d = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let set = CaptureSet::capturing(vec![d.clone()]);
        let out = process_keyboard_stroke(set.passthrough, set.is_captured(&d), &d, 30, KEY_UP, 8);
        assert!(!out.resend, "captured strokes must NOT reach the OS");
        assert_eq!(out.event.key, Key::A);
        assert!(!out.event.down);
    }

    #[test]
    fn unknown_device_passes_through_while_capturing() {
        let captured = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let other = dev("HID\\VID_046D&PID_C31C&REV_6402&MI_00");
        let set = CaptureSet::capturing(vec![captured]);
        let out = process_keyboard_stroke(
            set.passthrough,
            set.is_captured(&other),
            &other,
            75,
            KEY_E0 | KEY_DOWN,
            9,
        );
        assert!(out.resend, "unassigned keyboards must keep typing");
        assert_eq!(out.event.key, Key::Left, "correction still applies");
        assert!(out.event.down);
    }

    #[test]
    fn passthrough_mode_overrides_captured_set() {
        // SetPassthrough (or a watchdog trip) must leak captured devices to the
        // OS rather than black-hole them.
        let d = dev("X");
        let mut set = CaptureSet::capturing(vec![d.clone()]);
        set.passthrough = true;
        assert!(!set.is_captured(&d));
        let out =
            process_keyboard_stroke(set.passthrough, set.is_captured(&d), &d, 30, KEY_DOWN, 0);
        assert!(out.resend);
    }

    #[test]
    fn capture_set_membership_is_exact() {
        let a = dev("A");
        let b = dev("a"); // case-sensitive by DeviceId contract
        let set = CaptureSet::capturing(vec![a.clone()]);
        assert!(set.is_captured(&a));
        assert!(!set.is_captured(&b));
    }
}
