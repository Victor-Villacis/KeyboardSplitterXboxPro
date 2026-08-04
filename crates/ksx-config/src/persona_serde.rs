//! TOML glue for [`ksx_core::Persona`].
//!
//! ksx-core carries no serde dependency — its types cross this boundary by name
//! (same rule as `Key::from_name`), so the wire form lives here. Deliberately as
//! lenient as [`Persona::from_str`]: `persona = "ds4"` and `persona = "PS4"` are
//! things people will write in a hand-edited file, and rejecting them would be
//! pedantry rather than validation.

use std::str::FromStr;

use ksx_core::Persona;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(persona: &Persona, s: S) -> Result<S::Ok, S::Error> {
    // Always the canonical name, so a rewritten config normalizes aliases.
    s.serialize_str(persona.as_str())
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Persona, D::Error> {
    let raw = String::deserialize(d)?;
    Persona::from_str(&raw).map_err(serde::de::Error::custom)
}

/// `skip_serializing_if` target: the default persona is left out of the file
/// entirely, so an Xbox-only config stays byte-identical to a pre-persona one.
pub fn is_default(persona: &Persona) -> bool {
    *persona == Persona::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Holder {
        #[serde(default, with = "super", skip_serializing_if = "is_default")]
        persona: Persona,
    }

    #[test]
    fn missing_field_is_the_default_persona() {
        // The compatibility guarantee for every config written before personas.
        let h: Holder = toml::from_str("").unwrap();
        assert_eq!(h.persona, Persona::Xbox360);
    }

    #[test]
    fn aliases_are_accepted_and_normalized_on_write() {
        let h: Holder = toml::from_str("persona = \"ds4\"").unwrap();
        assert_eq!(h.persona, Persona::PlayStation);
        assert_eq!(
            toml::to_string(&h).unwrap().trim(),
            "persona = \"playstation\""
        );
    }

    #[test]
    fn the_default_persona_is_not_written_out() {
        let h = Holder {
            persona: Persona::Xbox360,
        };
        assert_eq!(toml::to_string(&h).unwrap().trim(), "");
    }

    #[test]
    fn an_unknown_persona_is_a_parse_error_that_names_the_options() {
        let err = toml::from_str::<Holder>("persona = \"gamecube\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("gamecube"), "{msg}");
        assert!(msg.contains("playstation"), "{msg}");
    }
}
