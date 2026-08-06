//! The preset-file function vocabulary (`docs/research/design-architecture.md` §4.1).
//!
//! Canonical names: buttons `A B X Y start back guide lb rb lthumb rthumb`,
//! triggers `lt rt`, axes `lx ly rx ry` with a `.min` / `.max` / `.<i16>`
//! suffix (custom axis values are first-class, e.g. `lx.-16384`), dpad
//! `dpad.up` / `dpad.down` / `dpad.left` / `dpad.right`, plus the output-less
//! [`CONSUME`] (`ksx_core::Binding::Consume`) that a consume-only chord lives
//! under — the `[bindings]` table is keyed by function name, so "emit nothing"
//! needs a name like everything else.
//!
//! Parsing is case-insensitive; emission is canonical (`-32768` emits as
//! `min`, `32767` as `max`).

use ksx_core::{Axis, Binding, DpadDirection, Trigger, XButton, AXIS_MAX, AXIS_MIN};

use crate::error::ConfigError;

/// The function name of a consume-only chord: it drives nothing, and *that*
/// is what it is called (docs/INPUT-TRANSFORMS.md §2.6).
pub const CONSUME: &str = "consume";

/// The prefix that makes a `[bindings]` row a MACRO TRIGGER rather than an
/// endpoint: `macro.hadouken = "P"` (docs/INPUT-TRANSFORMS.md §1c).
///
/// Macros are deliberately outside [`parse_function`]: a macro is named by the
/// preset's own `[macros]` table, not by the fixed pad vocabulary, so the name
/// cannot resolve to a [`Binding`] without knowing which preset it came from.
/// [`macro_name`] is the whole of the grammar.
pub const MACRO_PREFIX: &str = "macro.";

/// The macro a function name triggers, if it is a macro row at all.
///
/// Case-insensitive on the prefix, like every other function name; the macro
/// name itself is returned verbatim so the file keeps the author's spelling
/// (matching against the `[macros]` table is case-insensitive too).
pub fn macro_name(function: &str) -> Option<&str> {
    let (head, rest) = function.split_at_checked(MACRO_PREFIX.len())?;
    head.eq_ignore_ascii_case(MACRO_PREFIX).then_some(rest)
}

/// The canonical function name that triggers `name`.
pub fn macro_function_name(name: &str) -> String {
    format!("{MACRO_PREFIX}{name}")
}

/// Canonical function name for a binding.
pub fn function_name(binding: &Binding) -> String {
    match binding {
        Binding::Consume => CONSUME.to_owned(),
        Binding::Button(button) => button_name(*button).to_owned(),
        Binding::Trigger(Trigger::Left) => "lt".to_owned(),
        Binding::Trigger(Trigger::Right) => "rt".to_owned(),
        Binding::Axis { axis, value } => {
            let suffix = match *value {
                AXIS_MIN => "min".to_owned(),
                AXIS_MAX => "max".to_owned(),
                custom => custom.to_string(),
            };
            format!("{}.{}", axis_name(*axis), suffix)
        }
        Binding::Dpad(direction) => format!("dpad.{}", dpad_name(*direction)),
    }
}

/// Parse a function name (case-insensitive) into a binding.
pub fn parse_function(name: &str) -> Result<Binding, ConfigError> {
    let lower = name.to_ascii_lowercase();

    let simple = match lower.as_str() {
        "a" => Some(Binding::Button(XButton::A)),
        "b" => Some(Binding::Button(XButton::B)),
        "x" => Some(Binding::Button(XButton::X)),
        "y" => Some(Binding::Button(XButton::Y)),
        "start" => Some(Binding::Button(XButton::Start)),
        "back" => Some(Binding::Button(XButton::Back)),
        "guide" => Some(Binding::Button(XButton::Guide)),
        "lb" => Some(Binding::Button(XButton::LeftBumper)),
        "rb" => Some(Binding::Button(XButton::RightBumper)),
        "lthumb" => Some(Binding::Button(XButton::LeftThumb)),
        "rthumb" => Some(Binding::Button(XButton::RightThumb)),
        "lt" => Some(Binding::Trigger(Trigger::Left)),
        "rt" => Some(Binding::Trigger(Trigger::Right)),
        CONSUME => Some(Binding::Consume),
        _ => None,
    };
    if let Some(binding) = simple {
        return Ok(binding);
    }

    let Some((base, rest)) = lower.split_once('.') else {
        return Err(ConfigError::UnknownFunction(name.to_owned()));
    };

    match base {
        "dpad" => {
            let direction = match rest {
                "up" => DpadDirection::Up,
                "down" => DpadDirection::Down,
                "left" => DpadDirection::Left,
                "right" => DpadDirection::Right,
                _ => return Err(ConfigError::UnknownFunction(name.to_owned())),
            };
            Ok(Binding::Dpad(direction))
        }
        "lx" | "ly" | "rx" | "ry" => {
            let axis = match base {
                "lx" => Axis::X,
                "ly" => Axis::Y,
                "rx" => Axis::Rx,
                _ => Axis::Ry,
            };
            let value = match rest {
                "min" => AXIS_MIN,
                "max" => AXIS_MAX,
                custom => custom
                    .parse::<i16>()
                    .map_err(|_| ConfigError::InvalidAxisValue(custom.to_owned()))?,
            };
            Ok(Binding::Axis { axis, value })
        }
        _ => Err(ConfigError::UnknownFunction(name.to_owned())),
    }
}

const fn button_name(button: XButton) -> &'static str {
    match button {
        XButton::A => "A",
        XButton::B => "B",
        XButton::X => "X",
        XButton::Y => "Y",
        XButton::Start => "start",
        XButton::Back => "back",
        XButton::Guide => "guide",
        XButton::LeftBumper => "lb",
        XButton::RightBumper => "rb",
        XButton::LeftThumb => "lthumb",
        XButton::RightThumb => "rthumb",
    }
}

const fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "lx",
        Axis::Y => "ly",
        Axis::Rx => "rx",
        Axis::Ry => "ry",
    }
}

const fn dpad_name(direction: DpadDirection) -> &'static str {
    match direction {
        DpadDirection::Up => "up",
        DpadDirection::Down => "down",
        DpadDirection::Left => "left",
        DpadDirection::Right => "right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_round_trips_through_its_name() {
        let mut bindings: Vec<Binding> = Vec::new();
        bindings.extend(XButton::ALL.iter().map(|b| Binding::Button(*b)));
        bindings.extend(Trigger::ALL.iter().map(|t| Binding::Trigger(*t)));
        bindings.extend(DpadDirection::ALL.iter().map(|d| Binding::Dpad(*d)));
        bindings.push(Binding::Consume);
        for axis in Axis::ALL {
            for value in [AXIS_MIN, AXIS_MAX, -16384, 16384, 1000, -1, 0] {
                bindings.push(Binding::Axis { axis: *axis, value });
            }
        }
        for binding in bindings {
            let name = function_name(&binding);
            assert_eq!(
                parse_function(&name).unwrap(),
                binding,
                "round-trip failed for '{name}'"
            );
        }
    }

    #[test]
    fn doc_example_names() {
        assert_eq!(parse_function("A").unwrap(), Binding::Button(XButton::A));
        assert_eq!(
            parse_function("lt").unwrap(),
            Binding::Trigger(Trigger::Left)
        );
        assert_eq!(
            parse_function("lx.min").unwrap(),
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN
            }
        );
        assert_eq!(
            parse_function("lx.-16384").unwrap(),
            Binding::Axis {
                axis: Axis::X,
                value: -16384
            }
        );
        assert_eq!(
            parse_function("dpad.up").unwrap(),
            Binding::Dpad(DpadDirection::Up)
        );
    }

    #[test]
    fn parsing_is_case_insensitive_emission_canonical() {
        assert_eq!(
            parse_function("START").unwrap(),
            Binding::Button(XButton::Start)
        );
        assert_eq!(
            parse_function("Dpad.Up").unwrap(),
            Binding::Dpad(DpadDirection::Up)
        );
        assert_eq!(
            parse_function("LX.MIN").unwrap(),
            Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN
            }
        );
        assert_eq!(function_name(&Binding::Button(XButton::Start)), "start");
        assert_eq!(
            function_name(&Binding::Axis {
                axis: Axis::X,
                value: AXIS_MIN
            }),
            "lx.min"
        );
        assert_eq!(
            function_name(&Binding::Axis {
                axis: Axis::Ry,
                value: -16384
            }),
            "ry.-16384"
        );
        assert_eq!(
            function_name(&Binding::Dpad(DpadDirection::Right)),
            "dpad.right"
        );
    }

    /// The consume-only chord's function name: canonical, case-insensitive,
    /// and never produced by anything that actually drives an endpoint.
    #[test]
    fn consume_is_a_function_name() {
        assert_eq!(parse_function("consume").unwrap(), Binding::Consume);
        assert_eq!(parse_function("CONSUME").unwrap(), Binding::Consume);
        assert_eq!(function_name(&Binding::Consume), "consume");
        for button in XButton::ALL {
            assert_ne!(function_name(&Binding::Button(*button)), CONSUME);
        }
    }

    /// Macro rows are recognized by prefix and never by the pad vocabulary —
    /// `parse_function` must keep refusing them, or a macro trigger would
    /// silently become "unknown function" instead of a macro.
    #[test]
    fn macro_rows_are_their_own_grammar() {
        assert_eq!(macro_name("macro.hadouken"), Some("hadouken"));
        assert_eq!(macro_name("MACRO.Hadouken"), Some("Hadouken"));
        // Dots in a macro name are the author's business, not ours.
        assert_eq!(macro_name("macro.super.combo"), Some("super.combo"));
        assert_eq!(macro_name("macros.hadouken"), None);
        assert_eq!(macro_name("macro"), None);
        assert_eq!(macro_name("A"), None);
        assert_eq!(macro_function_name("hadouken"), "macro.hadouken");
        assert!(matches!(
            parse_function("macro.hadouken"),
            Err(ConfigError::UnknownFunction(_))
        ));
    }

    #[test]
    fn rejects_unknown_and_malformed() {
        assert!(matches!(
            parse_function("nope"),
            Err(ConfigError::UnknownFunction(_))
        ));
        assert!(matches!(
            parse_function("dpad.diagonal"),
            Err(ConfigError::UnknownFunction(_))
        ));
        assert!(matches!(
            parse_function("zz.min"),
            Err(ConfigError::UnknownFunction(_))
        ));
        assert!(matches!(
            parse_function("lx.99999"),
            Err(ConfigError::InvalidAxisValue(_))
        ));
        assert!(matches!(
            parse_function("lx."),
            Err(ConfigError::InvalidAxisValue(_))
        ));
    }
}
