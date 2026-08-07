//! `ksx device scan` — what is plugged in, grouped the way a person thinks
//! about it.
//!
//! # Why this exists beside `ksx devices`
//!
//! `ksx devices` answers "what can the capture layer see", one row per devnode,
//! and that is the right shape for diagnosing a backend. It is the wrong shape
//! for CHOOSING. On the reference cabinet it prints 29 USB interfaces; three of
//! them are one I-PAC, one is a trackball, and the rest are a mouse, an AURA
//! controller, a fan controller, an audio device and — until `ksx pads --prune`
//! is run — fifteen of ksx's own virtual pads.
//!
//! Nobody picks a keyboard out of that. So this groups interfaces into the
//! physical boards they belong to, names each board, says which one interface
//! is the keyboard, and shows what ksx would write if you picked it.
//!
//! Read-only and daemon-free. It enumerates and prints; it opens nothing,
//! claims nothing, and writes nothing. `ksx device pick` is the writer, and it
//! is a separate verb precisely so that looking is never a commitment.

use ksx_api::{DevicesView, UsbRow};

/// One physical board: every interface that shares a composite parent.
///
/// The grouping key is [`UsbRow::board`], which the enumerator gets for free —
/// an I-PAC's `MI_00`/`MI_01`/`MI_02` are one device to a human and three
/// devnodes to Windows, and they share that parent.
pub struct Board<'a> {
    /// What to call it: the vendor table's name, else the device's own product
    /// string, else the parent path. Never empty.
    pub name: String,
    pub interfaces: Vec<&'a UsbRow>,
}

impl<'a> Board<'a> {
    /// The interface a keyboard would be captured through, if any.
    ///
    /// Order matters and each step is a different question. A board ksx already
    /// HOLDS reads as the one it holds. Otherwise prefer an interface that
    /// declares the boot keyboard protocol, because "claimable" only means "it
    /// is HID" — on this cabinet a mouse, an LED controller and a fan
    /// controller all satisfy that. Only then fall back to any HID interface.
    pub fn keyboard(&self) -> Option<&'a UsbRow> {
        self.interfaces
            .iter()
            .find(|r| r.state == "claimed")
            .or_else(|| {
                self.interfaces
                    .iter()
                    .find(|r| r.state == "claimable" && r.boot_keyboard)
            })
            .or_else(|| self.interfaces.iter().find(|r| r.state == "claimable"))
            .copied()
    }

    /// Does anything on this board say it is a keyboard?
    ///
    /// Used to sort probable keyboards to the top and to word the rest
    /// honestly, rather than to exclude them — a board that reports protocol 0
    /// can still be a perfectly good NKRO keyboard.
    pub fn looks_like_a_keyboard(&self) -> bool {
        self.interfaces
            .iter()
            .any(|r| r.boot_keyboard || r.state == "claimed")
    }

    /// Is any interface of this board bound to a `[[device]]` entry?
    pub fn alias(&self) -> Option<&'a str> {
        self.interfaces.iter().find_map(|r| r.alias.as_deref())
    }
}

/// Group a view's interfaces into physical boards, in first-seen order.
///
/// Order is deliberately the enumerator's rather than alphabetical: it is
/// stable across runs on one machine, and it puts a board where the user last
/// saw it. Sorting by name would reshuffle the list the moment a device is
/// renamed by a vendor-table edit.
pub fn boards(view: &DevicesView) -> Vec<Board<'_>> {
    let mut out: Vec<Board<'_>> = Vec::new();
    for row in &view.usb {
        let key = row.board.as_deref().unwrap_or(row.instance_id.as_str());
        if let Some(existing) = out
            .iter_mut()
            .find(|b| b.interfaces.first().is_some_and(|f| board_key(f) == key))
        {
            existing.interfaces.push(row);
            continue;
        }
        out.push(Board {
            name: name_of(row, key),
            interfaces: vec![row],
        });
    }
    out
}

fn board_key(row: &UsbRow) -> &str {
    row.board.as_deref().unwrap_or(row.instance_id.as_str())
}

/// Best available name, in the order a human would prefer it.
fn name_of(row: &UsbRow, key: &str) -> String {
    if let Some(vendor) = &row.vendor {
        return vendor.clone();
    }
    if !row.description.is_empty() {
        return row.description.clone();
    }
    key.to_owned()
}

/// Should this board appear without `--all`?
///
/// A board with no keyboard interface cannot be picked, so listing it in a
/// picker is noise — and on this cabinet it is *most* of the list. `--all`
/// exists because "ksx cannot see my board" is a real support question and the
/// answer is sometimes "it is there, it is just not a keyboard".
fn is_pickable(board: &Board<'_>) -> bool {
    board.keyboard().is_some()
}

/// The human report.
pub fn render(view: &DevicesView, all: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let boards = boards(view);
    let mut shown: Vec<&Board<'_>> = boards.iter().filter(|b| all || is_pickable(b)).collect();
    // Probable keyboards first. A stable sort keeps enumeration order within
    // each group, so a board stays where the user last saw it.
    shown.sort_by_key(|b| !b.looks_like_a_keyboard());

    let _ = writeln!(
        out,
        "Input devices ksx can see (read-only; nothing was opened or claimed)\n"
    );

    if shown.is_empty() {
        let _ = writeln!(
            out,
            "  No keyboard-capable boards found.{}",
            if boards.is_empty() {
                ""
            } else {
                " Re-run with --all to see every interface."
            }
        );
    }

    for board in &shown {
        let alias = board
            .alias()
            .map(|a| format!("  (configured as \"{a}\")"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {}{alias}\n    {} interface(s)",
            board.name,
            board.interfaces.len()
        );
        for row in &board.interfaces {
            // The state word is the CLI's own vocabulary, shared with
            // `ksx devices` and the cabinet — one interface, one description.
            let mark = match row.state.as_str() {
                "claimed" => "*",
                "claimable" => "-",
                _ => " ",
            };
            let _ = writeln!(out, "      {mark} {} [{}]", row.instance_id, row.state);
        }
        match board.keyboard() {
            Some(kb) => {
                let _ = writeln!(out, "    keyboard : {}", kb.instance_id);
                let _ = writeln!(out, "    {}", kb.verdict);
                if !board.looks_like_a_keyboard() {
                    // The honest caveat. Saying "ksx could claim it" and
                    // stopping would read as a recommendation, and claiming a
                    // fan controller's HID interface is a mistake nobody wants
                    // to make from a menu.
                    let _ = writeln!(
                        out,
                        "    NOT declared as a keyboard — this is an HID interface and \
                         may be something else entirely"
                    );
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "    no keyboard interface — ksx cannot capture this board"
                );
            }
        }
        out.push('\n');
    }

    let hidden = boards.len() - shown.len();
    if hidden > 0 && !all {
        let _ = writeln!(
            out,
            "{hidden} board(s) with no keyboard interface not shown — `--all` lists them.\n"
        );
    }

    for note in &view.notes {
        let _ = writeln!(out, "NOTE: {note}");
    }
    if !view.notes.is_empty() {
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "To use one: `ksx device pick <ID>` writes the config entry. It never \
         claims — claiming is `ksx winusb claim`, which stays a dry run until \
         --yes."
    );
    out
}

#[cfg(windows)]
pub fn run(all: bool, json: bool) -> anyhow::Result<()> {
    let view = crate::devices::to_view(&crate::devices::collect());
    if json {
        println!("{}", serde_json::to_string_pretty(&view)?);
    } else {
        print!("{}", render(&view, all));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_all: bool, _json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx device scan` enumerates Windows USB devices and is Windows-only")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, board: &str, state: &str, vendor: Option<&str>) -> UsbRow {
        UsbRow {
            instance_id: id.to_owned(),
            description: "USB Input Device".to_owned(),
            state: state.to_owned(),
            verdict: "on the keyboard stack; ksx could claim it".to_owned(),
            alias: None,
            selected: false,
            ready: false,
            boot_keyboard: state == "claimed",
            vendor: vendor.map(str::to_owned),
            board: Some(board.to_owned()),
        }
    }

    /// The cabinet, as it actually enumerates: one I-PAC wearing three
    /// devnodes, plus a trackball. A picker that offered four rows here would
    /// be asking the user to know which `MI_` number is the keyboard.
    fn cabinet() -> DevicesView {
        DevicesView {
            generated_at: "t".into(),
            keyboards: Vec::new(),
            interception_available: true,
            usb: vec![
                row(
                    r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000",
                    r"USB\VID_D209&PID_0430\4",
                    "claimed",
                    Some("Ultimarc I-PAC 4X"),
                ),
                row(
                    r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001",
                    r"USB\VID_D209&PID_0430\4",
                    "not-a-keyboard",
                    Some("Ultimarc I-PAC 4X"),
                ),
                row(
                    r"USB\VID_D209&PID_0430&MI_02\7&25EEA38C&0&0002",
                    r"USB\VID_D209&PID_0430\4",
                    "not-a-keyboard",
                    Some("Ultimarc I-PAC 4X"),
                ),
                row(
                    r"USB\VID_D209&PID_15A2\6",
                    r"USB\VID_D209&PID_15A2\6",
                    "claimable",
                    Some("Ultimarc SpinTrak"),
                ),
            ],
            usb_available: true,
            notes: Vec::new(),
        }
    }

    #[test]
    fn three_devnodes_of_one_board_are_one_entry() {
        let view = cabinet();
        let boards = boards(&view);
        assert_eq!(boards.len(), 2, "one I-PAC and one SpinTrak");
        assert_eq!(boards[0].name, "Ultimarc I-PAC 4X");
        assert_eq!(boards[0].interfaces.len(), 3);
        assert_eq!(boards[1].name, "Ultimarc SpinTrak");
        assert_eq!(boards[1].interfaces.len(), 1);
    }

    /// The board ksx already holds must read as the one it holds — not as
    /// whichever interface happened to enumerate first.
    #[test]
    fn the_keyboard_interface_is_the_claimed_one_when_there_is_one() {
        let view = cabinet();
        let boards = boards(&view);
        let kb = boards[0].keyboard().expect("the I-PAC has a keyboard");
        assert!(kb.instance_id.contains("MI_00"));
        assert_eq!(kb.state, "claimed");
    }

    /// A board with no keyboard interface cannot be picked, so it is noise in a
    /// picker — but "ksx cannot see my board" is a real support question, and
    /// `--all` is the answer to it.
    #[test]
    fn unpickable_boards_are_hidden_by_default_and_counted() {
        let mut view = cabinet();
        // Turn the SpinTrak into something with no keyboard interface.
        view.usb[3].state = "not-a-keyboard".into();

        let text = render(&view, false);
        assert!(text.contains("I-PAC"), "{text}");
        assert!(!text.contains("SpinTrak"), "hidden by default: {text}");
        assert!(
            text.contains("1 board(s) with no keyboard interface not shown"),
            "and COUNTED, so nothing vanishes silently: {text}"
        );

        let all = render(&view, true);
        assert!(all.contains("SpinTrak"), "--all shows it: {all}");
    }

    /// Looking must never read as committing. The report names the writer and
    /// says plainly that it does not claim.
    #[test]
    fn the_report_separates_looking_from_picking_from_claiming() {
        let text = render(&cabinet(), false);
        assert!(text.contains("ksx device pick"), "{text}");
        assert!(text.contains("never"), "{text}");
        assert!(text.contains("ksx winusb claim"), "{text}");
        assert!(
            text.contains("nothing was opened or claimed"),
            "the header must say the scan itself is read-only: {text}"
        );
    }

    /// The finding that came from running this on real hardware, not from a
    /// fixture: a mouse, an LED controller, a fan controller and a USB audio
    /// device all carry HID interfaces, so all four read as "claimable" and
    /// the picker offered every one of them as a keyboard.
    ///
    /// They are still listed — an NKRO board can report protocol 0 and be a
    /// perfectly good keyboard, so excluding them would hide real hardware —
    /// but they sort BELOW the real keyboards and say what they are.
    #[test]
    fn an_hid_interface_that_is_not_a_keyboard_is_ranked_last_and_says_so() {
        let mut view = cabinet();
        // A fan controller: HID, claimable, and not a keyboard by any measure.
        view.usb.insert(
            0,
            UsbRow {
                boot_keyboard: false,
                ..row(
                    r"USB\VID_1E71&PID_300E&MI_01\7&8FBF878&0&0001",
                    r"USB\VID_1E71&PID_300E\5",
                    "claimable",
                    None,
                )
            },
        );
        // And make the I-PAC's keyboard interface declare itself as one.
        view.usb[1].boot_keyboard = true;

        let text = render(&view, false);
        let fan = text.find("USB Input Device").expect("the fan is listed");
        let ipac = text.find("Ultimarc I-PAC 4X").expect("the I-PAC is listed");
        assert!(
            ipac < fan,
            "a real keyboard must rank above an HID device that merely could be \
             claimed:\n{text}"
        );
        assert!(
            text.contains("NOT declared as a keyboard"),
            "and the caveat must be on the page, or 'ksx could claim it' reads as \
             a recommendation:\n{text}"
        );
    }

    #[test]
    fn a_machine_with_no_pickable_board_says_so_rather_than_printing_nothing() {
        let mut view = cabinet();
        for row in &mut view.usb {
            row.state = "not-a-keyboard".into();
        }
        let text = render(&view, false);
        assert!(text.contains("No keyboard-capable boards found"), "{text}");
        assert!(text.contains("--all"), "and how to see the rest: {text}");
    }
}
