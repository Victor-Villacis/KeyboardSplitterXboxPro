//! XInput-shaped pad state and the legacy `VirtualXbox` ID tables.
//!
//! The numeric ids are contractual: legacy preset XML stores them
//! (`legacy/VirtualXbox/Enums/*.cs`), and [`PadState`] is kept in XInput wire
//! shape (== `vigem_client::XGamepad`) so no output backend needs a translation
//! layer.

use bitflags::bitflags;

/// Legacy `XboxAxisPosition.Min` — full negative deflection, **-32767 and not
/// `i16::MIN`**.
///
/// # Why one LSB is given away
///
/// `i16::MIN` is the one value in the range with no positive twin: `-(-32768)`
/// and `(-32768).abs()` are unrepresentable, and `-32768 / 32767.0` is
/// -1.00003, outside the [-1.0, 1.0] every normalizing consumer assumes. It is
/// a legal XInput value (Microsoft documents the range as -32768..=32767) and
/// most consumers survive it — but the ones that do not fail in a way that is
/// invisible from this side. Firefox shipped exactly this bug: its XInput
/// backend divided by 32767, produced -1.00003, and broke SDL2/Emscripten
/// games until it was fixed with a sign-dependent divisor
/// (Mozilla bug 1606562).
///
/// -32767 is 0.003% less deflection — below the resolution of any stick, any
/// game's deadzone, and any human — and it removes that entire class of
/// consumer bug for free. There is no configuration in which ksx's own "min"
/// hands a game a value only careful code survives.
///
/// A preset may still name the raw value literally (`lx.-32768`); [`safe_axis`]
/// is the wire guarantee that covers that case.
pub const AXIS_MIN: i16 = -32767;
/// Legacy `XboxAxisPosition.Center`.
pub const AXIS_CENTER: i16 = 0;
/// Legacy `XboxAxisPosition.Max`.
pub const AXIS_MAX: i16 = i16::MAX;

/// An axis value as it may go on the wire: [`i16::MIN`] folded to [`AXIS_MIN`],
/// everything else untouched.
///
/// [`AXIS_MIN`] explains why. This is the same rule applied at the *boundary*
/// rather than at authoring time, so a hand-written literal, a legacy import, or
/// a future binding source cannot route around it. Backends call it once, on the
/// way out; nothing in the engine needs to know.
pub const fn safe_axis(value: i16) -> i16 {
    if value == i16::MIN {
        AXIS_MIN
    } else {
        value
    }
}

bitflags! {
    /// XInput `wButtons` wire bits.
    ///
    /// The named button bits match legacy `XboxButton` exactly. The four
    /// `DPAD_*` bits exist because the XInput wire format carries the dpad in
    /// `wButtons`; legacy modeled the dpad separately (`XboxDpadDirection`)
    /// with these same bit values.
    #[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
    pub struct XButtons: u16 {
        const DPAD_UP = 0x0001;
        const DPAD_DOWN = 0x0002;
        const DPAD_LEFT = 0x0004;
        const DPAD_RIGHT = 0x0008;
        const START = 0x0010;
        const BACK = 0x0020;
        const LEFT_THUMB = 0x0040;
        const RIGHT_THUMB = 0x0080;
        const LEFT_BUMPER = 0x0100;
        const RIGHT_BUMPER = 0x0200;
        const GUIDE = 0x0400;
        const A = 0x1000;
        const B = 0x2000;
        const X = 0x4000;
        const Y = 0x8000;
    }
}

/// A single xbox button — legacy `XboxButton` (`VirtualXbox/Enums/XboxButton.cs`).
#[repr(u16)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum XButton {
    Start = 0x0010,
    Back = 0x0020,
    LeftThumb = 0x0040,
    RightThumb = 0x0080,
    LeftBumper = 0x0100,
    RightBumper = 0x0200,
    Guide = 0x0400,
    A = 0x1000,
    B = 0x2000,
    X = 0x4000,
    Y = 0x8000,
}

impl XButton {
    pub const ALL: &'static [XButton] = &[
        XButton::Start,
        XButton::Back,
        XButton::LeftThumb,
        XButton::RightThumb,
        XButton::LeftBumper,
        XButton::RightBumper,
        XButton::Guide,
        XButton::A,
        XButton::B,
        XButton::X,
        XButton::Y,
    ];

    /// Legacy `XboxButton` numeric id (as stored in preset XML `<button id>`).
    pub const fn legacy_id(self) -> u32 {
        self as u16 as u32
    }

    pub fn from_legacy_id(id: u32) -> Option<Self> {
        match id {
            0x0010 => Some(XButton::Start),
            0x0020 => Some(XButton::Back),
            0x0040 => Some(XButton::LeftThumb),
            0x0080 => Some(XButton::RightThumb),
            0x0100 => Some(XButton::LeftBumper),
            0x0200 => Some(XButton::RightBumper),
            0x0400 => Some(XButton::Guide),
            0x1000 => Some(XButton::A),
            0x2000 => Some(XButton::B),
            0x4000 => Some(XButton::X),
            0x8000 => Some(XButton::Y),
            _ => Option::None,
        }
    }

    /// The wire bit for this button.
    pub const fn flag(self) -> XButtons {
        XButtons::from_bits_truncate(self as u16)
    }
}

/// Legacy `XboxTrigger` (`Left = 0x10000`, `Right = 0x20000`).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trigger {
    Left,
    Right,
}

impl Trigger {
    pub const ALL: &'static [Trigger] = &[Trigger::Left, Trigger::Right];

    /// Legacy `XboxTrigger` numeric id (as stored in preset XML `<trigger id>`).
    pub const fn legacy_id(self) -> u32 {
        match self {
            Trigger::Left => 0x10000,
            Trigger::Right => 0x20000,
        }
    }

    pub fn from_legacy_id(id: u32) -> Option<Self> {
        match id {
            0x10000 => Some(Trigger::Left),
            0x20000 => Some(Trigger::Right),
            _ => Option::None,
        }
    }
}

/// Legacy `XboxAxis` (`X = 1`, `Y = 2`, `Rx = 4`, `Ry = 8`).
#[repr(u32)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    X = 1,
    Y = 2,
    Rx = 4,
    Ry = 8,
}

impl Axis {
    pub const ALL: &'static [Axis] = &[Axis::X, Axis::Y, Axis::Rx, Axis::Ry];

    /// Legacy `XboxAxis` numeric id (as stored in preset XML `<axis id>`).
    pub const fn legacy_id(self) -> u32 {
        self as u32
    }

    pub fn from_legacy_id(id: u32) -> Option<Self> {
        match id {
            1 => Some(Axis::X),
            2 => Some(Axis::Y),
            4 => Some(Axis::Rx),
            8 => Some(Axis::Ry),
            _ => Option::None,
        }
    }
}

/// Legacy `XboxDpadDirection` (`Up = 1`, `Down = 2`, `Left = 4`, `Right = 8`).
///
/// Legacy also defines `Off = 0`; it is deliberately absent here — a binding
/// always names a concrete direction, and "no direction" is the absence of
/// pressed dpad bits in [`XButtons`].
#[repr(i32)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum DpadDirection {
    Up = 1,
    Down = 2,
    Left = 4,
    Right = 8,
}

impl DpadDirection {
    pub const ALL: &'static [DpadDirection] = &[
        DpadDirection::Up,
        DpadDirection::Down,
        DpadDirection::Left,
        DpadDirection::Right,
    ];

    /// Legacy `XboxDpadDirection` numeric id (preset XML `<dpad direction>`).
    pub const fn legacy_id(self) -> i32 {
        self as i32
    }

    pub fn from_legacy_id(id: i32) -> Option<Self> {
        match id {
            1 => Some(DpadDirection::Up),
            2 => Some(DpadDirection::Down),
            4 => Some(DpadDirection::Left),
            8 => Some(DpadDirection::Right),
            _ => Option::None,
        }
    }

    /// The `wButtons` wire bit for this direction (numerically identical to the
    /// legacy id).
    pub const fn flag(self) -> XButtons {
        XButtons::from_bits_truncate(self as i32 as u16)
    }
}

/// Virtual pad state in XInput wire shape (== `XINPUT_GAMEPAD` /
/// `vigem_client::XGamepad`): no output backend needs a translation layer.
///
/// `Default` is the all-neutral state: no buttons, triggers at 0, sticks
/// centered.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub struct PadState {
    pub buttons: XButtons,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_bits_match_legacy_xbox_button() {
        // Bit-for-bit against legacy/VirtualXbox/Enums/XboxButton.cs.
        assert_eq!(XButton::Start.legacy_id(), 0x0010);
        assert_eq!(XButton::Back.legacy_id(), 0x0020);
        assert_eq!(XButton::LeftThumb.legacy_id(), 0x0040);
        assert_eq!(XButton::RightThumb.legacy_id(), 0x0080);
        assert_eq!(XButton::LeftBumper.legacy_id(), 0x0100);
        assert_eq!(XButton::RightBumper.legacy_id(), 0x0200);
        assert_eq!(XButton::Guide.legacy_id(), 0x0400);
        assert_eq!(XButton::A.legacy_id(), 0x1000);
        assert_eq!(XButton::B.legacy_id(), 0x2000);
        assert_eq!(XButton::X.legacy_id(), 0x4000);
        assert_eq!(XButton::Y.legacy_id(), 0x8000);

        assert_eq!(XButtons::START.bits(), 0x0010);
        assert_eq!(XButtons::BACK.bits(), 0x0020);
        assert_eq!(XButtons::LEFT_THUMB.bits(), 0x0040);
        assert_eq!(XButtons::RIGHT_THUMB.bits(), 0x0080);
        assert_eq!(XButtons::LEFT_BUMPER.bits(), 0x0100);
        assert_eq!(XButtons::RIGHT_BUMPER.bits(), 0x0200);
        assert_eq!(XButtons::GUIDE.bits(), 0x0400);
        assert_eq!(XButtons::A.bits(), 0x1000);
        assert_eq!(XButtons::B.bits(), 0x2000);
        assert_eq!(XButtons::X.bits(), 0x4000);
        assert_eq!(XButtons::Y.bits(), 0x8000);
    }

    #[test]
    fn fixture_button_ids_resolve() {
        // Decimal ids as they appear in splitter_presets.xml.
        assert_eq!(XButton::from_legacy_id(4096), Some(XButton::A));
        assert_eq!(XButton::from_legacy_id(8192), Some(XButton::B));
        assert_eq!(XButton::from_legacy_id(16384), Some(XButton::X));
        assert_eq!(XButton::from_legacy_id(32768), Some(XButton::Y));
        assert_eq!(XButton::from_legacy_id(256), Some(XButton::LeftBumper));
        assert_eq!(XButton::from_legacy_id(512), Some(XButton::RightBumper));
        assert_eq!(XButton::from_legacy_id(32), Some(XButton::Back));
        assert_eq!(XButton::from_legacy_id(16), Some(XButton::Start));
        assert_eq!(XButton::from_legacy_id(64), Some(XButton::LeftThumb));
        assert_eq!(XButton::from_legacy_id(128), Some(XButton::RightThumb));
        assert_eq!(XButton::from_legacy_id(1024), Some(XButton::Guide));
        assert_eq!(XButton::from_legacy_id(0), None);
        assert_eq!(XButton::from_legacy_id(0x30), None); // combined flags rejected
    }

    #[test]
    fn trigger_ids_match_legacy() {
        assert_eq!(Trigger::Left.legacy_id(), 0x10000);
        assert_eq!(Trigger::Right.legacy_id(), 0x20000);
        // Decimal ids as they appear in splitter_presets.xml.
        assert_eq!(Trigger::from_legacy_id(65536), Some(Trigger::Left));
        assert_eq!(Trigger::from_legacy_id(131072), Some(Trigger::Right));
        assert_eq!(Trigger::from_legacy_id(3), None);
    }

    #[test]
    fn axis_ids_match_legacy() {
        assert_eq!(Axis::X.legacy_id(), 1);
        assert_eq!(Axis::Y.legacy_id(), 2);
        assert_eq!(Axis::Rx.legacy_id(), 4);
        assert_eq!(Axis::Ry.legacy_id(), 8);
        for axis in Axis::ALL {
            assert_eq!(Axis::from_legacy_id(axis.legacy_id()), Some(*axis));
        }
        assert_eq!(Axis::from_legacy_id(3), None);
    }

    #[test]
    fn axis_positions_match_legacy() {
        // Legacy's Min was i16::MIN; ksx's is one LSB in from it, on purpose —
        // see AXIS_MIN. Everything else is bit-identical.
        assert_eq!(AXIS_MIN, -32767);
        assert_eq!(AXIS_CENTER, 0);
        assert_eq!(AXIS_MAX, 32767);
    }

    /// The property the whole `-32767` decision rests on: full deflection is
    /// SYMMETRIC, so every arithmetic a consumer might do on it is total.
    #[test]
    fn full_deflection_is_symmetric_and_survives_consumer_arithmetic() {
        assert_eq!(AXIS_MIN, -AXIS_MAX);
        // Negation and abs are the two operations i16::MIN has no answer for.
        assert_eq!(AXIS_MIN.checked_neg(), Some(AXIS_MAX));
        assert_eq!(AXIS_MIN.checked_abs(), Some(AXIS_MAX));
        assert_eq!(i16::MIN.checked_neg(), None);
        assert_eq!(i16::MIN.checked_abs(), None);
        // ...and normalizing by 32767 — what a game does before it computes an
        // angle — stays inside the range every consumer assumes.
        assert!((f64::from(AXIS_MIN) / f64::from(AXIS_MAX)) >= -1.0);
        assert!((f64::from(i16::MIN) / f64::from(AXIS_MAX)) < -1.0);
    }

    /// The wire guarantee, which covers what authoring cannot: a literal.
    #[test]
    fn safe_axis_folds_only_the_one_dangerous_value() {
        assert_eq!(safe_axis(i16::MIN), AXIS_MIN);
        assert_eq!(safe_axis(AXIS_MIN), AXIS_MIN);
        assert_eq!(safe_axis(AXIS_MAX), AXIS_MAX);
        assert_eq!(safe_axis(0), 0);
        assert_eq!(safe_axis(-1), -1);
        assert_eq!(safe_axis(-32767), -32767);
    }

    #[test]
    fn dpad_ids_match_legacy_and_wire_bits() {
        // Verified against legacy/VirtualXbox/Enums/XboxDpadDirection.cs:
        // Off=0 (not a binding), Up=1, Down=2, Left=4, Right=8.
        assert_eq!(DpadDirection::Up.legacy_id(), 1);
        assert_eq!(DpadDirection::Down.legacy_id(), 2);
        assert_eq!(DpadDirection::Left.legacy_id(), 4);
        assert_eq!(DpadDirection::Right.legacy_id(), 8);
        for dir in DpadDirection::ALL {
            assert_eq!(DpadDirection::from_legacy_id(dir.legacy_id()), Some(*dir));
            // XInput wire dpad bits coincide with the legacy ids.
            assert_eq!(dir.flag().bits(), dir.legacy_id() as u16);
        }
        assert_eq!(DpadDirection::from_legacy_id(0), None);
        assert_eq!(DpadDirection::from_legacy_id(3), None);
    }

    #[test]
    fn button_flag_round_trip() {
        for button in XButton::ALL {
            assert_eq!(button.flag().bits() as u32, button.legacy_id());
            assert_eq!(XButton::from_legacy_id(button.legacy_id()), Some(*button));
        }
    }

    #[test]
    fn pad_state_default_is_neutral() {
        let state = PadState::default();
        assert_eq!(state.buttons, XButtons::empty());
        assert_eq!(state.lt, 0);
        assert_eq!(state.rt, 0);
        assert_eq!(state.lx, 0);
        assert_eq!(state.ly, 0);
        assert_eq!(state.rx, 0);
        assert_eq!(state.ry, 0);
    }
}
