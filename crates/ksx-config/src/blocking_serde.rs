//! TOML/JSON glue for [`ksx_core::Blocking`], the `block_keyboards` setting.
//!
//! Same rule as [`crate::socd_serde`]: ksx-core carries no serde dependency, so
//! the wire form lives here. What is different — and the whole reason this
//! module is not a two-line copy of that one — is that this field already had a
//! wire form. It was a bool, in every config.toml and games.toml ksx has ever
//! written.
//!
//! So the two states that had a bool spelling keep it EXACTLY: `Whole` is
//! `true`, `Off` is `false`, on the way in and on the way out. A config written
//! before this module existed reads back with the same meaning and re-serializes
//! to the same bytes; the emission snapshots in `tests/emission.rs` are what
//! pins that. Only the third state needs a name of its own, and it is the
//! string `"bound-keys"`.
//!
//! That is also why there is no `is_default`/`skip_serializing_if` here. Both
//! fields that use this module are written unconditionally today, and dropping
//! `block_keyboards = true` from the output would change the bytes of every
//! existing file — the exact thing this module exists to avoid.

use std::fmt;
use std::str::FromStr;

use ksx_core::Blocking;
use serde::de::Visitor;
use serde::{Deserializer, Serializer};

pub fn serialize<S: Serializer>(blocking: &Blocking, s: S) -> Result<S::Ok, S::Error> {
    match blocking {
        Blocking::Whole => s.serialize_bool(true),
        Blocking::Off => s.serialize_bool(false),
        Blocking::BoundKeys => s.serialize_str(blocking.as_str()),
    }
}

/// `deserialize_any` rather than a typed hook, because the field is genuinely
/// two shapes: a bool from every file written so far, a string from anything
/// that chose the third state. Both TOML and JSON are self-describing, which is
/// the precondition `deserialize_any` needs and the only two formats ksx reads.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Blocking, D::Error> {
    d.deserialize_any(BlockingVisitor)
}

struct BlockingVisitor;

impl Visitor<'_> for BlockingVisitor {
    type Value = Blocking;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("true, false, or \"bound-keys\"")
    }

    fn visit_bool<E: serde::de::Error>(self, block: bool) -> Result<Blocking, E> {
        Ok(Blocking::from(block))
    }

    fn visit_str<E: serde::de::Error>(self, raw: &str) -> Result<Blocking, E> {
        Blocking::from_str(raw).map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Holder {
        #[serde(default, with = "super")]
        block_keyboards: Blocking,
    }

    /// The compatibility half, and the one that must never change: the two
    /// spellings that already exist on disk mean what they always meant, and
    /// come back out as the same bytes.
    #[test]
    fn the_bool_spellings_keep_their_meaning_and_their_bytes() {
        let on: Holder = toml::from_str("block_keyboards = true").unwrap();
        assert_eq!(on.block_keyboards, Blocking::Whole);
        assert_eq!(
            toml::to_string(&on).unwrap().trim(),
            "block_keyboards = true"
        );

        let off: Holder = toml::from_str("block_keyboards = false").unwrap();
        assert_eq!(off.block_keyboards, Blocking::Off);
        assert_eq!(
            toml::to_string(&off).unwrap().trim(),
            "block_keyboards = false"
        );
    }

    #[test]
    fn the_third_state_round_trips_as_a_string() {
        let partial: Holder = toml::from_str("block_keyboards = \"bound-keys\"").unwrap();
        assert_eq!(partial.block_keyboards, Blocking::BoundKeys);
        let text = toml::to_string(&partial).unwrap();
        assert_eq!(text.trim(), "block_keyboards = \"bound-keys\"");
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), partial);
    }

    /// The string form of a state that normally writes as a bool is accepted
    /// (people hand-edit) and NORMALIZED back to the bool — otherwise a config
    /// would drift to a spelling ksx never emits just by being loaded once.
    #[test]
    fn a_hand_written_string_normalizes_back_to_the_bool_form() {
        let whole: Holder = toml::from_str("block_keyboards = \"whole\"").unwrap();
        assert_eq!(whole.block_keyboards, Blocking::Whole);
        assert_eq!(
            toml::to_string(&whole).unwrap().trim(),
            "block_keyboards = true"
        );
    }

    /// JSON is the other format these types cross (`ksx config export`), and
    /// `deserialize_any` has to hold there too.
    #[test]
    fn both_shapes_survive_json_as_well() {
        let on: Holder = serde_json::from_str(r#"{"block_keyboards": true}"#).unwrap();
        assert_eq!(on.block_keyboards, Blocking::Whole);
        assert_eq!(
            serde_json::to_string(&on).unwrap(),
            r#"{"block_keyboards":true}"#
        );

        let partial: Holder = serde_json::from_str(r#"{"block_keyboards": "bound-keys"}"#).unwrap();
        assert_eq!(partial.block_keyboards, Blocking::BoundKeys);
        assert_eq!(
            serde_json::to_string(&partial).unwrap(),
            r#"{"block_keyboards":"bound-keys"}"#
        );
    }

    #[test]
    fn an_unknown_mode_names_the_options() {
        let err = toml::from_str::<Holder>("block_keyboards = \"sometimes\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("sometimes"), "{msg}");
        assert!(msg.contains("bound-keys"), "{msg}");
    }

    #[test]
    fn a_missing_field_is_the_whole_device() {
        let h: Holder = toml::from_str("").unwrap();
        assert_eq!(h.block_keyboards, Blocking::Whole);
    }
}
