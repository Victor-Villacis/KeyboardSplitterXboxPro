//! `splitter_games.xml` -> [`ksx_config::GamesFile`].
//!
//! Legacy schema (`legacy/KeyboardSplitter/Models/{GameData,Game,SlotData}.cs`):
//! `<Games>` -> `<Game Title Notes Path Arguments BlockKeyboards BlockMice>` →
//! `<Slot Number GamepadUserIndex Keyboard Mouse Preset/>`. Device values are
//! hardware-id strings; they are carried into the TOML **verbatim** (legacy
//! already persisted HWIDs here). Empty `Keyboard`/`Mouse` attributes mean "no
//! device" and become absent fields.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::report::Warning;
use crate::xml;

pub struct ParsedGames {
    pub games: ksx_config::GamesFile,
    pub warnings: Vec<Warning>,
}

/// Parse a decoded games XML document. Unmappable slots/attributes warn and
/// are skipped; the rest of the file is still imported.
pub fn parse_games(text: &str, file: &str) -> ParsedGames {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut games: Vec<ksx_config::GameEntry> = Vec::new();
    let mut warnings = Vec::new();
    let mut current: Option<ksx_config::GameEntry> = None;

    loop {
        let event = reader.read_event();
        let (el, self_closing) = match &event {
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
            Ok(Event::Start(el)) => (el, false),
            Ok(Event::Empty(el)) => (el, true),
            Ok(Event::End(el)) => {
                if el.name().as_ref() == b"Game" {
                    if let Some(done) = current.take() {
                        games.push(done);
                    }
                }
                continue;
            }
            Ok(_) => continue,
        };

        let name = String::from_utf8_lossy(el.name().as_ref()).into_owned();
        let mut skip_subtree = false;
        match name.as_str() {
            "Games" => {}
            "Game" => {
                if let Some(done) = current.take() {
                    games.push(done);
                }
                match parse_game(el, file, &mut warnings) {
                    Some(game) if self_closing => games.push(game),
                    Some(game) => current = Some(game),
                    None => skip_subtree = true,
                }
            }
            "Slot" => {
                let game_title = current.as_ref().map(|g| g.title.clone());
                match current.as_mut() {
                    Some(game) => {
                        if let Some(slot) =
                            parse_slot(el, file, game_title.as_deref(), &mut warnings)
                        {
                            game.slots.push(slot);
                        }
                    }
                    None => warnings.push(Warning::entry(
                        file,
                        None,
                        "Slot",
                        "<Slot> outside a <Game> -- skipped",
                    )),
                }
                skip_subtree = true;
            }
            other => {
                warnings.push(Warning::entry(
                    file,
                    current.as_ref().map(|g| g.title.as_str()),
                    other.to_owned(),
                    "unknown element -- skipped",
                ));
                skip_subtree = true;
            }
        }
        if skip_subtree && !self_closing {
            let end = el.name().to_owned();
            if reader.read_to_end(end).is_err() {
                break;
            }
        }
    }
    if let Some(done) = current.take() {
        games.push(done);
    }

    ParsedGames {
        games: ksx_config::GamesFile { games },
        warnings,
    }
}

fn parse_game(
    el: &BytesStart<'_>,
    file: &str,
    warnings: &mut Vec<Warning>,
) -> Option<ksx_config::GameEntry> {
    let attrs = match xml::attrs(el) {
        Ok(attrs) => attrs,
        Err(reason) => {
            warnings.push(Warning::entry(
                file,
                None,
                "Game",
                format!("{reason} -- game skipped"),
            ));
            return None;
        }
    };
    let title = xml::attr(&attrs, "Title").unwrap_or_default().to_owned();
    let path = xml::attr(&attrs, "Path").unwrap_or_default().to_owned();
    if title.is_empty() {
        warnings.push(Warning::entry(
            file,
            None,
            xml::describe("Game", &attrs),
            "game has no Title",
        ));
    }
    if path.is_empty() {
        warnings.push(Warning::entry(
            file,
            None,
            xml::describe("Game", &attrs),
            "game has no Path",
        ));
    }
    let parse_flag = |name: &str, default: bool, warnings: &mut Vec<Warning>| -> bool {
        match xml::attr(&attrs, name) {
            None => default,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                other => {
                    warnings.push(Warning::entry(
                        file,
                        Some(&title),
                        format!("{name}=\"{other}\""),
                        format!("not a boolean -- default {default} used"),
                    ));
                    default
                }
            },
        }
    };
    let block_keyboards = parse_flag("BlockKeyboards", true, warnings);
    let block_mice = parse_flag("BlockMice", false, warnings);
    Some(ksx_config::GameEntry {
        title,
        notes: xml::attr(&attrs, "Notes").unwrap_or_default().to_owned(),
        path,
        arguments: xml::attr(&attrs, "Arguments")
            .unwrap_or_default()
            .to_owned(),
        // No legacy equivalent: the C# app had no launcher-handoff tracking at
        // all (a short-lived launch simply meant "never report an exit"). An
        // imported profile therefore starts without one, and `ksx run --game`
        // tells the user which line to add if it turns out to need it.
        process_name: None,
        // Same reasoning: legacy had no notion of a launcher grace at all, so an
        // imported profile takes ksx's default and can be tuned per profile
        // afterwards if its launcher turns out to be slower still.
        launcher_grace_ms: None,
        block_keyboards,
        block_mice,
        slots: Vec::new(),
    })
}

fn parse_slot(
    el: &BytesStart<'_>,
    file: &str,
    game: Option<&str>,
    warnings: &mut Vec<Warning>,
) -> Option<ksx_config::GameSlotEntry> {
    let attrs = match xml::attrs(el) {
        Ok(attrs) => attrs,
        Err(reason) => {
            warnings.push(Warning::entry(
                file,
                game,
                "Slot",
                format!("{reason} -- slot skipped"),
            ));
            return None;
        }
    };
    let number = match xml::attr(&attrs, "Number").map(str::parse::<u8>) {
        Some(Ok(n)) => n,
        Some(Err(_)) | None => {
            warnings.push(Warning::entry(
                file,
                game,
                xml::describe("Slot", &attrs),
                "Slot Number is missing or not a number -- slot skipped",
            ));
            return None;
        }
    };
    let user_index = match xml::attr(&attrs, "GamepadUserIndex") {
        None => None,
        Some(v) => match v.parse::<u8>() {
            Ok(n) => Some(n),
            Err(_) => {
                warnings.push(Warning::entry(
                    file,
                    game,
                    xml::describe("Slot", &attrs),
                    format!("GamepadUserIndex '{v}' is not a number -- dropped"),
                ));
                None
            }
        },
    };
    // HWIDs verbatim; empty string = no device (legacy sentinel).
    let device = |name: &str| {
        xml::attr(&attrs, name)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let preset = match xml::attr(&attrs, "Preset") {
        Some(p) if !p.is_empty() => p.to_owned(),
        _ => {
            warnings.push(Warning::entry(
                file,
                game,
                xml::describe("Slot", &attrs),
                "slot has no Preset name",
            ));
            String::new()
        }
    };
    Some(ksx_config::GameSlotEntry {
        number,
        user_index,
        keyboard: device("Keyboard"),
        mouse: device("Mouse"),
        preset,
        // Legacy predates personas and only ever emulated Xbox 360 pads, so
        // every imported slot is exactly what it was. Same for SOCD, which
        // legacy had no concept of: `off` is "behave as legacy did".
        persona: ksx_core::Persona::default(),
        socd: ksx_core::Socd::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_mouse_becomes_absent_and_hwids_are_verbatim() {
        let xml = r#"<Games><Game Title="T" Path="C:\g.exe" BlockKeyboards="true" BlockMice="false">
            <Slot Number="1" GamepadUserIndex="1" Keyboard="HID\VID_D209&amp;PID_0430&amp;REV_0056&amp;MI_00" Mouse="" Preset="P"/>
            </Game></Games>"#;
        let parsed = parse_games(xml, "f.xml");
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let slot = &parsed.games.games[0].slots[0];
        assert_eq!(
            slot.keyboard.as_deref(),
            Some(r"HID\VID_D209&PID_0430&REV_0056&MI_00")
        );
        assert_eq!(slot.mouse, None);
        assert_eq!(slot.user_index, Some(1));
    }

    #[test]
    fn missing_flags_use_legacy_defaults() {
        let xml = r#"<Games><Game Title="T" Path="C:\g.exe"/></Games>"#;
        let parsed = parse_games(xml, "f.xml");
        assert!(parsed.warnings.is_empty());
        assert!(parsed.games.games[0].block_keyboards);
        assert!(!parsed.games.games[0].block_mice);
    }

    #[test]
    fn bad_slot_number_warns_and_skips_that_slot_only() {
        let xml = r#"<Games><Game Title="T" Path="C:\g.exe">
            <Slot Number="x" Preset="P"/>
            <Slot Number="2" Preset="P2"/>
            </Game></Games>"#;
        let parsed = parse_games(xml, "f.xml");
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.games.games[0].slots.len(), 1);
        assert_eq!(parsed.games.games[0].slots[0].number, 2);
    }
}
