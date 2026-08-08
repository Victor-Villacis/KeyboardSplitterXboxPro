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

// ---------------------------------------------------------------------------
// derivations (mirrored in studio-ui/src/DevicesIsland.ts)
// ---------------------------------------------------------------------------
//
// What is NOT here any more, and must never come back: the pickable/other
// PARTITION, the board and entry COUNTS, the three summary sentences, the
// "ksx run will refuse" verdict, the ELEVATED command leads and the HID
// caveat. Every one of those was computed here AND again in the island's
// TypeScript, which is `docs/SURFACES.md` §1 broken twice over — and the
// `usb_available` bug proved the cost, because only one of the two copies had
// to forget the flag for the page to tell a cabinet with four boards plugged
// in that it had none.
//
// They are `ksx_api::DeviceScanView::read`'s now. What survives below is
// genuinely this page's: which CSS class a value maps to, and how a row's
// element ids are spelled.

/// The pill class for a value `ksx_api` has already judged.
///
/// The level word travels; the class is built from it, so adding a level in the
/// backend cannot leave a surface silently rendering the wrong colour — it
/// renders `pill pill-<level>`, and an unstyled level is visible rather than
/// wrong. `pill-none` is the hidden one (studio.css).
fn pill_of(level: &str) -> String {
    format!("pill pill-{level}")
}

/// A `(text, class)` pair for a line that is rendered on every row and hidden
/// when it has nothing to say — the constraint this page's module docs open
/// with: a `createShow` inside a `createList` is not a shape this compiler
/// emits.
fn optional_line(text: &str, class: &'static str) -> (String, String) {
    if text.is_empty() {
        (String::new(), format!("{class} dv-hide"))
    } else {
        (text.to_owned(), class.to_owned())
    }
}

fn scalar_slots(payload: &DevicesPayload, flash: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "generatedAt": payload.scan.generated_at,
        "sessionLine": payload.session.line,
        "flashLine": flash.unwrap_or(""),
        "unavailableLine": payload.unavailable.trim(),
        "configuredSummary": payload.scan.configured_summary,
        "boardsSummary": payload.scan.boards_summary,
        "otherSummary": payload.scan.other_summary,
    })
}

/// One configured `[[device]]` entry as the row the page draws.
///
/// `index` only ever becomes an element id (`dv-force-3`), so the checkbox and
/// its label can be associated without inventing an id out of an alias a user
/// chose — an alias may contain spaces and quotes, and an `id` attribute built
/// from one would be a selector nobody can write.
fn configured_row(device: &ConfiguredDevice, index: usize) -> SlotValue {
    // The claim state and the one combination that is actually a fault are
    // `DeviceScanView::read`'s judgement, not this page's — it is a verdict
    // about what `ksx run` does, and `run` is not something a render seam can
    // see. All that happens here is level → class.
    let (command_lead, command_cls) = optional_line(&device.command_lead, "dv-cmd");

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
        ("rung".to_owned(), SlotValue::Text(device.rung.clone())),
        // Both RENDERED, on their own line beside the id. `backend` used to be
        // computed into this object and read by nothing, which meant the page
        // never said whether an entry was `winusb` or `interception` — the one
        // field the health verdict above it is reasoning about — and `rung`
        // was not carried at all.
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
        (
            "claimText".to_owned(),
            SlotValue::Text(device.health_line.clone()),
        ),
        (
            "claimCls".to_owned(),
            SlotValue::Text(pill_of(&device.health_level)),
        ),
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
        (
            "command".to_owned(),
            SlotValue::Text(device.command.clone()),
        ),
        ("commandCls".to_owned(), SlotValue::Text(command_cls)),
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
    let (command_lead, command_cls) = optional_line(&board.command_lead, "dv-cmd");
    let (caveat, caveat_cls) = optional_line(&board.caveat, "dv-warn");
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
        // The honest caveat, worded by `ksx_api` (`CAVEAT_NOT_A_KEYBOARD`).
        // Without it "ksx could claim it" reads as a recommendation, and on a
        // real cabinet a mouse, an LED controller and a fan controller all
        // satisfy "it is HID".
        ("caveat".to_owned(), SlotValue::Text(caveat)),
        ("caveatCls".to_owned(), SlotValue::Text(caveat_cls)),
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
        ("command".to_owned(), SlotValue::Text(board.command.clone())),
        ("commandCls".to_owned(), SlotValue::Text(command_cls)),
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
    // `b.pickable`, never `b.keyboard.is_some()`: the partition is
    // `DeviceScanView::read`'s single decision, and re-deriving it here is how
    // the seam and the island came to disagree about the count in the sentence
    // above the list.
    let boards = SlotValue::array(
        payload
            .scan
            .boards
            .iter()
            .filter(|b| b.pickable)
            .enumerate()
            .map(|(i, b)| board_row(b, i))
            .collect(),
    );
    let other = SlotValue::array(
        payload
            .scan
            .boards
            .iter()
            .filter(|b| !b.pickable)
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
    let scan = &payload.scan;
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
        ("show:hasConfigured", !scan.configured.is_empty()),
        // Deliberately NOT the complement of `hasConfigured`, and deliberately
        // not `configured.is_empty()` either. `no_configured_device` and
        // `no_pickable_board_found` are the two flags that license this page to
        // say "there is nothing here", and `DeviceScanView` only ever sets them
        // when the list is empty AND something was actually read. A refusal
        // arrives as `DeviceScanView::default()`, where both are false, so the
        // empty-state paragraphs stay off the screen and the refusal banner is
        // the only thing that speaks.
        ("show:noConfigured", scan.no_configured_device),
        ("show:hasBoards", scan.pickable_boards > 0),
        ("show:noBoards", scan.no_pickable_board_found),
        ("show:hasOther", scan.other_boards > 0),
        ("show:hasNotes", !scan.notes.is_empty()),
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
    use crate::render::assert_complete_head;
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
            // The selector `scan` would write for this row. Deliberately a
            // constant and not derived from `id` here: `UsbRow::selector` exists
            // precisely so no surface re-derives what the writer decided
            // (`docs/SURFACES.md` §1), and a fixture that computed it would be
            // re-deriving it in a third place to test the other two.
            selector: Some("usb:d209:0430:00".to_owned()),
        }
    }

    /// The reference cabinet, in the shape the api serves: one claimed I-PAC
    /// wearing two devnodes, one fan controller with no keyboard interface,
    /// and one configured entry whose id is PORT-PINNED.
    ///
    /// Built through `DeviceScanView::read`, deliberately — a fixture that
    /// wrote the summary lines, the counts, the health verdict and the elevated
    /// leads as literals would be a fixture that already contains the answers
    /// these tests are asking about, and it could not disagree with the page
    /// even when the page was wrong.
    fn cabinet_scan() -> DeviceScanView {
        DeviceScanView::read(
            "2026-08-07 12:00:00 UTC".into(),
            true,
            vec![
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
                    ..ksx_api::BoardRow::default()
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
                    ..ksx_api::BoardRow::default()
                },
            ],
            vec![ksx_api::ConfiguredDevice {
                alias: "panel".into(),
                id: "port=7&25EEA38C&0&0000".into(),
                backend: "winusb".into(),
                rung: "port".into(),
                survives_replug: false,
                means: "this exact USB socket".into(),
                port_pinned_warning: Some(ksx_app_port_pinned_warning_stand_in().to_owned()),
                present: true,
                board: Some("Ultimarc I-PAC 4X".into()),
                instance_id: Some(PANEL.to_owned()),
                claimed: true,
                claim_command: None,
                release_command: Some(format!("ksx winusb release {PANEL} --yes")),
                used_by: vec!["slot 1 (keyboard)".into()],
                ..ksx_api::ConfiguredDevice::default()
            }],
            vec!["Interception is installed but ksx is not using it".into()],
        )
    }

    /// ksx-studio does not depend on ksx-app, so the paragraph cannot be
    /// imported. It is reproduced with the two halves the tests assert on and
    /// nothing else, and `ksx-app`'s own
    /// `the_port_pinned_warning_says_both_halves` pins the real constant — so
    /// the two cannot silently diverge on the parts that matter.
    fn ksx_app_port_pinned_warning_stand_in() -> &'static str {
        "PORT-PINNED — nothing weaker than the Windows instance path separates this board from \
         its twin, so this entry matches only while Windows keeps reporting that exact path. \
         Moving the board to another USB socket is the usual way that changes, and the entry then \
         stops matching. It is also specific to THIS machine, so do not copy this config to \
         another cabinet — run `ksx device pick` there instead."
    }

    fn cabinet() -> DevicesPayload {
        DevicesPayload {
            scan: cabinet_scan(),
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
        assert_complete_head("/devices", &out.html);
    }

    /// **Every list ITEM field the seam fills is bound, and every one the
    /// island binds is filled — both ways.**
    ///
    /// `assert_island_slot_contract` cannot state this: it checks scalars,
    /// `list:*:array` names and `show:*` names, and stops. So a row field could
    /// be computed on both sides and read by neither (`backend` was, for the
    /// whole life of this page) or bound by the island and never filled by the
    /// seam (which renders the authored default forever, server-side).
    ///
    /// The compiler names an item binding `list:<signal>:<field>`, so the IR
    /// answers this exactly. FAILS against the shipped page on `backend`.
    #[test]
    fn every_row_field_is_bound_and_every_bound_row_field_is_filled() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let module = &page.module;
        let ir_names: std::collections::BTreeSet<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let payload = cabinet();
        for (list_slot, value) in list_values(&payload) {
            // "list:configuredRows:array" → "configuredRows"
            let signal = list_slot
                .strip_prefix("list:")
                .and_then(|s| s.strip_suffix(":array"))
                .expect("list slot names are list:<signal>:array");

            let SlotValue::Array(rows) = &value else {
                panic!("{list_slot} is not an array");
            };
            let first = rows.first().unwrap_or_else(|| {
                panic!("the cabinet fixture must populate {signal}, or this proves nothing")
            });
            let SlotValue::Object(fields) = first else {
                panic!("{signal} rows are not objects");
            };

            let filled: std::collections::BTreeSet<String> =
                fields.iter().map(|(k, _)| k.clone()).collect();
            // `array` is the list itself and `item` is the compiler's own
            // per-iteration handle; neither is a field the seam fills.
            let bound: std::collections::BTreeSet<String> = ir_names
                .iter()
                .filter_map(|n| n.strip_prefix(&format!("list:{signal}:")))
                .filter(|f| *f != "array" && *f != "item")
                .map(str::to_owned)
                .collect();

            let unread: Vec<&String> = filled.difference(&bound).collect();
            assert!(
                unread.is_empty(),
                "{signal} rows carry field(s) the island never reads, so the page is silent \
                 about them: {unread:?}"
            );
            let unfilled: Vec<&String> = bound.difference(&filled).collect();
            assert!(
                unfilled.is_empty(),
                "the island binds {signal} field(s) the seam never fills, so the SSR paint \
                 renders their authored defaults: {unfilled:?}"
            );
        }
    }

    /// **Every field the row object carries is RENDERED.**
    ///
    /// FAILS against the shipped page for two of them. `backend` was computed
    /// into the row object and into `ConfiguredTile`, and the h() tree read it
    /// nowhere — so the page never said whether an entry was `winusb` or
    /// `interception`, which is the exact field the health pill above it is
    /// reasoning about. `rung` was not carried at all, though the commit
    /// summary listed it. `assert_island_slot_contract` cannot catch either:
    /// it checks scalars, `list:*:array` names and `show:*` names, never list
    /// ITEM fields.
    ///
    /// Asserted on the rendered HTML, because "the row object has the key" is
    /// precisely what was true while the page stayed silent.
    #[test]
    fn every_configured_row_field_reaches_the_html() {
        let page = EmbeddedPage::load("/devices").unwrap();
        let out = render_devices(&page, &cabinet(), None);
        let scan = cabinet_scan();
        let device = &scan.configured[0];

        // A value that appears in the html only as part of a LONGER string
        // would pass a naive `contains`, so each is checked in the labelled
        // position the tree puts it in.
        for (label, value) in [
            ("backend", device.backend.as_str()),
            ("rung", device.rung.as_str()),
        ] {
            let marker = format!(">{label}</span>");
            let at = out
                .html
                .find(&marker)
                .unwrap_or_else(|| panic!("the '{label}' label is not on the page: {}", out.html));
            let after = &out.html[at + marker.len()..];
            assert!(
                after
                    .split("</span>")
                    .next()
                    .is_some_and(|cell| cell.contains(value)),
                "'{label}' is labelled on the page but its value ({value:?}) does not follow \
                 it: {}",
                &after[..after.len().min(200)]
            );
        }

        // The rest of the row, in the same spirit: present in the output, not
        // merely present in the object.
        assert!(out.html.contains(&device.alias), "{}", out.html);
        assert!(out.html.contains(&device.means), "{}", out.html);
        assert!(out.html.contains("slots naming it"), "{}", out.html);
    }

    /// The health verdict is `ksx_api`'s, and the page renders BOTH halves of
    /// it — the sentence and the severity the sentence is worth.
    ///
    /// FAILS against the shipped page, which minted its own verdict from
    /// `present && !claimed && backend == "winusb"` and told an entry no slot
    /// names that "ksx run will refuse", which it does not.
    #[test]
    fn the_health_verdict_and_its_severity_come_from_the_backend() {
        let page = EmbeddedPage::load("/devices").unwrap();

        let loose = |used_by: Vec<String>| {
            let scan = DeviceScanView::read(
                "t".into(),
                true,
                Vec::new(),
                vec![ksx_api::ConfiguredDevice {
                    alias: "panel".into(),
                    id: "usb:d209:0430:00".into(),
                    backend: "winusb".into(),
                    present: true,
                    claimed: false,
                    used_by,
                    ..ksx_api::ConfiguredDevice::default()
                }],
                Vec::new(),
            );
            let expected = scan.configured[0].clone();
            let out = render_devices(
                &page,
                &DevicesPayload {
                    scan,
                    ..DevicesPayload::default()
                },
                None,
            );
            (out.html, expected)
        };

        let (named, expected) = loose(vec!["slot 1 (keyboard)".into()]);
        assert!(named.contains("refuses to start"), "{named}");
        assert!(
            named.contains(&format!("pill pill-{}", expected.health_level)),
            "the severity the backend judged ({}) is not the class the page drew: {named}",
            expected.health_level
        );
        assert!(named.contains("pill pill-warn"), "{named}");

        let (orphan, _) = loose(Vec::new());
        assert!(
            !orphan.contains("refuses to start"),
            "no slot names this alias, so nothing refuses — the page must not send the user to \
             debug a session that works: {orphan}"
        );
        assert!(orphan.contains("NOT claimed"), "{orphan}");
        assert!(!orphan.contains("pill pill-warn"), "{orphan}");
    }

    /// The PORT-PINNED paragraph must survive to the page IN FULL — both
    /// halves. The second one is the half people miss, and it is the reason the
    /// warning lives on the ENTRY rather than in `pick`'s console output: a
    /// config that gets copied to a second cabinet silently stops matching.
    ///
    /// A transport check, not a wording check: the fixture supplies the
    /// paragraph and this proves the seam carries it whole, so a `portWarn`
    /// that got dropped or truncated fails here. `ksx-app`'s
    /// `the_port_pinned_warning_says_both_halves_and_promises_neither_too_hard`
    /// is what pins the WORDS.
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

    /// **The three empty states, and the assertion each one is allowed to
    /// make.**
    ///
    /// There are three, not two, and the page must tell them apart: the scan
    /// REFUSED, the scan RAN and found nothing, and the ENUMERATION ITSELF
    /// FAILED. Only the middle one licenses "there is nothing here". The other
    /// two are "I could not read this", and a user acts on those differently —
    /// this project's signature bug is a session reporting success while the
    /// arcade panel was dead.
    ///
    /// FAILS against the shipped page. Its failed-enumeration block asserted
    /// only that "nothing could be READ" appeared and checked no absence at
    /// all, so the contradicting sentence — "No board here exposes a keyboard
    /// interface" printed directly beneath the banner saying nothing could be
    /// read — sailed through. Every block below now asserts BOTH what the state
    /// says and what it must not say.
    #[test]
    fn the_three_empty_states_are_three_different_pages() {
        let page = EmbeddedPage::load("/devices").unwrap();

        // The sentences only ONE of the three states may print. Named through
        // ksx_api so a reworded page cannot quietly stop being checked.
        let absence_claims = [
            ksx_api::NO_BOARDS_LINE,
            "no board it found exposes a",
            "No board is configured yet",
            "no [[device]] entries in config.toml",
        ];
        let unreadable_claims = ["nothing could be READ"];

        // (1) THE SCAN REFUSED. `unavailable` is set and the scan degrades to
        // `DeviceScanView::default()`, exactly as server.rs's error arms build
        // it.
        let refused = render_devices(
            &page,
            &DevicesPayload {
                unavailable: "listing devices is not available on this surface — run `ksx devices`"
                    .to_owned(),
                ..DevicesPayload::default()
            },
            None,
        );
        assert!(
            refused.html.contains("could not be read"),
            "{}",
            refused.html
        );
        assert!(
            refused.html.contains("run `ksx devices`"),
            "{}",
            refused.html
        );
        for claim in absence_claims {
            assert!(
                !refused.html.contains(claim),
                "a refused read printed an assertion of absence ({claim:?}): {}",
                refused.html
            );
        }

        // (2) THE ENUMERATION FAILED. No refusal banner — the surface answered
        // — but `usb_available` is false, so the list is empty because nothing
        // could be READ. This is the block that used to be one-sided.
        let blind = render_devices(
            &page,
            &DevicesPayload {
                scan: DeviceScanView::read(
                    "2026-08-07 12:00:00 UTC".into(),
                    false,
                    Vec::new(),
                    Vec::new(),
                    vec!["the USB enumeration returned no interfaces".into()],
                ),
                ..DevicesPayload::default()
            },
            None,
        );
        for claim in unreadable_claims {
            assert!(blind.html.contains(claim), "{}", blind.html);
        }
        for claim in [ksx_api::NO_BOARDS_LINE, "no board it found exposes a"] {
            assert!(
                !blind.html.contains(claim),
                "a failed enumeration printed the empty-machine sentence ({claim:?}) — on a \
                 cabinet with four boards plugged in this is the worst answer the page can \
                 give: {}",
                blind.html
            );
        }

        // (3) THE MACHINE REALLY IS EMPTY. The enumeration answered and found
        // nothing, so the page says so — and must NOT hedge with "could not be
        // read", or the fix above would just be silence everywhere.
        let empty = render_devices(
            &page,
            &DevicesPayload {
                scan: DeviceScanView::read(
                    "2026-08-07 12:00:00 UTC".into(),
                    true,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                ),
                ..DevicesPayload::default()
            },
            None,
        );
        assert!(
            empty.html.contains(ksx_api::NO_BOARDS_LINE),
            "{}",
            empty.html
        );
        assert!(
            empty.html.contains("no board it found exposes a"),
            "{}",
            empty.html
        );
        assert!(
            empty.html.contains("No board is configured yet"),
            "{}",
            empty.html
        );
        for claim in unreadable_claims {
            assert!(
                !claim.is_empty() && !empty.html.contains(claim),
                "a machine that WAS read must not claim it could not be: {}",
                empty.html
            );
        }

        // …and the three pages are three different pages, not three spellings
        // of one. If any two ever render identically the distinction has been
        // lost regardless of which sentences are present.
        assert_ne!(refused.html, blind.html);
        assert_ne!(blind.html, empty.html);
        assert_ne!(refused.html, empty.html);
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
        assert!(out.html.contains(r#"href="/pads""#), "{}", out.html);
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
