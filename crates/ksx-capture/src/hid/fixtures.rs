//! **Synthetic** HID descriptors and report builders for the parser tests.
//!
//! Every byte here is hand-assembled from the HID 1.11 spec and the Device
//! Class Definition for HID (Appendix B.1, "Protocol 1 (Keyboard)"), not dumped
//! from the cabinet's I-PAC. That is a hard rule for M6: the board is
//! production hardware on a machine whose arcade panel must keep working, so
//! the test corpus is built from the specification and the *documented*
//! enumeration in `docs/research/keyboard-capture-2026.md` §5 — never by
//! opening or claiming a live device.
//!
//! Available in non-test builds too, so `ksx doctor`-style tooling and the
//! fuzz target (`docs/PLAYBOOK.md`: "M3/M6 fuzzing … the raw NKRO HID report
//! parser") can share exactly the corpus the unit tests use.

/// HID 1.11 Appendix B.1 — the boot-protocol keyboard, verbatim.
///
/// 8-byte report: modifier bitmap, one constant byte, six usage slots.
pub const BOOT_KEYBOARD_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0xE0, //   Usage Minimum (LeftControl)
    0x29, 0xE7, //   Usage Maximum (RightGUI)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data, Variable, Absolute)   <- modifiers
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x01, //   Input (Constant)                   <- reserved byte
    0x95, 0x05, //   Report Count (5)
    0x75, 0x01, //   Report Size (1)
    0x05, 0x08, //   Usage Page (LEDs)
    0x19, 0x01, //   Usage Minimum (Num Lock)
    0x29, 0x05, //   Usage Maximum (Kana)
    0x91, 0x02, //   Output (Data, Variable, Absolute)  <- must NOT move input
    0x95, 0x01, //   Report Count (1)
    0x75, 0x03, //   Report Size (3)
    0x91, 0x01, //   Output (Constant)                  <- ditto
    0x95, 0x06, //   Report Count (6)
    0x75, 0x08, //   Report Size (8)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x65, //   Logical Maximum (101)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0x65, //   Usage Maximum (101)
    0x81, 0x00, //   Input (Data, Array)                <- the six key slots
    0xC0, // End Collection
];

/// Report ID the synthetic NKRO descriptor uses.
pub const NKRO_REPORT_ID: u8 = 0x02;

/// An NKRO keyboard in the shape N-key-rollover firmware actually ships:
/// a report ID, the 8 modifier bits, then a 240-bit usage bitmap covering
/// usages 0x00..=0xEF. 32 bytes on the wire (1 ID + 31 payload).
pub const NKRO_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x02, //   Report ID (NKRO_REPORT_ID)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0xE0, //   Usage Minimum (LeftControl)
    0x29, 0xE7, //   Usage Maximum (RightGUI)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data, Variable, Absolute)  <- modifiers
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0xEF, //   Usage Maximum (239)
    0x95, 0xF0, //   Report Count (240)
    0x81, 0x02, //   Input (Data, Variable, Absolute)  <- the NKRO bitmap
    0xC0, // End Collection
];

/// A keyboard collection (report ID 1) followed by a consumer-control
/// collection (report ID 3) on the same interface — the shape that catches a
/// parser leaking one report's bit cursor into another's.
pub const KEYBOARD_PLUS_CONSUMER_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x01, //   Report ID (1)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0xE0, 0x29, 0xE7, //   Usage Min/Max (modifiers)
    0x15, 0x00, 0x25, 0x01, //   Logical 0..1
    0x75, 0x01, 0x95, 0x08, //   1 bit x 8
    0x81, 0x02, //   Input (Data, Var, Abs)
    0x95, 0x01, 0x75, 0x08, //   8 bits
    0x81, 0x01, //   Input (Constant) reserved
    0x95, 0x06, 0x75, 0x08, //   6 x 8
    0x15, 0x00, 0x25, 0x65, //   Logical 0..101
    0x19, 0x00, 0x29, 0x65, //   Usage 0..101
    0x81, 0x00, //   Input (Data, Array)
    0xC0, // End Collection
    0x05, 0x0C, // Usage Page (Consumer)
    0x09, 0x01, // Usage (Consumer Control)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x03, //   Report ID (3)
    0x19, 0x00, 0x2A, 0x3C, 0x02, //   Usage Min 0, Usage Max 0x023C
    0x15, 0x00, 0x26, 0x3C, 0x02, //   Logical 0..0x023C
    0x75, 0x10, 0x95, 0x02, //   16 bits x 2
    0x81, 0x00, //   Input (Data, Array)  <- consumer page: not ours
    0xC0, // End Collection
];

/// Exercises Push/Pop of the global item state around a non-keyboard page.
pub const PUSH_POP_DESCRIPTOR: &[u8] = &[
    0x05, 0x07, // Usage Page (Keyboard/Keypad)
    0x15, 0x00, 0x25, 0x01, // Logical 0..1
    0x75, 0x01, 0x95, 0x08, // 1 bit x 8
    0xA4, // Push
    0x05, 0x09, //   Usage Page (Button)
    0x81, 0x02, //   Input (Data, Var, Abs)  <- 8 button bits, skipped
    0xB4, // Pop  (page is Keyboard again)
    0x19, 0xE0, 0x29, 0xE7, // Usage Min/Max (modifiers)
    0x81, 0x02, // Input (Data, Var, Abs)   <- at bit 8
];

/// A plain 3-button mouse: valid HID, zero keyboard usages.
pub const MOUSE_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x02, // Usage (Mouse)
    0xA1, 0x01, // Collection (Application)
    0x09, 0x01, //   Usage (Pointer)
    0xA1, 0x00, //   Collection (Physical)
    0x05, 0x09, //     Usage Page (Button)
    0x19, 0x01, 0x29, 0x03, //     Usage 1..3
    0x15, 0x00, 0x25, 0x01, //     Logical 0..1
    0x95, 0x03, 0x75, 0x01, //     3 x 1 bit
    0x81, 0x02, //     Input (Data, Var, Abs)
    0x95, 0x01, 0x75, 0x05, //     padding
    0x81, 0x01, //     Input (Constant)
    0x05, 0x01, //     Usage Page (Generic Desktop)
    0x09, 0x30, 0x09, 0x31, //     Usage X, Y
    0x15, 0x81, 0x25, 0x7F, //     Logical -127..127
    0x75, 0x08, 0x95, 0x02, //     8 bits x 2
    0x81, 0x06, //     Input (Data, Var, Rel)
    0xC0, //   End Collection
    0xC0, // End Collection
];

/// Build an 8-byte boot-protocol report: modifier bitmap + up to six usages.
///
/// Panics if more than six usages are given — the caller means
/// [`boot_rollover`] in that case, and silently truncating would hide the very
/// condition the rollover tests exist to pin down.
pub fn boot_report(modifiers: u8, keys: &[u8]) -> [u8; 8] {
    assert!(
        keys.len() <= 6,
        "the boot report holds six slots; use boot_rollover() for the 7th key"
    );
    let mut report = [0u8; 8];
    report[0] = modifiers;
    for (slot, &usage) in report[2..].iter_mut().zip(keys) {
        *slot = usage;
    }
    report
}

/// The phantom/rollover report: every slot carries `ErrorRollOver` (0x01).
///
/// HID 1.11 §10 — a boot keyboard that cannot resolve which keys are down
/// reports this instead of key data. The modifier bitmap stays valid.
pub fn boot_rollover(modifiers: u8) -> [u8; 8] {
    let mut report = [0u8; 8];
    report[0] = modifiers;
    for slot in &mut report[2..] {
        *slot = super::usage::USAGE_ERROR_ROLLOVER;
    }
    report
}

/// Build an NKRO report (report ID + modifier byte + 30-byte usage bitmap).
pub fn nkro_report(modifiers: u8, keys: &[u8]) -> Vec<u8> {
    let mut report = vec![0u8; 32];
    report[0] = NKRO_REPORT_ID;
    report[1] = modifiers;
    for &usage in keys {
        // Bitmap bit `usage` lives at payload bit 8 + usage, i.e. report byte
        // 1 + 1 + usage/8. Usages 0xF0.. are outside the declared 240 bits.
        assert!(
            usage < 0xF0,
            "usage {usage:#04x} is outside the NKRO bitmap"
        );
        let bit = 8 + usize::from(usage);
        report[1 + bit / 8] |= 1 << (bit % 8);
    }
    report
}

#[cfg(test)]
mod descriptor_id_agreement {
    /// The descriptor spells its report ID as a literal so the byte table stays
    /// two-per-line readable; this keeps the constant honest about it.
    #[test]
    fn nkro_descriptor_declares_the_advertised_report_id() {
        let bytes = super::NKRO_DESCRIPTOR;
        let at = bytes
            .windows(2)
            .position(|w| w[0] == 0x85)
            .expect("a Report ID item");
        assert_eq!(bytes[at + 1], super::NKRO_REPORT_ID);
    }
}
