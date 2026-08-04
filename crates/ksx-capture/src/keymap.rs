//! Scancode + E0/E1 state → [`ksx_core::Key`], a faithful port of legacy
//! `Interceptor/KeysHelper.cs::GetCorrectedKey` and the `isDown` rule from
//! `Interception.cs::GetKeyboardKeys`.
//!
//! The driver hands us a set-1 make code plus a state word; extended keys carry
//! E0 (0x02) or E1 (0x04) bits on top of Up (0x01). The correction collapses
//! (code, state) into the single logical key vocabulary that presets are written
//! against — e.g. E0+`Numpad4` is `Key::Left` (the arrow), plain `Numpad4` stays
//! `Key::Numpad4`; Pause arrives as E1+`LeftControl`.
//!
//! State constants come from the driver header (`interception.h`), which both
//! the legacy C# `KeyState` enum and `interception-sys` agree on. Do NOT use
//! `kanata_interception::KeyState` here: its bitflags define `E1 = 3` instead of
//! `0x04`, which is wrong against the driver.

use ksx_core::Key;

/// `INTERCEPTION_KEY_DOWN` — a make (press) stroke has no bits set.
pub const KEY_DOWN: u16 = 0x00;
/// `INTERCEPTION_KEY_UP` — break (release) bit.
pub const KEY_UP: u16 = 0x01;
/// `INTERCEPTION_KEY_E0` — extended-key prefix bit.
pub const KEY_E0: u16 = 0x02;
/// `INTERCEPTION_KEY_E1` — the rarer extended prefix (Pause sequence).
pub const KEY_E1: u16 = 0x04;

/// Legacy `KeysHelper.GetCorrectedKey`, ported case for case.
///
/// Divergence (documented, deliberate): legacy operated on a C# enum cast, so a
/// scancode with no named enum member flowed through as an undefined enum value.
/// Rust cannot represent that, so any (code, state) pair legacy would have left
/// as an undefined value maps to [`Key::Unknown`] — which is also what legacy
/// produced for every unmatched *extended* code, so behavior only differs for
/// unnamed plain scancodes that no preset could have referenced anyway.
pub fn corrected_key(code: u16, state: u16) -> Key {
    // Plain make/break: the code is already the legacy key value (0..=93).
    if state == KEY_DOWN || state == KEY_UP {
        return Key::from_value(code).unwrap_or(Key::Unknown);
    }

    // E1 (with or without Up) + LeftControl == Pause.
    if (state == KEY_E1 || state == (KEY_E1 | KEY_UP)) && code == Key::LeftControl.value() {
        return Key::Pause;
    }

    // E0-extended make/break: the correction table.
    if state == (KEY_E0 | KEY_DOWN) || state == (KEY_E0 | KEY_UP) {
        return match code {
            // Media keys share codes with letters when E0-prefixed.
            16 => Key::MediaPreviousTrack, // Q
            34 => Key::MediaPlayPause,     // G
            25 => Key::MediaNextTrack,     // P
            48 => Key::VolumeUp,           // B
            46 => Key::VolumeDown,         // C
            32 => Key::VolumeMute,         // D

            // Numpad vs the navigation cluster.
            82 => Key::Insert,   // Numpad0
            79 => Key::End,      // Numpad1
            80 => Key::Down,     // Numpad2
            81 => Key::PageDown, // Numpad3
            75 => Key::Left,     // Numpad4
            // Numpad5 has no E0 form (legacy: "5 is not used").
            77 => Key::Right,  // Numpad6
            71 => Key::Home,   // Numpad7
            72 => Key::Up,     // Numpad8
            73 => Key::PageUp, // Numpad9

            55 => Key::PrintScreen, // NumpadAsterisk
            83 => Key::Delete,      // NumpadDelete

            29 => Key::RightControl,  // LeftControl
            56 => Key::RightAlt,      // LeftAlt
            93 => Key::Menu,          // Menu (E0-native, identity-mapped)
            120 => Key::Oem13,        // raw 120, unnamed in the enum
            18 => Key::Oem2,          // E
            23 => Key::Oem3,          // I
            95 => Key::Oem5,          // raw 95
            10 => Key::Oem6,          // Nine
            50 => Key::Oem7,          // M
            91 => Key::LeftWindows,   // LeftWindows (E0-native, identity-mapped)
            110 => Key::Oem4,         // raw 110
            42 => Key::ShiftModifier, // LeftShift (fake-shift prefix around PrintScreen etc.)
            70 => Key::Break,         // ScrollLock
            1 => Key::Oem0,           // Escape
            53 => Key::NumpadDivide,  // ForwardSlashQuestionMark
            92 => Key::RightWindows,  // RightWindows (E0-native, identity-mapped)
            28 => Key::NumpadEnter,   // Enter

            _ => Key::Unknown,
        };
    }

    // Anything else (TERMSRV bits, unexpected combinations): legacy fell through
    // to Unknown.
    Key::Unknown
}

/// Legacy `GetKeyboardKeys` press rule: only plain Down (0x00) and E0|Down
/// (0x02) count as a press. Everything else — including the E1 Pause make —
/// reports as a release, exactly as legacy did (Pause never registered "down").
pub const fn is_down(state: u16) -> bool {
    state == KEY_DOWN || state == (KEY_DOWN | KEY_E0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tricky rows of the legacy table, enumerated make + break.
    #[test]
    fn e0_correction_table_tricky_cases() {
        let cases: &[(u16, Key)] = &[
            // arrows / navigation vs numpad
            (72, Key::Up),
            (80, Key::Down),
            (75, Key::Left),
            (77, Key::Right),
            (71, Key::Home),
            (73, Key::PageUp),
            (79, Key::End),
            (81, Key::PageDown),
            (82, Key::Insert),
            (83, Key::Delete),
            // right-hand modifiers from left-hand codes
            (29, Key::RightControl),
            (56, Key::RightAlt),
            // PrintScreen: E0 + NumpadAsterisk, plus its fake-shift prefix
            (55, Key::PrintScreen),
            (42, Key::ShiftModifier),
            // NumpadEnter and NumpadDivide
            (28, Key::NumpadEnter),
            (53, Key::NumpadDivide),
            // Break (E0 + ScrollLock, i.e. Ctrl+Pause)
            (70, Key::Break),
            // media keys
            (16, Key::MediaPreviousTrack),
            (34, Key::MediaPlayPause),
            (25, Key::MediaNextTrack),
            (48, Key::VolumeUp),
            (46, Key::VolumeDown),
            (32, Key::VolumeMute),
            // OEM oddballs incl. raw codes with no enum name
            (120, Key::Oem13),
            (18, Key::Oem2),
            (23, Key::Oem3),
            (95, Key::Oem5),
            (10, Key::Oem6),
            (50, Key::Oem7),
            (110, Key::Oem4),
            (1, Key::Oem0),
            // E0-native keys that map to themselves
            (91, Key::LeftWindows),
            (92, Key::RightWindows),
            (93, Key::Menu),
        ];
        for &(code, expect) in cases {
            assert_eq!(
                corrected_key(code, KEY_E0 | KEY_DOWN),
                expect,
                "E0 make of code {code}"
            );
            assert_eq!(
                corrected_key(code, KEY_E0 | KEY_UP),
                expect,
                "E0 break of code {code}"
            );
        }
    }

    #[test]
    fn plain_strokes_pass_through_uncorrected() {
        // Non-extended numpad stays numpad; arrows only exist via E0.
        assert_eq!(corrected_key(75, KEY_DOWN), Key::Numpad4);
        assert_eq!(corrected_key(75, KEY_UP), Key::Numpad4);
        assert_eq!(corrected_key(72, KEY_DOWN), Key::Numpad8);
        assert_eq!(corrected_key(29, KEY_DOWN), Key::LeftControl);
        assert_eq!(corrected_key(56, KEY_UP), Key::LeftAlt);
        assert_eq!(corrected_key(28, KEY_DOWN), Key::Enter);
        assert_eq!(corrected_key(1, KEY_UP), Key::Escape);
        assert_eq!(corrected_key(30, KEY_DOWN), Key::A);
    }

    #[test]
    fn pause_is_e1_left_control() {
        assert_eq!(corrected_key(29, KEY_E1), Key::Pause);
        assert_eq!(corrected_key(29, KEY_E1 | KEY_UP), Key::Pause);
        // E1 on any other code was never corrected by legacy -> Unknown.
        assert_eq!(corrected_key(69, KEY_E1), Key::Unknown);
        // Pause's second scancode (0x45 NumLock, plain state) stays NumLock.
        assert_eq!(corrected_key(69, KEY_DOWN), Key::NumLock);
    }

    #[test]
    fn unmatched_extended_and_termsrv_states_are_unknown() {
        assert_eq!(corrected_key(30, KEY_E0 | KEY_DOWN), Key::Unknown); // E0+A
        assert_eq!(corrected_key(76, KEY_E0 | KEY_DOWN), Key::Unknown); // E0+Numpad5
        assert_eq!(corrected_key(30, 0x08), Key::Unknown); // TERMSRV_SET_LED
        assert_eq!(corrected_key(30, 0x20 | KEY_UP), Key::Unknown); // VKPACKET
    }

    #[test]
    fn unnamed_plain_scancode_maps_to_unknown() {
        // 85 is a hole in the legacy enum (documented divergence: legacy kept an
        // undefined enum value; we say Unknown).
        assert_eq!(corrected_key(85, KEY_DOWN), Key::Unknown);
    }

    #[test]
    fn is_down_matches_legacy_rule() {
        assert!(is_down(KEY_DOWN));
        assert!(is_down(KEY_DOWN | KEY_E0));
        assert!(!is_down(KEY_UP));
        assert!(!is_down(KEY_UP | KEY_E0));
        // The E1 Pause make never counts as down — legacy parity.
        assert!(!is_down(KEY_E1));
        assert!(!is_down(KEY_E1 | KEY_UP));
    }
}
