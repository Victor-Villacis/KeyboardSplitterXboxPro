//! The /pads render seam: embedded FMIR + one [`PadsPayload`] → HTML.
//!
//! Same four-part slot seam as [`crate::render`] and [`crate::render_map`] —
//! scalars, lists, shows, and the layout test that pins all three — so read
//! `render.rs`'s module docs for the protocol. What is worth writing down here
//! is what this seam deliberately does NOT do.
//!
//! **Nothing on this page is decided here, and nothing on it is WORDED here.**
//! The pad summary, the devnode line, the owner line, the elevation warning,
//! the confirm panel's lead, the XInput ceiling paragraph, the prune plan's
//! prose and its `kind`, and the label on every single `<option>` all arrive
//! composed from `ksx_api::MachineSource::pads_view`. This file turns them
//! into slot values and picks which statically-styled variant renders.
//!
//! Four of those sentences used to be composed here AND again in
//! `studio-ui/src/PadsIsland.ts`, in two languages, with only the Rust half
//! tested: the server painted one wording and the 2 s poller overwrote it
//! with the other. They were not neutral formatting either — one asserts
//! "this prune WILL be refused" and another "Every pad listed here goes, at
//! once". They live in `crate::pads::surface` now and arrive as
//! [`ksx_api::PadsView`] fields, which is what docs/SURFACES.md §1 asks for.
//!
//! The reason that boundary is drawn hard here rather than loosely: the number
//! four is not a fact about a web page. `ksx pads --count 8 --persona xbox360`
//! plugs eight pads and Windows hands four of them to nobody, and the page's
//! whole job is to say so BEFORE the button is pressed. A page
//! that worked that out for itself would be a second answer to a question the
//! CLI already answers, and docs/SURFACES.md §1 names that exact failure.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{
    art_for, body_prefix, body_prefix_no_refresh, with_icon_links, EmbeddedPage, PERSONALITY_CSS,
};
use crate::snapshot::PadsPayload;

/// List array slot names, BINDING-derived (compiler 0.2.0+), in slot-table
/// (== document) order. Rename a list signal in PadsIsland.ts and the layout
/// test fails until these match again.
const LIST_SLOT_PADS: &str = "list:padTiles:array";
const LIST_SLOT_GHOST_PADS: &str = "list:ghostTiles:array";
const LIST_SLOT_COUNTS: &str = "list:countOptions:array";
const LIST_SLOT_PERSONAS: &str = "list:personaOptions:array";
const LIST_SLOT_HOLDS: &str = "list:holdOptions:array";
const LIST_SLOT_PRUNE_ROWS: &str = "list:pruneRows:array";

/// The island table this page compiles to: exactly one island — the whole
/// screen. Its name is the `activateIslands` registry key in
/// `studio-ui/src/pads.ts`.
#[cfg(test)]
const ISLAND_COMPONENT: &str = "PadsIsland";

/// How many `createShow` pairs this page has; the layout test pins both the
/// count and every name.
const SHOW_COUNT: usize = 14;

/// Bare-named slots this page renders and the seam deliberately never fills.
/// EMPTY, and that is the claim: every signal `PadsIsland.ts` binds to the DOM
/// gets a server value on every request.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots this page compiles to. EMPTY — every
/// attribute value and every text child is either a named signal binding, a
/// list item's member read, or static markup.
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

/// The minimum number of pad tiles the grid shows — mirrors
/// `render.rs::PAD_TILE_FLOOR` and `PadsIsland.ts`'s copy, so an idle 4-slot
/// cabinet still LOOKS like a 4-slot cabinet on both pages.
const PAD_TILE_FLOOR: usize = 4;

/// Scalar slot values, keyed by the signal names in PadsIsland.ts.
fn scalar_slots(payload: &PadsPayload) -> serde_json::Value {
    let view = &payload.pads;
    serde_json::json!({
        "generatedAt": view.generated_at,
        "padsSummary": view.summary,
        "busLine": view.bus_line,
        "ownersLine": view.owners_line,
        "xinputLine": view.xinput_line,
        "sessionLine": payload.session.line,
        "pruneDetail": view.prune.detail,
        "pruneCommand": view.prune.command.clone().unwrap_or_default(),
        "elevationLine": view.elevation_line,
        "spawnNote": view.spawn.note,
        "spawnRefusal": view.spawn.refused.clone().unwrap_or_default(),
        "confirmLine": view.confirm_line,
        // The banner's h2. The PROVIDER's sentence (`PadsView::unreadable`),
        // never this file's or the island's — a heading is not exempt from
        // "this seam words nothing".
        "unreadableHeading": view.unreadable_heading,
        "unavailableLine": payload.unavailable.clone().unwrap_or_default(),
        "flashLine": payload.flash.clone().unwrap_or_default(),
    })
}

/// One `<option>` array — value on the wire, label for the human. The labels
/// are the provider's, verbatim: "8 pads — only 4 readable, 4 invisible to
/// games" is the whole warning, and re-wording it here would be re-deciding
/// it.
fn options(rows: &[ksx_api::SpawnOption]) -> SlotValue {
    SlotValue::array(
        rows.iter()
            .map(|o| {
                SlotValue::object(vec![
                    ("value".to_owned(), SlotValue::Text(o.value.clone())),
                    ("label".to_owned(), SlotValue::Text(o.label.clone())),
                ])
            })
            .collect(),
    )
}

/// The list array payloads, keyed by their (unique) slot names.
fn list_values(payload: &PadsPayload) -> [(&'static str, SlotValue); 6] {
    let view = &payload.pads;
    let pads = SlotValue::array(
        view.pads
            .iter()
            .enumerate()
            .map(|(i, p)| {
                SlotValue::object(vec![
                    ("player".to_owned(), SlotValue::Text(format!("P{}", i + 1))),
                    ("persona".to_owned(), SlotValue::Text(p.persona.clone())),
                    (
                        "instance".to_owned(),
                        SlotValue::Text(p.instance_id.clone()),
                    ),
                    (
                        "art".to_owned(),
                        SlotValue::Text(art_for(&p.persona).to_owned()),
                    ),
                ])
            })
            .collect(),
    );
    // Ghost tiles are the "this cabinet has four slots and they are empty"
    // picture, and they are a CLAIM: four "empty" tiles under a failed read
    // say the bus is clean, which is the one thing this page must never say
    // by accident. A read that failed shows no tiles and the banner instead.
    let ghost_slots = if payload.unavailable.is_none() {
        view.pads.len()..PAD_TILE_FLOOR
    } else {
        0..0
    };
    let ghosts = SlotValue::array(
        ghost_slots
            .map(|i| {
                SlotValue::object(vec![(
                    "slot".to_owned(),
                    SlotValue::Text(format!("P{}", i + 1)),
                )])
            })
            .collect(),
    );
    // The confirm panel lists exactly what the restart takes, which is every
    // pad on the bus — a devnode restart has no per-pad granularity, and a
    // panel that showed a subset would be describing an operation ksx cannot
    // perform.
    let prune_rows = SlotValue::array(
        view.pads
            .iter()
            .map(|p| {
                SlotValue::object(vec![
                    (
                        "instance".to_owned(),
                        SlotValue::Text(p.instance_id.clone()),
                    ),
                    ("persona".to_owned(), SlotValue::Text(p.persona.clone())),
                ])
            })
            .collect(),
    );
    [
        (LIST_SLOT_PADS, pads),
        (LIST_SLOT_GHOST_PADS, ghosts),
        (LIST_SLOT_COUNTS, options(&view.spawn.counts)),
        (LIST_SLOT_PERSONAS, options(&view.spawn.personas)),
        (LIST_SLOT_HOLDS, options(&view.spawn.holds)),
        (LIST_SLOT_PRUNE_ROWS, prune_rows),
    ]
}

/// Every show slot on this page, BY NAME, with the boolean the server wants.
///
/// The shows are all SIBLINGS — no `createShow` on this page is nested inside
/// another's branch — so every combined condition is computed once, here and
/// in `PadsIsland.ts`'s `applyPads`, instead of relying on a parent branch
/// having rendered. `pruneArm` and `pruneArmed` are the pair that matters:
/// exactly one of them can be true, and neither can be true unless the plan is
/// actually a restart, so the destructive button cannot render beside a plan
/// that would refuse it.
fn show_values(payload: &PadsPayload) -> [(&'static str, bool); SHOW_COUNT] {
    let view = &payload.pads;
    let session = &payload.session;
    let flash_err = payload
        .flash
        .as_deref()
        .is_some_and(|f| f.starts_with("error"));
    let readable = payload.unavailable.is_none();
    let restart = view.prune.kind == "restart";
    let can_spawn = view.spawn.refused.is_none();
    [
        ("show:pillRunning", session.reachable && session.running),
        ("show:pillIdle", session.reachable && !session.running),
        ("show:pillDown", !session.reachable),
        ("show:unavailable", payload.unavailable.is_some()),
        ("show:flashOk", payload.flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        ("show:canSpawn", can_spawn),
        ("show:spawnBlocked", !can_spawn),
        // The ceiling card's standing paragraph ("ksx will plug whatever you
        // ask for … Windows will still only ever hand four of them to a
        // game") is a CLAIM about this machine's spawn menu, so it is gated
        // on the read having happened: under a failed read it would sit two
        // lines below an xinput_line saying ksx cannot count the slots,
        // flatly contradicting it.
        ("show:busRead", readable),
        // "Nothing for this panel to do right now" is an assertion of
        // absence, so it is gated on the read having HAPPENED. A refused read
        // gets the banner; it does not get told its bus is clean.
        ("show:pruneIdle", !restart && readable),
        ("show:pruneArm", restart && !payload.confirm),
        ("show:pruneArmed", restart && payload.confirm),
        ("show:notElevated", restart && view.elevated == Some(false)),
        ("show:hasCommand", restart && view.prune.command.is_some()),
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

/// Populate every server-injected slot.
fn build_slots(module: &IrModule, payload: &PadsPayload) -> SlotData {
    let scalars = scalar_slots(payload).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));
    for (name, value) in list_values(payload) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    for (name, value) in show_values(payload) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, SlotValue::Bool(value));
        }
    }
    slots
}

/// Render /pads for one payload: SSR slots for first paint, the same data as
/// island props for hydration.
pub(crate) fn render_pads(page: &EmbeddedPage, payload: &PadsPayload) -> PageOutput {
    let slots = build_slots(&page.module, payload);
    // The no-JS refresh targets "/pads" WITHOUT a query string on purpose: a
    // flash shows for one cycle and then the URL is clean.
    //
    // The ARMED render has no refresh at all. `?confirm=1` would be dropped by
    // that same rule, so a no-JS user reading the confirmation — which asks
    // them to check every device id in a list that can be fifteen rows long —
    // would be navigated back to the disarmed view at five seconds, every
    // time. There would be no window in which a full bus could be confirmed.
    // A page that has asked a question waits for the answer.
    let prefix = if payload.confirm {
        body_prefix_no_refresh(payload)
    } else {
        body_prefix(payload, "/pads")
    };
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — virtual pads",
        route_pattern: "/pads",
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

    /// The rendered document with the `__ksx-payload` data block removed.
    ///
    /// Every field of the payload is embedded verbatim as JSON for the client
    /// to hydrate from, so a naive `html.contains(sentence)` passes for any
    /// sentence in the PAYLOAD whether or not the page renders it — which is
    /// how a test meant to catch the seam re-wording things passed against a
    /// seam that had (caught by mutating `busLine` back to a locally composed
    /// sentence: the assertion still found the provider's string, in the JSON).
    /// Assertions about what a reader sees go through this.
    fn rendered(html: &str) -> String {
        let Some(start) = html.find("<script id=\"__ksx-payload\"") else {
            return html.to_owned();
        };
        let end = html[start..]
            .find("</script>")
            .map_or(html.len(), |at| start + at + "</script>".len());
        format!("{}{}", &html[..start], &html[end..])
    }

    /// The island source, compiled IN so the cross-language guard below cannot
    /// silently stop reading anything: move or rename the file and this crate
    /// fails to build, which is the only failure mode a lint over another
    /// language's source is allowed to have.
    const PADS_ISLAND_TS: &str = include_str!("../../../studio-ui/src/PadsIsland.ts");

    fn view() -> ksx_api::PadsView {
        ksx_api::PadsView {
            generated_at: "2026-08-07 12:00:00 UTC".into(),
            summary: "2 virtual pads on the ViGEm bus:".into(),
            bus_instance_id: Some(r"ROOT\SYSTEM\0002".into()),
            bus_line: r"ROOT\SYSTEM\0002".into(),
            pads: vec![
                ksx_api::VirtualPadRow {
                    instance_id: r"USB\VID_045E&PID_028E\2&AA&0&01".into(),
                    hardware_id: r"USB\VID_045E&PID_028E".into(),
                    persona: "Xbox 360 pad".into(),
                    xinput: true,
                },
                ksx_api::VirtualPadRow {
                    instance_id: r"USB\VID_054C&PID_05C4\2&AA&0&02".into(),
                    hardware_id: r"USB\VID_054C&PID_05C4".into(),
                    persona: "PlayStation (DS4) pad".into(),
                    xinput: false,
                },
            ],
            owners: vec!["ksx.exe (pid 4242)".into()],
            owners_line: "ksx.exe (pid 4242)".into(),
            session_running: false,
            xinput_ceiling: 4,
            xinput_in_use: Some(1),
            xinput_line: "Windows exposes exactly 4 XInput slots".into(),
            elevated: Some(false),
            elevation_line: "ksx is NOT running elevated, and ksx never self-elevates — this \
                             prune will be refused."
                .into(),
            confirm_line: "This removes 2 pad(s) by restarting the ViGEmBus devnode. Every pad \
                           listed here goes, at once:"
                .into(),
            unreadable_heading: String::new(),
            prune: ksx_api::PrunePlanView {
                kind: "restart".into(),
                count: 2,
                command: Some(r#"pnputil /restart-device "ROOT\SYSTEM\0002""#.into()),
                detail: "DRY RUN — would clear 2 virtual pad(s)".into(),
            },
            spawn: ksx_api::SpawnOffer {
                counts: vec![
                    ksx_api::SpawnOption {
                        value: "1".into(),
                        label: "1 pad".into(),
                    },
                    ksx_api::SpawnOption {
                        value: "8".into(),
                        label: "8 pads — only 3 readable as xbox360, all 8 as playstation".into(),
                    },
                ],
                personas: vec![ksx_api::SpawnOption {
                    value: "xbox360".into(),
                    label: "xbox360 — takes one of the 4 XInput slots".into(),
                }],
                holds: vec![ksx_api::SpawnOption {
                    value: "10".into(),
                    label: "10 seconds".into(),
                }],
                note: "A spawn is a TEST".into(),
                refused: None,
            },
        }
    }

    fn idle_session() -> SessionView {
        SessionView {
            reachable: true,
            running: false,
            line: "idle — daemon reachable".into(),
            profile: None,
        }
    }

    fn payload() -> PadsPayload {
        PadsPayload {
            pads: view(),
            session: idle_session(),
            confirm: false,
            unavailable: None,
            flash: None,
        }
    }

    /// The payload a refused `MachineSource::pads_view` produces, built the
    /// way `server.rs` builds it — through the constructor, never through
    /// `PadsView::default()`.
    fn unreadable_payload() -> PadsPayload {
        PadsPayload {
            pads: ksx_api::PadsView::unreadable("the pad collection panicked"),
            session: idle_session(),
            confirm: false,
            unavailable: Some("the pad collection panicked".into()),
            flash: None,
        }
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load("/pads").expect("embedded page must load");
        assert_eq!(page.module().header.version, 2);
    }

    /// The slot-table contract this seam depends on, both directions: every
    /// name the seam injects is one the island RENDERS, and every scalar the
    /// island renders is one the seam injects. Read
    /// [`crate::render::assert_island_slot_contract`] before touching this —
    /// "the slot exists" is not the check.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let module = page.module();
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&PadsPayload::default());
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
                LIST_SLOT_PADS,
                LIST_SLOT_GHOST_PADS,
                LIST_SLOT_COUNTS,
                LIST_SLOT_PERSONAS,
                LIST_SLOT_HOLDS,
                LIST_SLOT_PRUNE_ROWS,
            ],
            "list slot names drifted between the compiler/PadsIsland.ts and the \
             LIST_SLOT_* constants; slots: {names:?}"
        );
        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> = show_values(&PadsPayload::default())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between PadsIsland.ts and show_values()"
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
            .chain(list_values(&PadsPayload::default()).iter().map(|(n, _)| *n))
            .chain(seam_shows.iter().copied())
            .collect();
        crate::render::assert_island_slot_contract(
            module,
            &injected,
            &CLIENT_ONLY_SLOTS,
            &ANONYMOUS_SLOTS,
        );
    }

    #[test]
    fn render_injects_the_real_bus_reading_into_ssr_html() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(&page, &payload());
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        assert!(out.html.contains("2 virtual pads on the ViGEm bus"));
        assert!(out
            .html
            .contains("USB\\VID_045E&amp;PID_028E\\2&amp;AA&amp;0&amp;01"));
        assert!(out.html.contains("PlayStation (DS4) pad"));
        assert!(out.html.contains("ksx.exe (pid 4242)"));
        assert!(out.html.contains("ROOT\\SYSTEM\\0002"));
        assert!(out.html.contains("2026-08-07 12:00:00 UTC"));
        assert_complete_head("/pads", &out.html);
        assert!(
            out.html.contains(
                r#"<noscript><meta http-equiv="refresh" content="5; url=/pads"></noscript>"#
            ),
            "{}",
            out.html
        );
    }

    /// **Task #16, on the page — and the warning has to be about the click
    /// the user is actually making.**
    ///
    /// Fails against the version this replaced in two independent ways. That
    /// version rendered once and grepped for "invisible to games", so it could
    /// not fail on a label that named no persona (the same `<option>` said "4
    /// invisible" beside a persona `<select>` offering "playstation — plain
    /// HID, takes no XInput slot"), and it could not fail on a label that
    /// never changed when the machine did. Both are checked here: the label
    /// names its persona, and re-rendering with a different reading of the
    /// same machine produces a different label.
    #[test]
    fn the_xinput_ceiling_is_on_the_page_before_the_button_is_pressed() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(&page, &payload());

        // 1. The standing sentence, from the provider.
        assert!(
            out.html.contains("Windows exposes exactly 4 XInput slots"),
            "the ceiling paragraph must render: {}",
            out.html
        );
        // 2. The option itself, with the persona its warning applies to. A
        //    dropdown entry reading "8" is a lie on a four-slot machine, and
        //    one reading "8 pads — 4 invisible" is a lie for a HID pad.
        assert!(
            out.html
                .contains("8 pads — only 3 readable as xbox360, all 8 as playstation"),
            "the count option must carry its own persona-named warning: {}",
            out.html
        );
        // …and the submit is a real form, so the warning is not JS-only.
        assert!(out.html.contains(r#"action="/pads/spawn""#), "{}", out.html);
        assert!(out.html.contains(r#"name="count""#), "{}", out.html);

        // 3. It TRACKS the machine. A page that painted this once and then
        //    kept it is a page whose warning is true for two seconds.
        let mut busier = payload();
        busier.pads.spawn.counts[1].label =
            "8 pads — none readable as xbox360, all 8 as playstation".into();
        busier.pads.xinput_line =
            "Windows exposes exactly 4 XInput slots and 4 of them are in use".into();
        let after = render_pads(&page, &busier);
        assert!(
            after.html.contains("none readable as xbox360"),
            "a changed reading must produce a changed label: {}",
            after.html
        );
        assert!(
            !after.html.contains("only 3 readable"),
            "the stale label must be gone, not merely joined: {}",
            after.html
        );
    }

    /// **The other half of that warning lives in TypeScript, and it froze.**
    ///
    /// forma's `createList` reconciles by KEY, and with `updateOnItemChange`
    /// defaulting to "none" a matching key runs an update function that stores
    /// the new item and patches no DOM — the runtime's own comment is "reused
    /// rows keep their DOM". So a row keyed on `o.value` alone keeps the label
    /// it was built with forever: the count dropdown painted "8 pads" on an
    /// idle bus and still said "8 pads" after four Xbox pads had taken every
    /// XInput slot, which is precisely when the warning matters.
    ///
    /// This fails against that version. It is a lint over another language's
    /// source because the failure is a *browser* behaviour and this repo's
    /// only browser harness (`studio-ui/pwtest`) needs Playwright's downloaded
    /// runtime, which CI does not have; a rule the compiler can check beats a
    /// test that never runs. The rule is general: **the key must mention every
    /// member the row body renders**, so a future list cannot reintroduce it.
    #[test]
    fn every_list_row_reconciles_on_every_field_it_renders() {
        let calls = createlist_args(PADS_ISLAND_TS);
        assert_eq!(
            calls.len(),
            6,
            "PadsIsland.ts should have six lists (pads, ghosts, counts, personas, \
             holds, prune rows); found {}",
            calls.len()
        );
        for (source, key, body) in calls {
            let param = arrow_param(&key)
                .unwrap_or_else(|| panic!("could not read the key's parameter in: {key}"));
            let body_param = arrow_param(&body)
                .unwrap_or_else(|| panic!("could not read the body's parameter in: {body}"));
            for member in member_reads(&body, &body_param) {
                assert!(
                    key.contains(&format!("{param}.{member}")),
                    "list `{source}` renders `{body_param}.{member}` but keys on `{key}` — \
                     forma reconciles by key and does NOT patch a matching row, so this \
                     field freezes at its first paint"
                );
            }
        }
    }

    /// Split every `createList(a, b, c)` in `src` into its three arguments.
    fn createlist_args(src: &str) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(at) = rest.find("createList(") {
            let after = &rest[at + "createList(".len()..];
            let mut depth = 1usize;
            let mut end = after.len();
            let mut args: Vec<String> = Vec::new();
            let mut start = 0usize;
            for (i, ch) in after.char_indices() {
                match ch {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    ',' if depth == 1 => {
                        args.push(after[start..i].trim().to_owned());
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            let last = after[start..end].trim();
            if !last.is_empty() {
                args.push(last.to_owned());
            }
            assert!(
                args.len() >= 3,
                "createList needs (source, key, body); got {args:?}"
            );
            out.push((args[0].clone(), args[1].clone(), args[2].clone()));
            rest = &after[end..];
        }
        out
    }

    /// The single parameter name of an arrow function like `(o) => …`.
    fn arrow_param(arrow: &str) -> Option<String> {
        let open = arrow.find('(')?;
        let close = arrow[open..].find(')')? + open;
        let name = arrow[open + 1..close].trim();
        (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then(|| name.to_owned())
    }

    /// Every `param.member` read in `body`.
    fn member_reads(body: &str, param: &str) -> std::collections::BTreeSet<String> {
        let needle = format!("{param}.");
        let mut out = std::collections::BTreeSet::new();
        let mut rest = body;
        while let Some(at) = rest.find(&needle) {
            // Not a member read if the character before is part of a longer
            // identifier (`p.` inside `pad.`).
            let preceded_ok = rest[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '.');
            let after = &rest[at + needle.len()..];
            let member: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if preceded_ok && !member.is_empty() {
                out.insert(member);
            }
            rest = &rest[at + needle.len()..];
        }
        out
    }

    /// The destructive path, at both of its two states.
    ///
    /// Unarmed there must be NO submit that restarts the bus anywhere in the
    /// document — not hidden, not disabled, absent — because the SSR paint is
    /// what a no-JS browser gets and a `display:none` button is one CSS
    /// failure away from being pressable.
    #[test]
    fn prune_needs_an_explicit_confirm_and_names_every_pad_first() {
        let page = EmbeddedPage::load("/pads").unwrap();

        let unarmed = render_pads(&page, &payload());
        assert!(
            !unarmed.html.contains(r#"name="confirm" value="yes""#),
            "an unarmed page must not carry the confirmed submit at all: {}",
            unarmed.html
        );
        assert!(
            unarmed.html.contains(r#"href="/pads?confirm=1""#),
            "the arming link must be there: {}",
            unarmed.html
        );

        let armed = render_pads(
            &page,
            &PadsPayload {
                confirm: true,
                ..payload()
            },
        );
        assert!(
            armed.html.contains(r#"action="/pads/prune""#)
                && armed.html.contains(r#"name="confirm" value="yes""#),
            "the armed panel must carry the real submit: {}",
            armed.html
        );
        // Exactly what will be removed, by instance id, not a count.
        assert!(
            armed.html.contains("This removes 2 pad(s)"),
            "{}",
            armed.html
        );
        assert!(
            armed
                .html
                .contains("USB\\VID_054C&amp;PID_05C4\\2&amp;AA&amp;0&amp;02"),
            "every pad the restart takes must be named: {}",
            armed.html
        );
        // And the elevation warning, before the click rather than after the
        // refusal.
        assert!(
            armed.html.contains("NOT running elevated"),
            "{}",
            armed.html
        );
    }

    /// **A confirmation must not navigate itself away while it is being read.**
    ///
    /// Fails against the version this replaced, which emitted
    /// `<meta http-equiv="refresh" content="5; url=/pads">` on EVERY render.
    /// The refresh URL carries no query string by design, so `?confirm=1` went
    /// with it: a no-JS user arming a prune on a bus carrying fifteen ghosts
    /// was asked to check every instance id in the list and then dropped back
    /// to the disarmed view five seconds later, every time. There was no
    /// window in which a full bus could be confirmed.
    #[test]
    fn the_armed_confirmation_does_not_reload_itself_out_from_under_the_reader() {
        let page = EmbeddedPage::load("/pads").unwrap();

        let armed = render_pads(
            &page,
            &PadsPayload {
                confirm: true,
                ..payload()
            },
        );
        assert!(
            !armed.html.contains("http-equiv=\"refresh\""),
            "an armed confirmation must not carry a refresh timer: {}",
            armed.html
        );
        // …but the payload block it shares with the poller is still there, so
        // hydration is unaffected.
        assert!(
            armed.html.contains(r#"id="__ksx-payload""#),
            "{}",
            armed.html
        );

        // And the ordinary page keeps its no-JS liveness.
        let unarmed = render_pads(&page, &payload());
        assert!(
            unarmed.html.contains(r#"content="5; url=/pads""#),
            "the unarmed page is a live view and keeps refreshing: {}",
            unarmed.html
        );
    }

    /// A plan that is not a restart must not render an arming link at all — a
    /// "Prune…" button beside "a session is running" would be a button whose
    /// only possible outcome is a refusal.
    #[test]
    fn a_refused_plan_offers_no_prune_button() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let mut busy = payload();
        busy.pads.prune.kind = "session-running".into();
        busy.pads.prune.command = None;
        busy.pads.spawn.refused = Some("a session is running".into());
        let out = render_pads(&page, &busy);
        assert!(
            !out.html.contains(r#"href="/pads?confirm=1""#),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains(r#"name="confirm" value="yes""#),
            "{}",
            out.html
        );
        // The spawn form goes with it, and says why.
        assert!(
            !out.html.contains(r#"action="/pads/spawn""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("a session is running"), "{}", out.html);
    }

    /// **A REFUSED READ IS NOT AN EMPTY BUS.**
    ///
    /// This project's signature bug, on this page: a session once reported
    /// success while the arcade panel was dead because a board had fallen back
    /// to a driver nobody asked for. "I could not read this" and "there is
    /// nothing here" are different sentences and a user acts on them
    /// differently — the first sends them to `ksx doctor`, the second tells
    /// them the machine is fine.
    ///
    /// Fails against the version this replaced, which handed the seam a
    /// `PadsView::default()` and rendered, on a bus it had never managed to
    /// look at: "none present — ViGEmBus is not installed" as the devnode,
    /// four ghost tiles reading "empty", and "Nothing for this panel to do
    /// right now" over the prune. Three assertions of absence about a failed
    /// read, under a banner saying the read failed.
    #[test]
    fn a_refused_read_is_never_rendered_as_an_empty_bus() {
        let page = EmbeddedPage::load("/pads").unwrap();
        // The rendered document only: the payload block carries every field as
        // JSON for hydration, and a claim nobody can see is not a claim.
        let out = rendered(&render_pads(&page, &unreadable_payload()).html);

        assert!(
            out.contains("ksx could not read the ViGEm bus"),
            "the banner must say the read failed: {out}"
        );
        assert!(
            out.contains("the pad collection panicked"),
            "…and why: {out}"
        );

        for absence in [
            // The devnode line, claiming a driver is not installed.
            "not installed",
            // Ghost tiles, drawing four empty slots.
            ">empty<",
            // The prune panel, saying there is nothing to do.
            "Nothing for this panel to do",
            // The summary, counting a bus that was never counted.
            "no virtual pads on the ViGEm bus",
            // The ceiling card's standing paragraph. "Every count below
            // carries its own label" is a sentence about a spawn menu that is
            // NOT below (the offer is refused), and "Windows will still only
            // ever hand four of them to a game" is machine advice under a
            // banner saying the machine could not be read — the exact
            // contradiction the `show:busRead` gate exists to prevent.
            "carries its own label",
            "PlayStation pads are the way past it",
        ] {
            assert!(
                !out.contains(absence),
                "a failed read rendered '{absence}', which is a claim about a machine \
                 ksx never managed to look at: {out}"
            );
        }

        // And no verb is offered: an empty `<select>` beside a live Spawn
        // button is a page inviting a click it cannot describe.
        assert!(!out.contains(r#"action="/pads/spawn""#), "{out}");
        assert!(!out.contains(r#"href="/pads?confirm=1""#), "{out}");
    }

    /// The flash arrives from a query string and is attacker-writable.
    #[test]
    fn the_flash_is_escaped() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(
            &page,
            &PadsPayload {
                flash: Some("<script>alert(1)</script>".into()),
                ..payload()
            },
        );
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );
        assert!(out.html.contains("&lt;script&gt;"), "{}", out.html);
    }

    /// An error flash colours the error side of the pair, and only that side.
    #[test]
    fn an_error_flash_picks_the_error_variant() {
        let shows = |flash: Option<&str>| {
            let payload = PadsPayload {
                flash: flash.map(str::to_owned),
                ..payload()
            };
            let values = show_values(&payload);
            let get = |name: &str| {
                values
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| *v)
                    .unwrap()
            };
            (get("show:flashOk"), get("show:flashError"))
        };
        assert_eq!(shows(None), (false, false));
        assert_eq!(shows(Some("4 pads plugged")), (true, false));
        assert_eq!(shows(Some("error: a session is running")), (false, true));
    }

    /// Specialist screens keep the customer rail intact: Setup → Controls →
    /// Test. Pad maintenance is not promoted to a fourth workflow stage.
    #[test]
    fn the_nav_reaches_the_other_screens() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(&page, &payload());
        assert!(
            out.html
                .contains(r#"<a class="navlink" href="/start">Setup</a>"#),
            "{}",
            out.html
        );
        assert!(
            out.html
                .contains(r#"<a class="navlink" href="/map">Controls</a>"#),
            "{}",
            out.html
        );
        assert!(
            out.html
                .contains(r#"<a class="navlink" href="/check">Test</a>"#),
            "{}",
            out.html
        );
    }

    /// The page embeds the SAME struct `/api/pads` serves, so the poller's
    /// writes and the first paint can never describe different shapes.
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let payload = payload();
        let json = crate::render::payload_json(&payload);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("payload parse");
        assert_eq!(
            parsed,
            serde_json::to_value(&payload).unwrap(),
            "the payload block must be byte-compatible with /api/pads"
        );
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(&page, &payload);
        assert!(out.html.contains(&json), "{}", out.html);
    }

    /// **This seam words nothing.** Every sentence on the page is the
    /// provider's, so a provider that changed its mind changes the page.
    ///
    /// Fails against the version this replaced, which composed the devnode
    /// line, the owner line, the elevation warning and the confirm lead HERE
    /// and again in `PadsIsland.ts` — the server painted one wording, the 2 s
    /// poller overwrote it with the other, and nothing anywhere compared them.
    #[test]
    fn every_sentence_on_the_page_is_the_providers() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let mut odd = payload();
        odd.pads.bus_line = "BUS-LINE-FROM-THE-PROVIDER".into();
        odd.pads.owners_line = "OWNERS-LINE-FROM-THE-PROVIDER".into();
        odd.pads.elevation_line = "ELEVATION-LINE-FROM-THE-PROVIDER".into();
        odd.pads.confirm_line = "CONFIRM-LINE-FROM-THE-PROVIDER".into();
        odd.confirm = true;
        let out = rendered(&render_pads(&page, &odd).html);
        for sentence in [
            "BUS-LINE-FROM-THE-PROVIDER",
            "OWNERS-LINE-FROM-THE-PROVIDER",
            "ELEVATION-LINE-FROM-THE-PROVIDER",
            "CONFIRM-LINE-FROM-THE-PROVIDER",
        ] {
            assert!(
                out.contains(sentence),
                "{sentence} did not reach the RENDERED page — the seam is composing its own \
                 wording again: {out}"
            );
        }
        // …and the wording it used to own is nowhere in this crate's output.
        assert!(
            !out.contains("ViGEmBus is not installed, or its devnode has gone"),
            "the seam still has its own copy of the devnode sentence: {out}"
        );
        assert!(
            !PADS_ISLAND_TS.contains("never self-elevates"),
            "PadsIsland.ts still composes the elevation warning itself"
        );
        assert!(
            !PADS_ISLAND_TS.contains("Every pad listed here goes"),
            "PadsIsland.ts still composes the confirm lead itself"
        );
        assert!(
            !PADS_ISLAND_TS.contains("no known splitter"),
            "PadsIsland.ts still composes the owner line itself"
        );
        assert!(
            !PADS_ISLAND_TS.contains("could not read the ViGEm bus"),
            "PadsIsland.ts still hardcodes the failed-read banner heading — it is \
             `PadsView::unreadable_heading` now, and a second copy in TypeScript is \
             the drift docs/SURFACES.md §1a names"
        );
    }

    /// **The poller must hand back the user's `<select>` choice.**
    ///
    /// The key rule above makes every option row rebuild when its label
    /// changes — that is the point, the label IS the warning — but a rebuilt
    /// `<option>` list resets its `<select>` to the first entry. On a page
    /// whose labels track a 2 s reading, that silently discarded whatever the
    /// user had picked every time the XInput occupancy or the bus headroom
    /// moved between polls. `pads.ts` owns the fix: capture the three spawn
    /// selects before `applyPads`, restore any still-offered value after.
    ///
    /// A source lint, same justification as the key rule: the failure is a
    /// browser behaviour and the repo's only browser harness needs a runtime
    /// CI does not have.
    #[test]
    fn the_poller_restores_the_spawn_choice_the_rebuild_discards() {
        const PADS_TS: &str = include_str!("../../../studio-ui/src/pads.ts");
        let capture = PADS_TS
            .find("const chosen = spawnChoices()")
            .expect("pads.ts must capture the spawn selects before applying a payload");
        let apply = PADS_TS[capture..]
            .find("applyPads(")
            .map(|at| capture + at)
            .expect("pads.ts must apply the payload after capturing");
        let restore = PADS_TS[apply..]
            .find("restoreSpawnChoices(chosen)")
            .map(|at| apply + at)
            .expect("pads.ts must restore the captured choices after applying");
        assert!(
            capture < apply && apply < restore,
            "capture must precede applyPads and the restore must follow it"
        );
        // And the restore must never resurrect a value the offer withdrew —
        // the backend's ordering IS the default when the choice is gone.
        assert!(
            PADS_TS.contains("some((o) => o.value === value)"),
            "restoreSpawnChoices must check the option still exists before selecting it"
        );
    }
}
