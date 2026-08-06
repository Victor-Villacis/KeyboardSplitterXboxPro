//! Axes as HID usages, and the profile-declared map from *role* to usage.
//!
//! **The bug this module exists to not repeat** (`padforge-code-audit.md`
//! §3.3): HIDMaestro axes are HID usages, not positions in an "XInput-standard"
//! axis list. Sony profiles declare `Z → rightStickX` and `Rx → leftTrigger`,
//! the exact inverse of the XInput convention (`Rx/Ry → right stick`,
//! `Z/Rz → triggers`). PadForge routed positionally at first, so a centered
//! right stick (0.5) landed on the *trigger* byte; the OS read 0x80, decided
//! L2/R2 were half-pulled, and auto-asserted the coupled digital buttons. Every
//! PlayStation output had a phantom 50% trigger pull that no amount of
//! deadzone tuning could remove.
//!
//! So: routing goes through [`AxisMap`], which is loaded from the profile.
//! There is deliberately no "default positional map" in this crate to fall back
//! on — a profile that declares no map is an error, not a guess.

use std::fmt;

/// A HID usage as HIDMaestro keys it: `page << 8 | usage`.
///
/// e.g. Generic Desktop (page 0x01) usage Z (0x32) is `0x0132`.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct HmAxis(pub u16);

impl HmAxis {
    /// Generic Desktop page — the page every gamepad axis lives on, and the one
    /// a 2-digit profile key is promoted to.
    pub const GENERIC_DESKTOP: u8 = 0x01;

    pub const X: Self = Self(0x0130);
    pub const Y: Self = Self(0x0131);
    pub const Z: Self = Self(0x0132);
    pub const RX: Self = Self(0x0133);
    pub const RY: Self = Self(0x0134);
    pub const RZ: Self = Self(0x0135);
    pub const SLIDER: Self = Self(0x0136);
    pub const DIAL: Self = Self(0x0137);

    pub const fn new(page: u8, usage: u8) -> Self {
        Self((page as u16) << 8 | usage as u16)
    }

    pub const fn page(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn usage(self) -> u8 {
        self.0 as u8
    }

    /// Parses a profile `AxisMap` key.
    ///
    /// Keys are hex; **2-digit keys are promoted to page 1** (documented
    /// behavior — the catalog writes bare Generic Desktop usages like `"32"`).
    /// 3- and 4-digit keys carry their own page. A `0x` prefix is tolerated
    /// because hand-written profiles have it.
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        let key = key
            .strip_prefix("0x")
            .or(key.strip_prefix("0X"))
            .unwrap_or(key);
        if key.is_empty() || key.len() > 4 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let raw = u16::from_str_radix(key, 16).ok()?;
        Some(if key.len() <= 2 {
            Self::new(Self::GENERIC_DESKTOP, raw as u8)
        } else {
            Self(raw)
        })
    }
}

impl fmt::Display for HmAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            HmAxis::X => "X",
            HmAxis::Y => "Y",
            HmAxis::Z => "Z",
            HmAxis::RX => "Rx",
            HmAxis::RY => "Ry",
            HmAxis::RZ => "Rz",
            HmAxis::SLIDER => "Slider",
            HmAxis::DIAL => "Dial",
            _ => return write!(f, "usage {:#06x}", self.0),
        };
        f.write_str(name)
    }
}

/// The six analog roles ksx knows how to drive, named the way a *profile* names
/// them — never "axis 0", "axis 1".
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum AxisRole {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

impl AxisRole {
    pub const ALL: [AxisRole; 6] = [
        AxisRole::LeftStickX,
        AxisRole::LeftStickY,
        AxisRole::RightStickX,
        AxisRole::RightStickY,
        AxisRole::LeftTrigger,
        AxisRole::RightTrigger,
    ];
    pub const COUNT: usize = Self::ALL.len();

    /// The profile's own spelling for this role.
    pub const fn name(self) -> &'static str {
        match self {
            AxisRole::LeftStickX => "leftStickX",
            AxisRole::LeftStickY => "leftStickY",
            AxisRole::RightStickX => "rightStickX",
            AxisRole::RightStickY => "rightStickY",
            AxisRole::LeftTrigger => "leftTrigger",
            AxisRole::RightTrigger => "rightTrigger",
        }
    }

    /// Case- and separator-insensitive parse of a profile role name
    /// (`"leftStickX"`, `"left_stick_x"`, `"LeftStickX"` all work).
    pub fn from_name(name: &str) -> Option<Self> {
        let normalized: String = name
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect();
        Self::ALL
            .into_iter()
            .find(|r| r.name().eq_ignore_ascii_case(&normalized))
    }

    const fn index(self) -> usize {
        self as usize
    }

    /// Whether this role's neutral value is 0.0 (a trigger) or 0.5 (a stick).
    ///
    /// The distinction that turned the positional-routing mistake into a
    /// *visible* bug rather than a subtle one.
    pub const fn is_trigger(self) -> bool {
        matches!(self, AxisRole::LeftTrigger | AxisRole::RightTrigger)
    }

    /// Neutral value in HIDMaestro's normalized `[0,1]` domain.
    pub const fn neutral(self) -> f32 {
        if self.is_trigger() {
            0.0
        } else {
            0.5
        }
    }
}

/// A profile's role → HID-usage routing table.
///
/// Fixed-size and `Copy`: built once at profile load, then read on the submit
/// path with no allocation and no hashing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AxisMap {
    roles: [Option<HmAxis>; AxisRole::COUNT],
    /// Second key each trigger must ALSO be written to — see [`AxisMap::mirror`].
    mirrors: [Option<HmAxis>; 2],
}

impl AxisMap {
    pub const fn new() -> Self {
        Self {
            roles: [None; AxisRole::COUNT],
            mirrors: [None; 2],
        }
    }

    /// Declares where a role's value goes on the wire.
    pub fn set(&mut self, role: AxisRole, axis: HmAxis) -> &mut Self {
        self.roles[role.index()] = Some(axis);
        self
    }

    /// Declares a trigger's **mirror** key.
    ///
    /// Trigger writes must land on two keys: the canonical usage the descriptor
    /// declares, and the trigger row's own wire-field key. HIDMaestro's HID and
    /// GIP/XUSB lanes disagreed across SDK generations about which of the two
    /// they read, and a one-sided write pinned XInput/WGI triggers at 50%
    /// forever (HIDMaestro discussion #130, via `padforge-code-audit.md` §3.3).
    /// Writing both is always safe; writing one is a coin flip on the consumer.
    ///
    /// Roles other than the two triggers are ignored — mirroring a stick has no
    /// meaning and silently doubling stick writes would be a real bug.
    pub fn set_mirror(&mut self, role: AxisRole, axis: HmAxis) -> &mut Self {
        match role {
            AxisRole::LeftTrigger => self.mirrors[0] = Some(axis),
            AxisRole::RightTrigger => self.mirrors[1] = Some(axis),
            _ => {}
        }
        self
    }

    pub fn axis(&self, role: AxisRole) -> Option<HmAxis> {
        self.roles[role.index()]
    }

    /// The trigger mirror key for `role`, if the profile declared one and it is
    /// not the same key as the canonical one (writing the same key twice is a
    /// no-op worth skipping on the hot path).
    pub fn mirror(&self, role: AxisRole) -> Option<HmAxis> {
        let mirror = match role {
            AxisRole::LeftTrigger => self.mirrors[0],
            AxisRole::RightTrigger => self.mirrors[1],
            _ => None,
        }?;
        (self.axis(role) != Some(mirror)).then_some(mirror)
    }

    /// Roles the profile did not route. A submit against an incomplete map
    /// leaves those axes untouched rather than guessing a position.
    pub fn missing(&self) -> impl Iterator<Item = AxisRole> + '_ {
        AxisRole::ALL
            .into_iter()
            .filter(|r| self.roles[r.index()].is_none())
    }

    pub fn is_complete(&self) -> bool {
        self.missing().next().is_none()
    }

    /// The map an XInput-convention device declares. Xbox profiles only —
    /// naming it `xinput_convention` rather than `default` is the point: it is
    /// one profile's declaration, not a fallback for profiles that lack one.
    pub fn xinput_convention() -> Self {
        let mut m = Self::new();
        m.set(AxisRole::LeftStickX, HmAxis::X)
            .set(AxisRole::LeftStickY, HmAxis::Y)
            .set(AxisRole::RightStickX, HmAxis::RX)
            .set(AxisRole::RightStickY, HmAxis::RY)
            .set(AxisRole::LeftTrigger, HmAxis::Z)
            .set(AxisRole::RightTrigger, HmAxis::RZ);
        m
    }

    /// The map Sony profiles declare — the inverse assignment that broke
    /// positional routing. Kept next to [`AxisMap::xinput_convention`] so the
    /// difference is one screen apart and impossible to miss.
    pub fn sony_convention() -> Self {
        let mut m = Self::new();
        m.set(AxisRole::LeftStickX, HmAxis::X)
            .set(AxisRole::LeftStickY, HmAxis::Y)
            .set(AxisRole::RightStickX, HmAxis::Z)
            .set(AxisRole::RightStickY, HmAxis::RZ)
            .set(AxisRole::LeftTrigger, HmAxis::RX)
            .set(AxisRole::RightTrigger, HmAxis::RY);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_packing_matches_the_documented_encoding() {
        // The one worked example in the audit: Z is 0x0132.
        assert_eq!(HmAxis::Z.0, 0x0132);
        assert_eq!(HmAxis::Z.page(), HmAxis::GENERIC_DESKTOP);
        assert_eq!(HmAxis::Z.usage(), 0x32);
        assert_eq!(HmAxis::new(0x01, 0x32), HmAxis::Z);
    }

    #[test]
    fn two_digit_profile_keys_are_promoted_to_page_one() {
        assert_eq!(HmAxis::from_key("32"), Some(HmAxis::Z));
        assert_eq!(HmAxis::from_key("0x32"), Some(HmAxis::Z));
        assert_eq!(HmAxis::from_key("30"), Some(HmAxis::X));
        // 3- and 4-digit keys carry their own page and are NOT promoted.
        assert_eq!(HmAxis::from_key("0132"), Some(HmAxis::Z));
        assert_eq!(HmAxis::from_key("132"), Some(HmAxis::Z));
        assert_eq!(HmAxis::from_key("0232"), Some(HmAxis(0x0232)));
        assert_eq!(HmAxis::from_key(""), None);
        assert_eq!(HmAxis::from_key("nope"), None);
        assert_eq!(HmAxis::from_key("00132"), None);
    }

    #[test]
    fn role_names_round_trip_case_and_separator_insensitively() {
        for role in AxisRole::ALL {
            assert_eq!(AxisRole::from_name(role.name()), Some(role));
        }
        assert_eq!(
            AxisRole::from_name("left_stick_x"),
            Some(AxisRole::LeftStickX)
        );
        assert_eq!(
            AxisRole::from_name("Right Trigger"),
            Some(AxisRole::RightTrigger)
        );
        assert_eq!(AxisRole::from_name("axis0"), None);
    }

    /// The regression this crate is shaped around: the same *role* resolves to
    /// different *usages* under the two conventions, so a positional router
    /// would put a centered stick (0.5) on a trigger key.
    #[test]
    fn sony_and_xinput_conventions_disagree_exactly_where_the_bug_was() {
        let x = AxisMap::xinput_convention();
        let sony = AxisMap::sony_convention();

        // Sticks agree on the left, disagree on the right.
        assert_eq!(
            x.axis(AxisRole::LeftStickX),
            sony.axis(AxisRole::LeftStickX)
        );
        assert_eq!(x.axis(AxisRole::RightStickX), Some(HmAxis::RX));
        assert_eq!(sony.axis(AxisRole::RightStickX), Some(HmAxis::Z));

        // And the collision itself: under XInput, Z is the LEFT TRIGGER; under
        // Sony it is the RIGHT STICK X. Route positionally and a resting stick
        // writes 0.5 to a trigger byte -> 0x80 -> phantom half-pull.
        assert_eq!(x.axis(AxisRole::LeftTrigger), Some(HmAxis::Z));
        assert_eq!(sony.axis(AxisRole::RightStickX), Some(HmAxis::Z));
        assert_eq!(AxisRole::RightStickX.neutral(), 0.5);
        assert_eq!(AxisRole::LeftTrigger.neutral(), 0.0);

        assert!(x.is_complete() && sony.is_complete());
    }

    #[test]
    fn an_incomplete_map_names_what_is_missing_instead_of_guessing() {
        let mut m = AxisMap::new();
        m.set(AxisRole::LeftStickX, HmAxis::X);
        assert!(!m.is_complete());
        let missing: Vec<_> = m.missing().collect();
        assert_eq!(missing.len(), AxisRole::COUNT - 1);
        assert!(!missing.contains(&AxisRole::LeftStickX));
        // No fallback exists: an unrouted role is None, never "axis N".
        assert_eq!(m.axis(AxisRole::RightTrigger), None);
    }

    #[test]
    fn mirrors_are_trigger_only_and_never_duplicate_the_canonical_key() {
        let mut m = AxisMap::sony_convention();
        m.set_mirror(AxisRole::LeftTrigger, HmAxis::SLIDER);
        assert_eq!(m.mirror(AxisRole::LeftTrigger), Some(HmAxis::SLIDER));
        // Same key as canonical -> nothing to mirror (skip the second write).
        m.set_mirror(AxisRole::RightTrigger, HmAxis::RY);
        assert_eq!(m.axis(AxisRole::RightTrigger), Some(HmAxis::RY));
        assert_eq!(m.mirror(AxisRole::RightTrigger), None);
        // Sticks cannot be mirrored.
        m.set_mirror(AxisRole::LeftStickX, HmAxis::DIAL);
        assert_eq!(m.mirror(AxisRole::LeftStickX), None);
    }
}
