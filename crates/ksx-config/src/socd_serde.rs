//! TOML glue for [`ksx_core::Socd`], the per-slot SOCD cleaning policy.
//!
//! Same rule as [`crate::persona_serde`]: ksx-core carries no serde
//! dependency, so the wire form lives here and is exactly as lenient as
//! [`Socd::from_str`] (`up_priority` and `UpPriority` are things people type).

use std::str::FromStr;

use ksx_core::Socd;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(socd: &Socd, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(socd.as_str())
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Socd, D::Error> {
    let raw = String::deserialize(d)?;
    Socd::from_str(&raw).map_err(serde::de::Error::custom)
}

/// `skip_serializing_if` target. `socd = "off"` is the default and is never
/// written, so every config that predates SOCD stays byte-identical — which is
/// the same thing as saying the feature is opt-in.
pub fn is_default(socd: &Socd) -> bool {
    *socd == Socd::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Holder {
        #[serde(default, with = "super", skip_serializing_if = "is_default")]
        socd: Socd,
    }

    #[test]
    fn missing_field_is_off() {
        let h: Holder = toml::from_str("").unwrap();
        assert_eq!(h.socd, Socd::Off);
    }

    #[test]
    fn policies_round_trip_and_off_is_never_written() {
        for &policy in Socd::ALL {
            let text = format!("socd = \"{policy}\"");
            let h: Holder = toml::from_str(&text).unwrap();
            assert_eq!(h.socd, policy);
            let written = toml::to_string(&h).unwrap();
            if policy == Socd::Off {
                assert_eq!(written.trim(), "");
            } else {
                assert_eq!(written.trim(), text);
            }
        }
    }

    #[test]
    fn aliases_are_accepted_and_normalized_on_write() {
        let h: Holder = toml::from_str("socd = \"up_priority\"").unwrap();
        assert_eq!(h.socd, Socd::UpPriority);
        assert_eq!(
            toml::to_string(&h).unwrap().trim(),
            "socd = \"up-priority\""
        );
    }

    #[test]
    fn an_unknown_policy_names_the_options() {
        let err = toml::from_str::<Holder>("socd = \"snaptap\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("snaptap"), "{msg}");
        assert!(msg.contains("up-priority"), "{msg}");
    }
}
