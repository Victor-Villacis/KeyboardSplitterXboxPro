//! TOML glue for the macro interruption policies
//! ([`ksx_core::OnRelease`], [`ksx_core::Retrigger`]).
//!
//! Same rule as [`crate::socd_serde`]: ksx-core carries no serde dependency, so
//! the wire form lives here and is exactly as lenient as the two `FromStr`
//! impls (`on_release = "Abort"` and `retrigger = "re-start"` are things people
//! type). Both defaults are skipped on write, so a macro that takes the
//! fighting-game behavior stays a `steps` list and nothing else.

/// `skip_serializing_if` for a plain `bool` field that defaults to `false`.
pub fn is_false(value: &bool) -> bool {
    !*value
}

/// `serde(default = …)` for a flag whose absence means TRUE — `enabled`, and so
/// far only `enabled`. Serde's own `default` for `bool` is `false`, which would
/// silently disable every macro in every file written before the setting
/// existed.
pub fn default_true() -> bool {
    true
}

/// `skip_serializing_if` for the same flag: `enabled = true` is the default and
/// is never written, so a preset that never touches it round-trips byte for
/// byte.
pub fn is_true(value: &bool) -> bool {
    *value
}

/// A slot's macro MASTER switch (`macros = "on" | "off"`) — the tournament
/// setting. Same shape as [`crate::socd_serde`], and `"on"` is skipped on write.
pub mod switch {
    use std::str::FromStr;

    use ksx_core::MacroSwitch;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(switch: &MacroSwitch, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(switch.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<MacroSwitch, D::Error> {
        let raw = String::deserialize(d)?;
        MacroSwitch::from_str(&raw).map_err(serde::de::Error::custom)
    }

    /// `on` is the default and is never written — every config that predates
    /// the switch serializes to exactly the bytes it always did.
    pub fn is_default(switch: &MacroSwitch) -> bool {
        *switch == MacroSwitch::default()
    }
}

pub mod on_release {
    use std::str::FromStr;

    use ksx_core::OnRelease;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(policy: &OnRelease, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(policy.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<OnRelease, D::Error> {
        let raw = String::deserialize(d)?;
        OnRelease::from_str(&raw).map_err(serde::de::Error::custom)
    }

    /// `finish` is the default and is never written.
    pub fn is_default(policy: &OnRelease) -> bool {
        *policy == OnRelease::default()
    }
}

pub mod interrupt {
    use std::str::FromStr;

    use ksx_core::Interrupt;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(policy: &Interrupt, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(policy.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Interrupt, D::Error> {
        let raw = String::deserialize(d)?;
        Interrupt::from_str(&raw).map_err(serde::de::Error::custom)
    }

    /// `none` is the default and is never written.
    pub fn is_default(policy: &Interrupt) -> bool {
        *policy == Interrupt::default()
    }
}

pub mod repeat {
    use std::str::FromStr;

    use ksx_core::Repeat;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(policy: &Repeat, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(policy.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Repeat, D::Error> {
        let raw = String::deserialize(d)?;
        Repeat::from_str(&raw).map_err(serde::de::Error::custom)
    }

    /// `once` is the default and is never written — a preset that predates the
    /// setting serializes to exactly the bytes it always did.
    pub fn is_default(policy: &Repeat) -> bool {
        *policy == Repeat::default()
    }
}

pub mod retrigger {
    use std::str::FromStr;

    use ksx_core::Retrigger;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(policy: &Retrigger, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(policy.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Retrigger, D::Error> {
        let raw = String::deserialize(d)?;
        Retrigger::from_str(&raw).map_err(serde::de::Error::custom)
    }

    /// `ignore` is the default and is never written.
    pub fn is_default(policy: &Retrigger) -> bool {
        *policy == Retrigger::default()
    }
}

#[cfg(test)]
mod tests {
    use ksx_core::{OnRelease, Retrigger};

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Holder {
        #[serde(
            default,
            with = "super::on_release",
            skip_serializing_if = "super::on_release::is_default"
        )]
        on_release: OnRelease,
        #[serde(
            default,
            with = "super::retrigger",
            skip_serializing_if = "super::retrigger::is_default"
        )]
        retrigger: Retrigger,
    }

    #[test]
    fn missing_fields_are_the_fighting_game_defaults() {
        let h: Holder = toml::from_str("").unwrap();
        assert_eq!(h.on_release, OnRelease::Finish);
        assert_eq!(h.retrigger, Retrigger::Ignore);
        // ...and writing them back adds not one byte.
        assert_eq!(toml::to_string(&h).unwrap().trim(), "");
    }

    #[test]
    fn policies_round_trip_and_aliases_normalize() {
        let h: Holder = toml::from_str("on_release = \"Abort\"\nretrigger = \"re-start\"").unwrap();
        assert_eq!(h.on_release, OnRelease::Abort);
        assert_eq!(h.retrigger, Retrigger::Restart);
        let text = toml::to_string(&h).unwrap();
        assert!(text.contains("on_release = \"abort\""), "{text}");
        assert!(text.contains("retrigger = \"restart\""), "{text}");
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), h);
    }

    #[test]
    fn an_unknown_policy_names_the_options() {
        let err = toml::from_str::<Holder>("on_release = \"maybe\"").unwrap_err();
        assert!(err.to_string().contains("abort"), "{err}");
        let err = toml::from_str::<Holder>("retrigger = \"maybe\"").unwrap_err();
        assert!(err.to_string().contains("restart"), "{err}");
    }
}
