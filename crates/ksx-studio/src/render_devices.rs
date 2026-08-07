//! The `/devices` render seam: embedded FMIR + one per-request
//! [`DevicesPayload`] → HTML, with the same data emitted twice — slots for the
//! SSR first paint, the source payload for client hydration.
//!
//! Structurally identical to `render.rs` and `render_map.rs`, and deliberately
//! so: four seams (scalars, lists, shows, `build_slots`), one page entry, one
//! layout test that calls [`crate::render::assert_island_slot_contract`]. Read
//! `render.rs`'s module docs for why the data is emitted twice and why "the
//! slot exists" is not the check.
//!
//! # What this page decides, and what it must not
//!
//! Nothing here reads hardware, resolves a device id, or judges whether a
//! board can be picked. All of that is [`ksx_api::DeviceScanView`], composed by
//! `ksx-app`'s `device_scan::view` from the same enumeration `ksx device scan`
//! prints — `docs/SURFACES.md` §1: a capability is a typed spec and a pure plan
//! in the backend, and surfaces render the result. What this file does is turn
//! that view into the exact strings the island draws, and it does that because
//! the island's twin (`studio-ui/src/DevicesIsland.ts`) has to produce the
//! identical strings per poll. Every function below has a mirror there; the
//! tests at the bottom pin this side.
//!
//! # Three rules this page inherits and one it adds
//!
//! Inherited: no logic in the page (§1), no elevated verb on this surface
//! (§3 marks WinUSB claim/release "never" for the browser — the commands are
//! rendered as text), and every mutating route is a 303 post-redirect-get with
//! the outcome in `?flash=`.
//!
//! Added: **an optional line is rendered and hidden, never omitted.** A
//! `createShow` inside a `createList` is not a shape this compiler emits, so a
//! row that sometimes carries a PORT-PINNED warning carries the element on
//! every row and a per-row class (`portWarnCls`) decides whether it is
//! visible. That is why so many fields below come in `(text, class)` pairs.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};
use ksx_api::{BoardRow, ConfiguredDevice};

use crate::render::{body_prefix, with_icon_links, EmbeddedPage, PERSONALITY_CSS};
use crate::snapshot::DevicesPayload;

/// List slot names, binding-derived (compiler 0.2.0): a `createList` reading
/// `() => configuredRows()` compiles to `list:configuredRows:array`. Rename a
/// list signal in `DevicesIsland.ts` and the layout test fails until these
/// match again. No `#2` suffixes here — every list on this page is rendered
/// exactly once.
const LIST_SLOT_CONFIGURED: &str = "list:configuredRows:array";
const LIST_SLOT_BOARDS: &str = "list:boardRows:array";
const LIST_SLOT_OTHER: &str = "list:otherRows:array";
const LIST_SLOT_NOTES: &str = "list:noteRows:array";

#[cfg(test)]
const ISLAND_COMPONENT: &str = "DevicesIsland";

/// How many `createShow` pairs this page has. Name-addressable since compiler
/// 0.3.1, so this is a staleness tripwire rather than a mapping.
const SHOW_COUNT: usize = 13;

/// Bare-named slots the island renders and the seam deliberately never fills.
/// EMPTY, and that is the claim: every signal `DevicesIsland.ts` binds to the
/// DOM gets a server value on every request.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots this page compiles to. EMPTY, and it is
/// enforced by construction: `DevicesIsland.ts` contains no string
/// concatenation inside the h() tree at all. Every composed sentence is
/// composed HERE and shipped as a signal value, precisely because an anonymous
/// slot can never be injected — it renders its compile-time default and
/// nothing else (render.rs ledger #10/#20).
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

/// The elevated half, worded once and shared with the island's twin.
///
/// Both commands need an admin shell, so `docs/SURFACES.md` §3 marks them
/// "never" for the browser and this page shows them instead of running them.
/// The lead has to say ELEVATED, or the only feedback a user gets is "access
/// denied" from a command a web page handed them.
const CLAIM_LEAD: &str = "To move it to the WinUSB backend it must be claimed — it then stops \
     typing to Windows, which is a separate, consented step. Run this in an ELEVATED shell:";
const RELEASE_LEAD: &str = "It is claimed, so Windows sees no keyboard on it. To put it back on \
     the keyboard driver, run this in an ELEVATED shell:";

// ---------------------------------------------------------------------------
// derivations (mirrored in studio-ui/src/DevicesIsland.ts)
// ---------------------------------------------------------------------------

fn configured_summary(count: usize) -> String {
    match count {
        0 => "no [[device]] entries in config.toml".to_owned(),
        1 => "1 [[device]] entry in config.toml:".to_owned(),
        n => format!("{n} [[device]] entries in config.toml:"),
    }
}

/// The boards line — and the one case where an empty list is NOT the same as
/// an empty machine.
///
/// `usb_available == false` means the enumeration itself failed, so the list is
/// empty because nothing could be READ. Printing "no keyboard-capable board
/// found" there would be a confident wrong answer on a cabinet with four boards
/// plugged in.
fn boards_summary(count: usize, usb_available: bool) -> String {
    if !usb_available {
        return "USB enumeration failed — this list is empty because nothing could be READ, not \
                because nothing is plugged in"
            .to_owned();
    }
    match count {
        0 => "no keyboard-capable board found".to_owned(),
        1 => "1 keyboard-capable board:".to_owned(),
        n => format!("{n} keyboard-capable boards:"),
    }
}

fn other_summary(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 board has no keyboard interface — ksx cannot capture it:".to_owned(),
        n => format!("{n} boards have no keyboard interface — ksx cannot capture them:"),
    }
}

/// `(lead, command, class)` for the elevated command a row shows, or the
/// hidden triple when there is nothing to show.
fn command_cell(claim: Option<&str>, release: Option<&str>) -> (String, String, &'static str) {
    match (release, claim) {
        (Some(cmd), _) => (RELEASE_LEAD.to_owned(), cmd.to_owned(), "dv-cmd"),
        (None, Some(cmd)) => (CLAIM_LEAD.to_owned(), cmd.to_owned(), "dv-cmd"),
        (None, None) => (String::new(), String::new(), "dv-cmd dv-hide"),
    }
}

fn scalar_slots(payload: &DevicesPayload, flash: Option<&str>) -> serde_json::Value {
    let pickable = payload
        .scan
        .boards
        .iter()
        .filter(|b| b.keyboard.is_some())
        .count();
    let other = payload.scan.boards.len() - pickable;
    serde_json::json!({
        "generatedAt": payload.scan.generated_at,
        "sessionLine": payload.session.line,
        "flashLine": flash.unwrap_or(""),
        "unavailableLine": payload.unavailable.trim(),
        "configuredSummary": configured_summary(payload.scan.configured.len()),
        "boardsSummary": boards_summary(pickable, payload.scan.usb_available),
        "otherSummary": other_summary(other),
    })
}

/// One configured `[[device]]` entry as the row the page draws.
///
/// `index` only ever becomes an element id (`dv-force-3`), so the checkbox and
/// its label can be associated without inventing an id out of an alias a user
/// chose — an alias may contain spaces and quotes, and an `id` attribute built
/// from one would be a selector nobody can write.
fn configured_row(device: &ConfiguredDevice, index: usize) -> SlotValue {
    // The claim state, with the one combination that is actually a fault
    // called out: `backend = "winusb"` on a board that is NOT bound to
    // winusb.sys is a config `ksx run` refuses to start on, and it looks
    // identical to a healthy row unless someone says so.
    let winusb_but_loose = device.present && !device.claimed && device.backend == "winusb";
    let (claim_text, claim_cls) = if device.present && device.claimed {
        ("claimed — bound to winusb.sys".to_owned(), "pill pill-ok")
    } else if winusb_but_loose {
        (
            "backend is winusb but the board is NOT claimed — ksx run will refuse".to_owned(),
            "pill pill-warn",
        )
    } else if device.present {
        ("on the Windows keyboard stack".to_owned(), "pill pill-idle")
    } else {
        // Not present: there is no interface to say anything about, and a
        // stale claim pill would be a claim about a board that is not here.
        (String::new(), "pill dv-hide")
    };

    let (command_lead, command, command_cls) = command_cell(
        device.claim_command.as_deref(),
        device.release_command.as_deref(),
    );

    let board = match (device.present, device.instance_id.as_deref()) {
        (true, Some(instance)) => format!(
            "{} — {instance}",
            device.board.as_deref().unwrap_or("unknown board")
        ),
        _ => "the id resolves to no connected interface right now — unplugged, moved to another \
              socket, or never here"
            .to_owned(),
    };

    let used_by = if device.used_by.is_empty() {
        String::new()
    } else {
        format!(
            "slots naming it: {} — removing it breaks them, so it needs the box below",
            device.used_by.join(", ")
        )
    };

    SlotValue::object(vec![
        ("alias".to_owned(), SlotValue::Text(device.alias.clone())),
        ("id".to_owned(), SlotValue::Text(device.id.clone())),
        (
            "backend".to_owned(),
            SlotValue::Text(device.backend.clone()),
        ),
        ("means".to_owned(), SlotValue::Text(device.means.clone())),
        (
            "presence".to_owned(),
            SlotValue::Text(if device.present {
                "connected".to_owned()
            } else {
                "not connected right now".to_owned()
            }),
        ),
        (
            "presenceCls".to_owned(),
            SlotValue::Text(
                if device.present {
                    "pill pill-ok"
                } else {
                    "pill pill-warn"
                }
                .to_owned(),
            ),
        ),
        ("claimText".to_owned(), SlotValue::Text(claim_text)),
        ("claimCls".to_owned(), SlotValue::Text(claim_cls.to_owned())),
        ("board".to_owned(), SlotValue::Text(board)),
        (
            "boardCls".to_owned(),
            SlotValue::Text(
                if device.present {
                    "dv-line mono"
                } else {
                    "dv-line dv-miss"
                }
                .to_owned(),
            ),
        ),
        ("commandLead".to_owned(), SlotValue::Text(command_lead)),
        ("command".to_owned(), SlotValue::Text(command)),
        (
            "commandCls".to_owned(),
            SlotValue::Text(command_cls.to_owned()),
        ),
        // The whole paragraph, straight from the writer that decided it —
        // including the half people miss, that a `port=` value names THIS PC's
        // USB topology and must not be copied to another cabinet.
        (
            "portWarn".to_owned(),
            SlotValue::Text(device.port_pinned_warning.clone().unwrap_or_default()),
        ),
        (
            "portWarnCls".to_owned(),
            SlotValue::Text(
                if device.port_pinned_warning.is_some() {
                    "dv-warn"
                } else {
                    "dv-warn dv-hide"
                }
                .to_owned(),
            ),
        ),
        (
            "usedByCls".to_owned(),
            SlotValue::Text(
                if device.used_by.is_empty() {
                    "dv-used dv-hide"
                } else {
                    "dv-used"
                }
                .to_owned(),
            ),
        ),
        ("usedBy".to_owned(), SlotValue::Text(used_by)),
        (
            "forceId".to_owned(),
            SlotValue::Text(format!("dv-force-{index}")),
        ),
        (
            "forceCls".to_owned(),
            SlotValue::Text(
                if device.used_by.is_empty() {
                    "dv-force dv-hide"
                } else {
                    "dv-force"
                }
                .to_owned(),
            ),
        ),
    ])
}

/// One pickable board as the row the page draws. Only boards WITH a keyboard
/// interface reach this — the rest go to [`other_row`], because a pick form on
/// a board ksx cannot capture is an offer that always refuses.
fn board_row(board: &BoardRow, index: usize) -> SlotValue {
    let keyboard = board.keyboard.clone().unwrap_or_default();
    let (command_lead, command, command_cls) = command_cell(
        board.claim_command.as_deref(),
        board.release_command.as_deref(),
    );
    SlotValue::object(vec![
        ("name".to_owned(), SlotValue::Text(board.name.clone())),
        (
            "ifaces".to_owned(),
            SlotValue::Text(format!(
                "{} interface(s) · keyboard on {keyboard}",
                board.interfaces.len()
            )),
        ),
        (
            "verdict".to_owned(),
            SlotValue::Text(board.keyboard_verdict.clone()),
        ),
        // The honest caveat. Without it "ksx could claim it" reads as a
        // recommendation, and on a real cabinet a mouse, an LED controller and
        // a fan controller all satisfy "it is HID".
        (
            "caveat".to_owned(),
            SlotValue::Text(if board.looks_like_a_keyboard {
                String::new()
            } else {
                "NOT declared as a keyboard. This is an HID interface and may be something else \
                 entirely — on a real cabinet a mouse, an LED controller and a fan controller all \
                 read as claimable."
                    .to_owned()
            }),
        ),
        (
            "caveatCls".to_owned(),
            SlotValue::Text(
                if board.looks_like_a_keyboard {
                    "dv-warn dv-hide"
                } else {
                    "dv-warn"
                }
                .to_owned(),
            ),
        ),
        (
            "configured".to_owned(),
            SlotValue::Text(match &board.alias {
                Some(alias) => format!("configured as \"{alias}\""),
                None => String::new(),
            }),
        ),
        (
            "configuredCls".to_owned(),
            SlotValue::Text(
                if board.alias.is_some() {
                    "pill pill-ok"
                } else {
                    "pill dv-hide"
                }
                .to_owned(),
            ),
        ),
        (
            "claimText".to_owned(),
            SlotValue::Text(
                if board.claimed {
                    "claimed — bound to winusb.sys"
                } else {
                    "on the Windows keyboard stack"
                }
                .to_owned(),
            ),
        ),
        (
            "claimCls".to_owned(),
            SlotValue::Text(
                if board.claimed {
                    "pill pill-ok"
                } else {
                    "pill pill-idle"
                }
                .to_owned(),
            ),
        ),
        ("commandLead".to_owned(), SlotValue::Text(command_lead)),
        ("command".to_owned(), SlotValue::Text(command)),
        (
            "commandCls".to_owned(),
            SlotValue::Text(command_cls.to_owned()),
        ),
        // What the form posts: the KEYBOARD interface's instance path, not the
        // board's parent. `plan_pick` resolves it through the same resolver
        // `ksx winusb claim` uses, so the page never has to know which of an
        // I-PAC's three devnodes carries the keys — `Board::keyboard()` decided
        // that, once, in the backend.
        ("query".to_owned(), SlotValue::Text(keyboard)),
        (
            "aliasId".to_owned(),
            SlotValue::Text(format!("dv-alias-{index}")),
        ),
        (
            "aliasHint".to_owned(),
            SlotValue::Text(board.alias.clone().unwrap_or_else(|| board.name.clone())),
        ),
        (
            "pickLabel".to_owned(),
            SlotValue::Text(
                if board.alias.is_some() {
                    "Re-pick — update this entry"
                } else {
                    "Use this board"
                }
                .to_owned(),
            ),
        ),
    ])
}

fn other_row(board: &BoardRow) -> SlotValue {
    SlotValue::object(vec![
        ("name".to_owned(), SlotValue::Text(board.name.clone())),
        (
            "ifaces".to_owned(),
            SlotValue::Text(format!(
                "{} interface(s) · no keyboard interface",
                board.interfaces.len()
            )),
        ),
    ])
}

fn list_values(payload: &DevicesPayload) -> [(&'static str, SlotValue); 4] {
    let configured = SlotValue::array(
        payload
            .scan
            .configured
            .iter()
            .enumerate()
            .map(|(i, d)| configured_row(d, i))
            .collect(),
    );
    let boards = SlotValue::array(
        payload
            .scan
            .boards
            .iter()
            .filter(|b| b.keyboard.is_some())
            .enumerate()
            .map(|(i, b)| board_row(b, i))
            .collect(),
    );
    let other = SlotValue::array(
        payload
            .scan
            .boards
            .iter()
            .filter(|b| b.keyboard.is_none())
            .map(other_row)
            .collect(),
    );
    let notes = SlotValue::array(
        payload
            .scan
            .notes
            .iter()
            .map(|note| SlotValue::object(vec![("note".to_owned(), SlotValue::Text(note.clone()))]))
            .collect(),
    );
    [
        (LIST_SLOT_CONFIGURED, configured),
        (LIST_SLOT_BOARDS, boards),
        (LIST_SLOT_OTHER, other),
        (LIST_SLOT_NOTES, notes),
    ]
}

fn show_values(
    payload: &DevicesPayload,
    flash: Option<&str>,
) -> [(&'static str, bool); SHOW_COUNT] {
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let unavailable = !payload.unavailable.trim().is_empty();
    let pickable = payload
        .scan
        .boards
        .iter()
        .filter(|b| b.keyboard.is_some())
        .count();
    let other = payload.scan.boards.len() - pickable;
    let session = &payload.session;
    [
        ("show:pillRunning", session.reachable && session.running),
        ("show:pillIdle", session.reachable && !session.running),
        ("show:pillDown", !session.reachable),
        ("show:showUnavailable", unavailable),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        // The only caution this page owes a running cabinet: the write lands in
        // config.toml, and the session already running keeps the devices it
        // opened until it is restarted.
        ("show:sessionLive", session.reachable && session.running),
        ("show:hasConfigured", !payload.scan.configured.is_empty()),
        ("show:noConfigured", payload.scan.configured.is_empty()),
        ("show:hasBoards", pickable > 0),
        // Deliberately NOT the complement: when the scan itself refused, the
        // empty list is not a reading of this machine, and saying "no
        // keyboard-capable board found" under the refusal banner would answer
        // a question nothing asked.
        ("show:noBoards", pickable == 0 && !unavailable),
        ("show:hasOther", other > 0),
        ("show:hasNotes", !payload.scan.notes.is_empty()),
    ]
}

/// Slot ids of every slot named `name`, in slot-table (== document) order.
fn named_slot_ids(module: &IrModule, name: &str) -> Vec<u16> {
    module
        .slots
        .entries()
        .iter()
        .filter(|e| module.strings.get(e.name_str_idx).is_ok_and(|n| n == name))
        .map(|e| e.slot_id)
        .collect()
}

fn build_slots(module: &IrModule, payload: &DevicesPayload, flash: Option<&str>) -> SlotData {
    let scalars = scalar_slots(payload, flash).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));

    for (name, value) in list_values(payload) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    for (name, value) in show_values(payload, flash) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, SlotValue::Bool(value));
        }
    }
    slots
}

/// Render `/devices` for one payload: SSR slots for the first paint, the same
/// data as the source payload for hydration.
pub(crate) fn render_devices(
    page: &EmbeddedPage,
    payload: &DevicesPayload,
    flash: Option<&str>,
) -> PageOutput {
    let slots = build_slots(&page.module, payload, flash);
    let prefix = body_prefix(payload, "/devices");
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — devices",
        route_pattern: "/devices",
        manifest: &page.manifest,
        config_script: None,
        config_json: None,
        body_class: None,
        personality_css: Some(PERSONALITY_CSS),
        body_prefix: Some(&prefix),
        render_mode: RenderMode::Phase2SsrReconcile,
        ir_module: Some(&page.module),
        slots: Some(&slots),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::SessionView;
    use crate::render::assert_icon_links_in_head;
    use ksx_api::{DeviceScanView, UsbRow};

    const PANEL: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
    const AUX: &str = r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001";
    const FAN: &str = r"USB\VID_1E71&PID_300E&MI_01\7&8FBF878&0&0001";

    fn iface(id: &str, state: &str) -> UsbRow {
        UsbRow {
            instance_id: id.to_owned(),
            description: "USB Input Device".to_owned(),
            state: state.to_owned(),
            verdict: "bound to winusb.sys — ksx can capture this".to_owned(),
            alias: None,
            selected: false,
            ready: false,
            vendor: Some("Ultimarc I-PAC 4X".to_owned()),
            board: Some(r"USB\VID_D209&PID_0430\4".to_owned()),
            boot_keyboard: true,
        }
    }

    /// The reference cabinet, in the shape the api serves: one claimed I-PAC
    /// wearing two devnodes, one fan controller with no keyboard interface,
    /// and one configured entry whose id is PORT-PINNED.
    fn cabinet() -> DevicesPayload {
        DevicesPayload {
            scan: DeviceScanView {
                generated_at: "2026-08-07 12:00:00 UTC".into(),
                usb_available: true,
                boards: vec![
                    ksx_api::BoardRow {
                        name: "Ultimarc I-PAC 4X".into(),
                        interfaces: vec![iface(PANEL, "claimed"), iface(AUX, "not-a-keyboard")],
                        keyboard: Some(PANEL.to_owned()),
                        keyboard_verdict: "bound to winusb.sys — ksx can capture this".into(),
                        looks_like_a_keyboard: true,
                        claimed: true,
                        alias: Some("panel".into()),
                        claim_command: None,
                        release_command: Some(format!("ksx winusb release {PANEL} --yes")),
                    },
                    ksx_api::BoardRow {
                        name: "NZXT fan controller".into(),
                        interfaces: vec![UsbRow {
                            boot_keyboard: false,
                            ..iface(FAN, "not-a-keyboard")
                        }],
                        keyboard: None,
                        keyboard_verdict: "no keyboard interface — ksx cannot capture this board"
                            .into(),
                        looks_like_a_keyboard: false,
                        claimed: false,
                        alias: None,
                        claim_command: None,
                        release_command: None,
                    },
                ],
                configured: vec![ksx_api::ConfiguredDevice {
                    alias: "panel".into(),
                    id: "port=7&25EEA38C&0&0000".into(),
                    backend: "winusb".into(),
                    rung: "port".into(),
                    survives_replug: false,
                    means: "this exact USB socket".into(),
                    port_pinned_warning: Some(
                        "PORT-PINNED — nothing weaker than the socket separates this board from \
                         its twin. Move it to another USB port and this entry stops matching. It \
                         is also specific to THIS machine: the port names this PC's USB topology, \
                         so do not copy this config to another cabinet — run `ksx device pick` \
                         there instead."
                            .into(),
                    ),
                    present: true,
                    board: Some("Ultimarc I-PAC 4X".into()),
                    instance_id: Some(PANEL.to_owned()),
                    claimed: true,
                    claim_command: None,
                    release_command: Some(format!("ksx winusb release {PANEL} --yes")),
                    used_by: vec!["slot 1 (keyboard)".into()],
                }],
                notes: vec!["Interception is installed but ksx is not using it".into()],
            },
            session: SessionView {
                reachable: true,
                running: false,
                line: "idle — daemon reachable".into(),
                profile: None,
            },
            unavailable: String::new(),
            flash: None,
        }
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load("/devices").expect("embedded page must load");
        assert_eq!(page.module.header.version, 2);
    }

    /// The gate every page must call. Pins the scalar names, the exact list
    /// slot names, the exact `show:` name set, the island table — and then the
    /// contract a name-exists check cannot state: injected == rendered, both
    /// ways. See `render.rs::assert_island_slot_contract` for why.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let module = &page.module;
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&DevicesPayload::default(), None);
        for key in scalars.as_object().unwrap().keys() {
            assert!(
                names.contains(&key.as_str()),
                "scalar slot '{key}' missing from the embedded IR; slots: {names:?}"
            );
        }

        let array_slots: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("list:") && n.ends_with(":array"))
            .collect();
        assert_eq!(
            array_slots,
            [
                LIST_SLOT_CONFIGURED,
                LIST_SLOT_BOARDS,
                LIST_SLOT_OTHER,
                LIST_SLOT_NOTES
            ],
            "list slot names drifted between DevicesIsland.ts and the \
             LIST_SLOT_* constants; slots: {names:?}"
        );

        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> =
            show_values(&DevicesPayload::default(), None)
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between DevicesIsland.ts and show_values()"
        );
        assert_eq!(
            ir_shows.len(),
            SHOW_COUNT,
            "SHOW_COUNT is stale; slots: {names:?}"
        );

        let islands = module.islands.entries();
        assert_eq!(islands.len(), 1, "expected exactly one island");
        assert_eq!(
            module.strings.get(islands[0].name_str_idx).unwrap(),
            ISLAND_COMPONENT
        );
        assert!(
            !islands[0].slot_ids.is_empty(),
            "island slot_ids are empty — native data-forma-props will not be emitted"
        );

        let injected: Vec<&str> = scalars
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .chain(
                list_values(&DevicesPayload::default())
                    .iter()
                    .map(|(n, _)| *n),
            )
            .chain(seam_shows.iter().copied())
            .collect();
        crate::render::assert_island_slot_contract(
            module,
            &injected,
            &CLIENT_ONLY_SLOTS,
            &ANONYMOUS_SLOTS,
        );
    }

    /// The whole point of the page: a board, a configured entry, and the
    /// picker's hidden `query` carrying the KEYBOARD interface rather than the
    /// board's parent path.
    #[test]
    fn render_injects_real_scan_data_into_ssr_html() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(&page, &cabinet(), None);
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        assert!(out.html.contains("Ultimarc I-PAC 4X"), "{}", out.html);
        assert!(out.html.contains("NZXT fan controller"), "{}", out.html);
        assert!(
            out.html.contains("1 keyboard-capable board"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("1 [[device]] entry in config.toml"),
            "{}",
            out.html
        );
        // The pick form posts the interface, canonical form and all.
        assert!(
            out.html
                .contains("USB\\VID_D209&amp;PID_0430&amp;MI_00\\7&amp;25EEA38C&amp;0&amp;0000"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"action="/devices/pick""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"action="/devices/remove""#),
            "{}",
            out.html
        );
        assert!(
            out.html
                .contains(r#"<noscript><meta http-equiv="refresh" content="5; url=/devices">"#),
            "{}",
            out.html
        );
        assert_icon_links_in_head("/devices", &out.html);
    }

    /// The PORT-PINNED paragraph must survive to the page IN FULL — both
    /// halves. The second one is the half people miss, and it is the reason the
    /// warning lives on the ENTRY rather than in `pick`'s console output: a
    /// config that gets copied to a second cabinet silently stops matching.
    #[test]
    fn the_port_pinned_warning_reaches_the_page_including_the_machine_specific_half() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(&page, &cabinet(), None);
        assert!(out.html.contains("PORT-PINNED"), "{}", out.html);
        assert!(
            out.html.contains("stops matching"),
            "the replug half is missing: {}",
            out.html
        );
        assert!(
            out.html
                .contains("do not copy this config to another cabinet"),
            "the MACHINE-SPECIFIC half is missing — this is the half people \
             miss, and a warning that only says 'do not move it' is a warning \
             that lets a config travel: {}",
            out.html
        );
    }

    /// Claiming needs elevation, so `docs/SURFACES.md` §3 marks it "never" for
    /// the browser. The command must be on the page as TEXT, and there must be
    /// no form that posts it.
    #[test]
    fn the_claim_and_release_commands_are_shown_and_never_posted() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(&page, &cabinet(), None);
        assert!(out.html.contains("ksx winusb release"), "{}", out.html);
        assert!(
            out.html.contains("ELEVATED shell"),
            "a command a page hands out without saying it needs elevation \
             produces one 'access denied' and no explanation: {}",
            out.html
        );
        for forbidden in [
            r#"action="/devices/claim""#,
            r#"action="/winusb/claim""#,
            r#"action="/devices/release""#,
            r#"action="/winusb/release""#,
        ] {
            assert!(
                !out.html.contains(forbidden),
                "{forbidden} is a form on a surface that cannot elevate: {}",
                out.html
            );
        }
    }

    /// The three removals are routinely confused, so the page names all three
    /// and says what each one does NOT do.
    #[test]
    fn the_page_distinguishes_the_three_removals() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(&page, &cabinet(), None);
        assert!(
            out.html.contains("Three different removals"),
            "{}",
            out.html
        );
        assert!(out.html.contains("ksx pads --prune"), "{}", out.html);
        assert!(out.html.contains("ksx winusb release"), "{}", out.html);
        assert!(out.html.contains("Remove entry"), "{}", out.html);
    }

    /// A refused scan must never render as an empty machine. On a cabinet with
    /// four boards plugged in, "no keyboard-capable board found" is the worst
    /// answer this page could give.
    #[test]
    fn a_refused_scan_says_so_instead_of_drawing_an_empty_machine() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let payload = DevicesPayload {
            unavailable: "listing devices is not available on this surface — run `ksx devices`"
                .to_owned(),
            ..DevicesPayload::default()
        };
        let out = render_devices(&page, &payload, None);
        assert!(out.html.contains("could not be read"), "{}", out.html);
        assert!(out.html.contains("run `ksx devices`"), "{}", out.html);
        assert!(
            !out.html.contains("no keyboard-capable board found"),
            "the empty-machine sentence must not appear under a refusal: {}",
            out.html
        );

        // …and the ordinary empty machine still says its own sentence. Note
        // `usb_available: true` — three states, not two, and the page must
        // tell them apart: the scan refused, the scan ran and found nothing,
        // and the enumeration itself failed. `DeviceScanView::default()` is
        // the THIRD (nothing has been read), which is why it cannot stand in
        // for the second.
        let empty = render_devices(
            &page,
            &DevicesPayload {
                scan: DeviceScanView {
                    usb_available: true,
                    ..DeviceScanView::default()
                },
                ..DevicesPayload::default()
            },
            None,
        );
        assert!(
            empty.html.contains("no keyboard-capable board found"),
            "{}",
            empty.html
        );

        // And the enumeration failing is its own sentence again: an empty list
        // on a cabinet with four boards plugged in must never read as an empty
        // cabinet.
        let blind = render_devices(&page, &DevicesPayload::default(), None);
        assert!(
            blind.html.contains("nothing could be READ"),
            "{}",
            blind.html
        );
    }

    /// A hostile flash is a query-string value and is attacker-writable. It
    /// must arrive escaped, on this page exactly as on the other two.
    #[test]
    fn the_flash_is_escaped() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(
            &page,
            &cabinet(),
            Some("error: <script>alert(1)</script> & \"quotes\""),
        );
        assert!(!out.html.contains("<script>alert(1)"), "{}", out.html);
        assert!(out.html.contains("&lt;script&gt;"), "{}", out.html);
    }

    /// One struct, one serializer: the block the page embeds is the shape
    /// `GET /api/devices` serves, so the seed and every poll agree.
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let payload = cabinet();
        let out = render_devices(&page, &payload, None);
        let api = serde_json::to_value(&payload).unwrap();
        let embedded = crate::render::payload_json(&payload).replace("\\u003c", "<");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&embedded).unwrap(),
            api
        );
        assert!(out.html.contains(r#"id="__ksx-payload""#), "{}", out.html);
    }

    /// The nav is static markup duplicated per island, so a page is invisible
    /// until every sibling links to it. Pin the whole rail from here as well as
    /// from the other two pages' tests.
    #[test]
    fn the_nav_reaches_every_sibling_page() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(&page, &cabinet(), None);
        assert!(out.html.contains(r#"href="/""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/map""#), "{}", out.html);
        assert!(
            out.html.contains(r#"aria-current="page""#),
            "the current route must be marked: {}",
            out.html
        );
    }

    /// A running session keeps the devices it already opened. Writing config
    /// while one is up is legal and useful (that is the whole point of a
    /// daemon-free write path), but the page has to say what it will NOT do.
    #[test]
    fn a_running_session_gets_the_restart_caution() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let mut payload = cabinet();
        payload.session.running = true;
        let out = render_devices(&page, &payload, None);
        assert!(
            out.html.contains("stopped and started again"),
            "{}",
            out.html
        );
    }
}
