//! `splitter_settings.xml` -> [`ksx_config::Settings`].
//!
//! Legacy schema (`legacy/KeyboardSplitter/GlobalSettings.cs`, root
//! `<SplitterSettings>`): `MouseMoveDeadZone` (0..=12),
//! `StartingVirtualControllerUserIndex` (1..=4), plus two UI-only toggles —
//! `DisplayEmulationInformation` and `SuggestInputDevicesForNewSlots` -- that
//! have no ksx equivalent and are dropped silently (known, documented).
//! Block-keyboards/block-mice were runtime state in legacy, so they keep their
//! ksx defaults (true/false).

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::report::Warning;
use crate::xml;

pub struct ParsedSettings {
    pub settings: ksx_config::Settings,
    pub warnings: Vec<Warning>,
}

/// Parse a decoded settings XML document. Out-of-range values are clamped with
/// a warning; unknown elements warn (legacy threw on any unknown node).
pub fn parse_settings(text: &str, file: &str) -> ParsedSettings {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut settings = ksx_config::Settings::default();
    let mut warnings = Vec::new();
    let mut recognized = 0usize;

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
                if name == "SplitterSettings" {
                    continue;
                }
                let value = match xml::element_text(&mut reader, &el) {
                    Ok(v) => v,
                    Err(reason) => {
                        warnings.push(Warning::entry(
                            file,
                            None,
                            name.clone(),
                            format!("{reason} -- element skipped"),
                        ));
                        continue;
                    }
                };
                match name.as_str() {
                    "MouseMoveDeadZone" => {
                        recognized += 1;
                        match value.parse::<u8>() {
                            Ok(v) if v <= 12 => settings.mouse_move_deadzone = v,
                            Ok(v) => {
                                settings.mouse_move_deadzone = 12;
                                warnings.push(Warning::entry(
                                    file,
                                    None,
                                    name.clone(),
                                    format!("value {v} out of range 0..=12 -- clamped to 12"),
                                ));
                            }
                            Err(_) => warnings.push(Warning::entry(
                                file,
                                None,
                                name.clone(),
                                format!("value '{value}' is not a number -- default kept"),
                            )),
                        }
                    }
                    "StartingVirtualControllerUserIndex" => {
                        recognized += 1;
                        match value.parse::<u8>() {
                            Ok(v @ 1..=4) => settings.starting_user_index = v,
                            Ok(v) => {
                                settings.starting_user_index = v.clamp(1, 4);
                                warnings.push(Warning::entry(
                                    file,
                                    None,
                                    name.clone(),
                                    format!(
                                        "value {v} out of range 1..=4 -- clamped to {}",
                                        settings.starting_user_index
                                    ),
                                ));
                            }
                            Err(_) => warnings.push(Warning::entry(
                                file,
                                None,
                                name.clone(),
                                format!("value '{value}' is not a number -- default kept"),
                            )),
                        }
                    }
                    // UI-only legacy toggles with no ksx equivalent.
                    "DisplayEmulationInformation" | "SuggestInputDevicesForNewSlots" => {
                        recognized += 1;
                    }
                    other => warnings.push(Warning::entry(
                        file,
                        None,
                        other.to_owned(),
                        "unknown settings element -- skipped",
                    )),
                }
            }
            Ok(_) => {}
        }
    }

    if recognized == 0 {
        warnings.push(Warning::file_level(file, "no recognized settings found"));
    }

    ParsedSettings { settings, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAB: &str = r#"<?xml version="1.0" encoding="utf-16"?>
<SplitterSettings xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <MouseMoveDeadZone>1</MouseMoveDeadZone>
  <DisplayEmulationInformation>false</DisplayEmulationInformation>
  <SuggestInputDevicesForNewSlots>true</SuggestInputDevicesForNewSlots>
  <StartingVirtualControllerUserIndex>1</StartingVirtualControllerUserIndex>
</SplitterSettings>"#;

    #[test]
    fn cab_settings_map_cleanly() {
        let parsed = parse_settings(CAB, "splitter_settings.xml");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        // The importer carries the real legacy dead-zone (1), not the ksx
        // default (5).
        assert_eq!(parsed.settings.mouse_move_deadzone, 1);
        assert_eq!(parsed.settings.starting_user_index, 1);
        assert!(parsed.settings.block_keyboards);
        assert!(!parsed.settings.block_mice);
    }

    #[test]
    fn out_of_range_values_clamp_with_warning() {
        let xml = "<SplitterSettings><MouseMoveDeadZone>99</MouseMoveDeadZone>\
                   <StartingVirtualControllerUserIndex>7</StartingVirtualControllerUserIndex>\
                   </SplitterSettings>";
        let parsed = parse_settings(xml, "f.xml");
        assert_eq!(parsed.settings.mouse_move_deadzone, 12);
        assert_eq!(parsed.settings.starting_user_index, 4);
        assert_eq!(parsed.warnings.len(), 2);
    }

    #[test]
    fn unknown_elements_warn_but_do_not_abort() {
        let xml = "<SplitterSettings><FutureThing>1</FutureThing>\
                   <MouseMoveDeadZone>3</MouseMoveDeadZone></SplitterSettings>";
        let parsed = parse_settings(xml, "f.xml");
        assert_eq!(parsed.settings.mouse_move_deadzone, 3);
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.warnings[0].entry.as_deref(), Some("FutureThing"));
    }
}
