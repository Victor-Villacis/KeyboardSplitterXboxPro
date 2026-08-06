//! Profiles: the descriptor + axis map a virtual device is created from.
//!
//! HIDMaestro embeds 225+ profile JSONs (`LoadDefaultProfiles` returns the
//! count). ksx does not need a catalog browser — it needs exactly three slugs,
//! one per M8 persona, and the rules for handling them.

use ksx_core::Persona;

use crate::axis::{AxisMap, AxisRole, HmAxis};
use crate::error::HmError;
use crate::feedback::SonyFamily;

/// The byte pattern that says a descriptor carries a HID-PID force-feedback
/// block: `Usage(Set Effect Report), Collection(Logical)`.
///
/// Detection matters because of *ordering*, not features: if this is present,
/// the PID pool must be published **before** the device enumerates
/// ([`crate::context`]).
pub const PID_BLOCK_SIGNATURE: [u8; 4] = [0x09, 0x21, 0xA1, 0x02];

/// Does this HID report descriptor declare a PID FFB block?
pub fn has_pid_block(descriptor: &[u8]) -> bool {
    descriptor
        .windows(PID_BLOCK_SIGNATURE.len())
        .any(|w| w == PID_BLOCK_SIGNATURE)
}

/// Transport the profile emulates. Not cosmetic: it selects the feedback
/// layout (Xbox Series BT short form vs legacy HID) and it is why the Xbox slug
/// below is the BT one.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Bluetooth,
}

/// One HIDMaestro profile, reduced to what a submit path needs.
#[derive(Clone, Debug)]
pub struct HmProfile {
    /// Catalog slug, e.g. `xbox-series-xs-bt`.
    pub slug: String,
    pub vid: u16,
    pub pid: u16,
    pub transport: Transport,
    /// The captured HID report descriptor. **Empty means not deployable** —
    /// HIDMaestro's own catalog flags these, and `CreateController` throws on
    /// them, so ksx refuses them one step earlier with a name attached.
    pub descriptor: Vec<u8>,
    /// Role → HID usage. The only routing table; see [`crate::axis`].
    pub axis_map: AxisMap,
    /// Sony report family, when this is a Sony pad — selects the feedback gate.
    pub sony_family: Option<SonyFamily>,
}

impl HmProfile {
    pub fn is_deployable(&self) -> bool {
        !self.descriptor.is_empty()
    }

    pub fn has_pid_block(&self) -> bool {
        has_pid_block(&self.descriptor)
    }

    /// Rejects a profile ksx cannot drive correctly, with the reason named.
    ///
    /// Two failure modes, both fatal at *create* time rather than at submit
    /// time: no descriptor (the device cannot exist) and an incomplete axis map
    /// (some role would be silently unrouted — the class of bug this crate is
    /// built to prevent, so it is refused rather than degraded).
    pub fn validate(&self) -> Result<(), HmError> {
        if !self.is_deployable() {
            return Err(HmError::ProfileNotDeployable(self.slug.clone()));
        }
        if let Some(missing) = self.axis_map.missing().next() {
            return Err(HmError::UnknownProfile(format!(
                "{} declares no axis for role '{}'",
                self.slug,
                missing.name()
            )));
        }
        Ok(())
    }
}

/// The catalog slug ksx asks for, per persona.
///
/// **`xbox-series-xs-bt`, not a wired Xbox profile**, and that is a deliberate
/// choice inherited from PadForge (`padforge-code-audit.md` §3.5): WGI /
/// GameInput consumers do not route force feedback to the X360 XUSB companion,
/// so a wired profile vibrates in nothing that browsers or GameInput-era titles
/// drive. The Series BT profile takes the HID output path, which they do drive.
///
/// Returns `None` for the personas that never reach this backend
/// ([`Persona::backend`] routes them to ViGEmBus) — a HIDMaestro Xbox 360 pad
/// would be a synthesis layer replacing the genuine article.
pub const fn slug_for(persona: Persona) -> Option<&'static str> {
    match persona {
        Persona::DualSense => Some("dualsense"),
        Persona::SwitchPro => Some("switch-pro"),
        Persona::XboxSeries => Some("xbox-series-xs-bt"),
        Persona::Xbox360 | Persona::PlayStation => None,
    }
}

/// A minimal descriptor stub used when the real catalog is unavailable, so the
/// *shape* of a profile is testable. Carries a PID block so PID-ordering logic
/// has something to trigger on.
#[doc(hidden)]
pub fn stub_descriptor(with_pid: bool) -> Vec<u8> {
    let mut d = vec![0x05, 0x01, 0x09, 0x05, 0xA1, 0x01];
    if with_pid {
        d.extend_from_slice(&PID_BLOCK_SIGNATURE);
    }
    d.push(0xC0);
    d
}

/// The three profiles ksx asks HIDMaestro for, described from the documented
/// facts. **The `descriptor` here is a stub**: real descriptors come from the
/// installed catalog via `LoadDefaultProfiles`. VID/PIDs are the real vendor
/// ids; the axis maps are the documented conventions.
pub fn expected_profile(persona: Persona) -> Option<HmProfile> {
    let slug = slug_for(persona)?;
    let (vid, pid, transport, axis_map, sony_family) = match persona {
        Persona::DualSense => (
            0x054C,
            0x0CE6,
            Transport::Usb,
            AxisMap::sony_convention(),
            Some(SonyFamily::Ds5),
        ),
        Persona::SwitchPro => (
            0x057E,
            0x2009,
            Transport::Usb,
            // Switch Pro reports sticks on X/Y and Rx/Ry and has no analog
            // triggers; ZL/ZR are digital. They are still routed as roles so a
            // preset's LT/RT reach the digital bits through the threshold in
            // `state::buttons` rather than through a positional guess.
            {
                let mut m = AxisMap::new();
                m.set(AxisRole::LeftStickX, HmAxis::X)
                    .set(AxisRole::LeftStickY, HmAxis::Y)
                    .set(AxisRole::RightStickX, HmAxis::RX)
                    .set(AxisRole::RightStickY, HmAxis::RY)
                    .set(AxisRole::LeftTrigger, HmAxis::Z)
                    .set(AxisRole::RightTrigger, HmAxis::RZ);
                m
            },
            None,
        ),
        Persona::XboxSeries => (
            0x045E,
            0x0B13,
            Transport::Bluetooth,
            AxisMap::xinput_convention(),
            None,
        ),
        Persona::Xbox360 | Persona::PlayStation => return None,
    };
    Some(HmProfile {
        slug: slug.to_string(),
        vid,
        pid,
        transport,
        descriptor: stub_descriptor(true),
        axis_map,
        sony_family,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_block_detection_matches_the_documented_signature() {
        assert_eq!(PID_BLOCK_SIGNATURE, [0x09, 0x21, 0xA1, 0x02]);
        assert!(has_pid_block(&stub_descriptor(true)));
        assert!(!has_pid_block(&stub_descriptor(false)));
        // It must be a subsequence search, not a prefix check.
        let mut buried = vec![0u8; 300];
        buried[200..204].copy_from_slice(&PID_BLOCK_SIGNATURE);
        assert!(has_pid_block(&buried));
        // ...and a near miss must not fire.
        assert!(!has_pid_block(&[0x09, 0x21, 0xA1, 0x01]));
        assert!(!has_pid_block(&[]));
    }

    #[test]
    fn only_the_hidmaestro_personas_have_slugs() {
        assert_eq!(slug_for(Persona::XboxSeries), Some("xbox-series-xs-bt"));
        assert_eq!(slug_for(Persona::DualSense), Some("dualsense"));
        assert_eq!(slug_for(Persona::SwitchPro), Some("switch-pro"));
        // The two ViGEm personas must NOT be reachable here: a HIDMaestro X360
        // pad is the synthesis layer we deliberately refuse.
        assert_eq!(slug_for(Persona::Xbox360), None);
        assert_eq!(slug_for(Persona::PlayStation), None);
        assert!(expected_profile(Persona::Xbox360).is_none());
    }

    #[test]
    fn the_xbox_slug_is_the_bluetooth_profile_on_purpose() {
        let p = expected_profile(Persona::XboxSeries).unwrap();
        assert_eq!(p.transport, Transport::Bluetooth);
        assert!(
            p.slug.ends_with("-bt"),
            "a wired Xbox profile gets no FFB from WGI/GameInput consumers"
        );
    }

    #[test]
    fn every_shipped_profile_validates() {
        for persona in [Persona::DualSense, Persona::SwitchPro, Persona::XboxSeries] {
            let p = expected_profile(persona).expect("slug exists");
            p.validate().unwrap_or_else(|e| panic!("{persona}: {e}"));
            assert!(p.axis_map.is_complete());
            assert!(p.has_pid_block());
        }
        // Sony family is set exactly where the Sony feedback gate applies.
        assert_eq!(
            expected_profile(Persona::DualSense).unwrap().sony_family,
            Some(SonyFamily::Ds5)
        );
        assert!(expected_profile(Persona::XboxSeries)
            .unwrap()
            .sony_family
            .is_none());
    }

    #[test]
    fn an_undeployable_profile_is_refused_by_name() {
        let mut p = expected_profile(Persona::DualSense).unwrap();
        p.descriptor.clear();
        let err = p.validate().unwrap_err();
        assert!(matches!(err, HmError::ProfileNotDeployable(s) if s == "dualsense"));
    }

    #[test]
    fn an_incomplete_axis_map_is_refused_at_create_time_not_at_submit_time() {
        let mut p = expected_profile(Persona::DualSense).unwrap();
        p.axis_map = AxisMap::new();
        let err = p.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("leftStickX"),
            "{msg} must name the missing role"
        );
    }
}
