//! How much of a bound keyboard a session takes away from Windows — the
//! setting `block_keyboards` names.
//!
//! The vocabulary lives here, next to [`crate::Socd`] and
//! [`crate::MacroSwitch`], for the same reason those do: ksx-core carries no
//! serde, so a hand-edited config file and a running session agree on the words
//! without the engine crate learning about file formats. The TOML/JSON shape is
//! `ksx_config::blocking_serde`.

use std::fmt;
use std::str::FromStr;

/// What ksx does with strokes from a keyboard that is bound to a slot.
///
/// # Why this is not a bool
///
/// It used to be, and the bool could only describe two of the three things
/// people actually want:
///
/// - a cabinet encoder exists to drive pads. Letting its *unmapped* buttons
///   reach Windows sprays junk into whatever has focus, so ksx takes the whole
///   device — [`Blocking::Whole`], and the default, because that is what every
///   config written so far means;
/// - a desk keyboard split into two pads still has to type. Its owner needs
///   Enter, Escape and the letters for as long as the session runs, and until
///   [`Blocking::BoundKeys`] existed they lost all of them;
/// - and [`Blocking::Off`] is "drive the pads, block nothing", which the bool
///   already spelled `false`.
///
/// The wire form keeps the bool spellings for the two states that had one
/// (`true`/`false`), so every existing config round-trips byte for byte and
/// keeps its meaning; the third state is the string `"bound-keys"`.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Blocking {
    /// Suppress every stroke from the device, mapped or not (`true`).
    #[default]
    Whole,
    /// Suppress only the keys the slots on this device actually bind; every
    /// other key types normally (`"bound-keys"`).
    BoundKeys,
    /// Suppress nothing: the pads are driven and the keyboard keeps typing
    /// as well (`false`).
    Off,
}

impl Blocking {
    pub const ALL: &'static [Blocking] = &[Blocking::Whole, Blocking::BoundKeys, Blocking::Off];

    /// Canonical name. Only [`Blocking::BoundKeys`] is ever *written* as this
    /// string — see the module docs for why the other two stay booleans on
    /// disk — but all three need a name for `--json`, logs and UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Blocking::Whole => "whole",
            Blocking::BoundKeys => "bound-keys",
            Blocking::Off => "off",
        }
    }

    /// One phrase for a human report, which is a different job from
    /// [`Self::as_str`]: it has to answer "will my keyboard still work" without
    /// the reader knowing the vocabulary.
    pub const fn label(self) -> &'static str {
        match self {
            Blocking::Whole => "yes (the whole device)",
            Blocking::BoundKeys => "bound keys only (everything else keeps typing)",
            Blocking::Off => "no",
        }
    }

    /// Does this mode suppress anything at all? The guard on every code path
    /// that would arm capture.
    pub const fn captures(self) -> bool {
        !matches!(self, Blocking::Off)
    }

    /// Is the per-key set load-bearing here? `false` means the device is taken
    /// whole (or not at all) and no key set needs computing.
    pub const fn is_per_key(self) -> bool {
        matches!(self, Blocking::BoundKeys)
    }
}

/// The legacy two-state spelling, in one place: `true` took the whole device
/// and `false` took nothing. Every importer and every pre-existing config goes
/// through here, which is what makes "an old config keeps its meaning" a fact
/// rather than a promise.
impl From<bool> for Blocking {
    fn from(block: bool) -> Self {
        if block {
            Blocking::Whole
        } else {
            Blocking::Off
        }
    }
}

impl fmt::Display for Blocking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown blocking mode '{0}' (expected true, false, or one of: whole, bound-keys, off)")]
pub struct UnknownBlocking(pub String);

impl FromStr for Blocking {
    type Err = UnknownBlocking;

    /// Case- and separator-insensitive like [`crate::Socd`], and it also accepts
    /// `"true"`/`"false"`: config files are hand-edited, and someone converting
    /// a `block_keyboards = true` line into the string form must not be told
    /// their own previous value is unknown.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect();
        match normalized.as_str() {
            "whole" | "all" | "device" | "true" | "yes" | "on" => Ok(Blocking::Whole),
            "boundkeys" | "bound" | "keys" | "mapped" | "partial" => Ok(Blocking::BoundKeys),
            "off" | "none" | "false" | "no" => Ok(Blocking::Off),
            _ => Err(UnknownBlocking(s.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compatibility promise, as an assertion: the mode a config that never
    /// heard of this setting gets is the one it has always had.
    #[test]
    fn the_default_is_taking_the_whole_device() {
        assert_eq!(Blocking::default(), Blocking::Whole);
        assert_eq!(Blocking::from(true), Blocking::Whole);
        assert_eq!(Blocking::from(false), Blocking::Off);
    }

    #[test]
    fn only_off_declines_to_capture_and_only_bound_keys_needs_a_key_set() {
        assert!(Blocking::Whole.captures());
        assert!(Blocking::BoundKeys.captures());
        assert!(!Blocking::Off.captures());

        assert!(Blocking::BoundKeys.is_per_key());
        assert!(!Blocking::Whole.is_per_key());
        assert!(!Blocking::Off.is_per_key());
    }

    #[test]
    fn every_name_round_trips_and_the_bool_spellings_are_accepted() {
        for &mode in Blocking::ALL {
            assert_eq!(Blocking::from_str(mode.as_str()), Ok(mode));
        }
        // What someone typing over their old `block_keyboards = true` writes.
        assert_eq!(Blocking::from_str("true"), Ok(Blocking::Whole));
        assert_eq!(Blocking::from_str("false"), Ok(Blocking::Off));
        // ...and the spellings hand-editing produces.
        assert_eq!(Blocking::from_str("bound_keys"), Ok(Blocking::BoundKeys));
        assert_eq!(Blocking::from_str("Bound Keys"), Ok(Blocking::BoundKeys));
    }

    #[test]
    fn an_unknown_mode_names_the_options() {
        let err = Blocking::from_str("sometimes").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("sometimes"), "{msg}");
        assert!(msg.contains("bound-keys"), "{msg}");
    }
}
