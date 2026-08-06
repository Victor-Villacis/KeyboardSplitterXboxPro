//! `HMGamepadState` — the frame published into the shared latch, and the
//! `PadState` → frame routing.
//!
//! Two properties this module owes the submit path, both enforced by its shape
//! rather than by discipline:
//!
//! 1. **No allocation.** Axes live in a fixed inline array, not a map. The
//!    upstream C# surface is a `Dictionary<HMAxis, float>`; at 1 kHz × 8 pads
//!    that is 8000 hash lookups and a rehash risk per second for at most six
//!    live entries, so this is a linear scan over an array instead.
//! 2. **No positional routing.** The only way to set an analog value is through
//!    an [`AxisMap`] role (see [`route`]); `set_axis` takes a [`HmAxis`], never
//!    an index.

use ksx_core::pad::XButtons;
use ksx_core::PadState;

use crate::axis::{AxisMap, AxisRole, HmAxis};

/// Inline axis capacity. Six analog roles + two trigger mirrors = 8 in the
/// worst profile we know of; 12 leaves room for gyro/touch extensions without
/// making the frame cache-hostile.
pub const MAX_AXES: usize = 12;

/// Buttons as HIDMaestro's `[Flags] uint`: bits 0..=12 are the named face /
/// shoulder / stick / system buttons, and bits beyond map to descriptor button
/// positions, so a 128-button profile rides the same mask.
///
/// Named per the audit's "bits 0..12 named A..Share" ordering
/// (`padforge-code-audit.md` §3.1). **Written to spec, not verified against a
/// live driver** — if a real install shows a different order, this table is the
/// single place to fix it.
pub mod button {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LEFT_SHOULDER: u32 = 1 << 4;
    pub const RIGHT_SHOULDER: u32 = 1 << 5;
    pub const LEFT_TRIGGER: u32 = 1 << 6;
    pub const RIGHT_TRIGGER: u32 = 1 << 7;
    pub const BACK: u32 = 1 << 8;
    pub const START: u32 = 1 << 9;
    pub const LEFT_THUMB: u32 = 1 << 10;
    pub const RIGHT_THUMB: u32 = 1 << 11;
    pub const GUIDE: u32 = 1 << 12;
}

/// The 8-way hat, plus centered.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
#[repr(u8)]
pub enum HmHat {
    North = 0,
    NorthEast = 1,
    East = 2,
    SouthEast = 3,
    South = 4,
    SouthWest = 5,
    West = 6,
    NorthWest = 7,
    #[default]
    Centered = 8,
}

impl HmHat {
    /// Collapses XInput's four independent D-pad bits onto the hat.
    ///
    /// **Opposing pairs cancel**, identically to the ViGEm DS4 mapper
    /// (`ksx-output/src/ds4.rs`) and for the same reason: this is a pure
    /// function of one frame with no history, so "last input wins" is not
    /// expressible here, and neutral is the safer failure. Keeping the two
    /// mappers' rules identical is what makes `dpad_is_hat` a single documented
    /// exception rather than a per-backend surprise.
    pub fn from_xinput(buttons: XButtons) -> Self {
        let up = buttons.contains(XButtons::DPAD_UP);
        let down = buttons.contains(XButtons::DPAD_DOWN);
        let left = buttons.contains(XButtons::DPAD_LEFT);
        let right = buttons.contains(XButtons::DPAD_RIGHT);
        let vert = i8::from(up && !down) - i8::from(down && !up);
        let horiz = i8::from(right && !left) - i8::from(left && !right);
        match (vert, horiz) {
            (1, 0) => HmHat::North,
            (1, 1) => HmHat::NorthEast,
            (0, 1) => HmHat::East,
            (-1, 1) => HmHat::SouthEast,
            (-1, 0) => HmHat::South,
            (-1, -1) => HmHat::SouthWest,
            (0, -1) => HmHat::West,
            (1, -1) => HmHat::NorthWest,
            _ => HmHat::Centered,
        }
    }
}

/// One published frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HmGamepadState {
    axes: [(HmAxis, f32); MAX_AXES],
    axis_len: u8,
    pub buttons: u32,
    pub hat: HmHat,
}

impl Default for HmGamepadState {
    fn default() -> Self {
        Self {
            axes: [(HmAxis(0), 0.0); MAX_AXES],
            axis_len: 0,
            buttons: 0,
            hat: HmHat::Centered,
        }
    }
}

/// Fixed wire size of an encoded frame: buttons(4) + hat(1) + len(1) + pad(2)
/// + MAX_AXES × (usage u16 + value f32).
pub const FRAME_BYTES: usize = 4 + 1 + 1 + 2 + MAX_AXES * 6;

impl HmGamepadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of live axis entries.
    pub fn axis_count(&self) -> usize {
        self.axis_len as usize
    }

    /// Sets (or overwrites) one axis. Values are clamped into HIDMaestro's
    /// normalized `[0,1]` domain; NaN becomes the neutral 0.5 rather than
    /// reaching the wire, because a NaN axis byte is a stuck controller.
    ///
    /// Returns `false` if the frame is full — the caller's map declared more
    /// axes than [`MAX_AXES`], which is a profile bug, not a runtime condition.
    pub fn set_axis(&mut self, axis: HmAxis, value: f32) -> bool {
        let value = if value.is_nan() {
            0.5
        } else {
            value.clamp(0.0, 1.0)
        };
        for slot in &mut self.axes[..self.axis_len as usize] {
            if slot.0 == axis {
                slot.1 = value;
                return true;
            }
        }
        if self.axis_len as usize == MAX_AXES {
            return false;
        }
        self.axes[self.axis_len as usize] = (axis, value);
        self.axis_len += 1;
        true
    }

    pub fn axis(&self, axis: HmAxis) -> Option<f32> {
        self.axes[..self.axis_len as usize]
            .iter()
            .find(|(a, _)| *a == axis)
            .map(|(_, v)| *v)
    }

    /// Live axis entries, in insertion order.
    pub fn axes(&self) -> &[(HmAxis, f32)] {
        &self.axes[..self.axis_len as usize]
    }

    /// Serializes into the latch payload. Fixed length, little-endian, no
    /// allocation; `out` must be at least [`FRAME_BYTES`].
    ///
    /// This framing is **ours**: HIDMaestro's SDK is in-process C# and the
    /// byte layout of its shared section is not published in the material we
    /// hold, so the field set is taken from the documented `HMGamepadState`
    /// surface and packed here. Anything that turns out to disagree with a real
    /// driver is a change to this one function.
    pub fn encode(&self, out: &mut [u8]) -> usize {
        assert!(out.len() >= FRAME_BYTES, "frame buffer too small");
        out[0..4].copy_from_slice(&self.buttons.to_le_bytes());
        out[4] = self.hat as u8;
        out[5] = self.axis_len;
        out[6..8].copy_from_slice(&[0, 0]);
        let mut at = 8;
        for (axis, value) in self.axes() {
            out[at..at + 2].copy_from_slice(&axis.0.to_le_bytes());
            out[at + 2..at + 6].copy_from_slice(&value.to_le_bytes());
            at += 6;
        }
        // Zero the tail so two frames with equal content encode to equal bytes —
        // the idle-dedup comparison in `keepalive` is a byte compare.
        out[at..FRAME_BYTES].fill(0);
        FRAME_BYTES
    }
}

/// Half of the symmetric XInput range: `AXIS_MIN..=i16::MAX` is
/// `-32767..=32767`, so 65534 steps span the full `[0,1]` output.
const AXIS_SPAN: f32 = 65534.0;

/// XInput signed axis → HIDMaestro normalized `[0,1]`, center **exactly** 0.5.
///
/// Scaled over the *symmetric* range, not over `i16`'s asymmetric one. Dividing
/// by 65535 after adding 32768 (the obvious form) puts a centered stick at
/// `0.500008`: harmless-looking, and then a game with a 0.5-centered deadzone
/// reads a permanent sub-deadzone drift. `ksx_core::safe_axis` folds `i16::MIN`
/// to `AXIS_MIN` first, which is what makes the range symmetric — the same fold
/// `ksx-output/src/vigem.rs` applies for the same reason.
fn stick(v: i16) -> f32 {
    0.5 + f32::from(ksx_core::safe_axis(v)) / AXIS_SPAN
}

/// Same, inverted: **HID convention is Y+ = down**, XInput is Y+ = up
/// (`padforge-code-audit.md` §3.1), so every Y axis flips.
///
/// Subtracting rather than negating-then-scaling: because [`stick`] is
/// symmetric about 0.5, `0.5 - x` is exact at the center and at both extremes,
/// where the equivalent trick on an *asymmetric* scale (`ds4.rs`'s `255 - byte`)
/// would rest every pad one step off center.
fn stick_inverted(v: i16) -> f32 {
    0.5 - f32::from(ksx_core::safe_axis(v)) / AXIS_SPAN
}

fn trigger(v: u8) -> f32 {
    f32::from(v) / 255.0
}

/// Routes a ksx [`PadState`] onto the wire **by role**, through the profile's
/// [`AxisMap`].
///
/// Every analog value goes: role → `map.axis(role)` → `set_axis`. Nothing here
/// can address an axis by position, which is the whole point (see the module
/// docs of [`crate::axis`]). Triggers are additionally written to their mirror
/// key when the profile declares one.
///
/// Roles the map does not route are skipped silently at this level — the map is
/// validated once at profile load (`AxisMap::is_complete`), not re-litigated at
/// 1 kHz.
pub fn route(map: &AxisMap, state: &PadState, out: &mut HmGamepadState) {
    let value_of = |role: AxisRole| match role {
        AxisRole::LeftStickX => stick(state.lx),
        AxisRole::LeftStickY => stick_inverted(state.ly),
        AxisRole::RightStickX => stick(state.rx),
        AxisRole::RightStickY => stick_inverted(state.ry),
        AxisRole::LeftTrigger => trigger(state.lt),
        AxisRole::RightTrigger => trigger(state.rt),
    };

    for role in AxisRole::ALL {
        let Some(axis) = map.axis(role) else { continue };
        let value = value_of(role);
        out.set_axis(axis, value);
        if let Some(mirror) = map.mirror(role) {
            out.set_axis(mirror, value);
        }
    }

    out.buttons = buttons(state.buttons, state.lt, state.rt);
    out.hat = HmHat::from_xinput(state.buttons);
}

/// XInput's `XINPUT_GAMEPAD_TRIGGER_THRESHOLD`. Same value and same reasoning as
/// `ksx-output/src/ds4.rs`: ksx presets are written against XInput semantics, so
/// "LT is pressed" must latch where XInput games test it, whatever persona is
/// mounted.
const TRIGGER_THRESHOLD: u8 = 30;

fn buttons(x: XButtons, lt: u8, rt: u8) -> u32 {
    let mut out = 0u32;
    let mut bit = |flag: XButtons, wire: u32| {
        if x.contains(flag) {
            out |= wire;
        }
    };
    bit(XButtons::A, button::A);
    bit(XButtons::B, button::B);
    bit(XButtons::X, button::X);
    bit(XButtons::Y, button::Y);
    bit(XButtons::LEFT_BUMPER, button::LEFT_SHOULDER);
    bit(XButtons::RIGHT_BUMPER, button::RIGHT_SHOULDER);
    bit(XButtons::BACK, button::BACK);
    bit(XButtons::START, button::START);
    bit(XButtons::LEFT_THUMB, button::LEFT_THUMB);
    bit(XButtons::RIGHT_THUMB, button::RIGHT_THUMB);
    bit(XButtons::GUIDE, button::GUIDE);
    // The D-pad is NOT in the button mask: it rides the hat. Setting both would
    // give a Sony/Nintendo consumer two conflicting sources for one direction.
    if lt >= TRIGGER_THRESHOLD {
        out |= button::LEFT_TRIGGER;
    }
    if rt >= TRIGGER_THRESHOLD {
        out |= button::RIGHT_TRIGGER;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routed(map: &AxisMap, state: &PadState) -> HmGamepadState {
        let mut out = HmGamepadState::new();
        route(map, state, &mut out);
        out
    }

    /// The headline regression test: the SAME neutral pad, routed through the
    /// two conventions, must put 0.5 on different usages — and must never put
    /// 0.5 on whatever that profile calls a trigger.
    #[test]
    fn routing_by_name_never_lands_a_centered_stick_on_a_trigger() {
        for map in [AxisMap::xinput_convention(), AxisMap::sony_convention()] {
            let frame = routed(&map, &PadState::default());
            for role in AxisRole::ALL {
                let axis = map.axis(role).expect("complete map");
                let value = frame.axis(axis).expect("routed");
                assert_eq!(
                    value,
                    role.neutral(),
                    "{role:?} on {axis} must rest at its own neutral"
                );
            }
            // Explicitly: no trigger key carries 0.5 — the phantom half-pull.
            for role in [AxisRole::LeftTrigger, AxisRole::RightTrigger] {
                assert_eq!(frame.axis(map.axis(role).unwrap()), Some(0.0));
            }
        }
    }

    /// The counterfactual, so the test above is not vacuous: a positional
    /// router (roles assigned to a fixed usage order) *does* reproduce the bug.
    #[test]
    fn positional_routing_would_have_reproduced_the_phantom_trigger() {
        const POSITIONAL: [HmAxis; 6] = [
            HmAxis::X,
            HmAxis::Y,
            HmAxis::RX,
            HmAxis::RY,
            HmAxis::Z,
            HmAxis::RZ,
        ];
        let sony = AxisMap::sony_convention();
        // What a positional writer would put on each wire key...
        let mut positional = HmGamepadState::new();
        for (role, axis) in AxisRole::ALL.into_iter().zip(POSITIONAL) {
            positional.set_axis(axis, role.neutral());
        }
        // ...vs what Sony's map says those keys mean. Rx is Sony's LEFT
        // TRIGGER, and the positional writer just put a stick's 0.5 on it.
        assert_eq!(sony.axis(AxisRole::LeftTrigger), Some(HmAxis::RX));
        assert_eq!(positional.axis(HmAxis::RX), Some(0.5));
        // By-name routing puts 0.0 there instead.
        assert_eq!(
            routed(&sony, &PadState::default()).axis(HmAxis::RX),
            Some(0.0)
        );
    }

    #[test]
    fn triggers_are_written_to_both_the_canonical_and_the_mirror_key() {
        let mut map = AxisMap::xinput_convention();
        map.set_mirror(AxisRole::LeftTrigger, HmAxis::SLIDER);
        map.set_mirror(AxisRole::RightTrigger, HmAxis::DIAL);
        let state = PadState {
            lt: 255,
            rt: 128,
            ..PadState::default()
        };
        let frame = routed(&map, &state);
        assert_eq!(frame.axis(HmAxis::Z), Some(1.0));
        assert_eq!(frame.axis(HmAxis::SLIDER), Some(1.0), "LT mirror");
        assert_eq!(frame.axis(HmAxis::RZ), frame.axis(HmAxis::DIAL));
        assert_eq!(frame.axis(HmAxis::DIAL), Some(128.0 / 255.0), "RT mirror");
    }

    #[test]
    fn y_axes_invert_and_center_stays_exactly_centered() {
        let up = PadState {
            ly: i16::MAX,
            ry: i16::MAX,
            ..PadState::default()
        };
        let map = AxisMap::xinput_convention();
        let frame = routed(&map, &up);
        // XInput up (+32767) is HID 0.0 (Y+ = down), exactly.
        assert_eq!(frame.axis(HmAxis::Y), Some(0.0));
        // And the center must be dead center, not off by one step — a
        // 0.500008 rest position is a permanent sub-deadzone drift.
        let center = routed(&map, &PadState::default());
        assert_eq!(center.axis(HmAxis::Y), Some(0.5));
        assert_eq!(center.axis(HmAxis::X), Some(0.5));
        // i16::MIN is folded to AXIS_MIN, so full deflection stays full
        // deflection instead of overflowing or clipping asymmetrically.
        let down = PadState {
            ly: i16::MIN,
            lx: i16::MIN,
            ..PadState::default()
        };
        let frame = routed(&map, &down);
        assert_eq!(frame.axis(HmAxis::Y), Some(1.0));
        assert_eq!(frame.axis(HmAxis::X), Some(0.0));
    }

    #[test]
    fn the_dpad_rides_the_hat_and_opposing_pairs_cancel() {
        let hat = |b: XButtons| {
            routed(
                &AxisMap::xinput_convention(),
                &PadState {
                    buttons: b,
                    ..PadState::default()
                },
            )
            .hat
        };
        assert_eq!(hat(XButtons::DPAD_UP), HmHat::North);
        assert_eq!(
            hat(XButtons::DPAD_UP | XButtons::DPAD_RIGHT),
            HmHat::NorthEast
        );
        assert_eq!(hat(XButtons::empty()), HmHat::Centered);
        // The collapse rule, identical to ds4.rs.
        assert_eq!(
            hat(XButtons::DPAD_UP | XButtons::DPAD_DOWN),
            HmHat::Centered
        );
        assert_eq!(
            hat(XButtons::DPAD_LEFT | XButtons::DPAD_RIGHT),
            HmHat::Centered
        );
        assert_eq!(
            hat(XButtons::DPAD_UP | XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT),
            HmHat::East
        );
        // And no D-pad bit leaks into the button mask.
        let frame = routed(
            &AxisMap::xinput_convention(),
            &PadState {
                buttons: XButtons::DPAD_UP | XButtons::A,
                ..PadState::default()
            },
        );
        assert_eq!(frame.buttons, button::A);
    }

    #[test]
    fn triggers_set_their_digital_bit_at_the_xinput_threshold() {
        let bits = |lt, rt| {
            routed(
                &AxisMap::xinput_convention(),
                &PadState {
                    lt,
                    rt,
                    ..PadState::default()
                },
            )
            .buttons
        };
        assert_eq!(bits(29, 29), 0);
        assert_eq!(bits(30, 0), button::LEFT_TRIGGER);
        assert_eq!(bits(0, 255), button::RIGHT_TRIGGER);
    }

    #[test]
    fn axis_writes_are_idempotent_and_bounded() {
        let mut frame = HmGamepadState::new();
        assert!(frame.set_axis(HmAxis::X, 0.25));
        assert!(frame.set_axis(HmAxis::X, 0.75));
        assert_eq!(frame.axis_count(), 1, "overwrite, not append");
        assert_eq!(frame.axis(HmAxis::X), Some(0.75));
        // Out-of-domain values are clamped, NaN becomes neutral.
        frame.set_axis(HmAxis::Y, 5.0);
        frame.set_axis(HmAxis::Z, -1.0);
        frame.set_axis(HmAxis::RX, f32::NAN);
        assert_eq!(frame.axis(HmAxis::Y), Some(1.0));
        assert_eq!(frame.axis(HmAxis::Z), Some(0.0));
        assert_eq!(frame.axis(HmAxis::RX), Some(0.5));
        // Capacity is a hard edge, not a panic.
        let mut full = HmGamepadState::new();
        for i in 0..MAX_AXES {
            assert!(full.set_axis(HmAxis(i as u16), 0.5));
        }
        assert!(!full.set_axis(HmAxis(999), 0.5));
    }

    #[test]
    fn equal_frames_encode_to_equal_bytes() {
        // The property idle-dedup rests on: no uninitialized tail, no ordering
        // wobble, so "unchanged" can be a byte compare.
        let map = AxisMap::sony_convention();
        let state = PadState {
            lt: 200,
            lx: -4000,
            buttons: XButtons::A | XButtons::DPAD_LEFT,
            ..PadState::default()
        };
        let mut a = [0u8; FRAME_BYTES];
        let mut b = [0xFFu8; FRAME_BYTES];
        assert_eq!(routed(&map, &state).encode(&mut a), FRAME_BYTES);
        assert_eq!(routed(&map, &state).encode(&mut b), FRAME_BYTES);
        assert_eq!(a, b);
        // A different frame must differ.
        let mut c = [0u8; FRAME_BYTES];
        routed(&map, &PadState { lt: 201, ..state }).encode(&mut c);
        assert_ne!(a, c);
    }
}
