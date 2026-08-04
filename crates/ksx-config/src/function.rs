//! The preset-file function vocabulary (`docs/research/design-architecture.md` §4.1).
//!
//! Canonical names: buttons `A B X Y start back guide lb rb lthumb rthumb`,
//! triggers `lt rt`, axes `lx ly rx ry` with a `.min` / `.max` / `.<i16>`
//! suffix (custom axis values are first-class, e.g. `lx.-16384`), dpad
//! `dpad.up` / `dpad.down` / `dpad.left` / `dpad.right`.
//!
//! Parsing is case-insensitive; emission is canonical (`-32768` emits as
//! `min`, `32767` as `max`).

use ksx_core::{Axis, Binding, DpadDirection, Trigger, XButton, AXIS_MAX, AXIS_MIN};

use crate::error::ConfigError;

/// Canonical function name for a binding.
pub fn function_name(binding: &Binding) -> String {
    match binding {
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
