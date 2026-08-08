//! The `/start` render seam — **the first run**, `docs/FIRST-RUN.md` moments 4
//! to 7: choose a keyboard, choose a controller, map it and answer the one
//! question, then save or play.
//!
//! Structurally identical to `render_devices.rs`: four seams (scalars, lists,
//! shows, `build_slots`), one page entry, one layout test that calls
//! [`crate::render::assert_island_slot_contract`]. Read `render.rs`'s module
//! docs for why the data is emitted twice and why "the slot exists" is not the
//! check.
//!
//! # Why a NEW page and not a rebuilt `/setup`
//!
//! `/setup` is the CONFIGURATION: export it, import it, wire a slot in the
//! config that exists, prove a key lands. Every one of its steps reads
//! `config.toml` and writes to it, which `docs/SURFACES.md` §9 flow 1 states
//! outright — "each reads the config as it stands and writes one complete
//! thing". That is the right page for someone who has a cabinet and is changing
//! it.
//!
//! This page is for someone who has nothing, and its defining property is the
//! opposite one: **it does not read the config and does not write it** until a
//! button that says Save is pressed (§2). Rebuilding `/setup` around a staged
//! value would have left one screen holding both rules — some controls writing
//! immediately, some not — and the whole point of staging is that a user can
//! tell which is which without being told. Two pages, two contracts, and the
//! flash after Save is the moment the first becomes the second.
//!
//! # What this page decides, and what it must not
//!
//! Nothing here reads hardware, judges a persona, counts an XInput ceiling or
//! words the split-or-freeze question. Those are `ksx_api::StagedSetupView`
//! (rosters, ceilings, §3's wording) and `ksx_api::DeviceScanView` (the boards
//! and what can reach them), and everything this page SAYS about them is
//! composed once in `snapshot.rs`'s `StartLines` / `StartFlags` / `StartRows`.
//! What is left here is placement: which composed string goes in which slot.
//!
//! # The rule this page exists to keep
//!
//! **Looking is never a commitment.** No GET on this page claims a board,
//! plugs a pad or writes a file — including the Rescan control, which is a link
//! back to `/start` (`FIRST-RUN.md` §5's "a visible rescan", and §6's "nothing
//! that looks like a menu choice claims a board"). Every write is a POST, and
//! there are exactly two of them that touch anything outside the daemon's
//! memory: `/start/save` and `/start/play`.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{body_prefix, with_icon_links, EmbeddedPage, PERSONALITY_CSS};
use crate::snapshot::{
    StartBlockingRow, StartBoardRow, StartGapRow, StartLayoutRow, StartOptionRow, StartOtherRow,
    StartPayload, StartSlotRow, StartTextRow,
};

/// List slot names, binding-derived (compiler 0.2.0): a `createList` reading
/// `() => boardRows()` compiles to `list:boardRows:array`. Rename a list signal
/// in `StartIsland.ts` and the layout test fails until these match again.
const LIST_SLOT_BOARDS: &str = "list:boardRows:array";
const LIST_SLOT_OTHER: &str = "list:otherRows:array";
const LIST_SLOT_NOTES: &str = "list:noteRows:array";
const LIST_SLOT_SLOTS: &str = "list:slotRows:array";
const LIST_SLOT_PERSONAS: &str = "list:personaOptions:array";
const LIST_SLOT_GAPS: &str = "list:gapRows:array";
const LIST_SLOT_BLOCKING: &str = "list:blockingRows:array";
/// The layout menu appears TWICE — once on "Add a controller", once on "give
/// slot N this layout" — so the second occurrence gets the `#2` suffix by
/// document order, exactly like the mapper's `slotTabs`/`slotTabs#2` pair.
/// Both receive the same array: it is one menu, drawn in two places.
const LIST_SLOT_LAYOUTS: &str = "list:layoutOptions:array";
const LIST_SLOT_LAYOUTS_2: &str = "list:layoutOptions#2:array";
const LIST_SLOT_LAYOUT_ROWS: &str = "list:layoutRows:array";
const LIST_SLOT_SLOT_OPTIONS: &str = "list:slotOptions:array";

#[cfg(test)]
const ISLAND_COMPONENT: &str = "StartIsland";

/// How many `createShow` pairs this page has. Name-addressable since compiler
/// 0.3.1, so this is a staleness tripwire rather than a mapping.
const SHOW_COUNT: usize = 23;

/// Bare-named slots the island renders and the seam deliberately never fills.
/// EMPTY, and that is the claim.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots this page compiles to. EMPTY, and it is
/// enforced by construction: `StartIsland.ts` does no string concatenation
/// inside the h() tree at all. Every composed sentence is composed in
/// `snapshot.rs` and shipped as a signal value, precisely because an anonymous
/// slot can never be injected — it renders its compile-time default and nothing
/// else (render.rs ledger #10/#20).
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

fn scalar_slots(payload: &StartPayload, flash: Option<&str>) -> serde_json::Value {
    let lines = &payload.lines;
    serde_json::json!({
        "sessionLine": payload.session.line,
        "deviceLine": lines.device_line,
        "deviceDetail": lines.device_detail,
        "boardsLine": lines.boards_line,
        "controllerLine": lines.controller_line,
        "xinputLine": lines.xinput_line,
        "blockingLine": lines.blocking_line,
        "presetLine": lines.preset_line,
        "mapperLine": lines.mapper_line,
        "readyLine": lines.ready_line,
        "playLine": lines.play_line,
        "guideLine": lines.guide_line,
        // §3's two must-says, straight off the staged view. Composed in
        // `ksx-api` beside the type that answers the question, never here.
        "escapeLine": payload.staged.escape_hatch,
        "scopeLine": payload.staged.blocking_scope,
        "stageError": lines.stage_error,
        "scanError": lines.scan_error,
        "presetsError": lines.presets_error,
        // The preset name "Add a controller" posts. SERVED, because it becomes
        // a file name — a surface composing "Player 1" would be naming a
        // first-run user's files.
        "nextPreset": payload.staged.next_preset.clone().unwrap_or_default(),
        "flashLine": flash.unwrap_or(""),
    })
}

fn board_row(board: &StartBoardRow) -> SlotValue {
    SlotValue::object(vec![
        ("name".to_owned(), SlotValue::Text(board.name.clone())),
        (
            "transport".to_owned(),
            SlotValue::Text(board.transport.clone()),
        ),
        (
            "backends".to_owned(),
            SlotValue::Text(board.backends.clone()),
        ),
        ("verdict".to_owned(), SlotValue::Text(board.verdict.clone())),
        ("caveat".to_owned(), SlotValue::Text(board.caveat.clone())),
        (
            "caveat_cls".to_owned(),
            SlotValue::Text(board.caveat_cls.clone()),
        ),
        (
            "cannot_type".to_owned(),
            SlotValue::Text(board.cannot_type.clone()),
        ),
        (
            "cannot_type_cls".to_owned(),
            SlotValue::Text(board.cannot_type_cls.clone()),
        ),
        ("path".to_owned(), SlotValue::Text(board.path.clone())),
        (
            "selector".to_owned(),
            SlotValue::Text(board.selector.clone()),
        ),
        ("alias".to_owned(), SlotValue::Text(board.alias.clone())),
        (
            "chosen_cls".to_owned(),
            SlotValue::Text(board.chosen_cls.clone()),
        ),
        ("button".to_owned(), SlotValue::Text(board.button.clone())),
    ])
}

fn other_row(board: &StartOtherRow) -> SlotValue {
    SlotValue::object(vec![
        ("name".to_owned(), SlotValue::Text(board.name.clone())),
        (
            "transport".to_owned(),
            SlotValue::Text(board.transport.clone()),
        ),
        ("reason".to_owned(), SlotValue::Text(board.reason.clone())),
        (
            "backends".to_owned(),
            SlotValue::Text(board.backends.clone()),
        ),
    ])
}

fn slot_row(slot: &StartSlotRow) -> SlotValue {
    SlotValue::object(vec![
        ("number".to_owned(), SlotValue::Text(slot.number.clone())),
        ("title".to_owned(), SlotValue::Text(slot.title.clone())),
        ("state".to_owned(), SlotValue::Text(slot.state.clone())),
        ("persona".to_owned(), SlotValue::Text(slot.persona.clone())),
        ("xinput".to_owned(), SlotValue::Text(slot.xinput.clone())),
        ("preset".to_owned(), SlotValue::Text(slot.preset.clone())),
        (
            "bindings".to_owned(),
            SlotValue::Text(slot.bindings.clone()),
        ),
    ])
}

fn option_row(option: &StartOptionRow) -> SlotValue {
    SlotValue::object(vec![
        ("value".to_owned(), SlotValue::Text(option.value.clone())),
        ("label".to_owned(), SlotValue::Text(option.label.clone())),
    ])
}

fn gap_row(gap: &StartGapRow) -> SlotValue {
    SlotValue::object(vec![
        ("label".to_owned(), SlotValue::Text(gap.label.clone())),
        ("gap".to_owned(), SlotValue::Text(gap.gap.clone())),
        ("instead".to_owned(), SlotValue::Text(gap.instead.clone())),
    ])
}

fn blocking_row(option: &StartBlockingRow) -> SlotValue {
    SlotValue::object(vec![
        ("name".to_owned(), SlotValue::Text(option.name.clone())),
        ("title".to_owned(), SlotValue::Text(option.title.clone())),
        ("detail".to_owned(), SlotValue::Text(option.detail.clone())),
        (
            "chosen_cls".to_owned(),
            SlotValue::Text(option.chosen_cls.clone()),
        ),
        ("button".to_owned(), SlotValue::Text(option.button.clone())),
    ])
}

fn text_row(row: &StartTextRow) -> SlotValue {
    SlotValue::object(vec![("text".to_owned(), SlotValue::Text(row.text.clone()))])
}

fn layout_row(layout: &StartLayoutRow) -> SlotValue {
    SlotValue::object(vec![
        ("label".to_owned(), SlotValue::Text(layout.label.clone())),
        ("panel".to_owned(), SlotValue::Text(layout.panel.clone())),
        (
            "players".to_owned(),
            SlotValue::Text(layout.players.clone()),
        ),
    ])
}

fn list_values(payload: &StartPayload) -> [(&'static str, SlotValue); 11] {
    let rows = &payload.rows;
    let layouts = || SlotValue::array(rows.layouts.iter().map(option_row).collect());
    [
        (
            LIST_SLOT_BOARDS,
            SlotValue::array(rows.boards.iter().map(board_row).collect()),
        ),
        (
            LIST_SLOT_OTHER,
            SlotValue::array(rows.other.iter().map(other_row).collect()),
        ),
        (
            LIST_SLOT_NOTES,
            SlotValue::array(rows.notes.iter().map(text_row).collect()),
        ),
        (
            LIST_SLOT_SLOTS,
            SlotValue::array(rows.slots.iter().map(slot_row).collect()),
        ),
        (
            LIST_SLOT_PERSONAS,
            SlotValue::array(rows.personas.iter().map(option_row).collect()),
        ),
        (
            LIST_SLOT_GAPS,
            SlotValue::array(rows.gaps.iter().map(gap_row).collect()),
        ),
        (
            LIST_SLOT_BLOCKING,
            SlotValue::array(rows.blocking.iter().map(blocking_row).collect()),
        ),
        (LIST_SLOT_LAYOUTS, layouts()),
        (LIST_SLOT_LAYOUTS_2, layouts()),
        (
            LIST_SLOT_LAYOUT_ROWS,
            SlotValue::array(rows.layout_details.iter().map(layout_row).collect()),
        ),
        (
            LIST_SLOT_SLOT_OPTIONS,
            SlotValue::array(rows.slot_numbers.iter().map(option_row).collect()),
        ),
    ]
}

fn show_values(payload: &StartPayload, flash: Option<&str>) -> [(&'static str, bool); SHOW_COUNT] {
    let f = &payload.flags;
    // The FLASH booleans are the one pair decided here rather than in
    // `StartFlags`, for the reason `SetupFlags` gives: a flash is one-shot
    // action feedback the client owns and clears on a timer, so it is not a
    // fact about the machine and a poll must never rewrite it.
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    [
        ("show:pillRunning", f.pill_running),
        ("show:pillIdle", f.pill_idle),
        ("show:pillDown", f.pill_down),
        ("show:stageDown", f.stage_down),
        ("show:scanDown", f.scan_down),
        ("show:presetsDown", f.presets_down),
        ("show:hasDevice", f.has_device),
        ("show:hasBoards", f.has_boards),
        ("show:noBoards", f.no_boards),
        ("show:hasOther", f.has_other),
        ("show:hasNotes", f.has_notes),
        ("show:hasSlots", f.has_slots),
        ("show:canAdd", f.can_add),
        ("show:slotsFull", f.slots_full),
        ("show:hasGaps", f.has_gaps),
        ("show:canLayout", f.can_layout),
        ("show:blockingAnswered", f.blocking_answered),
        ("show:ready", f.ready),
        ("show:notReady", f.not_ready),
        ("show:canDiscard", f.can_discard),
        ("show:sessionLive", f.session_live),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
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

fn build_slots(module: &IrModule, payload: &StartPayload, flash: Option<&str>) -> SlotData {
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

/// Render `/start` for one payload: SSR slots for the first paint, the same
/// data as the source payload for hydration.
pub(crate) fn render_start(
    page: &EmbeddedPage,
    payload: &StartPayload,
    flash: Option<&str>,
) -> PageOutput {
    let slots = build_slots(&page.module, payload, flash);
    let prefix = body_prefix(payload, "/start");
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — get started",
        route_pattern: "/start",
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
    use ksx_api::{DeviceScanView, StagedSetupView, UsbRow};

    const PANEL: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
    const AUX: &str = r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001";
    const FAN: &str = r"USB\VID_1E71&PID_300E&MI_01\7&8FBF878&0&0001";
    const SELECTOR: &str = "usb:d209:0430:00";

    /// Backend eligibility from `ksx_core::Reach`, never spelled by hand — the
    /// same rule `render_devices.rs`'s fixture follows: a fixture that wrote
    /// its own answer could not disagree with the page even when the page was
    /// wrong, and the transport rule is what these tests hold.
    fn reach(transport: ksx_api::Transport, keyboard: bool, can_type: bool) -> UsbRow {
        let reach = ksx_core::Reach {
            transport,
            keyboard,
            claimed: false,
            can_type,
        };
        let eligibility = reach.eligibility();
        UsbRow {
            transport: transport.code().to_owned(),
            interception_eligible: eligibility.interception,
            winusb_eligible: eligibility.winusb,
            backends: eligibility.line,
            can_type,
            ..UsbRow::default()
        }
    }

    fn iface(id: &str, selector: Option<&str>, keyboard: bool) -> UsbRow {
        UsbRow {
            instance_id: id.to_owned(),
            description: "USB Input Device".to_owned(),
            state: if keyboard {
                "claimable"
            } else {
                "not-a-keyboard"
            }
            .to_owned(),
            verdict: "on the Windows keyboard stack — ksx can capture this".to_owned(),
            boot_keyboard: keyboard,
            selector: selector.map(str::to_owned),
            ..reach(ksx_api::Transport::Usb, keyboard, true)
        }
    }

    /// The reference desk: one I-PAC wearing two devnodes and one fan
    /// controller with no keyboard interface. Built through
    /// `DeviceScanView::read`, deliberately — the partition, the counts, the
    /// summary lines and the served selector are that constructor's to decide.
    fn scan() -> DeviceScanView {
        DeviceScanView::read(
            "2026-08-08 12:00:00 UTC".into(),
            true,
            true,
            vec![
                ksx_api::BoardRow {
                    name: "Ultimarc I-PAC 4X".into(),
                    interfaces: vec![
                        iface(AUX, Some("usb:d209:0430:01"), false),
                        iface(PANEL, Some(SELECTOR), true),
                    ],
                    keyboard: Some(PANEL.to_owned()),
                    keyboard_verdict: "on the Windows keyboard stack — ksx can capture this".into(),
                    looks_like_a_keyboard: true,
                    ..ksx_api::BoardRow::default()
                },
                ksx_api::BoardRow {
                    name: "NZXT fan controller".into(),
                    interfaces: vec![iface(FAN, Some("usb:1e71:300e:01"), false)],
                    keyboard: None,
                    keyboard_verdict: "no keyboard interface — ksx cannot capture this board"
                        .into(),
                    looks_like_a_keyboard: false,
                    ..ksx_api::BoardRow::default()
                },
            ],
            Vec::new(),
            vec!["Interception is installed but ksx is not using it".into()],
        )
    }

    /// A staged setup driven through `StageEdit` — the same path a form post
    /// takes, so no fixture can stage something the wire could not.
    fn stage(edits: &[ksx_api::StageEdit]) -> StagedSetupView {
        let mut setup = ksx_core::stage::StagedSetup::new();
        for edit in edits {
            setup = edit.apply(&setup).expect("a legal edit");
        }
        StagedSetupView::of(&setup)
    }

    fn choose() -> ksx_api::StageEdit {
        ksx_api::StageEdit::ChooseDevice {
            selector: SELECTOR.into(),
            alias: "Ultimarc I-PAC 4X".into(),
            label: "Ultimarc I-PAC 4X".into(),
        }
    }

    /// "Add a controller" AS THE PAGE POSTS IT: with the served default
    /// layout, which is what the form's first `<option>` carries.
    fn add(persona: &str) -> ksx_api::StageEdit {
        ksx_api::StageEdit::AddSlot {
            number: None,
            persona: persona.into(),
            preset: "Player 1".into(),
            layout: Some("arcade-6button".into()),
        }
    }

    /// The same click with the layout menu set to the blank one — a
    /// controller that binds nothing, which is what EVERY controller used to
    /// be.
    fn add_blank(persona: &str) -> ksx_api::StageEdit {
        ksx_api::StageEdit::AddSlot {
            number: None,
            persona: persona.into(),
            preset: "Player 1".into(),
            layout: None,
        }
    }

    /// §3's answer, so a fixture can reach the state where Save and Play are
    /// offered at all.
    fn answer() -> ksx_api::StageEdit {
        ksx_api::StageEdit::SetBlocking {
            blocking: "bound-keys".into(),
        }
    }

    fn payload(staged: StagedSetupView) -> StartPayload {
        StartPayload {
            staged,
            scan: scan(),
            session: SessionView {
                reachable: true,
                running: false,
                line: "idle — daemon reachable".into(),
                profile: None,
            },
            ..StartPayload::default()
        }
        .composed()
    }

    /// Nothing staged, everything readable — the very first paint.
    fn fresh() -> StartPayload {
        payload(stage(&[]))
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load("/start").expect("embedded page must load");
        assert_eq!(page.module.header.version, 2);
    }

    /// The gate every page must call. Pins the scalar names, the exact list
    /// slot names, the exact `show:` name set, the island table — and then the
    /// contract a name-exists check cannot state: injected == rendered, both
    /// ways. See `render.rs::assert_island_slot_contract`.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load("/start").unwrap();
        let module = &page.module;
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&StartPayload::default(), None);
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
        let mut expected: Vec<&str> = list_values(&StartPayload::default())
            .iter()
            .map(|(n, _)| *n)
            .collect();
        expected.sort_unstable();
        let mut found = array_slots.clone();
        found.sort_unstable();
        assert_eq!(
            found, expected,
            "list slot names drifted between StartIsland.ts and the LIST_SLOT_* \
             constants; slots: {names:?}"
        );

        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> =
            show_values(&StartPayload::default(), None)
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between StartIsland.ts and show_values()"
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
                list_values(&StartPayload::default())
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

    /// **Every list ITEM field the seam fills is bound, and every one the
    /// island binds is filled — both ways.**
    ///
    /// `assert_island_slot_contract` cannot state this: it checks scalars,
    /// `list:*:array` names and `show:*` names, and stops. So a row field could
    /// be computed on both sides and read by neither, or bound by the island
    /// and never filled by the seam (which then renders the authored default
    /// forever, server-side). `/devices` shipped the first of those for the
    /// whole life of the page.
    #[test]
    fn every_row_field_is_bound_and_every_bound_row_field_is_filled() {
        let page = EmbeddedPage::load("/start").unwrap();
        let module = &page.module;
        let ir_names: std::collections::BTreeSet<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        // A payload that populates EVERY list, or this proves nothing.
        let mut p = payload(stage(&[
            choose(),
            add("xbox360"),
            ksx_api::StageEdit::SetBlocking {
                blocking: "whole".into(),
            },
        ]));
        assert!(
            !p.rows.gaps.is_empty(),
            "the roster carries un-pluggable personas"
        );
        p.flash = Some("saved".into());

        for (list_slot, value) in list_values(&p) {
            let signal = list_slot
                .strip_prefix("list:")
                .and_then(|s| s.strip_suffix(":array"))
                .expect("list slot names are list:<signal>:array");

            let SlotValue::Array(rows) = &value else {
                panic!("{list_slot} is not an array");
            };
            let first = rows.first().unwrap_or_else(|| {
                panic!("the fixture must populate {signal}, or this proves nothing")
            });
            let SlotValue::Object(fields) = first else {
                panic!("{signal} rows are not objects");
            };

            let filled: std::collections::BTreeSet<String> =
                fields.iter().map(|(k, _)| k.clone()).collect();
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

    /// **Moment 4, as HTML.** The board is named the way a human names it, the
    /// path is present but not the identifier, and the form posts the SERVED
    /// selector.
    ///
    /// FAILS against a page that posted `board.keyboard` — the instance path —
    /// which is what a picker naturally reaches for and what `FIRST-RUN.md` §5
    /// bans from being the id. It also fails against one that showed the path
    /// as the row's title.
    #[test]
    fn a_board_is_named_like_a_human_names_it_and_picked_by_selector() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(&page, &fresh(), None);

        assert!(out.html.contains("Ultimarc I-PAC 4X"), "{}", out.html);
        // The name leads the row; the path is inside a disclosure below it.
        let name_at = out.html.find("Ultimarc I-PAC 4X").unwrap();
        let path_at = out
            .html
            .find("USB\\VID_D209&amp;PID_0430&amp;MI_00")
            .expect("the path is on the page as small print");
        assert!(
            name_at < path_at,
            "the device path precedes the human name, so it is reading as the identifier"
        );
        assert!(
            out.html.contains("Windows device path (for support)"),
            "the path must be labelled as support small print: {}",
            out.html
        );

        // What the form posts is the SELECTOR, and there is no text input
        // anywhere for anybody to type an id into (§6).
        assert!(
            out.html
                .contains(r#"name="selector" value="usb:d209:0430:00""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"action="/start/device""#),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains(r#"type="text""#),
            "a first-run flow that asks anybody to type is a first-run flow that fails: {}",
            out.html
        );
        assert_complete_head("/start", &out.html);
    }

    /// **What each device can DO is on the row, both transports.**
    ///
    /// A Bluetooth keyboard can be split but never WinUSB-claimed, and a board
    /// with no keyboard interface cannot be picked at all. Neither is guessable
    /// from a name, and both are `DeviceScanView::read`'s served sentences —
    /// this asserts the seam carries them rather than composing its own.
    #[test]
    fn every_row_says_what_can_reach_it_and_the_unpickable_say_why_not() {
        let page = EmbeddedPage::load("/start").unwrap();
        let bt = ksx_api::BoardRow {
            name: "Bluetooth Keyboard".into(),
            interfaces: vec![UsbRow {
                instance_id: r"BTHENUM\X".into(),
                selector: Some("bt:0011223344".into()),
                boot_keyboard: true,
                ..reach(ksx_api::Transport::Bluetooth, true, true)
            }],
            keyboard: Some(r"BTHENUM\X".into()),
            keyboard_verdict: "a Bluetooth keyboard on the Windows input stack".into(),
            looks_like_a_keyboard: true,
            ..ksx_api::BoardRow::default()
        };
        let mut boards = scan().boards;
        boards.push(bt);
        let mut p = fresh();
        p.scan = DeviceScanView::read(
            "t".into(),
            true,
            true,
            boards,
            Vec::new(),
            p.scan.notes.clone(),
        );
        let out = render_start(&page, &p.composed(), None);

        assert!(out.html.contains(">USB<"), "{}", out.html);
        assert!(out.html.contains(">Bluetooth<"), "{}", out.html);
        assert!(out.html.contains("winusb: never"), "{}", out.html);
        assert!(
            out.html.contains("no USB interface to bind"),
            "the transport FACT, not a vague refusal: {}",
            out.html
        );
        assert!(out.html.contains("interception: yes, now"), "{}", out.html);
        // The board that cannot be picked is listed, with the reason — not
        // hidden, because "why is my device not here" needs an answer.
        assert!(out.html.contains("NZXT fan controller"), "{}", out.html);
        assert!(out.html.contains("no keyboard interface"), "{}", out.html);
        // ...and it carries no pick form of its own.
        let fan_at = out.html.find("NZXT fan controller").unwrap();
        assert!(
            !out.html[fan_at..].starts_with("NZXT fan controller</span><form"),
            "a pick form on a board ksx cannot capture is an offer that always refuses"
        );
    }

    /// **Moment 5's promise, on the page: nothing has happened yet.**
    ///
    /// The screen has to make that legible rather than asking to be trusted, so
    /// the staged controller renders WITH the three facts that are true of it —
    /// no pad, no file, no trace — and the Save/Play pair is the only place
    /// either word appears as an action.
    ///
    /// FAILS against a page that rendered a staged slot the way `/pads` renders
    /// a live one: identical rows for "this pad exists" and "this pad is an
    /// intention" is exactly the confusion staging was built to remove.
    #[test]
    fn a_staged_controller_says_it_does_not_exist_yet() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(&page, &payload(stage(&[choose(), add("ps4")])), None);

        assert!(out.html.contains("Player 1"), "{}", out.html);
        assert!(out.html.contains("PlayStation"), "{}", out.html);
        // BOTH halves of moment 5. §1 says the controller appears READY; §2
        // says nothing has been plugged, claimed or written. A row carrying
        // only the second reads as half-finished work rather than a decision
        // that has been made — which is the state a first-run user is trying
        // to get OUT of.
        assert!(
            out.html
                .contains("ready — it will exist the moment you press Play"),
            "{}",
            out.html
        );
        assert!(out.html.contains("no pad is on the bus"), "{}", out.html);
        assert!(out.html.contains("Remove leaves no trace"), "{}", out.html);
        // It arrived with a LAYOUT, so the row says what it binds rather than
        // promising a controller that does nothing.
        assert!(out.html.contains("controls bound"), "{}", out.html);
        assert!(
            out.html.contains(r#"action="/start/controller/remove""#),
            "{}",
            out.html
        );
    }

    /// **A controller that binds nothing says so, on its own row, and does not
    /// call itself ready.**
    ///
    /// FAILS against the shipped row, which said "ready — it will exist the
    /// moment you press Play" for every staged slot including the ones with an
    /// empty preset. That is `FIRST-RUN.md` §6's "a screen reports success
    /// while nothing works", one line long — and it was the ordinary case,
    /// because `AddSlot` staged an empty preset for every controller.
    #[test]
    fn a_controller_with_no_bindings_is_not_called_ready() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(
            &page,
            &payload(stage(&[choose(), add_blank("xbox360"), answer()])),
            None,
        );

        assert!(
            out.html.contains("not ready — nothing is bound to it"),
            "{}",
            out.html
        );
        assert!(
            out.html
                .contains("this pad would plug and do nothing, so Play refuses it by name"),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains("ready — it will exist the moment"),
            "a dead pad must not wear the ready sentence: {}",
            out.html
        );
        // ...and neither button is offered, with ksx-core's own reason on the
        // page instead.
        assert!(
            !out.html.contains(r#"action="/start/play""#),
            "Play was offered for a pad that binds nothing: {}",
            out.html
        );
        assert!(
            !out.html.contains(r#"action="/start/save""#),
            "Save was offered for a pad that binds nothing: {}",
            out.html
        );
        assert!(
            out.html.contains("would plug a pad that does"),
            "{}",
            out.html
        );
    }

    /// **Moment 6 happens HERE: the layouts are served, offered, and land in
    /// the stage.**
    ///
    /// FAILS against the shipped page, whose step 3 was a link to the mapper
    /// and a paragraph admitting the consequence — "Mapping happens in the
    /// mapper, and the mapper edits preset FILES. Save first". That sent a
    /// first-run user out of a flow that had written nothing, told them to
    /// write something they had not decided on, and then played the staged,
    /// still-empty preset when they came back.
    ///
    /// It also fails against a page that spelled the layout list itself: every
    /// id, every panel note and the recommended default are
    /// `ksx_core::templates`', served through `StagedSetupView::layouts`.
    #[test]
    fn the_layouts_are_served_offered_and_land_in_the_stage() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(&page, &payload(stage(&[choose(), add("xbox360")])), None);

        // The menu on "Add a controller" AND the one that re-dresses a staged
        // slot — both from the served roster.
        assert!(
            out.html.contains(r#"action="/start/controller/layout""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"value="arcade-6button""#),
            "{}",
            out.html
        );
        assert!(out.html.contains(r#"value="keyboard-2p""#), "{}", out.html);
        // The RECOMMENDED default is the first option, because that is what a
        // select shows: a user who never opens the menu still gets a pad that
        // does something.
        let menu = out
            .html
            .find(r#"id="layout""#)
            .expect("the layout menu on Add a controller");
        let first = out.html[menu..]
            .find(r#"<option value=""#)
            .map(|at| &out.html[menu + at..menu + at + 40])
            .unwrap_or_default();
        assert!(
            first.contains("arcade-6button"),
            "the first layout option is not the served default: {first}"
        );
        // The panel note is ksx-core's, verbatim, so somebody can tell the
        // layouts apart.
        assert!(
            out.html.contains("factory chart"),
            "the panel note must be on the page: {}",
            out.html
        );
        // The one that binds nothing is offered WITH what it costs.
        assert!(out.html.contains("Binds nothing at all"), "{}", out.html);
        assert!(
            out.html.contains("Play will refuse it by name"),
            "{}",
            out.html
        );
        // ...and the mapper is still described as what it is — a FILE editor —
        // rather than as step 3 of a flow that has saved nothing.
        assert!(
            out.html
                .contains("no file is written and no mapper is opened"),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains("Save first: that writes one preset"),
            "the page must no longer send a first-run user to the mapper to map: {}",
            out.html
        );
        // The SEAM is stated where the link is. Mapping a button in the mapper
        // does not change what Play does here, and a person cannot guess that
        // — they would find it out by playing a pad that ignores the button
        // they just mapped.
        assert!(
            out.html
                .contains("Play on this page always starts exactly what is shown above"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("Open the mapper (edits saved files)"),
            "the link must say what it opens: {}",
            out.html
        );
    }

    /// **§3's one question, asked once and NOT pre-answered.**
    ///
    /// Both answers are offered in their own words, neither is marked as
    /// chosen until it is, and the two must-says are on the screen.
    ///
    /// FAILS against a page that rendered Freeze pre-selected — the obvious
    /// implementation, since `Blocking::default()` IS `Whole` — which answers
    /// the one question in this flow for the user, and against one that dropped
    /// the escape hatch, which is the only thing standing between a frozen
    /// keyboard and a reboot.
    #[test]
    fn the_one_question_is_asked_and_never_pre_answered() {
        let page = EmbeddedPage::load("/start").unwrap();
        let unasked = render_start(&page, &payload(stage(&[choose(), add("xbox360")])), None);

        assert!(
            unasked.html.contains("Freeze this keyboard"),
            "{}",
            unasked.html
        );
        assert!(
            unasked.html.contains("Split this keyboard"),
            "{}",
            unasked.html
        );
        assert!(unasked.html.contains("Not asked yet"), "{}", unasked.html);
        assert!(
            !blocking_card(&unasked.html).contains("pill-ok"),
            "an unanswered question must not mark an option as the answer: {}",
            unasked.html
        );
        // Both must-says, verbatim from ksx-api.
        assert!(
            unasked.html.contains("LeftCtrl five times"),
            "{}",
            unasked.html
        );
        assert!(
            unasked.html.contains("both modes"),
            "the hatch works under Freeze AND Split: {}",
            unasked.html
        );
        assert!(
            unasked.html.contains("this session only"),
            "freezing is not permanent and not global: {}",
            unasked.html
        );

        let answered = render_start(
            &page,
            &payload(stage(&[
                choose(),
                add("xbox360"),
                ksx_api::StageEdit::SetBlocking {
                    blocking: "bound-keys".into(),
                },
            ])),
            None,
        );
        assert!(
            answered.html.contains("Answered: Split this keyboard."),
            "{}",
            answered.html
        );
        assert!(
            blocking_card(&answered.html).contains("pill-ok"),
            "{}",
            answered.html
        );
    }

    /// Just the split-or-freeze card. The "chosen" pill class appears on the
    /// board rows too, so a whole-page `contains` would pass against a
    /// pre-selected answer — which is the version this test exists to fail.
    fn blocking_card(html: &str) -> &str {
        let start = html
            .find("Freeze this keyboard, or split it?")
            .expect("the question's heading");
        let end = html[start..]
            .find("4 &middot; Play")
            .or_else(|| html[start..].find("4 · Play"))
            .map(|at| start + at)
            .unwrap_or(html.len());
        &html[start..end]
    }

    /// **Moment 7: two acts, two buttons, and neither implies the other.**
    ///
    /// FAILS against a page with one "Save and play" button, which is the
    /// shape `FIRST-RUN.md` §2 rules out in a sentence — and against one that
    /// offered them while the setup cannot be committed, which would put
    /// ksx-core's refusal after the click instead of before it.
    #[test]
    fn saving_and_playing_are_two_buttons_and_appear_only_when_they_would_work() {
        let page = EmbeddedPage::load("/start").unwrap();

        let ready = render_start(
            &page,
            &payload(stage(&[choose(), add("xbox360"), answer()])),
            None,
        );
        assert!(
            ready.html.contains(r#"action="/start/save""#),
            "{}",
            ready.html
        );
        assert!(
            ready.html.contains(r#"action="/start/play""#),
            "{}",
            ready.html
        );
        assert!(
            ready.html.contains("Either works without the other"),
            "{}",
            ready.html
        );
        // WHAT PLAY DOES, before the button rather than after it: a pad
        // appears and the keyboard changes behaviour, and both are reversible.
        // Under Freeze the second one means the keyboard stops typing, which is
        // not a thing to discover by pressing a button.
        assert!(
            ready.html.contains("becomes a controller"),
            "{}",
            ready.html
        );
        assert!(
            ready.html.contains("gives the keyboard back"),
            "the way out has to be beside the way in: {}",
            ready.html
        );
        // The Guide fact — moment 7's one thing about the pad itself.
        assert!(ready.html.contains("Game Bar"), "{}", ready.html);

        // A keyboard with no controller drives nothing, and ksx-core says so.
        // The page must show THAT sentence rather than an enabled button.
        let half = render_start(&page, &payload(stage(&[choose()])), None);
        assert!(
            !half.html.contains(r#"action="/start/play""#),
            "Play was offered for a setup commit() refuses: {}",
            half.html
        );
        assert!(half.html.contains("controller"), "{}", half.html);
        assert!(half.html.contains("disabled"), "{}", half.html);
    }

    /// **§3 unanswered means Save and Play are not offered — and the page says
    /// which question is missing.**
    ///
    /// FAILS against the shipped page. `ready` was `commit().is_ok()` and
    /// `commit()` resolved an unanswered question through
    /// `effective_blocking()`, so both buttons were live with the one question
    /// in this flow still open — and pressing Save wrote
    /// `block_keyboards = "whole"` from an answer nobody gave. On a returning
    /// user's machine that overwrote the answer they had chosen last time.
    ///
    /// The other half is what must NOT happen instead: the fix is a refusal,
    /// never a pre-selected option. This asserts both — no answer is marked,
    /// and neither button is there.
    #[test]
    fn an_unanswered_question_disables_both_buttons_without_answering_it() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(&page, &payload(stage(&[choose(), add("xbox360")])), None);

        assert!(
            !out.html.contains(r#"action="/start/save""#),
            "Save was offered with §3 unanswered — it would write Freeze: {}",
            out.html
        );
        assert!(
            !out.html.contains(r#"action="/start/play""#),
            "Play was offered with §3 unanswered: {}",
            out.html
        );
        assert!(out.html.contains("disabled"), "{}", out.html);
        // The reason names the question, so the disabled button is not a
        // mystery.
        assert!(out.html.contains("split-or-freeze"), "{}", out.html);
        // ...and the question is STILL not answered on the user's behalf.
        assert!(out.html.contains("Not asked yet"), "{}", out.html);
        assert!(
            !blocking_card(&out.html).contains("pill-ok"),
            "the fix must not be a pre-selected answer: {}",
            out.html
        );

        // Answering it — with SPLIT, the non-default — is what opens both.
        let answered = render_start(
            &page,
            &payload(stage(&[choose(), add("xbox360"), answer()])),
            None,
        );
        assert!(
            answered.html.contains(r#"action="/start/save""#),
            "{}",
            answered.html
        );
        assert!(
            answered.html.contains(r#"action="/start/play""#),
            "{}",
            answered.html
        );
    }

    /// **The three ways this page can be blind, and none of them draws an
    /// empty machine.**
    ///
    /// `SURFACES.md` §1b, and this page has THREE reads that can refuse
    /// independently: the daemon (no staged setup), the enumeration (no
    /// boards) and the presets folder. Each says what did not happen; only the
    /// enumeration that ANSWERED may say the machine is empty.
    ///
    /// FAILS against the obvious implementation, where an unreachable daemon
    /// degrades to `StagedSetupView::default()` — `reachable: false` with
    /// `empty: true` renders as "you have staged nothing", which is a confident
    /// wrong sentence about a read that never happened.
    #[test]
    fn a_read_that_failed_never_renders_as_an_absence() {
        let page = EmbeddedPage::load("/start").unwrap();

        // (1) NO DAEMON. The board list is still a real reading of the machine,
        // so it stays — what is refused is the staging.
        let mut p = fresh();
        p.staged = StagedSetupView::unreachable("no daemon answered the control pipe");
        let down = render_start(&page, &p.composed(), None);
        assert!(down.html.contains("No staged setup"), "{}", down.html);
        assert!(
            down.html.contains("no daemon answered the control pipe"),
            "{}",
            down.html
        );
        assert!(
            down.html.contains("Ultimarc I-PAC 4X"),
            "the enumeration answered, so its result must survive a dead pipe: {}",
            down.html
        );
        assert!(
            !down
                .html
                .contains("Pick the keyboard you want to play with"),
            "a page that cannot stage anything must not invite a pick: {}",
            down.html
        );

        // (2) THE SCAN REFUSED. `DeviceScanView::default()` is the unreadable
        // view, and its flags forbid every absence sentence.
        let refused = render_start(
            &page,
            &StartPayload {
                scan: DeviceScanView::default(),
                unavailable: "listing devices is not available on this surface".into(),
                ..fresh()
            }
            .composed(),
            None,
        );
        assert!(
            refused.html.contains("could not be read"),
            "{}",
            refused.html
        );
        assert!(
            !refused
                .html
                .contains("No board on this PC exposes a keyboard interface"),
            "a refused read asserted an absence: {}",
            refused.html
        );

        // (3) THE MACHINE REALLY IS EMPTY — the enumeration answered.
        let empty = render_start(
            &page,
            &StartPayload {
                scan: DeviceScanView::read("t".into(), true, true, Vec::new(), Vec::new(), vec![]),
                ..fresh()
            }
            .composed(),
            None,
        );
        assert!(
            empty
                .html
                .contains("No board on this PC exposes a keyboard interface"),
            "{}",
            empty.html
        );
        assert!(
            !empty.html.contains("could not be read"),
            "a machine that WAS read must not claim it could not be: {}",
            empty.html
        );

        // (4) THE PRESETS FOLDER REFUSED. The mapper step must not then claim
        // that saving would create the names — it does not know.
        let blind = render_start(
            &page,
            &StartPayload {
                presets_error: "the presets folder could not be read".into(),
                ..payload(stage(&[choose(), add("xbox360")]))
            }
            .composed(),
            None,
        );
        assert!(
            blind.html.contains("the presets folder could not be read"),
            "{}",
            blind.html
        );
        assert!(
            !blind.html.contains("so saving creates them"),
            "a failed preset read asserted what the folder holds: {}",
            blind.html
        );

        // ...and the four pages are four different pages.
        assert_ne!(down.html, refused.html);
        assert_ne!(refused.html, empty.html);
        assert_ne!(empty.html, blind.html);
    }

    /// **Saving over an existing preset is said BEFORE the click.**
    ///
    /// `ksx-app`'s `stage::apply` keeps a timestamped copy, and its flash says
    /// so afterwards. This is the half that arrives in time to change the
    /// decision.
    #[test]
    fn a_preset_name_already_on_disk_is_a_warning_not_a_surprise() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(
            &page,
            &StartPayload {
                presets: vec![ksx_api::PresetRow {
                    name: "Player 1".into(),
                    bound: 7,
                    ..ksx_api::PresetRow::default()
                }],
                ..payload(stage(&[choose(), add("xbox360")]))
            }
            .composed(),
            None,
        );
        assert!(out.html.contains("already exists on disk"), "{}", out.html);
        assert!(out.html.contains("REPLACES it"), "{}", out.html);
        assert!(out.html.contains("Restore backup"), "{}", out.html);
    }

    /// **Nothing on this page claims, plugs or installs, and no GET writes.**
    ///
    /// `FIRST-RUN.md` §5 and §6, and `SURFACES.md` §3, which marks WinUSB claim
    /// "never" for the browser. The rescan is a LINK, which is the point: a
    /// user must be able to re-read the machine without wondering what else the
    /// click did.
    #[test]
    fn nothing_here_claims_plugs_or_installs() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(&page, &payload(stage(&[choose(), add("xbox360")])), None);

        for forbidden in [
            r#"action="/winusb/claim""#,
            r#"action="/start/claim""#,
            r#"action="/devices/pick""#,
            r#"action="/pads/spawn""#,
            r#"action="/install-drivers""#,
        ] {
            assert!(
                !out.html.contains(forbidden),
                "{forbidden} is a write this flow must never perform as a side effect: {}",
                out.html
            );
        }
        assert!(
            out.html.contains(r#"class="btn btn-ghost" href="/start""#),
            "the rescan must be a plain link back to this page: {}",
            out.html
        );
        // The footer states the contract the whole page rests on.
        assert!(
            out.html
                .contains("installs a driver or writes a file until you press Save"),
            "{}",
            out.html
        );
    }

    /// A hostile flash is a query-string value and is attacker-writable. It
    /// must arrive escaped, on this page exactly as on the others.
    #[test]
    fn the_flash_is_escaped() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(
            &page,
            &fresh(),
            Some("error: <script>alert(1)</script> & \"quotes\""),
        );
        assert!(!out.html.contains("<script>alert(1)"), "{}", out.html);
        assert!(out.html.contains("&lt;script&gt;"), "{}", out.html);
    }

    /// One struct, one serializer: the block the page embeds is the shape
    /// `GET /api/start` serves, so the seed and every poll agree.
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let page = EmbeddedPage::load("/start").unwrap();
        let p = payload(stage(&[choose(), add("xbox360")]));
        let out = render_start(&page, &p, None);
        let api = serde_json::to_value(&p).unwrap();
        let embedded = crate::render::payload_json(&p).replace("\\u003c", "<");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&embedded).unwrap(),
            api
        );
        assert!(out.html.contains(r#"id="__ksx-payload""#), "{}", out.html);
    }

    /// The nav is static markup duplicated per island, so a page is invisible
    /// until every sibling links to it — and this is the page a first-run user
    /// has to find first.
    #[test]
    fn the_nav_reaches_every_sibling_page() {
        let page = EmbeddedPage::load("/start").unwrap();
        let out = render_start(&page, &fresh(), None);
        for href in [
            "/",
            "/map",
            "/check",
            "/pads",
            "/devices",
            "/profiles",
            "/setup",
        ] {
            assert!(
                out.html.contains(&format!(r#"href="{href}""#)),
                "the nav does not reach {href}: {}",
                out.html
            );
        }
        assert!(
            out.html.contains(r#"aria-current="page""#),
            "the current route must be marked: {}",
            out.html
        );
    }

    /// **Every ceiling and every roster on this page came from the backend.**
    ///
    /// The specific bug `CLAUDE.md`'s one rule exists for is a number typed
    /// into TypeScript. This asserts the two ceilings on screen are the SERVED
    /// ones by moving them: a page holding its own copy renders 16 and 4
    /// whatever the backend says.
    #[test]
    fn the_ceilings_on_screen_are_the_served_ones() {
        let page = EmbeddedPage::load("/start").unwrap();
        let mut p = payload(stage(&[choose()]));
        p.staged.max_slots = 3;
        p.staged.max_xinput_slots = 2;
        let out = render_start(&page, &p.composed(), None);
        assert!(out.html.contains("Up to 3 controllers"), "{}", out.html);

        let mut p = payload(stage(&[choose(), add("xbox360")]));
        p.staged.max_xinput_slots = 2;
        let out = render_start(&page, &p.composed(), None);
        assert!(
            out.html.contains("1 of Windows' 2 XInput slots"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("players 3+ exist"),
            "the sentence past the ceiling counts from the SERVED ceiling: {}",
            out.html
        );
    }
}
