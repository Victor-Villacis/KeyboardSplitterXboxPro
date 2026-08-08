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

use ksx_config::{Backend, ConfigFile, DeviceEntry, GamesFile};
use ksx_core::{DeviceFacts, Match};

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
                    .find(|r| capturable(r) && r.boot_keyboard)
            })
            .or_else(|| self.interfaces.iter().find(|r| capturable(r)))
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

/// Could ksx capture this devnode through SOME backend?
///
/// Reads the backend eligibility the enumerator already decided, never the
/// state word. `state == "claimable"` means one specific thing — a USB
/// interface on the keyboard stack that a claim could bind — and a Bluetooth
/// keyboard is not that, while being perfectly capturable through Interception
/// today. Matching on the state word here is what made a Bluetooth keyboard
/// unpickable in a picker whose own row said it could be captured.
fn capturable(row: &UsbRow) -> bool {
    row.interception_eligible || row.winusb_eligible
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

/// The transport word for a board's header line.
///
/// Read off the interface a pick would name, else the first devnode — the same
/// rule `ksx_api::DeviceScanView::read` follows for the typed view, so the
/// terminal and the browser cannot label one device two ways. `?` when the
/// collector set none, which is a bug in the collector and must LOOK like one
/// rather than silently reading as USB.
fn transport_label(board: &Board<'_>) -> &'static str {
    let row = board
        .keyboard()
        .or_else(|| board.interfaces.first().copied());
    match row.map(|r| r.transport.as_str()) {
        Some("usb") => "USB",
        Some("bluetooth") => "Bluetooth",
        _ => "?",
    }
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
        // The transport, on the header line, because it is the fact that
        // decides everything below it — and because a list that mixes USB and
        // Bluetooth without saying which is which is a list you cannot act on.
        let _ = writeln!(
            out,
            "  [{}] {}{alias}\n    {} interface(s)",
            transport_label(board),
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
                // Which backends this device is eligible for — the line
                // `ksx_core::Reach` composed. It is on EVERY row, not only the
                // surprising ones: a rule stated only where it bites reads as a
                // special case rather than as the rule.
                let _ = writeln!(out, "    backends : {}", kb.backends);
                // PRESENT is not TYPING. A paired Bluetooth keyboard with dead
                // batteries sits in the device tree all day, and someone
                // reading this list to find the spare keyboard a claim needs
                // has to see the difference before they claim their panel.
                //
                // Not for a CLAIMED board, though: that one cannot type to
                // Windows because somebody deliberately took it off the
                // keyboard stack, the row above already says `claimed`, and the
                // backends line already says why. Repeating it here as an
                // alarm would put a warning on the one state that is working
                // exactly as asked.
                if kb.state != "claimed" && !kb.can_type && !kb.cannot_type_reason.is_empty() {
                    let _ = writeln!(
                        out,
                        "               ^ {} — it is present and CANNOT type right now, so it \
                         does not count as a spare keyboard",
                        kb.cannot_type_reason
                    );
                }
                // **The line §5 promised and nothing printed.** "A legacy entry
                // that still resolves is never silently rewritten: `ksx device
                // scan` prints the stronger selector it *would* write and leaves
                // the decision with them." Two user-facing strings repeated that
                // promise — `DeviceSelector::explain` for a legacy path, and
                // `ResolveError::Missing` — while this report printed a board
                // name, an interface list, a state and a verdict, and no
                // selector at all.
                //
                // The value is computed in the backend, not here: same call the
                // writer makes, so the suggestion cannot diverge from what
                // `pick` would commit (`docs/SURFACES.md` §1).
                if let Some(selector) = &kb.selector {
                    let _ = writeln!(out, "    id       : '{selector}'");
                    if selector.contains(":port=") {
                        // A port pin is what `strongest_for` falls back to when
                        // nothing weaker separates two boards — so printing it
                        // IS the ambiguity marking, and the trade has to be
                        // stated at the moment it is offered
                        // (`docs/DEVICE-IDENTITY.md` §2).
                        let _ = writeln!(
                            out,
                            "               ^ pinned to this socket: another board of the same \
                             model is connected and nothing weaker tells them apart"
                        );
                    }
                }
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
        "To use one: `ksx device pick <ID>` writes the config entry — the `id` \
         shown above is what it would put in it. It never claims — claiming is \
         `ksx winusb claim`, which stays a dry run until --yes."
    );
    out
}

// ---------------------------------------------------------------------------
// the same report, typed — for every surface that is not a terminal
// ---------------------------------------------------------------------------

/// [`render`]'s facts, as [`ksx_api::DeviceScanView`].
///
/// A translation, not a second collector — the same rule `crate::devices::
/// to_view` follows. Everything a surface needs to draw a PICKER is decided
/// here, once: which interface is the keyboard, whether the board declares
/// itself one, the exact `ksx winusb claim` line, and — for each `[[device]]`
/// entry — whether its id still resolves, whether the board is claimed, which
/// slots would break if it were removed, and whether the id is port-pinned.
///
/// Pure and cross-platform on purpose (like [`crate::device_edit::plan_pick`]):
/// it takes the enumeration rather than performing it, so the whole shape is
/// exercised in CI against a synthetic device tree on any platform.
///
/// `connected` is the live USB enumeration a `[[device]]` selector resolves
/// against. It is a separate argument from `devices` because the two answer
/// different questions — `devices` describes DEVNODES (what is bound to what),
/// `connected` describes DEVICES (vendor, product, serial, socket), and only
/// the second can decide whether an id written months ago still names one
/// board and one board only.
pub fn view(
    devices: &DevicesView,
    connected: &[DeviceFacts],
    config: &ConfigFile,
    games: &GamesFile,
) -> ksx_api::DeviceScanView {
    let found = boards(devices);

    let mut rows: Vec<ksx_api::BoardRow> = found.iter().map(board_row).collect();
    // Probable keyboards first, then the boards ksx cannot capture at all. A
    // STABLE sort inside each group keeps enumeration order, so a board stays
    // where the user last saw it — the same rule `render` follows, for the
    // same reason.
    rows.sort_by_key(|b| (b.keyboard.is_none(), !b.looks_like_a_keyboard));

    let configured: Vec<ksx_api::ConfiguredDevice> = config
        .devices
        .iter()
        .map(|entry| configured_row(entry, devices, connected, config, games, &found))
        .collect();

    // Through `read`, never a struct literal: the partition, the counts, the
    // summary lines and the per-entry health verdicts are that constructor's
    // to decide, so no caller — this one, a test fixture, or a second surface
    // — can hand a page a summary that disagrees with the list beneath it.
    ksx_api::DeviceScanView::read(
        devices.generated_at.clone(),
        devices.usb_available,
        devices.bluetooth_available,
        rows,
        configured,
        devices.notes.clone(),
    )
}

fn board_row(board: &Board<'_>) -> ksx_api::BoardRow {
    let keyboard = board.keyboard();
    ksx_api::BoardRow {
        name: board.name.clone(),
        interfaces: board.interfaces.iter().map(|r| (*r).clone()).collect(),
        keyboard: keyboard.map(|r| r.instance_id.clone()),
        keyboard_verdict: match keyboard {
            Some(row) => row.verdict.clone(),
            None => "no keyboard interface — ksx cannot capture this board".to_owned(),
        },
        looks_like_a_keyboard: board.looks_like_a_keyboard(),
        claimed: keyboard.is_some_and(|r| r.state == "claimed"),
        alias: board.alias().map(str::to_owned),
        claim_command: keyboard
            .filter(|r| r.state == "claimable")
            .map(|r| claim_command(&r.instance_id)),
        release_command: keyboard
            .filter(|r| r.state == "claimed")
            .map(|r| release_command(&r.instance_id)),
        // Derived by `DeviceScanView::read` from the fields above — the
        // partition, the elevated lead that must accompany the command, the
        // HID caveat, and the transport/backend cell it copies off the
        // keyboard interface. Setting any of them here would be the same
        // decision taken twice.
        pickable: false,
        command_lead: String::new(),
        command: String::new(),
        caveat: String::new(),
        transport: String::new(),
        transport_label: String::new(),
        interception_eligible: false,
        winusb_eligible: false,
        backends: String::new(),
        can_type: false,
        cannot_type_reason: String::new(),
        cannot_type_line: String::new(),
        selector: None,
        alias_hint: String::new(),
    }
}

/// The two commands a surface may only ever PRINT.
///
/// Both need elevation, so `docs/SURFACES.md` §3 marks them "never" for the
/// browser: a page that ran them would either fail for everyone or demand that
/// Studio itself run as admin. They are composed here rather than at each
/// surface so that the string a user pastes is the string the CLI documents —
/// `ksx winusb release` really does need `--yes`, and a page that dropped it
/// would hand out a command that does nothing.
fn claim_command(instance_id: &str) -> String {
    format!("ksx winusb claim {instance_id}")
}

fn release_command(instance_id: &str) -> String {
    format!("ksx winusb release {instance_id} --yes")
}

/// One `[[device]]` entry, resolved against the machine as it is right now.
fn configured_row(
    entry: &DeviceEntry,
    devices: &DevicesView,
    connected: &[DeviceFacts],
    config: &ConfigFile,
    games: &GamesFile,
    found: &[Board<'_>],
) -> ksx_api::ConfiguredDevice {
    let selector = entry.id.selector();
    // Exactly one, or nothing. `Match::Ambiguous` is deliberately folded into
    // "not present": an id that names two boards is not resolvable, and
    // reporting the first hit would be the precise failure `DeviceSelector`
    // exists to prevent (docs/DEVICE-IDENTITY.md §8).
    let hit = match selector.match_against(connected) {
        Match::One(facts) => Some(facts),
        Match::None | Match::Ambiguous(_) => None,
    };
    let interface = hit
        .and_then(|facts| {
            devices
                .usb
                .iter()
                .find(|row| row.instance_id.eq_ignore_ascii_case(facts.id.as_str()))
        })
        // A device with no USB interface — a Bluetooth keyboard — is named in
        // config by its devnode path, because no `usb:` selector can describe
        // one. `DeviceSelector::match_against` is USB-facts-shaped and answers
        // `false` for those spellings by design, so the path is matched against
        // the DEVICE LIST directly, byte-exact and case-insensitive: the same
        // semantics `DeviceSelector::HardwareId` documents.
        //
        // Without this a configured Bluetooth keyboard read as "not connected
        // right now" while it was connected and typing.
        .or_else(|| {
            let written = selector.to_string();
            devices
                .usb
                .iter()
                .find(|row| row.instance_id.eq_ignore_ascii_case(&written))
        });
    let claimed = interface.is_some_and(|row| row.state == "claimed");
    let instance_id = interface.map(|row| row.instance_id.clone());
    // The elevated commands are only ever shown for a device a claim could
    // actually bind. Offering `ksx winusb claim` beside a Bluetooth entry would
    // hand out a command that refuses every time it is run.
    let winusb_reachable = interface.is_some_and(|row| row.winusb_eligible);

    ksx_api::ConfiguredDevice {
        alias: entry.alias.clone(),
        id: entry.id.raw().to_owned(),
        backend: match entry.backend {
            Backend::Winusb => "winusb".to_owned(),
            Backend::Interception => "interception".to_owned(),
        },
        rung: selector.rung().to_owned(),
        survives_replug: selector.survives_replug(),
        means: selector.explain(),
        // The whole paragraph, verbatim from the writer that decided it. The
        // second half — that a `port=` value names THIS PC's USB topology and
        // does not travel to another cabinet — is the half people miss, and it
        // is a property of the ENTRY, not of the moment it was written, so it
        // has to keep being said every time the entry is listed.
        port_pinned_warning: (!selector.survives_replug())
            .then(|| crate::device_edit::PORT_PINNED_WARNING.to_owned()),
        present: instance_id.is_some(),
        board: interface.and_then(|row| board_named(found, &row.instance_id)),
        instance_id: instance_id.clone(),
        // Empty while the entry resolves to nothing: a transport is a fact
        // about a device that is here, and guessing one from the id's spelling
        // would be a claim about hardware nobody found.
        transport: interface
            .map(|row| row.transport.clone())
            .unwrap_or_default(),
        claimed,
        claim_command: instance_id
            .as_deref()
            .filter(|_| !claimed && winusb_reachable)
            .map(claim_command),
        release_command: instance_id
            .as_deref()
            .filter(|_| claimed)
            .map(release_command),
        used_by: crate::device_edit::references_to(config, games, &entry.alias)
            .iter()
            .map(ToString::to_string)
            .collect(),
        // Filled by `DeviceScanView::read` from the fields above — it is a
        // verdict about what `ksx run` does with this entry, and it reads
        // `used_by`, which is only complete once this row is built.
        health_line: String::new(),
        health_level: String::new(),
        command_lead: String::new(),
        command: String::new(),
    }
}

/// The name of the board one interface belongs to.
fn board_named(found: &[Board<'_>], instance_id: &str) -> Option<String> {
    found
        .iter()
        .find(|board| {
            board
                .interfaces
                .iter()
                .any(|row| row.instance_id.eq_ignore_ascii_case(instance_id))
        })
        .map(|board| board.name.clone())
}

/// # `--json` changed shape, and gained a failure the plain report does not have
///
/// **Shape.** `--json` used to print [`ksx_api::DevicesView`] — the raw
/// interface list — from enumeration alone. It now prints
/// [`ksx_api::DeviceScanView`], the grouped shape. `--json` on a verb whose
/// entire point is the grouping used to hand back `ksx devices`' 29 rows, which
/// meant a script had to re-implement `boards()` to use it: one capability, two
/// shapes, exactly what `docs/SURFACES.md` §1 forbids. Any consumer reading
/// `.usb[]` must move to `.boards[].interfaces[]`.
///
/// **Failure mode.** The grouped view includes the `[[device]]` entries
/// resolved against the live enumeration, so `--json` now READS `config.toml`
/// and `games.toml` where the plain report does not. A missing config is fine
/// (the store returns a default), but an unparseable one is a hard error — so
/// on a machine with a corrupt `config.toml`, plain `ksx device scan` still
/// prints the hardware and `--json` exits non-zero.
///
/// That asymmetry is deliberate rather than an oversight. The alternative is
/// printing `configured: []` beside `no_configured_device: true`, which is the
/// failure this whole page was rebuilt to remove: a read that did not happen
/// must never be served as an assertion that there is nothing there. The
/// context below says which file and which flag, so the exit is
/// self-explaining.
#[cfg(windows)]
pub fn run(all: bool, json: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let report = crate::devices::collect();
    let devices = crate::devices::to_view(&report);
    if json {
        let store = crate::device_edit::store()?;
        let context = "`ksx device scan --json` reports the [[device]] entries alongside the \
                       hardware, so it must parse the config; plain `ksx device scan` does not \
                       read it at all";
        let config = store.load_config().context(context)?.value;
        let games = store.load_games().context(context)?.value;
        let connected = crate::device_edit::connected_facts();
        println!(
            "{}",
            serde_json::to_string_pretty(&view(&devices, &connected, &config, &games))?
        );
    } else {
        print!("{}", render(&devices, all));
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
        // Built through `ksx_core::Reach` rather than by hand, deliberately: a
        // fixture that spelled its own eligibility could not disagree with the
        // page even when the page was wrong, and the transport rule is the one
        // thing these tests are here to hold.
        let reach = ksx_core::Reach {
            transport: ksx_core::Transport::Usb,
            keyboard: state == "claimed" || state == "claimable",
            claimed: state == "claimed",
            can_type: state != "claimed",
        };
        let eligibility = reach.eligibility();
        UsbRow {
            instance_id: id.to_owned(),
            description: "USB Input Device".to_owned(),
            transport: ksx_core::Transport::Usb.code().to_owned(),
            state: state.to_owned(),
            verdict: "on the keyboard stack; ksx could claim it".to_owned(),
            alias: None,
            selected: false,
            ready: false,
            boot_keyboard: state == "claimed",
            vendor: vendor.map(str::to_owned),
            board: Some(board.to_owned()),
            // What the backend would have computed for a board with no twin
            // connected — `crate::devices::suggested_selectors` owns the real
            // derivation and is tested there. `None` for a devnode no `usb:`
            // selector could name, which is the SpinTrak's shape below.
            selector: ksx_core::DeviceFacts::from_instance_path(id).map(|facts| {
                ksx_core::DeviceSelector::strongest_for(&facts, std::slice::from_ref(&facts))
                    .to_string()
            }),
            interception_eligible: eligibility.interception,
            winusb_eligible: eligibility.winusb,
            backends: eligibility.line,
            can_type: reach.can_type,
            cannot_type_reason: String::new(),
        }
    }

    /// Change a row's state **the way the collector would**.
    ///
    /// The state word and the backend eligibility are derived together from one
    /// enumeration pass, so a fixture that moved one without the other would be
    /// asserting against a row that cannot exist on any machine — and would
    /// happily pass while the code under test was wrong about the pair.
    fn restate(target: &mut UsbRow, state: &str) {
        let rebuilt = row(
            &target.instance_id,
            target.board.as_deref().unwrap_or_default(),
            state,
            target.vendor.as_deref(),
        );
        target.state = rebuilt.state;
        target.boot_keyboard = rebuilt.boot_keyboard;
        target.interception_eligible = rebuilt.interception_eligible;
        target.winusb_eligible = rebuilt.winusb_eligible;
        target.backends = rebuilt.backends;
        target.can_type = rebuilt.can_type;
    }

    /// A paired Bluetooth keyboard, in the shape the collector produces.
    ///
    /// `state` is the vocabulary word (`interception-only`), and every backend
    /// field comes from `ksx_core::Reach` — the rule lives in one place and
    /// this fixture reads it like the collector does.
    fn bt_row(id: &str, name: &str, can_type: bool) -> UsbRow {
        let reach = ksx_core::Reach {
            transport: ksx_core::Transport::Bluetooth,
            keyboard: true,
            claimed: false,
            can_type,
        };
        let eligibility = reach.eligibility();
        UsbRow {
            instance_id: id.to_owned(),
            description: name.to_owned(),
            transport: ksx_core::Transport::Bluetooth.code().to_owned(),
            state: "interception-only".to_owned(),
            verdict: "a Bluetooth keyboard on the Windows input stack — ksx can capture it \
                      through Interception and split it into virtual pads"
                .to_owned(),
            alias: None,
            selected: false,
            ready: false,
            boot_keyboard: true,
            vendor: None,
            board: Some(r"BTHENUM\001BDC0F1FE7".to_owned()),
            selector: Some(id.to_owned()),
            interception_eligible: eligibility.interception,
            winusb_eligible: eligibility.winusb,
            backends: eligibility.line,
            can_type,
            cannot_type_reason: if can_type {
                String::new()
            } else {
                "not connected (paired but absent?)".to_owned()
            },
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
            bluetooth_available: true,
            notes: Vec::new(),
        }
    }

    /// The same cabinet with a paired Bluetooth keyboard on it — the machine
    /// this whole task is about.
    fn cabinet_with_bluetooth(can_type: bool) -> DevicesView {
        let mut view = cabinet();
        view.usb.push(bt_row(
            r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&3562C725&0&001BDC0F1FE7_C00000000",
            "Bluetooth Keyboard",
            can_type,
        ));
        view
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
        restate(&mut view.usb[3], "not-a-keyboard");

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

    /// **The line `docs/DEVICE-IDENTITY.md` §5 promised, and nothing printed.**
    ///
    /// "A legacy entry that still resolves is never silently rewritten. `ksx
    /// device scan` prints the stronger selector it *would* write and leaves the
    /// decision with them." Two user-facing strings repeated that promise —
    /// `DeviceSelector::explain` for a legacy path, and `ResolveError::Missing`
    /// — while this report printed a board name, an interface list, a state and
    /// a verdict, and no selector anywhere. A user following the advice in a
    /// refusal arrived at a command that could not answer it.
    ///
    /// Breaks against: that version. `render` had no `usb:` in it at all, and
    /// the file did not even import `DeviceSelector`.
    #[test]
    fn the_report_prints_the_id_that_picking_this_board_would_write() {
        let text = render(&cabinet(), false);
        assert!(
            text.contains("id       : 'usb:d209:0430:00'"),
            "the board a user is choosing between must show what would land in \
             config, not just a devnode to paste:\n{text}"
        );
        assert!(
            text.contains("the `id` shown above is what it would put in it"),
            "and it must say that is what it is, or it reads as another devnode:\n{text}"
        );
    }

    // ── one list, two transports ──────────────────────────────────────────

    /// **The list this task exists to produce.** A Bluetooth keyboard is in the
    /// SAME grouped view as the USB boards, it is offered as pickable, and its
    /// row states which backends it is eligible for.
    ///
    /// FAILS against the shipped scan in two independent ways: it enumerated
    /// `nusb::list_devices()` so the row did not exist at all, and
    /// `Board::keyboard` matched `state == "claimable"` so even a hand-placed
    /// Bluetooth row would have been unpickable — a picker refusing to offer a
    /// device whose own verdict said ksx could capture it.
    #[test]
    fn a_bluetooth_keyboard_is_in_the_same_list_and_is_pickable() {
        let view = cabinet_with_bluetooth(true);
        let boards = boards(&view);
        let bt = boards
            .iter()
            .find(|b| b.name == "Bluetooth Keyboard")
            .expect("the Bluetooth keyboard is one of the devices");
        assert!(
            bt.keyboard().is_some(),
            "it is capturable through Interception, so it can be picked"
        );

        let text = render(&view, false);
        assert!(text.contains("[Bluetooth] Bluetooth Keyboard"), "{text}");
        assert!(
            text.contains("[USB] Ultimarc I-PAC 4X"),
            "and the USB half is labelled too — a transport column that only \
             appears on the surprising row reads as a special case:\n{text}"
        );
    }

    /// **The rule the list must teach, on the row.** Interception yes, WinUSB
    /// never, and the "never" names the transport fact rather than a missing
    /// feature.
    ///
    /// FAILS against any report that prints only the state word: `claimable` /
    /// `interception-only` tells a user nothing about WHY, and "not supported"
    /// invites waiting for a release that cannot come.
    #[test]
    fn the_bluetooth_row_states_both_backends_and_why_winusb_never_applies() {
        let text = render(&cabinet_with_bluetooth(true), false);
        let backends: Vec<&str> = text
            .lines()
            .filter(|l| l.trim_start().starts_with("backends :"))
            .collect();
        assert_eq!(
            backends.len(),
            text.lines()
                .filter(|l| l.trim_start().starts_with("keyboard :"))
                .count(),
            "EVERY row with a keyboard states its backends — a rule stated only \
             where it bites reads as a special case:\n{text}"
        );
        assert!(backends.len() >= 2, "{text}");

        let bt = backends
            .iter()
            .find(|l| l.contains("Bluetooth"))
            .expect("the Bluetooth row has a backends line");
        assert!(bt.contains("interception: yes"), "{bt}");
        assert!(bt.contains("never"), "{bt}");
        assert!(
            bt.contains("no USB interface to bind"),
            "the transport fact, not a vague refusal: {bt}"
        );
        assert!(
            !bt.contains("not supported"),
            "'not supported' invites waiting for a release that cannot come: {bt}"
        );
    }

    /// **The trap, on the screen someone reads before claiming their panel.**
    ///
    /// A paired-but-disconnected Bluetooth keyboard is PRESENT, so it must stay
    /// listed — hiding it would be its own lie — and the row must say it cannot
    /// type. Someone who reads "2 keyboards" here, claims their panel, and then
    /// finds the second one in a drawer with dead batteries is locked out of
    /// their own machine.
    ///
    /// FAILS against a row that carries presence only.
    #[test]
    fn a_paired_but_disconnected_bluetooth_keyboard_is_listed_and_says_it_cannot_type() {
        let text = render(&cabinet_with_bluetooth(false), false);
        assert!(text.contains("Bluetooth Keyboard"), "still listed:\n{text}");
        assert!(
            text.contains("CANNOT type right now"),
            "and it says so, in the words that stop someone counting it as a \
             spare:\n{text}"
        );
        assert!(
            text.contains("does not count as a spare keyboard"),
            "{text}"
        );

        // A CONNECTED one must not carry the caveat, or the caveat means
        // nothing when it appears.
        let live = render(&cabinet_with_bluetooth(true), false);
        assert!(!live.contains("CANNOT type right now"), "{live}");
    }

    /// A CLAIMED board cannot type to Windows either — that is what a claim
    /// does — and it must NOT wear the same alarm. The row already says
    /// `claimed` and the backends line already says why; repeating it as a
    /// warning puts an alarm on the one state that is working exactly as asked.
    ///
    /// FAILS against gating the caveat on `!can_type` alone, which is the
    /// obvious implementation and which decorated the reference cabinet's
    /// working I-PAC with "does not count as a spare keyboard".
    #[test]
    fn a_claimed_board_is_not_warned_about_being_unable_to_type() {
        let text = render(&cabinet(), false);
        assert!(text.contains("[claimed]"), "fixture precondition:\n{text}");
        assert!(
            !text.contains("CANNOT type right now"),
            "a deliberate claim is not a fault:\n{text}"
        );
        assert!(
            !text.contains("does not count as a spare keyboard"),
            "{text}"
        );
    }

    /// The typed view every non-terminal surface reads carries the same facts,
    /// on the board row — so Studio and the cabinet cannot tell a different
    /// story from the terminal.
    #[test]
    fn the_typed_view_carries_the_transport_and_the_backends_per_board() {
        let view = view(
            &cabinet_with_bluetooth(true),
            &connected(),
            &ConfigFile::default(),
            &GamesFile::default(),
        );
        let bt = view
            .boards
            .iter()
            .find(|b| b.name == "Bluetooth Keyboard")
            .expect("the Bluetooth keyboard reaches the typed view");
        assert_eq!(bt.transport, "bluetooth");
        assert!(bt.pickable);
        assert!(bt.interception_eligible);
        assert!(!bt.winusb_eligible);
        assert!(bt.backends.contains("no USB interface to bind"), "{bt:?}");
        // No elevated command: a claim on this device refuses every time, and
        // a page that printed one would be handing out a dead command.
        assert!(bt.claim_command.is_none());
        assert!(bt.release_command.is_none());
        assert_eq!(bt.command, "");

        let ipac = view
            .boards
            .iter()
            .find(|b| b.name == "Ultimarc I-PAC 4X")
            .expect("the USB board is still there");
        assert_eq!(ipac.transport, "usb");
        assert!(ipac.winusb_eligible);
    }

    /// A board no `usb:` selector can name gets **no** `id` line rather than a
    /// wrong one. Printing a guess here is worse than printing nothing: the
    /// whole value of the line is that it is what `pick` would commit.
    #[test]
    fn a_board_with_no_writable_selector_suggests_nothing_at_all() {
        let mut view = cabinet();
        // The SpinTrak enumerates as a non-composite devnode with no `MI_`, so
        // there is no interface number to build a selector from.
        restate(&mut view.usb[3], "claimable");
        assert!(view.usb[3].selector.is_none(), "fixture precondition");
        let text = render(&view, true);
        assert!(text.contains("Ultimarc SpinTrak"), "{text}");
        assert_eq!(
            text.matches("id       : ").count(),
            1,
            "only the board that HAS a selector gets an id line:\n{text}"
        );
    }

    /// **Ambiguity has to be visible in the picker, not only in the refusal.**
    ///
    /// The port rung is what `strongest_for` falls back to when a twin is
    /// connected and nothing weaker separates them — so a `port=` suggestion IS
    /// the "two identical boards are plugged in" signal, and offering it without
    /// saying so hands the user a config entry that silently stops working when
    /// they move the board (`docs/DEVICE-IDENTITY.md` §2: the trade is stated
    /// out loud at the moment it is made).
    ///
    /// Breaks against: printing the selector bare, which is the obvious first
    /// implementation of the test above.
    #[test]
    fn a_port_pinned_suggestion_says_why_it_is_pinned() {
        let mut view = cabinet();
        view.usb[0].selector = Some("usb:d209:0430:00:port=7&25EEA38C&0&0000".into());
        let text = render(&view, false);
        assert!(text.contains("port=7&25EEA38C&0&0000"), "{text}");
        assert!(
            text.contains("another board of the same model is connected"),
            "a socket-pinned id must say why it is pinned:\n{text}"
        );
        // ...and the un-pinned board next to it must not acquire the caveat.
        let plain = render(&cabinet(), false);
        assert!(!plain.contains("pinned to this socket"), "{plain}");
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

    // ── the typed view (`ksx_api::DeviceScanView`) ────────────────────────
    //
    // Pure, so every one of these runs in CI on any platform against a
    // synthetic device tree — the same property `device_edit`'s planners were
    // written for, and the reason a surface can be trusted with this shape.

    use ksx_config::{Backend, ConfigFile, DeviceEntry, GameEntry, GameSlotEntry, GamesFile};
    use ksx_core::{DeviceFacts, DeviceId};

    fn facts(id: &str, product: u16, interface: u8) -> DeviceFacts {
        DeviceFacts {
            id: DeviceId::new(id),
            vendor_id: 0xD209,
            product_id: product,
            interface_number: interface,
            // Both I-PAC 4X boards answer "4" — measured, not hypothetical, and
            // the reason the port rung exists at all.
            serial: Some("4".to_owned()),
            instance: DeviceFacts::instance_of(id).to_owned(),
        }
    }

    fn connected() -> Vec<DeviceFacts> {
        vec![
            facts(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000", 0x0430, 0),
            facts(r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001", 0x0430, 1),
            facts(r"USB\VID_D209&PID_0430&MI_02\7&25EEA38C&0&0002", 0x0430, 2),
            facts(r"USB\VID_D209&PID_15A2\6", 0x15A2, 0),
        ]
    }

    /// The cabinet plus a SECOND I-PAC 4X. Both answer `usb:d209:0430:00`, both
    /// report serial "4", and only the socket separates them — the measured
    /// case docs/DEVICE-IDENTITY.md §8 is written about.
    fn connected_twins() -> Vec<DeviceFacts> {
        let mut all = connected();
        all.push(facts(
            r"USB\VID_D209&PID_0430&MI_00\6&1B2C3D4E&0&0000",
            0x0430,
            0,
        ));
        all
    }

    fn entry(id: &str, alias: &str, backend: Backend) -> DeviceEntry {
        DeviceEntry {
            id: ksx_core::DeviceRef::parse(id).expect("test fixture id must parse"),
            alias: alias.to_owned(),
            backend,
        }
    }

    fn config_with(devices: Vec<DeviceEntry>) -> ConfigFile {
        ConfigFile {
            devices,
            ..ConfigFile::default()
        }
    }

    /// The claimed I-PAC, named in config by the PORT rung — the case the whole
    /// warning exists for.
    fn port_pinned_config() -> ConfigFile {
        config_with(vec![entry(
            "usb:d209:0430:00:port=7&25EEA38C&0&0000",
            "panel",
            Backend::Winusb,
        )])
    }

    /// Three devnodes of one board collapse into one row a person can choose
    /// from, and the keyboard interface is named for them. A picker that
    /// offered three rows here would be asking the user to know which `MI_`
    /// number carries the keys.
    #[test]
    fn the_view_groups_devnodes_into_boards_and_names_the_keyboard() {
        let view = view(
            &cabinet(),
            &connected(),
            &ConfigFile::default(),
            &GamesFile::default(),
        );
        assert_eq!(view.boards.len(), 2, "one I-PAC and one SpinTrak");
        let ipac = &view.boards[0];
        assert_eq!(ipac.name, "Ultimarc I-PAC 4X");
        assert_eq!(ipac.interfaces.len(), 3);
        assert_eq!(
            ipac.keyboard.as_deref(),
            Some(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000"),
            "the board ksx already holds must read as the one it holds"
        );
        assert!(ipac.claimed);
        assert!(ipac.looks_like_a_keyboard);
    }

    /// The two elevated commands are composed here, once, so every surface
    /// hands out the string the CLI documents — `release` really does need
    /// `--yes`, and a page that dropped it would offer a command that does
    /// nothing.
    #[test]
    fn a_claimed_board_offers_release_and_a_claimable_one_offers_claim() {
        let view = view(
            &cabinet(),
            &connected(),
            &ConfigFile::default(),
            &GamesFile::default(),
        );
        let ipac = &view.boards[0];
        assert!(ipac.claim_command.is_none(), "it is already claimed");
        assert_eq!(
            ipac.release_command.as_deref(),
            Some(r"ksx winusb release USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000 --yes")
        );

        let spintrak = view
            .boards
            .iter()
            .find(|b| b.name == "Ultimarc SpinTrak")
            .expect("the SpinTrak is listed");
        assert_eq!(
            spintrak.claim_command.as_deref(),
            Some(r"ksx winusb claim USB\VID_D209&PID_15A2\6")
        );
        assert!(spintrak.release_command.is_none());
    }

    /// A board with no keyboard interface stays in the list — "ksx cannot see
    /// my board" is a real support question — but it sorts BELOW the ones that
    /// can be picked, and it says why it cannot be.
    #[test]
    fn an_unpickable_board_is_listed_last_and_says_why() {
        let mut cabinet = cabinet();
        for row in &mut cabinet.usb {
            if row.instance_id.contains("15A2") {
                restate(row, "not-a-keyboard");
            }
        }
        let view = view(
            &cabinet,
            &connected(),
            &ConfigFile::default(),
            &GamesFile::default(),
        );
        let last = view.boards.last().expect("two boards");
        assert_eq!(last.name, "Ultimarc SpinTrak");
        assert!(last.keyboard.is_none());
        assert!(last.claim_command.is_none(), "nothing to offer");
        assert!(
            last.keyboard_verdict.contains("cannot capture"),
            "{}",
            last.keyboard_verdict
        );
    }

    /// The PORT-PINNED paragraph rides on the ENTRY, not on the moment it was
    /// written — and both halves survive, including the one people miss.
    #[test]
    fn a_port_pinned_entry_carries_the_whole_warning() {
        let view = view(
            &cabinet(),
            &connected(),
            &port_pinned_config(),
            &GamesFile::default(),
        );
        let device = &view.configured[0];
        assert_eq!(device.alias, "panel");
        assert!(!device.survives_replug);
        assert_eq!(device.rung, "port");
        let warning = device
            .port_pinned_warning
            .as_deref()
            .expect("a port rung must carry the warning");
        assert!(warning.contains("PORT-PINNED"), "{warning}");
        assert!(warning.contains("stops matching"), "{warning}");
        assert!(
            warning.contains("do not copy this config to another cabinet"),
            "the machine-specific half is the half people miss: {warning}"
        );
        assert_eq!(
            warning,
            crate::device_edit::PORT_PINNED_WARNING,
            "one wording, shared with what `ksx device pick` prints"
        );
    }

    /// A model-rung entry survives a replug, so it gets no warning at all — the
    /// warning has to mean something when it appears.
    #[test]
    fn an_entry_that_survives_a_replug_carries_no_warning() {
        let config = config_with(vec![entry(
            "usb:d209:0430:00",
            "panel",
            Backend::Interception,
        )]);
        let view = view(&cabinet(), &connected(), &config, &GamesFile::default());
        assert!(view.configured[0].survives_replug);
        assert!(view.configured[0].port_pinned_warning.is_none());
    }

    /// The entry resolves, so the row can say WHICH board and WHICH interface —
    /// and that the interface is claimed, which is what makes the release
    /// command the sensible one to show.
    #[test]
    fn a_configured_entry_resolves_to_the_board_it_names() {
        let view = view(
            &cabinet(),
            &connected(),
            &port_pinned_config(),
            &GamesFile::default(),
        );
        let device = &view.configured[0];
        assert!(device.present);
        assert_eq!(device.board.as_deref(), Some("Ultimarc I-PAC 4X"));
        assert_eq!(
            device.instance_id.as_deref(),
            Some(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000")
        );
        assert!(device.claimed);
        assert!(device.release_command.is_some());
        assert!(device.claim_command.is_none());
    }

    /// A configured board that is not plugged in looks IDENTICAL to a broken
    /// config from a slot's end, so the difference has to be said out loud
    /// rather than left to an empty field.
    #[test]
    fn an_entry_whose_board_is_absent_says_so_rather_than_looking_configured() {
        let view = view(
            &cabinet(),
            // Nothing enumerated: the board is unplugged.
            &[],
            &port_pinned_config(),
            &GamesFile::default(),
        );
        let device = &view.configured[0];
        assert!(!device.present);
        assert!(device.instance_id.is_none());
        assert!(device.board.is_none());
        assert!(!device.claimed);
        assert!(device.claim_command.is_none(), "nothing to claim");
        assert!(device.release_command.is_none(), "nothing to release");
    }

    /// Twins: an id that answers to two boards is not resolvable, and reporting
    /// the first hit would be exactly the failure `DeviceSelector` exists to
    /// prevent (docs/DEVICE-IDENTITY.md §8).
    #[test]
    fn an_ambiguous_id_reads_as_not_present_rather_than_picking_one() {
        let config = config_with(vec![entry("usb:d209:0430:00", "panel", Backend::Winusb)]);
        let view = view(
            &cabinet(),
            &connected_twins(),
            &config,
            &GamesFile::default(),
        );
        assert!(
            !view.configured[0].present,
            "two boards answer usb:d209:0430:00 and neither of them is THE one"
        );
        assert!(view.configured[0].instance_id.is_none());
    }

    /// The list `remove` refuses on, on the page BEFORE the click — from
    /// config.toml AND from every games.toml profile, because both resolve
    /// through the same `[[device]]` table and listing half of them would send
    /// someone off to fix a cabinet that is still broken.
    #[test]
    fn used_by_names_every_slot_in_both_files() {
        let mut config = port_pinned_config();
        config.slots.push(ksx_config::SlotEntry {
            number: 1,
            keyboard: Some("panel".to_owned()),
            mouse: None,
            preset: "P1".to_owned(),
            persona: Default::default(),
            socd: Default::default(),
            macros: Default::default(),
        });
        let games = GamesFile {
            games: vec![GameEntry {
                title: "Street Fighter".to_owned(),
                notes: String::new(),
                path: r"C:\sf.exe".to_owned(),
                arguments: String::new(),
                process_name: None,
                launcher_grace_ms: None,
                block_keyboards: Default::default(),
                block_mice: false,
                slots: vec![GameSlotEntry {
                    number: 2,
                    user_index: None,
                    keyboard: Some("panel".to_owned()),
                    mouse: None,
                    preset: "P2".to_owned(),
                    persona: Default::default(),
                    socd: Default::default(),
                    macros: Default::default(),
                }],
            }],
        };

        let view = view(&cabinet(), &connected(), &config, &games);
        let used = &view.configured[0].used_by;
        assert_eq!(used.len(), 2, "{used:?}");
        assert!(used.iter().any(|u| u == "slot 1 (keyboard)"), "{used:?}");
        assert!(
            used.iter()
                .any(|u| u == "slot 2 (keyboard) in profile \"Street Fighter\""),
            "{used:?}"
        );
    }

    /// An empty machine and a machine that could not be READ are different
    /// answers, and the flag is how a surface tells them apart.
    #[test]
    fn a_failed_enumeration_is_carried_as_a_flag_not_as_an_empty_list() {
        let mut blind = cabinet();
        blind.usb.clear();
        blind.usb_available = false;
        blind.notes.push("USB enumeration failed".to_owned());

        let view = view(
            &blind,
            &connected(),
            &ConfigFile::default(),
            &GamesFile::default(),
        );
        assert!(view.boards.is_empty());
        assert!(!view.usb_available);
        assert_eq!(view.notes, ["USB enumeration failed"], "notes survive");
    }

    #[test]
    fn a_machine_with_no_pickable_board_says_so_rather_than_printing_nothing() {
        let mut view = cabinet();
        for row in &mut view.usb {
            restate(row, "not-a-keyboard");
        }
        let text = render(&view, false);
        assert!(text.contains("No keyboard-capable boards found"), "{text}");
        assert!(text.contains("--all"), "and how to see the rest: {text}");
    }
}
