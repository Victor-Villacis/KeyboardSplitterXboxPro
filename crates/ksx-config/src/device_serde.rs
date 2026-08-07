//! TOML/JSON glue for [`ksx_core::DeviceRef`] — a `[[device]] id`.
//!
//! ksx-core carries no serde dependency, so the wire form lives here (same rule
//! as [`crate::persona_serde`]).
//!
//! # The one thing this module exists to guarantee
//!
//! **The raw string goes back to disk, byte for byte.**
//! [`ksx_core::DeviceSelector::parse`] uppercases legacy instance paths and
//! canonicalises `usb:` spellings, so serializing the *parsed* value would
//! rewrite every user's config file just by loading it. The cabinet's live
//! config holds
//!
//! ```text
//! id = 'USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000'
//! ```
//!
//! and a load/save cycle has to return those exact bytes (`ksx_config` pins
//! byte-identical round-trips, and `docs/DEVICE-IDENTITY.md` §5 makes it a
//! rule). [`serialize`] therefore writes [`ksx_core::DeviceRef::raw`] and
//! nothing else.
//!
//! # And why a bad value is a load error
//!
//! [`ksx_core::DeviceSelector::parse`] is deliberately lenient — any spelling
//! containing `\` is accepted as an opaque id, because ksx's own setup wizard
//! writes ACPI paths for laptop keyboards. What it refuses is a value with no
//! `\` and no known prefix, which cannot name a device on Windows at all. That
//! is a typo, and a named parse error at load beats a literal that silently
//! matches nothing at 2 a.m. with the panel dead.

use ksx_core::DeviceRef;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(id: &DeviceRef, s: S) -> Result<S::Ok, S::Error> {
    // NEVER `id.selector()`: see the module docs.
    s.serialize_str(id.raw())
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DeviceRef, D::Error> {
    let raw = String::deserialize(d)?;
    DeviceRef::parse(&raw).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Holder {
        #[serde(with = "super")]
        id: DeviceRef,
    }

    /// **The cabinet's live config, as bytes.** Value equality would prove
    /// nothing about what lands on disk, so this asserts the TEXT.
    #[test]
    fn a_legacy_instance_path_is_written_back_character_for_character() {
        const LINE: &str = "id = 'USB\\VID_D209&PID_0430&MI_00\\7&25EEA38C&0&0000'\n";
        let h: Holder = toml::from_str(LINE).unwrap();
        assert_eq!(toml::to_string(&h).unwrap(), LINE);
    }

    /// The failure this module was written to prevent, stated as a test: a
    /// lower-case path parses to an UPPERCASED selector, and writing that back
    /// would edit a file nobody asked to edit.
    #[test]
    fn the_case_the_selector_would_have_folded_survives_a_round_trip() {
        let lower = r"usb\vid_d209&pid_0430&mi_00\7&25eea38c&0&0000";
        let h: Holder = toml::from_str(&format!("id = '{lower}'\n")).unwrap();
        assert_eq!(h.id.raw(), lower);
        assert_ne!(
            h.id.selector().to_string(),
            lower,
            "the selector really does canonicalise — that is what makes this test necessary"
        );
        assert_eq!(toml::to_string(&h).unwrap(), format!("id = '{lower}'\n"));
    }

    /// JSON is the other half of the interop promise, through the same types.
    #[test]
    fn the_raw_spelling_survives_json_as_well() {
        let json = r#"{"id":"usb:D209:0430:00"}"#;
        let h: Holder = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&h).unwrap(), json);
    }

    #[test]
    fn a_value_that_can_never_name_a_device_is_refused_by_name() {
        let err = toml::from_str::<Holder>("id = \"my-keyboard\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("my-keyboard"), "{msg}");
        assert!(msg.contains("usb:<vid>:<pid>:<mi>"), "{msg}");
    }

    /// A keyboard on an enumerator ksx does not model — which is what ksx's own
    /// setup wizard writes for a laptop or PS/2 board — still loads.
    #[test]
    fn an_acpi_keyboard_path_loads_and_round_trips() {
        const LINE: &str = "id = 'ACPI\\PNP0303\\4&1A2B3C4D&0'\n";
        let h: Holder = toml::from_str(LINE).unwrap();
        assert_eq!(toml::to_string(&h).unwrap(), LINE);
    }
}
