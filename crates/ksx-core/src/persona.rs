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
    /// Sony DualSense (PS5) — **HIDMaestro only** (M8). ViGEmBus has no
    /// DualSense target and never will; the project is frozen.
    ///
    /// Distinct from [`Persona::PlayStation`] on purpose: a PS5-era game or
    /// emulator that sniffs for VID 054C/PID 0CE6 shows DualSense prompts and
    /// enables adaptive-trigger UI, which a DS4 clone does not get. Anyone who
    /// just wants ✕○△□ should stay on `playstation` — it needs no second driver.
    DualSense,
    /// Nintendo Switch Pro Controller — **HIDMaestro only** (M8).
    ///
    /// The persona that makes Switch/Wii U emulators (yuzu/ryujinx lineage,
    /// Cemu, Dolphin) show Nintendo prompts and the B/A swap without a
    /// per-emulator remap.
    SwitchPro,
    /// Xbox Series X|S pad — **HIDMaestro only** (M8), and specifically the
    /// *Bluetooth* profile (`xbox-series-xs-bt`).
    ///
    /// Not a better [`Persona::Xbox360`], and never a replacement for it: slots
    /// 1–4 stay on ViGEm's genuine `xusb22.sys` (see
    /// [`Persona::backend`]). This exists for titles that check for a
    /// current-generation pad, and it is the BT profile rather than a wired one
    /// because WGI/GameInput consumers (every browser gamepad tester, and the
    /// GameInput-era titles behind them) do not route force feedback to the
    /// X360 XUSB companion — they drive the HID output path instead
    /// (`docs/research/padforge-code-audit.md` §3.5).
    XboxSeries,
}

/// Which driver stack materializes a [`Persona`].
///
/// **Derived, never configured.** A ksx config expresses *intent* (`persona =
/// "dualsense"`); the backend is plumbing, and asking a user to name it would
/// let them write a combination that cannot exist (`persona = "xbox360",
/// backend = "hidmaestro"`) and then debug it.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum PadBackend {
    /// ViGEmBus — kernel bus, `xusb22.sys` for X360, plain HID for DS4.
    Vigem,
    /// HIDMaestro — user-mode (UMDF2) HID descriptor emulator. Adds the
    /// personas ViGEmBus cannot express, at the cost of a second driver.
    HidMaestro,
}

impl PadBackend {
    pub const ALL: &'static [PadBackend] = &[PadBackend::Vigem, PadBackend::HidMaestro];

    pub const fn as_str(self) -> &'static str {
        match self {
            PadBackend::Vigem => "vigem",
            PadBackend::HidMaestro => "hidmaestro",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            PadBackend::Vigem => "ViGEmBus",
            PadBackend::HidMaestro => "HIDMaestro",
        }
    }
}

impl fmt::Display for PadBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Persona {
    pub const ALL: &'static [Persona] = &[
        Persona::Xbox360,
        Persona::PlayStation,
        Persona::DualSense,
        Persona::SwitchPro,
        Persona::XboxSeries,
    ];

    /// Canonical name — what [`Persona`] serializes to and what error messages
    /// and `--json` output print. Stable; the aliases are free to grow.
    pub const fn as_str(self) -> &'static str {
        match self {
            Persona::Xbox360 => "xbox360",
            Persona::PlayStation => "playstation",
            Persona::DualSense => "dualsense",
            Persona::SwitchPro => "switchpro",
            Persona::XboxSeries => "xboxseries",
        }
    }

    /// Short label for humans (tray tooltips, `ksx pads` rows).
    pub const fn label(self) -> &'static str {
        match self {
            Persona::Xbox360 => "Xbox 360",
            Persona::PlayStation => "PlayStation",
            Persona::DualSense => "DualSense",
            Persona::SwitchPro => "Switch Pro",
            Persona::XboxSeries => "Xbox Series X|S",
        }
    }

    /// Which driver stack has to materialize this persona.
    ///
    /// **The migration rule, stated as code** (`docs/ENHANCEMENTS.md` E1/E4):
    /// Xbox 360 and PlayStation stay on ViGEmBus *forever*, even once
    /// HIDMaestro is installed and could emulate them. HIDMaestro's XInput is a
    /// synthesis layer — its GIP companion maps a HID device onto an XInput
    /// slot — and that layer is the direct cause of its WGI double-input bug
    /// (`docs/research/padforge-code-audit.md` §3.6). ViGEmBus's X360 target is
    /// the genuine `xusb22.sys` article. Slots 1–4 are the cabinet's whole
    /// reason to exist; they do not move to a synthesis layer to gain nothing.
    pub const fn backend(self) -> PadBackend {
        match self {
            Persona::Xbox360 | Persona::PlayStation => PadBackend::Vigem,
            Persona::DualSense | Persona::SwitchPro | Persona::XboxSeries => PadBackend::HidMaestro,
        }
    }

    /// Whether this persona occupies one of Windows' four XInput slots.
    ///
    /// Drives two things: the `MAX_XINPUT_SLOTS` validation rule, and whether
    /// the output backend bothers running slot correlation (600 ms per pad that
    /// can only ever fail for a HID pad — see `ksx-output/src/vigem.rs`).
    /// `true` for [`Persona::XboxSeries`] as well, and that is deliberate:
    /// HIDMaestro's GIP companion *does* map its Xbox devices onto an XInput
    /// slot (the companion's own watchdog zeroes "XInput state" after >500
    /// unchanged-SeqNo reads — `padforge-code-audit.md` §3.3, which is only
    /// meaningful because the slot exists). So an Xbox Series pad competes for
    /// the same four slots a real one would, and must be counted by the
    /// [`crate::MAX_XINPUT_SLOTS`] rule. Sony and Nintendo personas are plain
    /// HID and consume nothing.
    pub const fn is_xinput(self) -> bool {
        matches!(self, Persona::Xbox360 | Persona::XboxSeries)
    }

    /// Whether the pad reports its D-pad as a 4-bit hat instead of four
    /// independent buttons.
    ///
    /// **The one place a persona is not transparent.** ksx's aggregation can
    /// legitimately produce Up+Down at once (two keys held, or a custom function
    /// spanning both); XInput represents that faithfully, a hat cannot. The
    /// collapse rule is documented and tested in `ksx-output/src/ds4.rs`.
    ///
    /// True for every persona except Xbox 360: the DS4/DualSense/Switch Pro HID
    /// reports all carry a 4-bit hat, and so does the Xbox Series **Bluetooth**
    /// HID report (the one HIDMaestro's `xbox-series-xs-bt` profile presents).
    /// Xbox 360 over XUSB is the outlier that gets four independent bits.
    pub const fn dpad_is_hat(self) -> bool {
        !matches!(self, Persona::Xbox360)
    }

    /// Whether the bus can push rumble/LED state back to us for this persona.
    ///
    /// `false` for PlayStation: ViGEmBus exposes no notification IOCTL for DS4
    /// targets, so there is nothing to subscribe to. Not a gap in ksx — the
    /// driver simply has no such channel, and no lightbar or rumble will ever
    /// arrive on one of these pads.
    ///
    /// `false` for [`Persona::SwitchPro`] too, and that one is an *honest gap*
    /// rather than a driver fact: HIDMaestro delivers output packets for every
    /// device it hosts, but the decode table we hold
    /// (`padforge-code-audit.md` §3.4 — XInput IOCTL, Xbox Series BT short HID,
    /// Xbox legacy HID, Sony `validFlag0`-gated motors, HID-PID) has **no
    /// Switch rumble entry**. Claiming feedback we cannot decode would make
    /// `poll_feedback` a promise of silence. Flip this to `true` in the same
    /// commit that adds the Switch subcommand decode, not before.
    pub const fn has_feedback(self) -> bool {
        matches!(
            self,
            Persona::Xbox360 | Persona::DualSense | Persona::XboxSeries
        )
    }
}

impl fmt::Display for Persona {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPersona(pub String);

// Written by hand rather than derived so the "expected one of" list is
// generated from [`Persona::ALL`]: a new persona can never ship with an error
// message that forgets to mention it.
impl fmt::Display for UnknownPersona {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown persona '{}' (expected one of: ", self.0)?;
        for (i, p) in Persona::ALL.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(p.as_str())?;
        }
        f.write_str(")")
    }
}

impl std::error::Error for UnknownPersona {}

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
            // `ds5`/`ps5` deliberately do NOT fall through to PlayStation: a
            // user who typed a PS5 name and silently got a DS4 would see the
            // wrong prompts and no adaptive triggers, with nothing to explain
            // it. On a machine without HIDMaestro this parses fine and fails
            // later, loudly, naming the missing driver.
            "dualsense" | "ds5" | "ps5" | "dualsense5" => Ok(Persona::DualSense),
            "switchpro" | "switch" | "procontroller" | "switchprocontroller" | "nintendo" => {
                Ok(Persona::SwitchPro)
            }
            // `xboxseriesxsbt` is HIDMaestro's own profile slug, accepted so a
            // slug copied out of the catalog just works.
            "xboxseries" | "xboxseriesx" | "xboxseriess" | "xboxseriesx|s" | "xboxseriesxs"
            | "xboxseriesxsbt" | "series" | "xsx" => Ok(Persona::XboxSeries),
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

    /// The M8 personas, each flag justified in the doc comment it mirrors.
    /// Written to spec (`docs/research/padforge-code-audit.md` §3), NOT yet
    /// measured — HIDMaestro is not installed on the cabinet. Re-run this
    /// against a live install before trusting any of it.
    #[test]
    fn hidmaestro_persona_flags_are_stated_per_device_not_copied() {
        // Sony HID: no XInput slot, hat d-pad, and motors we CAN decode
        // (validFlag0-gated, §3.4).
        assert!(!Persona::DualSense.is_xinput());
        assert!(Persona::DualSense.dpad_is_hat());
        assert!(Persona::DualSense.has_feedback());

        // Switch Pro: the honest gap — no Switch entry in the decode table, so
        // feedback is false until one is written.
        assert!(!Persona::SwitchPro.is_xinput());
        assert!(Persona::SwitchPro.dpad_is_hat());
        assert!(!Persona::SwitchPro.has_feedback());

        // Xbox Series: the GIP companion DOES occupy an XInput slot, so it
        // counts against the four; the BT profile reports a hat.
        assert!(Persona::XboxSeries.is_xinput());
        assert!(Persona::XboxSeries.dpad_is_hat());
        assert!(Persona::XboxSeries.has_feedback());
    }

    #[test]
    fn xbox_series_counts_against_the_four_xinput_slots() {
        // Stated as its own test because it is the one flag that changes a
        // *validation* outcome: five xboxseries slots must be refused exactly
        // like five xbox360 slots, not waved through as "HID, plenty of room".
        let xinput = Persona::ALL.iter().filter(|p| p.is_xinput()).count();
        assert_eq!(xinput, 2, "only the two Xbox personas take an XInput slot");
    }

    #[test]
    fn backend_is_derived_and_slots_1_to_4_never_leave_vigem() {
        // The rule from ENHANCEMENTS E1/E4: the two personas the cabinet
        // actually runs on stay on the kernel bus, whatever else is installed.
        assert_eq!(Persona::Xbox360.backend(), PadBackend::Vigem);
        assert_eq!(Persona::PlayStation.backend(), PadBackend::Vigem);
        // The three that ViGEmBus cannot express at all.
        for p in [Persona::DualSense, Persona::SwitchPro, Persona::XboxSeries] {
            assert_eq!(p.backend(), PadBackend::HidMaestro, "{p}");
        }
        // Every persona resolves to exactly one backend, and both backends are
        // reachable — a persona list that all landed on one stack would mean
        // the routing layer is dead code.
        for &b in PadBackend::ALL {
            assert!(
                Persona::ALL.iter().any(|p| p.backend() == b),
                "no persona routes to {b}"
            );
        }
    }

    #[test]
    fn backend_names_round_trip_for_json() {
        for &b in PadBackend::ALL {
            assert_eq!(b.to_string(), b.as_str());
            assert!(!b.label().is_empty());
        }
        assert_eq!(PadBackend::HidMaestro.as_str(), "hidmaestro");
    }

    #[test]
    fn ps5_names_never_silently_degrade_to_a_ds4() {
        for alias in ["ds5", "PS5", "DualSense", "dual-sense"] {
            assert_eq!(alias.parse(), Ok(Persona::DualSense), "alias {alias}");
        }
        for alias in ["switch", "Switch Pro", "switch_pro", "nintendo"] {
            assert_eq!(alias.parse(), Ok(Persona::SwitchPro), "alias {alias}");
        }
        for alias in ["xboxseries", "Xbox Series X", "xbox-series-xs-bt", "xsx"] {
            assert_eq!(alias.parse(), Ok(Persona::XboxSeries), "alias {alias}");
        }
        // And the pre-M8 aliases still mean what they always meant.
        assert_eq!("ps4".parse(), Ok(Persona::PlayStation));
        assert_eq!("xbox".parse(), Ok(Persona::Xbox360));
    }

    #[test]
    fn display_matches_the_canonical_name() {
        for &p in Persona::ALL {
            assert_eq!(p.to_string(), p.as_str());
            assert!(!p.label().is_empty());
        }
    }
}
