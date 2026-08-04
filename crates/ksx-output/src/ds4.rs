//! `PadState` → DualShock 4 HID report.
//!
//! The Xbox 360 path is a field copy because [`PadState`] *is* the XInput wire
//! shape. This one is a real translation, and three of its rules are places
//! where a persona is not transparent. Each is stated here rather than buried:
//!
//! 1. **Y axes invert.** XInput is up-positive (`+32767` = up); the DS4 HID
//!    report is down-positive (`0x00` = up). Copying the byte through would put
//!    every stick upside down.
//! 2. **The D-pad is a 4-bit hat, not four bits.** Up+Down at once is
//!    representable in XInput and reachable through ksx's aggregation; a hat has
//!    no encoding for it. See [`hat`] for the collapse rule.
//! 3. **Triggers are both analog and digital.** A DS4 carries `trigger_l`/
//!    `trigger_r` *and* L2/R2 button bits; games read whichever they please, so
//!    both must be set consistently.
//!
//! Bit layout source: ViGEmClient's `ViGEm/Common.h` (`DS4_BUTTONS`,
//! `DS4_SPECIAL_BUTTONS`, `DS4_DPAD_DIRECTIONS`). The vendored client exposes
//! `buttons`/`special` as bare integers with no constants, so the values are
//! restated below — and confirmed on the wire by
//! `examples/ds4_report_probe.rs`, which reads a virtual pad's own HID
//! interface back and matched `0x0028` = CROSS | DPAD_NONE byte for byte.

use ksx_core::pad::XButtons;
use ksx_core::PadState;
use vigem_client::DS4Report;

/// `DS4_BUTTONS` — face, shoulder, and stick-click bits. The low nibble is the
/// hat, so nothing here uses bits 0..=3.
mod button {
    pub const SQUARE: u16 = 1 << 4;
    pub const CROSS: u16 = 1 << 5;
    pub const CIRCLE: u16 = 1 << 6;
    pub const TRIANGLE: u16 = 1 << 7;
    pub const SHOULDER_LEFT: u16 = 1 << 8;
    pub const SHOULDER_RIGHT: u16 = 1 << 9;
    pub const TRIGGER_LEFT: u16 = 1 << 10;
    pub const TRIGGER_RIGHT: u16 = 1 << 11;
    pub const SHARE: u16 = 1 << 12;
    pub const OPTIONS: u16 = 1 << 13;
    pub const THUMB_LEFT: u16 = 1 << 14;
    pub const THUMB_RIGHT: u16 = 1 << 15;
}

/// `DS4_SPECIAL_BUTTONS` — a separate byte, not part of `buttons`.
mod special {
    pub const PS: u8 = 1 << 0;
    /// No binding vocabulary maps here yet (a touchpad *click* has no Xbox
    /// equivalent to inherit). Left unset; wiring it needs a new `Binding`
    /// variant, not a change to this table.
    #[allow(dead_code)]
    pub const TOUCHPAD: u8 = 1 << 1;
}

/// `DS4_DPAD_DIRECTIONS` — a 4-bit hat in the low nibble of `buttons`.
/// Clockwise from north; `NONE` is 0x8, which is why a neutral DS4 report has
/// `buttons: 0x8` and not zero.
mod hat {
    pub const NORTH: u16 = 0x0;
    pub const NORTHEAST: u16 = 0x1;
    pub const EAST: u16 = 0x2;
    pub const SOUTHEAST: u16 = 0x3;
    pub const SOUTH: u16 = 0x4;
    pub const SOUTHWEST: u16 = 0x5;
    pub const WEST: u16 = 0x6;
    pub const NORTHWEST: u16 = 0x7;
    pub const NONE: u16 = 0x8;
}

/// XInput's `XINPUT_GAMEPAD_TRIGGER_THRESHOLD`. Above this, the digital L2/R2
/// bit is set alongside the analog value.
///
/// A physical DS4 latches its L2/R2 bits far earlier in the pull, but ksx's
/// bindings are written against XInput semantics: a preset that means "LT is
/// pressed" means it at the threshold games test. Matching XInput here keeps the
/// same preset behaving the same way under either persona. Keyboard-driven
/// triggers are 0 or 255 anyway, so this only matters for future analog sources.
const TRIGGER_THRESHOLD: u8 = 30;

/// Rescales a signed XInput axis to the DS4's unsigned byte.
///
/// `-32768 → 0x00`, `0 → 0x80`, `32767 → 0xFF`. Lossy by construction (16 bits
/// into 8) and exactly lossless for keyboard-driven input, which only ever
/// produces the three extremes.
fn axis(v: i16) -> u8 {
    ((i32::from(v) + 32768) >> 8) as u8
}

/// Rescales *and inverts* — for the Y axes, where the two protocols disagree
/// about which way is up (see module docs).
///
/// Inverts in the signed domain, before rescaling. `255 - axis(v)` looks
/// equivalent and is not: `axis(0)` is `0x80`, so that form maps a centered
/// stick to `0x7F` and every pad would rest one step off center forever. The
/// unsigned range has no exact midpoint; the signed one does, and negation
/// preserves it. `saturating_neg` because `-i16::MIN` overflows — it saturates
/// to `i16::MAX`, i.e. full deflection stays full deflection.
fn axis_inverted(v: i16) -> u8 {
    axis(v.saturating_neg())
}

/// Collapses XInput's four independent D-pad bits into the DS4's 4-bit hat.
///
/// **The collapse rule: opposing pairs cancel.** Up+Down held together yields
/// neither, and likewise Left+Right. This is forced, not chosen — the mapper is
/// a pure function of one [`PadState`] with no history, so "last input wins"
/// (the other common SOCD resolution) is not even expressible here. Neutral is
/// also the safer failure: a fighting game reading a cancelled axis sees the
/// player standing still, where an arbitrary winner would see a direction they
/// never asked for.
///
/// Reachable in practice through ksx's aggregation: two panel buttons bound to
/// Up and Down, or a custom function that spans both. Under an Xbox persona both
/// bits go out and the game decides; under this one, ksx decides. That
/// difference is the single documented exception to "the same preset behaves the
/// same way under any persona".
fn dpad_hat(buttons: XButtons) -> u16 {
    let up = buttons.contains(XButtons::DPAD_UP);
    let down = buttons.contains(XButtons::DPAD_DOWN);
    let left = buttons.contains(XButtons::DPAD_LEFT);
    let right = buttons.contains(XButtons::DPAD_RIGHT);
    // Cancel first, then read off the eight-way direction.
    let (north, south) = (up && !down, down && !up);
    let (west, east) = (left && !right, right && !left);
    match (north, east, south, west) {
        (true, false, false, false) => hat::NORTH,
        (true, true, false, false) => hat::NORTHEAST,
        (false, true, false, false) => hat::EAST,
        (false, true, true, false) => hat::SOUTHEAST,
        (false, false, true, false) => hat::SOUTH,
        (false, false, true, true) => hat::SOUTHWEST,
        (false, false, false, true) => hat::WEST,
        (true, false, false, true) => hat::NORTHWEST,
        _ => hat::NONE,
    }
}

/// `PadState` → [`DS4Report`].
///
/// Total and allocation-free: it runs on the output thread for every delta.
pub fn to_ds4report(state: &PadState) -> DS4Report {
    let b = state.buttons;
    let mut buttons = dpad_hat(b);
    // Face buttons, by position rather than by letter: A (bottom) is ✕, not ○.
    // This is what makes a PS1 emulator print the right glyph.
    for (flag, bit) in [
        (XButtons::A, button::CROSS),
        (XButtons::B, button::CIRCLE),
        (XButtons::X, button::SQUARE),
        (XButtons::Y, button::TRIANGLE),
        (XButtons::LEFT_BUMPER, button::SHOULDER_LEFT),
        (XButtons::RIGHT_BUMPER, button::SHOULDER_RIGHT),
        (XButtons::BACK, button::SHARE),
        (XButtons::START, button::OPTIONS),
        (XButtons::LEFT_THUMB, button::THUMB_LEFT),
        (XButtons::RIGHT_THUMB, button::THUMB_RIGHT),
    ] {
        if b.contains(flag) {
            buttons |= bit;
        }
    }
    // Analog triggers also drive the digital bits (see TRIGGER_THRESHOLD).
    if state.lt > TRIGGER_THRESHOLD {
        buttons |= button::TRIGGER_LEFT;
    }
    if state.rt > TRIGGER_THRESHOLD {
        buttons |= button::TRIGGER_RIGHT;
    }

    DS4Report {
        thumb_lx: axis(state.lx),
        thumb_ly: axis_inverted(state.ly),
        thumb_rx: axis(state.rx),
        thumb_ry: axis_inverted(state.ry),
        buttons,
        // Guide is the PS button; it is the only `special` bit ksx can produce.
        special: if b.contains(XButtons::GUIDE) {
            special::PS
        } else {
            0
        },
        trigger_l: state.lt,
        trigger_r: state.rt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_core::{AXIS_CENTER, AXIS_MAX, AXIS_MIN};

    fn with_buttons(buttons: XButtons) -> DS4Report {
        to_ds4report(&PadState {
            buttons,
            ..PadState::default()
        })
    }

    #[test]
    fn neutral_state_is_the_neutral_report() {
        // The vendored Default is the driver's idea of neutral (sticks centered,
        // hat NONE). Ours must agree exactly, or every pad would start off
        // holding a direction.
        assert_eq!(to_ds4report(&PadState::default()), DS4Report::default());
    }

    #[test]
    fn face_buttons_map_by_position() {
        // The whole point of the persona: A is the bottom button, so it is ✕.
        for (flag, bit) in [
            (XButtons::A, button::CROSS),
            (XButtons::B, button::CIRCLE),
            (XButtons::X, button::SQUARE),
            (XButtons::Y, button::TRIANGLE),
        ] {
            let r = with_buttons(flag);
            assert_eq!(r.buttons, bit | hat::NONE, "{flag:?} → {bit:#06x}");
        }
    }

    #[test]
    fn every_non_dpad_button_maps_to_exactly_one_distinct_bit() {
        let mapped = [
            XButtons::A,
            XButtons::B,
            XButtons::X,
            XButtons::Y,
            XButtons::LEFT_BUMPER,
            XButtons::RIGHT_BUMPER,
            XButtons::BACK,
            XButtons::START,
            XButtons::LEFT_THUMB,
            XButtons::RIGHT_THUMB,
        ];
        let mut seen = 0u16;
        for flag in mapped {
            let bits = with_buttons(flag).buttons & !0xF; // strip the hat nibble
            assert_eq!(bits.count_ones(), 1, "{flag:?} must set exactly one bit");
            assert_eq!(seen & bits, 0, "{flag:?} collides with an earlier button");
            seen |= bits;
        }
        // Guide is the one XInput button that lives outside `buttons`.
        assert_eq!(with_buttons(XButtons::GUIDE).special, special::PS);
        assert_eq!(with_buttons(XButtons::GUIDE).buttons, hat::NONE);
    }

    #[test]
    fn dpad_covers_all_eight_directions() {
        use XButtons as X;
        let cases = [
            (X::DPAD_UP, hat::NORTH),
            (X::DPAD_UP | X::DPAD_RIGHT, hat::NORTHEAST),
            (X::DPAD_RIGHT, hat::EAST),
            (X::DPAD_DOWN | X::DPAD_RIGHT, hat::SOUTHEAST),
            (X::DPAD_DOWN, hat::SOUTH),
            (X::DPAD_DOWN | X::DPAD_LEFT, hat::SOUTHWEST),
            (X::DPAD_LEFT, hat::WEST),
            (X::DPAD_UP | X::DPAD_LEFT, hat::NORTHWEST),
            (X::empty(), hat::NONE),
        ];
        for (flags, want) in cases {
            assert_eq!(with_buttons(flags).buttons & 0xF, want, "{flags:?}");
        }
    }

    #[test]
    fn opposing_dpad_directions_cancel() {
        use XButtons as X;
        // The documented collapse rule, and the one case where this persona
        // behaves differently from Xbox 360 for an identical preset.
        for flags in [
            X::DPAD_UP | X::DPAD_DOWN,
            X::DPAD_LEFT | X::DPAD_RIGHT,
            X::DPAD_UP | X::DPAD_DOWN | X::DPAD_LEFT | X::DPAD_RIGHT,
        ] {
            assert_eq!(with_buttons(flags).buttons & 0xF, hat::NONE, "{flags:?}");
        }
        // A cancelled pair must not smear onto the surviving axis.
        let r = with_buttons(X::DPAD_UP | X::DPAD_DOWN | X::DPAD_RIGHT);
        assert_eq!(r.buttons & 0xF, hat::EAST);
    }

    #[test]
    fn dpad_never_escapes_its_nibble() {
        // Exhaustive over every reachable button mask: the hat must stay in bits
        // 0..=3 and never collide with a face button.
        for raw in 0u16..=0xFFFF {
            let r = with_buttons(XButtons::from_bits_retain(raw));
            assert!(
                r.buttons & 0xF <= 0x8,
                "raw {raw:#06x} produced an invalid hat"
            );
        }
    }

    #[test]
    fn sticks_rescale_and_y_inverts() {
        let r = to_ds4report(&PadState {
            lx: AXIS_MAX,
            ly: AXIS_MAX,
            rx: AXIS_MIN,
            ry: AXIS_MIN,
            ..PadState::default()
        });
        assert_eq!(r.thumb_lx, 0xFF, "X right stays right");
        assert_eq!(r.thumb_ly, 0x00, "XInput up (+) is DS4 0x00");
        assert_eq!(r.thumb_rx, 0x00, "X left stays left");
        assert_eq!(r.thumb_ry, 0xFF, "XInput down (-) is DS4 0xFF");

        let c = to_ds4report(&PadState {
            lx: AXIS_CENTER,
            ly: AXIS_CENTER,
            ..PadState::default()
        });
        assert_eq!((c.thumb_lx, c.thumb_ly), (0x80, 0x80), "center is center");
    }

    #[test]
    fn axis_rescale_is_monotonic_and_total() {
        // No panic and no wraparound anywhere in the i16 range — this runs on
        // the output thread for every delta.
        let mut prev = axis(i16::MIN);
        assert_eq!(prev, 0);
        for v in (i16::MIN as i32 + 1)..=(i16::MAX as i32) {
            let cur = axis(v as i16);
            assert!(cur >= prev, "axis({v}) went backwards");
            prev = cur;
        }
        assert_eq!(prev, 255);
        // The inversion is a mirror image: monotonically decreasing, same ends
        // swapped, and — the part `255 - axis(v)` gets wrong — center-preserving.
        assert_eq!(axis_inverted(i16::MIN), 255);
        assert_eq!(axis_inverted(i16::MAX), 0);
        assert_eq!(axis_inverted(0), axis(0), "center must survive inversion");
        let mut prev = axis_inverted(i16::MIN);
        for v in (i16::MIN as i32 + 1)..=(i16::MAX as i32) {
            let cur = axis_inverted(v as i16);
            assert!(cur <= prev, "axis_inverted({v}) went backwards");
            prev = cur;
        }
    }

    #[test]
    fn triggers_carry_both_analog_and_digital() {
        let full = to_ds4report(&PadState {
            lt: 255,
            rt: 255,
            ..PadState::default()
        });
        assert_eq!((full.trigger_l, full.trigger_r), (255, 255));
        assert_eq!(
            full.buttons,
            button::TRIGGER_LEFT | button::TRIGGER_RIGHT | hat::NONE
        );

        // At or below the XInput threshold the analog value still travels, but
        // the digital bit stays clear — same as a game reading XInput.
        let light = to_ds4report(&PadState {
            lt: TRIGGER_THRESHOLD,
            ..PadState::default()
        });
        assert_eq!(light.trigger_l, TRIGGER_THRESHOLD);
        assert_eq!(light.buttons, hat::NONE);
    }

    #[test]
    fn the_probe_report_still_maps_the_way_the_wire_confirmed_it() {
        // `examples/ds4_report_probe.rs` read 0x0028 and thumb_lx 0xFF off a
        // real virtual pad's HID interface. If this assertion ever breaks, the
        // mapping changed out from under the one measurement that grounds it.
        let r = to_ds4report(&PadState {
            buttons: XButtons::A,
            lx: AXIS_MAX,
            ..PadState::default()
        });
        assert_eq!(r.buttons, 0x0028);
        assert_eq!(r.thumb_lx, 0xFF);
    }
}
