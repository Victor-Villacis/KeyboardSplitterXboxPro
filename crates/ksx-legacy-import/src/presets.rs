//! `splitter_presets.xml` -> [`ksx_config::PresetFile`] list.
//!
//! Two schemas are accepted transparently, distinguished per element by their
//! attribute spelling (XML attribute names are case-sensitive):
//!
//! - **v2** (current, `legacy/SplitterCore/Preset/*.cs`): lowercase attributes
//!   with numeric ids -- `<preset name>`, `<button id="4096">`,
//!   `<axis id="1" value="-32768">`, `<dpad direction="1">`,
//!   `<custom function="2">`.
//! - **v1** (`legacy/KeyboardSplitter/Presets/PresetUpgrader.cs`): capitalized
//!   attributes with enum *names* -- `<preset Name>`, `<button ID="A">`,
//!   `<axis ID="X" Position="Min">`, `<dpad ID="Up">` (also accepted as
//!   `<pov>`), `<custom ID="Button_A">`. The legacy upgrader's blunt
//!   text-level `LeftTrigger`→`Left` / `RightTrigger`→`Right` rewrite is
//!   applied surgically here: those spellings are accepted as trigger-id
//!   aliases instead of rewriting the whole document.
//!
//! Every numeric id goes through the bit-exact `ksx-core` conversion tables.
//! `<custom>` functions are expanded into plain [`Binding`]s (an axis custom
//! function always drove full-scale Min/Max -- `CustomFunctionHelper.cs`).
//! A key name of `None` means "function present, unbound": the entry is
//! omitted from the emitted TOML.

use ksx_core::{Axis, Binding, DpadDirection, Key, Preset, Trigger, XButton, AXIS_MAX, AXIS_MIN};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::report::Warning;
use crate::xml;

pub struct ParsedPresets {
    pub presets: Vec<ksx_config::PresetFile>,
    pub warnings: Vec<Warning>,
}

/// Parse a decoded presets XML document. `file` labels warnings
/// (e.g. `splitter_presets.xml`). Never fails: unmappable entries become
/// warnings and are skipped.
pub fn parse_presets(text: &str, file: &str) -> ParsedPresets {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut presets: Vec<Preset> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let mut current: Option<Preset> = None;

    loop {
        match reader.read_event() {
            Err(e) => {
                warnings.push(Warning::file_level(
                    file,
                    format!(
                        "XML parse error at byte {}: {e} -- rest of file skipped",
                        reader.buffer_position()
                    ),
                ));
                break;
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(el)) => {
                let name = String::from_utf8_lossy(el.name().as_ref()).into_owned();
                match name.as_str() {
                    "preset_data" => {}
                    "preset" => {
                        if let Some(done) = current.take() {
                            // Unclosed previous preset -- keep what it had.
                            presets.push(done);
                        }
                        match preset_name(&el) {
                            Ok(preset_name) => {
                                current = Some(Preset {
                                    name: preset_name,
                                    entries: Vec::new(),
                                    // Legacy XML has no chord vocabulary; an
                                    // imported preset never has one.
                                    chords: Vec::new(),
                                    macros: Default::default(),
                                    turbo: Vec::new(),
                                    protected: false,
                                });
                            }
                            Err(reason) => {
                                warnings.push(Warning::file_level(
                                    file,
                                    format!("{reason} -- preset skipped"),
                                ));
                                let end = el.name().to_owned();
                                if reader.read_to_end(end).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    "button" | "trigger" | "axis" | "dpad" | "pov" | "custom" => {
                        let attrs = match xml::attrs(&el) {
                            Ok(attrs) => attrs,
                            Err(reason) => {
                                warnings.push(Warning::entry(
                                    file,
                                    current.as_ref().map(|p| p.name.as_str()),
                                    name.clone(),
                                    format!("{reason} -- entry skipped"),
                                ));
                                let end = el.name().to_owned();
                                if reader.read_to_end(end).is_err() {
                                    break;
                                }
                                continue;
                            }
                        };
                        let key_text = match xml::element_text(&mut reader, &el) {
                            Ok(t) => t,
                            Err(reason) => {
                                warnings.push(Warning::entry(
                                    file,
                                    current.as_ref().map(|p| p.name.as_str()),
                                    xml::describe(&name, &attrs),
                                    format!("{reason} -- entry skipped"),
                                ));
                                continue;
                            }
                        };
                        let Some(preset) = current.as_mut() else {
                            warnings.push(Warning::entry(
                                file,
                                None,
                                xml::describe(&name, &attrs),
                                "binding element outside a <preset> -- entry skipped",
                            ));
                            continue;
                        };
                        match parse_binding(&name, &attrs, &key_text) {
                            Ok(Some(entry)) => preset.entries.push(entry),
                            Ok(None) => {} // key "None": function present, unbound
                            Err(reason) => warnings.push(Warning::entry(
                                file,
                                Some(preset.name.as_str()),
                                format!("{} -> '{key_text}'", xml::describe(&name, &attrs)),
                                format!("{reason} -- entry skipped"),
                            )),
                        }
                    }
                    other => {
                        warnings.push(Warning::entry(
                            file,
                            current.as_ref().map(|p| p.name.as_str()),
                            other.to_owned(),
                            "unknown element -- skipped",
                        ));
                        let end = el.name().to_owned();
                        if reader.read_to_end(end).is_err() {
                            break;
                        }
                    }
                }
            }
            Ok(Event::Empty(el)) => {
                let name = String::from_utf8_lossy(el.name().as_ref()).into_owned();
                match name.as_str() {
                    "preset" => match preset_name(&el) {
                        Ok(preset_name) => presets.push(Preset {
                            name: preset_name,
                            entries: Vec::new(),
                            chords: Vec::new(),
                            macros: Default::default(),
                            turbo: Vec::new(),
                            protected: false,
                        }),
                        Err(reason) => warnings.push(Warning::file_level(
                            file,
                            format!("{reason} -- preset skipped"),
                        )),
                    },
                    "button" | "trigger" | "axis" | "dpad" | "pov" | "custom" => {
                        let described = xml::attrs(&el)
                            .map(|attrs| xml::describe(&name, &attrs))
                            .unwrap_or(name);
                        warnings.push(Warning::entry(
                            file,
                            current.as_ref().map(|p| p.name.as_str()),
                            described,
                            "element has no key text -- entry skipped",
                        ));
                    }
                    other => warnings.push(Warning::entry(
                        file,
                        current.as_ref().map(|p| p.name.as_str()),
                        other.to_owned(),
                        "unknown element -- skipped",
                    )),
                }
            }
            Ok(Event::End(el)) => {
                if el.name().as_ref() == b"preset" {
                    if let Some(done) = current.take() {
                        presets.push(done);
                    }
                }
            }
            Ok(_) => {}
        }
    }
    if let Some(done) = current.take() {
        presets.push(done);
    }

    if presets.is_empty() {
        warnings.push(Warning::file_level(file, "no presets found"));
    }
    for preset in &presets {
        if preset.name == Preset::DEFAULT_NAME || preset.name == Preset::EMPTY_NAME {
            warnings.push(Warning::in_preset(
                file,
                &preset.name,
                "name collides with a ksx built-in preset; the imported file will shadow it",
            ));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    for preset in &presets {
        let lowered = preset.name.to_lowercase();
        if seen.contains(&lowered) {
            warnings.push(Warning::in_preset(
                file,
                &preset.name,
                "duplicate preset name; the later one overwrites the earlier on disk",
            ));
        } else {
            seen.push(lowered);
        }
    }

    ParsedPresets {
        presets: presets
            .iter()
            .map(ksx_config::PresetFile::from_core)
            .collect(),
        warnings,
    }
}

fn preset_name(el: &quick_xml::events::BytesStart<'_>) -> Result<String, String> {
    let attrs = xml::attrs(el)?;
    // v2 lowercase / v1 capitalized.
    xml::attr(&attrs, "name")
        .or_else(|| xml::attr(&attrs, "Name"))
        .map(str::to_owned)
        .ok_or_else(|| "<preset> without a name attribute".to_owned())
}

/// One binding element -> `(Key, Binding)`. `Ok(None)` = key was `None`
/// (function present, unbound -- omitted). `Err` = unmappable, warn + skip.
fn parse_binding(
    element: &str,
    attrs: &[(String, String)],
    key_text: &str,
) -> Result<Option<(Key, Binding)>, String> {
    let key = Key::from_name(key_text)
        .ok_or_else(|| format!("unknown key name '{key_text}' (exact legacy spelling required)"))?;

    let binding = match element {
        "button" => Binding::Button(parse_button(attrs)?),
        "trigger" => Binding::Trigger(parse_trigger(attrs)?),
        "axis" => parse_axis(attrs)?,
        "dpad" | "pov" => Binding::Dpad(parse_dpad(attrs)?),
        "custom" => parse_custom(attrs)?,
        other => return Err(format!("unknown binding element <{other}>")),
    };

    if key == Key::None {
        return Ok(None);
    }
    Ok(Some((key, binding)))
}

fn numeric<T: std::str::FromStr>(value: &str, what: &str) -> Result<T, String> {
    value
        .parse::<T>()
        .map_err(|_| format!("{what} '{value}' is not a valid number"))
}

fn parse_button(attrs: &[(String, String)]) -> Result<XButton, String> {
    if let Some(id) = xml::attr(attrs, "id") {
        let id: u32 = numeric(id, "button id")?;
        return XButton::from_legacy_id(id).ok_or_else(|| format!("unknown button id {id}"));
    }
    if let Some(name) = xml::attr(attrs, "ID") {
        return button_by_name(name).ok_or_else(|| format!("unknown v1 button name '{name}'"));
    }
    Err("button element without id/ID attribute".to_owned())
}

fn button_by_name(name: &str) -> Option<XButton> {
    Some(match name {
        "Start" => XButton::Start,
        "Back" => XButton::Back,
        "LeftThumb" => XButton::LeftThumb,
        "RightThumb" => XButton::RightThumb,
        "LeftBumper" => XButton::LeftBumper,
        "RightBumper" => XButton::RightBumper,
        "Guide" => XButton::Guide,
        "A" => XButton::A,
        "B" => XButton::B,
        "X" => XButton::X,
        "Y" => XButton::Y,
        _ => return None,
    })
}

fn parse_trigger(attrs: &[(String, String)]) -> Result<Trigger, String> {
    if let Some(id) = xml::attr(attrs, "id") {
        let id: u32 = numeric(id, "trigger id")?;
        return Trigger::from_legacy_id(id).ok_or_else(|| format!("unknown trigger id {id}"));
    }
    if let Some(name) = xml::attr(attrs, "ID") {
        // v1 files predate the Left/Right rename (PresetUpgrader.cs rewrote
        // `LeftTrigger`→`Left` textually); accept both spellings.
        return match name {
            "Left" | "LeftTrigger" => Ok(Trigger::Left),
            "Right" | "RightTrigger" => Ok(Trigger::Right),
            other => Err(format!("unknown v1 trigger name '{other}'")),
        };
    }
    Err("trigger element without id/ID attribute".to_owned())
}

fn parse_axis(attrs: &[(String, String)]) -> Result<Binding, String> {
    if let Some(id) = xml::attr(attrs, "id") {
        let id: u32 = numeric(id, "axis id")?;
        let axis = Axis::from_legacy_id(id).ok_or_else(|| format!("unknown axis id {id}"))?;
        let value = xml::attr(attrs, "value")
            .ok_or_else(|| "axis element without value attribute".to_owned())?;
        let value: i16 = numeric(value, "axis value")?;
        return Ok(Binding::Axis { axis, value });
    }
    if let Some(name) = xml::attr(attrs, "ID") {
        let axis = match name {
            "X" => Axis::X,
            "Y" => Axis::Y,
            "Rx" => Axis::Rx,
            "Ry" => Axis::Ry,
            other => return Err(format!("unknown v1 axis name '{other}'")),
        };
        let position = xml::attr(attrs, "Position")
            .ok_or_else(|| "v1 axis element without Position attribute".to_owned())?;
        let value = match position {
            "Min" => AXIS_MIN,
            "Center" => 0,
            "Max" => AXIS_MAX,
            other => numeric(other, "axis Position")?,
        };
        return Ok(Binding::Axis { axis, value });
    }
    Err("axis element without id/ID attribute".to_owned())
}

fn parse_dpad(attrs: &[(String, String)]) -> Result<DpadDirection, String> {
    if let Some(direction) = xml::attr(attrs, "direction") {
        let direction: i32 = numeric(direction, "dpad direction")?;
        return DpadDirection::from_legacy_id(direction)
            .ok_or_else(|| format!("unknown dpad direction {direction} (0/Off is not bindable)"));
    }
    if let Some(name) = xml::attr(attrs, "ID") {
        return match name {
            "Up" => Ok(DpadDirection::Up),
            "Down" => Ok(DpadDirection::Down),
            "Left" => Ok(DpadDirection::Left),
            "Right" => Ok(DpadDirection::Right),
            other => Err(format!(
                "unknown v1 dpad direction '{other}' (Off is not bindable)"
            )),
        };
    }
    Err("dpad element without direction/ID attribute".to_owned())
}

fn parse_custom(attrs: &[(String, String)]) -> Result<Binding, String> {
    if let Some(function) = xml::attr(attrs, "function") {
        let function: u32 = numeric(function, "custom function")?;
        return expand_custom(function)
            .ok_or_else(|| format!("unknown custom function id 0x{function:X}"));
    }
    if let Some(name) = xml::attr(attrs, "ID") {
        let function = custom_function_by_name(name)
            .ok_or_else(|| format!("unknown v1 custom function name '{name}'"))?;
        return Ok(expand_custom(function).expect("name table only contains valid ids"));
    }
    Err("custom element without function/ID attribute".to_owned())
}

/// Expand a flat `XboxCustomFunction` id into a plain [`Binding`]
/// (`legacy/VirtualXbox/Enums/XboxCustomFunction.cs`, bit-exact). Axis custom
/// functions always drive full-scale Min/Max (`CustomFunctionHelper.cs`).
fn expand_custom(function: u32) -> Option<Binding> {
    if function <= 0x8 {
        return DpadDirection::from_legacy_id(function as i32).map(Binding::Dpad);
    }
    if let Some(button) = XButton::from_legacy_id(function) {
        return Some(Binding::Button(button));
    }
    if let Some(trigger) = Trigger::from_legacy_id(function) {
        return Some(Binding::Trigger(trigger));
    }
    let (axis, value) = match function {
        0x0100000 => (Axis::X, AXIS_MIN),
        0x0200000 => (Axis::X, AXIS_MAX),
        0x0400000 => (Axis::Y, AXIS_MIN),
        0x0800000 => (Axis::Y, AXIS_MAX),
        0x1000000 => (Axis::Rx, AXIS_MIN),
        0x2000000 => (Axis::Rx, AXIS_MAX),
        0x4000000 => (Axis::Ry, AXIS_MIN),
        0x8000000 => (Axis::Ry, AXIS_MAX),
        _ => return None,
    };
    Some(Binding::Axis { axis, value })
}

fn custom_function_by_name(name: &str) -> Option<u32> {
    Some(match name {
        "Dpad_Up" => 0x0000001,
        "Dpad_Down" => 0x0000002,
        "Dpad_Left" => 0x0000004,
        "Dpad_Right" => 0x0000008,
        "Button_Start" => 0x0000010,
        "Button_Back" => 0x0000020,
        "Left_Thumb" => 0x0000040,
        "Right_Thumb" => 0x0000080,
        "Left_Bumper" => 0x0000100,
        "Right_Bumper" => 0x0000200,
        "Button_Guide" => 0x0000400,
        "Button_A" => 0x0001000,
        "Button_B" => 0x0002000,
        "Button_X" => 0x0004000,
        "Button_Y" => 0x0008000,
        "Left_Trigger" => 0x0010000,
        "Right_Trigger" => 0x0020000,
        "Axis_X_Min" => 0x0100000,
        "Axis_X_Max" => 0x0200000,
        "Axis_Y_Min" => 0x0400000,
        "Axis_Y_Max" => 0x0800000,
        "Axis_Rx_Min" => 0x1000000,
        "Axis_Rx_Max" => 0x2000000,
        "Axis_Ry_Min" => 0x4000000,
        "Axis_Ry_Max" => 0x8000000,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live cabinet XML stores full negative deflection as
    /// `value="-32768"`. That is `i16::MIN`, which ksx never puts in a preset
    /// (see `ksx_core::AXIS_MIN`) — so an imported axis must come out spelled
    /// `lx.min`, not as the raw number. The fold lives in `ksx-config`'s
    /// function naming; this pins that the importer actually gets it.
    #[test]
    fn legacy_i16_min_axis_imports_as_min() {
        // Verbatim shape of the live cab XML (tests/fixtures/splitter_presets.xml).
        let xml = r#"<preset_data>
            <preset name="Cab">
                <axis id="1" value="-32768">M</axis>
                <axis id="2" value="-32768">K</axis>
                <axis id="1" value="32767">N</axis>
            </preset>
        </preset_data>"#;
        let parsed = parse_presets(xml, "splitter_presets.xml");
        assert!(parsed.warnings.is_empty(), "{:#?}", parsed.warnings);

        let rendered = toml::to_string(&parsed.presets[0]).expect("render");
        assert!(rendered.contains(r#""lx.min" = "M""#), "{rendered}");
        assert!(rendered.contains(r#""ly.min" = "K""#), "{rendered}");
        assert!(rendered.contains(r#""lx.max" = "N""#), "{rendered}");
        assert!(!rendered.contains("-32768"), "{rendered}");
    }

    #[test]
    fn custom_expansion_is_bit_exact() {
        // Every XboxCustomFunction value, verbatim from the legacy enum.
        let table: &[(u32, Binding)] = &[
            (0x0000001, Binding::Dpad(DpadDirection::Up)),
            (0x0000002, Binding::Dpad(DpadDirection::Down)),
            (0x0000004, Binding::Dpad(DpadDirection::Left)),
            (0x0000008, Binding::Dpad(DpadDirection::Right)),
            (0x0000010, Binding::Button(XButton::Start)),
            (0x0000020, Binding::Button(XButton::Back)),
            (0x0000040, Binding::Button(XButton::LeftThumb)),
            (0x0000080, Binding::Button(XButton::RightThumb)),
            (0x0000100, Binding::Button(XButton::LeftBumper)),
            (0x0000200, Binding::Button(XButton::RightBumper)),
            (0x0000400, Binding::Button(XButton::Guide)),
            (0x0001000, Binding::Button(XButton::A)),
            (0x0002000, Binding::Button(XButton::B)),
            (0x0004000, Binding::Button(XButton::X)),
            (0x0008000, Binding::Button(XButton::Y)),
            (0x0010000, Binding::Trigger(Trigger::Left)),
            (0x0020000, Binding::Trigger(Trigger::Right)),
            (
                0x0100000,
                Binding::Axis {
                    axis: Axis::X,
                    value: AXIS_MIN,
                },
            ),
            (
                0x0200000,
                Binding::Axis {
                    axis: Axis::X,
                    value: AXIS_MAX,
                },
            ),
            (
                0x0400000,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MIN,
                },
            ),
            (
                0x0800000,
                Binding::Axis {
                    axis: Axis::Y,
                    value: AXIS_MAX,
                },
            ),
            (
                0x1000000,
                Binding::Axis {
                    axis: Axis::Rx,
                    value: AXIS_MIN,
                },
            ),
            (
                0x2000000,
                Binding::Axis {
                    axis: Axis::Rx,
                    value: AXIS_MAX,
                },
            ),
            (
                0x4000000,
                Binding::Axis {
                    axis: Axis::Ry,
                    value: AXIS_MIN,
                },
            ),
            (
                0x8000000,
                Binding::Axis {
                    axis: Axis::Ry,
                    value: AXIS_MAX,
                },
            ),
        ];
        assert_eq!(table.len(), 25);
        for (id, expected) in table {
            assert_eq!(expand_custom(*id).as_ref(), Some(expected), "id 0x{id:X}");
        }
        for bad in [0u32, 3, 5, 0x800, 0x40000, 0x80000, 0x10000000, u32::MAX] {
            assert_eq!(expand_custom(bad), None, "id 0x{bad:X} must not map");
        }
    }

    #[test]
    fn v1_name_table_matches_numeric_expansion() {
        for name in [
            "Dpad_Up",
            "Dpad_Down",
            "Dpad_Left",
            "Dpad_Right",
            "Button_Start",
            "Button_Back",
            "Left_Thumb",
            "Right_Thumb",
            "Left_Bumper",
            "Right_Bumper",
            "Button_Guide",
            "Button_A",
            "Button_B",
            "Button_X",
            "Button_Y",
            "Left_Trigger",
            "Right_Trigger",
            "Axis_X_Min",
            "Axis_X_Max",
            "Axis_Y_Min",
            "Axis_Y_Max",
            "Axis_Rx_Min",
            "Axis_Rx_Max",
            "Axis_Ry_Min",
            "Axis_Ry_Max",
        ] {
            let id = custom_function_by_name(name).unwrap();
            assert!(expand_custom(id).is_some(), "{name} (0x{id:X})");
        }
        assert_eq!(custom_function_by_name("Button_Z"), None);
    }

    #[test]
    fn none_key_omits_the_entry() {
        let attrs = vec![("id".to_owned(), "4096".to_owned())];
        assert_eq!(parse_binding("button", &attrs, "None"), Ok(None));
        assert_eq!(
            parse_binding("button", &attrs, "G"),
            Ok(Some((Key::G, Binding::Button(XButton::A))))
        );
    }

    #[test]
    fn unknown_key_is_an_error_not_a_none() {
        let attrs = vec![("id".to_owned(), "4096".to_owned())];
        assert!(parse_binding("button", &attrs, "NoSuchKey").is_err());
        assert!(parse_binding("button", &attrs, "").is_err());
    }
}
