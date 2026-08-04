//! HID Keyboard/Keypad usage (page 0x07) → the **same** key vocabulary the
//! Interception backend speaks.
//!
//! # Why this goes through scancodes instead of straight to [`ksx_core::Key`]
//!
//! Every preset on this cabinet, the legacy XML importer, and the recorded
//! replay corpus (`ksx monitor --record`) are written against
//! [`crate::keymap::corrected_key`] — a port of legacy `KeysHelper
//! .GetCorrectedKey`, whose input is a **set-1 scancode plus an Interception
//! state word**. A second, independent usage→`Key` table would be a second
//! source of truth: the day the two disagree about (say) whether E0+0x35 is
//! `NumpadDivide`, presets silently break on one backend only.
//!
//! So this module maps usage → `(set-1 code, E0/E1 prefix)` and hands that to
//! the existing `corrected_key`. There is exactly one key vocabulary, and the
//! WinUSB backend is a different *transport* for it, not a different language.
//!
//! Two consequences worth stating out loud, both of them parity, not bugs:
//!
//! - **Pause** (usage 0x48) becomes `(0x1D, E1)`, and legacy's `is_down` rule
//!   says an E1 stroke is never a press — so Pause reports two releases and no
//!   press, exactly as it does on the Interception backend.
//! - **Usages with no PS/2 equivalent produce no event at all** (F13–F24,
//!   Power, international/language keys, the whole consumer page). Interception
//!   maps unmatched scancodes to [`ksx_core::Key::Unknown`]; here there is no
//!   scancode to map, so the honest answer is silence rather than a stream of
//!   `Unknown` presses no preset can bind. Documented divergence.
//!
//! Table source: the USB HID Usage Tables keyboard page cross-referenced with
//! Microsoft's *Keyboard Scan Code Specification* (set-1 / "scan code set 1"
//! translation column). Verified against [`crate::keymap`] — every entry below
//! resolves to a named `Key`, which the unit tests assert exhaustively.

use crate::keymap::{KEY_DOWN, KEY_E0, KEY_E1, KEY_UP};

/// HID Usage Page 0x07 — Keyboard/Keypad. The only page this backend reads.
pub const PAGE_KEYBOARD: u16 = 0x07;

/// `ErrorRollOver` — reported in **every** array slot when more keys are held
/// than the array can carry (HID 1.11 §10, "phantom state").
pub const USAGE_ERROR_ROLLOVER: u8 = 0x01;
/// `POSTFail`.
pub const USAGE_POST_FAIL: u8 = 0x02;
/// `ErrorUndefined`.
pub const USAGE_ERROR_UNDEFINED: u8 = 0x03;

/// Is this usage one of the three HID error codes rather than a key?
pub const fn is_error_usage(usage: u8) -> bool {
    matches!(
        usage,
        USAGE_ERROR_ROLLOVER | USAGE_POST_FAIL | USAGE_ERROR_UNDEFINED
    )
}

/// Set-1 scancode + extended prefix for one keyboard-page usage.
///
/// `None` for usages with no PS/2 set-1 equivalent (see module docs).
pub const fn usage_to_scancode(usage: u8) -> Option<(u16, u16)> {
    let mapped: (u16, u16) = match usage {
        // --- letters ---------------------------------------------------
        0x04 => (0x1E, 0), // a
        0x05 => (0x30, 0), // b
        0x06 => (0x2E, 0), // c
        0x07 => (0x20, 0), // d
        0x08 => (0x12, 0), // e
        0x09 => (0x21, 0), // f
        0x0A => (0x22, 0), // g
        0x0B => (0x23, 0), // h
        0x0C => (0x17, 0), // i
        0x0D => (0x24, 0), // j
        0x0E => (0x25, 0), // k
        0x0F => (0x26, 0), // l
        0x10 => (0x32, 0), // m
        0x11 => (0x31, 0), // n
        0x12 => (0x18, 0), // o
        0x13 => (0x19, 0), // p
        0x14 => (0x10, 0), // q
        0x15 => (0x13, 0), // r
        0x16 => (0x1F, 0), // s
        0x17 => (0x14, 0), // t
        0x18 => (0x16, 0), // u
        0x19 => (0x2F, 0), // v
        0x1A => (0x11, 0), // w
        0x1B => (0x2D, 0), // x
        0x1C => (0x15, 0), // y
        0x1D => (0x2C, 0), // z

        // --- digit row -------------------------------------------------
        0x1E => (0x02, 0), // 1
        0x1F => (0x03, 0), // 2
        0x20 => (0x04, 0), // 3
        0x21 => (0x05, 0), // 4
        0x22 => (0x06, 0), // 5
        0x23 => (0x07, 0), // 6
        0x24 => (0x08, 0), // 7
        0x25 => (0x09, 0), // 8
        0x26 => (0x0A, 0), // 9
        0x27 => (0x0B, 0), // 0

        // --- control / punctuation -------------------------------------
        0x28 => (0x1C, 0), // Enter
        0x29 => (0x01, 0), // Escape
        0x2A => (0x0E, 0), // Backspace
        0x2B => (0x0F, 0), // Tab
        0x2C => (0x39, 0), // Space
        0x2D => (0x0C, 0), // - _
        0x2E => (0x0D, 0), // = +
        0x2F => (0x1A, 0), // [ {
        0x30 => (0x1B, 0), // ] }
        0x31 => (0x2B, 0), // \ |
        0x32 => (0x2B, 0), // Non-US # ~ (same set-1 code as backslash)
        0x33 => (0x27, 0), // ; :
        0x34 => (0x28, 0), // ' "
        0x35 => (0x29, 0), // ` ~
        0x36 => (0x33, 0), // , <
        0x37 => (0x34, 0), // . >
        0x38 => (0x35, 0), // / ?
        0x39 => (0x3A, 0), // CapsLock

        // --- function row ----------------------------------------------
        0x3A => (0x3B, 0), // F1
        0x3B => (0x3C, 0), // F2
        0x3C => (0x3D, 0), // F3
        0x3D => (0x3E, 0), // F4
        0x3E => (0x3F, 0), // F5
        0x3F => (0x40, 0), // F6
        0x40 => (0x41, 0), // F7
        0x41 => (0x42, 0), // F8
        0x42 => (0x43, 0), // F9
        0x43 => (0x44, 0), // F10
        0x44 => (0x57, 0), // F11
        0x45 => (0x58, 0), // F12

        // --- the E0 / E1 cluster ---------------------------------------
        0x46 => (0x37, KEY_E0), // PrintScreen (E0 2A E0 37 on the wire)
        0x47 => (0x46, 0),      // ScrollLock
        0x48 => (0x1D, KEY_E1), // Pause  (E1 1D 45) — see module docs
        0x49 => (0x52, KEY_E0), // Insert
        0x4A => (0x47, KEY_E0), // Home
        0x4B => (0x49, KEY_E0), // PageUp
        0x4C => (0x53, KEY_E0), // Delete
        0x4D => (0x4F, KEY_E0), // End
        0x4E => (0x51, KEY_E0), // PageDown
        0x4F => (0x4D, KEY_E0), // Right
        0x50 => (0x4B, KEY_E0), // Left
        0x51 => (0x50, KEY_E0), // Down
        0x52 => (0x48, KEY_E0), // Up

        // --- keypad ----------------------------------------------------
        0x53 => (0x45, 0),      // NumLock
        0x54 => (0x35, KEY_E0), // Keypad /
        0x55 => (0x37, 0),      // Keypad *
        0x56 => (0x4A, 0),      // Keypad -
        0x57 => (0x4E, 0),      // Keypad +
        0x58 => (0x1C, KEY_E0), // Keypad Enter
        0x59 => (0x4F, 0),      // Keypad 1
        0x5A => (0x50, 0),      // Keypad 2
        0x5B => (0x51, 0),      // Keypad 3
        0x5C => (0x4B, 0),      // Keypad 4
        0x5D => (0x4C, 0),      // Keypad 5
        0x5E => (0x4D, 0),      // Keypad 6
        0x5F => (0x47, 0),      // Keypad 7
        0x60 => (0x48, 0),      // Keypad 8
        0x61 => (0x49, 0),      // Keypad 9
        0x62 => (0x52, 0),      // Keypad 0
        0x63 => (0x53, 0),      // Keypad .

        // --- the last two that exist in the legacy vocabulary -----------
        0x64 => (0x56, 0),      // Non-US \ | (the 102nd key)
        0x65 => (0x5D, KEY_E0), // Application / Menu

        // --- modifiers (also reachable through the modifier bitmap) -----
        0xE0 => (0x1D, 0),      // LeftControl
        0xE1 => (0x2A, 0),      // LeftShift
        0xE2 => (0x38, 0),      // LeftAlt
        0xE3 => (0x5B, KEY_E0), // LeftGUI  → LeftWindows
        0xE4 => (0x1D, KEY_E0), // RightControl
        0xE5 => (0x36, 0),      // RightShift
        0xE6 => (0x38, KEY_E0), // RightAlt
        0xE7 => (0x5C, KEY_E0), // RightGUI → RightWindows

        _ => return None,
    };
    Some(mapped)
}

/// One usage transition → the `(code, state)` pair
/// [`crate::decision::key_event`] consumes, or `None` when the usage has no
/// set-1 equivalent.
pub const fn usage_to_stroke(usage: u8, down: bool) -> Option<(u16, u16)> {
    let Some((code, prefix)) = usage_to_scancode(usage) else {
        return None;
    };
    let updown = if down { KEY_DOWN } else { KEY_UP };
    Some((code, prefix | updown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::corrected_key;
    use ksx_core::Key;

    /// Resolve a usage the way the backend does, end to end.
    fn key_of(usage: u8, down: bool) -> Option<Key> {
        let (code, state) = usage_to_stroke(usage, down)?;
        Some(corrected_key(code, state))
    }

    /// The contract that keeps presets and the replay corpus valid: **every**
    /// usage this module claims to know resolves to a *named* key in the legacy
    /// vocabulary. A mapping that lands on `Unknown` is a table bug, not a
    /// device quirk — it would silently un-bind a preset entry.
    #[test]
    fn every_mapped_usage_resolves_to_a_named_key() {
        let mut mapped = 0;
        for usage in 0u8..=0xFF {
            let Some(key) = key_of(usage, true) else {
                continue;
            };
            mapped += 1;
            assert_ne!(
                key,
                Key::Unknown,
                "usage {usage:#04x} maps to a scancode the legacy table does not name"
            );
            assert_ne!(key, Key::None, "usage {usage:#04x} maps to Key::None");
            // ...and the release resolves to the same key.
            assert_eq!(
                key_of(usage, false),
                Some(key),
                "usage {usage:#04x} make/break disagree"
            );
        }
        // 26 letters (0x04..=0x1D) + 10 digits (0x1E..=0x27)
        // + 18 punctuation/control (0x28..=0x39) + 12 F-keys (0x3A..=0x45)
        // + 13 PrintScreen..Up (0x46..=0x52) + 17 keypad (0x53..=0x63)
        // + Non-US backslash and Application (0x64, 0x65) + 8 modifiers.
        assert_eq!(
            mapped, 106,
            "table size changed — update the count and say why"
        );
    }

    #[test]
    fn arrow_cluster_is_the_arrows_not_the_keypad() {
        // The single most important row for an arcade panel: the I-PAC ships
        // joysticks wired to arrow keys by default.
        assert_eq!(key_of(0x52, true), Some(Key::Up));
        assert_eq!(key_of(0x51, true), Some(Key::Down));
        assert_eq!(key_of(0x50, true), Some(Key::Left));
        assert_eq!(key_of(0x4F, true), Some(Key::Right));
        // ...and the keypad digits stay keypad digits.
        assert_eq!(key_of(0x60, true), Some(Key::Numpad8));
        assert_eq!(key_of(0x5A, true), Some(Key::Numpad2));
        assert_eq!(key_of(0x5C, true), Some(Key::Numpad4));
        assert_eq!(key_of(0x5E, true), Some(Key::Numpad6));
    }

    #[test]
    fn modifiers_split_left_and_right() {
        assert_eq!(key_of(0xE0, true), Some(Key::LeftControl));
        assert_eq!(key_of(0xE4, true), Some(Key::RightControl));
        assert_eq!(key_of(0xE1, true), Some(Key::LeftShift));
        assert_eq!(key_of(0xE5, true), Some(Key::RightShift));
        assert_eq!(key_of(0xE2, true), Some(Key::LeftAlt));
        assert_eq!(key_of(0xE6, true), Some(Key::RightAlt));
        assert_eq!(key_of(0xE3, true), Some(Key::LeftWindows));
        assert_eq!(key_of(0xE7, true), Some(Key::RightWindows));
    }

    /// `LeftCtrl x5` and `Ctrl+Alt+Del` must be expressible from HID usages —
    /// they are the escape hatch, and an escape hatch that only works on the
    /// old backend is not an escape hatch.
    #[test]
    fn the_escape_gestures_are_reachable_from_usages() {
        assert_eq!(key_of(0xE0, true), Some(Key::LeftControl));
        assert_eq!(key_of(0xE2, true), Some(Key::LeftAlt));
        assert_eq!(key_of(0x4C, true), Some(Key::Delete));
        assert_eq!(key_of(0xE4, true), Some(Key::RightControl));
    }

    #[test]
    fn pause_keeps_its_legacy_never_pressed_behaviour() {
        use crate::keymap::is_down;
        let (code, state) = usage_to_stroke(0x48, true).unwrap();
        assert_eq!((code, state), (0x1D, KEY_E1 | KEY_DOWN));
        assert_eq!(corrected_key(code, state), Key::Pause);
        assert!(
            !is_down(state),
            "legacy parity: an E1 stroke never counts as a press"
        );
    }

    #[test]
    fn keypad_enter_and_divide_are_the_extended_forms() {
        assert_eq!(key_of(0x58, true), Some(Key::NumpadEnter));
        assert_eq!(key_of(0x54, true), Some(Key::NumpadDivide));
        // ...and the main-row versions are not.
        assert_eq!(key_of(0x28, true), Some(Key::Enter));
        assert_eq!(key_of(0x38, true), Some(Key::ForwardSlashQuestionMark));
    }

    #[test]
    fn unmapped_usages_are_silent_not_unknown() {
        // Error codes.
        assert_eq!(usage_to_stroke(0x00, true), None);
        assert_eq!(usage_to_stroke(USAGE_ERROR_ROLLOVER, true), None);
        assert_eq!(usage_to_stroke(USAGE_POST_FAIL, true), None);
        assert_eq!(usage_to_stroke(USAGE_ERROR_UNDEFINED, true), None);
        // F13..F24, Power, Keypad =, international/language keys.
        for usage in [0x66u8, 0x67, 0x68, 0x73, 0x85, 0x87, 0x88, 0x90, 0xF0] {
            assert_eq!(usage_to_stroke(usage, true), None, "usage {usage:#04x}");
        }
    }

    /// The cross-crate closure of the key vocabulary: for every usage this
    /// table maps, the wire form `ksx_platform::inject` will *type* must be the
    /// same `(set-1 code, E0)` pair the usage came from.
    ///
    /// Neither `usage_to_scancode` nor `inject::stroke_for` alone can catch a
    /// divergence here, because each is self-consistent: the capture side turns
    /// a usage into a `Key`, and the injection side turns a `Key` back into
    /// bytes through an independently written table. If the two disagree for
    /// one key, that key drives the right pad button while typing the *wrong
    /// character* into Windows whenever passthrough is on — the escape-hatch
    /// case, when a player most needs the panel to behave. `Pause` is the one
    /// documented exception (`E1 1D 45` has no single-`KEYBDINPUT` form).
    #[test]
    fn every_usage_injects_the_wire_form_it_was_decoded_from() {
        use ksx_platform::inject::{stroke_for, KeyStroke};

        let mut checked = 0;
        for usage in 0u8..=0xFF {
            let Some((code, prefix)) = usage_to_scancode(usage) else {
                continue;
            };
            let key = corrected_key(code, prefix | KEY_DOWN);
            let stroke = stroke_for(key)
                .unwrap_or_else(|| panic!("usage {usage:#04x} -> {key} has no wire form"));

            if prefix == KEY_E1 {
                assert_eq!(
                    (usage, stroke),
                    (0x48, KeyStroke::VirtualKey { vk: 0x13 }),
                    "Pause is the only documented VirtualKey exception"
                );
                checked += 1;
                continue;
            }

            assert_eq!(
                stroke.scancode(),
                Some((code, prefix == KEY_E0)),
                "usage {usage:#04x} decodes as ({code:#04x}, e0={}) -> {key}, but injects as \
                 {stroke:?} — the panel would type a different key than it reports",
                prefix == KEY_E0,
            );
            checked += 1;
        }
        assert_eq!(checked, 106, "every mapped usage must be covered");
    }

    #[test]
    fn error_usages_are_classified() {
        assert!(is_error_usage(USAGE_ERROR_ROLLOVER));
        assert!(is_error_usage(USAGE_POST_FAIL));
        assert!(is_error_usage(USAGE_ERROR_UNDEFINED));
        assert!(!is_error_usage(0x00));
        assert!(!is_error_usage(0x04));
        assert!(!is_error_usage(0xE0));
    }
}
