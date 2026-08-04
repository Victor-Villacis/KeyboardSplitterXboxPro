//! Which *kind* of controller a slot presents itself as.
//!
//! A persona changes only the shape of the device Windows sees. It does not
//! change capture, presets, or bindings: the wire vocabulary stays Xbox-flavored
//! everywhere (`A`, `B`, `LeftBumper`), and ✕○△□ are display aliases, not a
//! second binding language. Re-persona-ing a slot must never require editing its
//! preset — the one documented exception is the D-pad, see [`Persona::dpad_is_hat`].
//!
//! Why it exists (measured, `docs/research/m6.5-ds4-findings.md`):
//! - **Correct prompts.** A PS1/PS2/PS3 emulator reading a PlayStation pad shows
//!   ✕ instead of A. That is the whole point for Victor's cabinet.
//! - **Players 5+.** Windows exposes exactly four XInput slots and no virtual bus
//!   can create a fifth. PlayStation targets are plain HID, so they neither
//!   consume nor compete for those four — six were enumerated on the cabinet
//!   while four X360 pads already held every slot.

use std::fmt;
use std::str::FromStr;

/// The controller a slot presents itself as.
///
/// Serialized by its canonical [`Persona::as_str`]; parsed leniently by
/// [`FromStr`], which accepts the aliases people actually type.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Persona {
    /// Xbox 360 wired pad — a genuine XInput device via Microsoft's own
    /// `xusb22.sys`. The most compatible gamepad persona ever made (every
    /// XInput title, 2006→today), which is why it is the default and why slots
    /// 1–4 should stay on it.
    #[default]
    Xbox360,
    /// Sony DualShock 4 wired pad — plain HID/DirectInput. Read by MAME,
    /// RetroArch, SDL, and Steam Input; *not* by XInput-only games.
    ///
    /// Named for the family, not the model: the wire shape is a DS4 because
    /// that is what ViGEmBus emulates, but users configure `persona =
    /// "playstation"` and are not asked to care which Sony pad it clones.
    PlayStation,
}

impl Persona {
    pub const ALL: &'static [Persona] = &[Persona::Xbox360, Persona::PlayStation];

    /// Canonical name — what [`Persona`] serializes to and what error messages
    /// and `--json` output print. Stable; the aliases are free to grow.
    pub const fn as_str(self) -> &'static str {
        match self {
            Persona::Xbox360 => "xbox360",
            Persona::PlayStation => "playstation",
        }
    }

    /// Short label for humans (tray tooltips, `ksx pads` rows).
    pub const fn label(self) -> &'static str {
        match self {
            Persona::Xbox360 => "Xbox 360",
            Persona::PlayStation => "PlayStation",
        }
    }

    /// Whether this persona occupies one of Windows' four XInput slots.
    ///
    /// Drives two things: the `MAX_XINPUT_SLOTS` validation rule, and whether
    /// the output backend bothers running slot correlation (600 ms per pad that
    /// can only ever fail for a HID pad — see `ksx-output/src/vigem.rs`).
    pub const fn is_xinput(self) -> bool {
        matches!(self, Persona::Xbox360)
    }

    /// Whether the pad reports its D-pad as a 4-bit hat instead of four
    /// independent buttons.
    ///
    /// **The one place a persona is not transparent.** ksx's aggregation can
    /// legitimately produce Up+Down at once (two keys held, or a custom function
    /// spanning both); XInput represents that faithfully, a hat cannot. The
    /// collapse rule is documented and tested in `ksx-output/src/ds4.rs`.
    pub const fn dpad_is_hat(self) -> bool {
        matches!(self, Persona::PlayStation)
    }

    /// Whether the bus can push rumble/LED state back to us for this persona.
    ///
    /// `false` for PlayStation: ViGEmBus exposes no notification IOCTL for DS4
    /// targets, so there is nothing to subscribe to. Not a gap in ksx — the
    /// driver simply has no such channel, and no lightbar or rumble will ever
    /// arrive on one of these pads.
    pub const fn has_feedback(self) -> bool {
        matches!(self, Persona::Xbox360)
    }
}

impl fmt::Display for Persona {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown persona '{0}' (expected one of: xbox360, playstation)")]
pub struct UnknownPersona(pub String);

impl FromStr for Persona {
    type Err = UnknownPersona;

    /// Case- and separator-insensitive, and generous with aliases: config files
    /// are hand-edited, so `ds4`, `PS4`, and `Xbox 360` all have to work.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect();
        match normalized.as_str() {
            "xbox360" | "x360" | "xbox" | "360" | "xinput" => Ok(Persona::Xbox360),
            "playstation" | "ps" | "ds4" | "ps4" | "dualshock" | "dualshock4" | "sony" => {
                Ok(Persona::PlayStation)
            }
            _ => Err(UnknownPersona(s.to_owned())),
        }
    }
}

// No serde impls here on purpose: ksx-core stays dependency-free and its types
// cross the config boundary by name (same rule as `Key::from_name`). The TOML
// glue lives in `ksx-config::persona_serde`, which is a thin wrapper over
// [`Persona::as_str`] and [`FromStr`] above.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_xbox360() {
        // Every config written before personas existed must keep behaving
        // exactly as it did; that is what makes `#[serde(default)]` safe.
        assert_eq!(Persona::default(), Persona::Xbox360);
    }

    #[test]
    fn canonical_names_round_trip() {
        for &p in Persona::ALL {
            assert_eq!(p.as_str().parse::<Persona>(), Ok(p), "{p} must round-trip");
        }
    }

    #[test]
    fn aliases_parse() {
        for alias in [
            "ds4",
            "DS4",
            "ps4",
            "PS4",
            "dualshock4",
            "Dual-Shock 4",
            "sony",
        ] {
            assert_eq!(alias.parse(), Ok(Persona::PlayStation), "alias {alias}");
        }
        for alias in ["xbox360", "Xbox 360", "x360", "XBOX_360", "xinput"] {
            assert_eq!(alias.parse(), Ok(Persona::Xbox360), "alias {alias}");
        }
    }

    #[test]
    fn unknown_persona_names_the_valid_ones() {
        let err = "gamecube".parse::<Persona>().unwrap_err();
        assert_eq!(err, UnknownPersona("gamecube".to_owned()));
        let msg = err.to_string();
        for &p in Persona::ALL {
            assert!(msg.contains(p.as_str()), "{msg} should list {p}");
        }
    }

    #[test]
    fn capability_flags_match_the_measured_driver_behavior() {
        assert!(Persona::Xbox360.is_xinput());
        assert!(Persona::Xbox360.has_feedback());
        assert!(!Persona::Xbox360.dpad_is_hat());
        // The three facts that make PlayStation slots different, all measured
        // on ViGEmBus 1.21.442.0 (docs/research/m6.5-ds4-findings.md).
        assert!(!Persona::PlayStation.is_xinput());
        assert!(!Persona::PlayStation.has_feedback());
        assert!(Persona::PlayStation.dpad_is_hat());
    }

    #[test]
    fn display_matches_the_canonical_name() {
        for &p in Persona::ALL {
            assert_eq!(p.to_string(), p.as_str());
            assert!(!p.label().is_empty());
        }
    }
}
