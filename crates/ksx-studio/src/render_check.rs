//! The /check render seam: embedded FMIR + one [`CheckPayload`] → HTML.
//!
//! **The button check** (docs/MAPPER-UX.md Build C): press a panel key, and
//! every virtual control it drives lights on every slot at once. Same four-part
//! slot seam as [`crate::render`] — scalars, lists, shows, and the layout test
//! that pins all three — so read `render.rs`'s module docs for the protocol.
//! What is worth writing down here is what is different about this page.
//!
//! # This page has two data channels, and only one of them is here
//!
//! Everything this seam renders is STRUCTURE, read from disk: which slots
//! exist, which controls each preset names, which keys drive them. The
//! lighting-up arrives on `GET /api/live` — Server-Sent Events over the
//! daemon's own feed pipe (`crate::live`) — at display rate, and never touches
//! a slot value.
//!
//! That split is the page. An echo carried in this payload would be as fast as
//! an HTTP poll, and a button check that answers "did that press arrive?"
//! two seconds later is not a button check. It also means the SSR paint is
//! genuinely useful with no daemon at all: the binding table is correct
//! whatever the pipe is doing, so the page still answers *what this key should
//! do* while honestly saying it cannot answer *whether it did*.
//!
//! # The seam decides nothing about controls
//!
//! The chip roster is `MapperSlot::bindings`' key set — every function the
//! preset names, unbound ones included, in the canonical spelling the preset
//! file, the mapper's legend and the live frame all use. Nothing here knows
//! that an Xbox pad has an `A` button. That matters more here than on most
//! pages, because a hardcoded roster would look right on a stock preset and
//! quietly omit exactly the control somebody added and is now standing at the
//! cabinet trying to test.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{body_prefix, with_icon_links, EmbeddedPage, PERSONALITY_CSS};
use crate::snapshot::CheckPayload;

/// List array slot names, BINDING-derived (compiler 0.2.0+), in slot-table
/// (== document) order. Rename a list signal in CheckIsland.ts and the layout
/// test fails until these match again.
const LIST_SLOT_KEYS: &str = "list:keyRows:array";
const LIST_SLOT_CHIPS: &str = "list:chips:array";

/// The island table this page compiles to: exactly one island — the whole
/// screen. Its name is the `activateIslands` registry key in
/// `studio-ui/src/check.ts`.
#[cfg(test)]
const ISLAND_COMPONENT: &str = "CheckIsland";

/// How many `createShow` pairs this page has; the layout test pins both the
/// count and every name.
const SHOW_COUNT: usize = 7;

/// Bare-named slots this page renders and the seam deliberately never fills.
/// EMPTY, and that is the claim.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots this page compiles to. EMPTY — every
/// attribute value and every text child is either a named signal binding, a
/// list item's member read, or static markup.
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

/// What a control with no key bound to it says on its chip.
///
/// One word, here, once — the island reads it off the payload rather than
/// spelling its own. An unbound control still gets a chip on purpose: it is the
/// answer to "I pressed the button and nothing happened", and a roster that
/// hid its unbound half would make that question unanswerable on the one
/// screen built to answer it.
const UNBOUND: &str = "unbound";

/// The sentence under the feed line: what this screen is watching.
///
/// Composed here rather than in TypeScript for the reason every other page's
/// prose is (docs/SURFACES.md §1) — and this one names the CLI equivalent,
/// because a cabinet with no browser still has a terminal.
fn feed_hint() -> String {
    "Frames come from the running daemon's input feed, live. `ksx monitor` \
     shows the same stream in a terminal."
        .to_owned()
}

/// The [`CheckPayload`] for one mapper read and one session view.
///
/// A constructor rather than a struct literal at the call site, so the hint
/// cannot be composed a second way by a second caller.
pub(crate) fn payload(
    mapper: ksx_api::MapperSnapshot,
    session: crate::control::SessionView,
) -> CheckPayload {
    CheckPayload {
        mapper,
        session,
        feed_hint: feed_hint(),
    }
}

/// Scalar slot values, keyed by the signal names in CheckIsland.ts.
fn scalar_slots(payload: &CheckPayload) -> serde_json::Value {
    serde_json::json!({
        "generatedAt": payload.mapper.generated_at,
        "sourceLine": payload.mapper.source,
        "feedHint": payload.feed_hint,
        "sessionLine": payload.session.line,
        // The FEED's own state is the client's to word — it is the only thing
        // on this page the server cannot know, because the stream is opened by
        // the browser. The SSR value says so rather than claiming a state:
        // "live" painted server-side would be a lie for however long the
        // EventSource takes to connect, and on a machine with no daemon it
        // would never stop being one.
        "feedLine": "opening the live feed…",
        // Loss counters are per-frame and arrive with the frames. Nothing to
        // say before the first one.
        "lossLine": "",
        "offPanelLine": "",
    })
}

/// One chip per control per slot, slot order then the preset's own control
/// order (`bindings` is a `BTreeMap`, so that order is stable and is the same
/// one the mapper's legend walks).
///
/// `slot` is the NUMBER as a string because it is rendered as a DOM attribute
/// and read back as one by `check.ts`'s `chipFor`. The pair (`data-slot`,
/// `data-control`) is the entire contract between the server-rendered
/// structure and the client's live echo — raw values on both sides, never a
/// composed id, so there is no string spelled in two languages to drift.
fn chip_values(payload: &CheckPayload) -> SlotValue {
    let mut chips = Vec::new();
    for slot in &payload.mapper.slots {
        for (control, keys) in &slot.bindings {
            let label = if keys.is_empty() {
                UNBOUND.to_owned()
            } else {
                keys.join(" · ")
            };
            chips.push(SlotValue::object(vec![
                ("slot".to_owned(), SlotValue::Text(slot.number.to_string())),
                (
                    "player".to_owned(),
                    SlotValue::Text(format!("P{}", slot.number)),
                ),
                ("control".to_owned(), SlotValue::Text(control.clone())),
                ("keys".to_owned(), SlotValue::Text(label)),
            ]));
        }
    }
    SlotValue::array(chips)
}

/// The list array payloads, keyed by their (unique) slot names.
///
/// The key strip is ALWAYS empty server-side, and that is a claim rather than
/// an omission: a key hit is something that happened at a moment, and the
/// server rendering this page has no moment to report. Painting a remembered
/// keystroke into an SSR document would put a press on screen that is not
/// happening.
fn list_values(payload: &CheckPayload) -> [(&'static str, SlotValue); 2] {
    [
        (LIST_SLOT_KEYS, SlotValue::array(Vec::new())),
        (LIST_SLOT_CHIPS, chip_values(payload)),
    ]
}

/// Every show slot on this page, BY NAME, with the boolean the server wants.
///
/// The shows are all SIBLINGS — none is nested inside another's branch — so
/// every combined condition is computed once, here and in `check.ts`, instead
/// of relying on a parent branch having rendered.
fn show_values(payload: &CheckPayload) -> [(&'static str, bool); SHOW_COUNT] {
    let has_slots = payload.mapper.slots.iter().any(|s| !s.bindings.is_empty());
    [
        ("show:hasSlots", has_slots),
        ("show:noSlots", !has_slots),
        // The feed is DOWN in every server paint, because the server has not
        // opened it — the browser does. Claiming "live" here would be a lie
        // for as long as the connection took, and forever on a machine with
        // no daemon.
        ("show:live", false),
        ("show:feedDown", true),
        // Nothing has arrived, so the strip says so rather than sitting blank.
        ("show:quiet", true),
        // Both counters are per-frame; there are no frames yet.
        ("show:hasLoss", false),
        ("show:hasOffPanel", false),
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
fn build_slots(module: &IrModule, payload: &CheckPayload) -> SlotData {
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

/// Render /check for one payload: SSR slots for first paint, the same data as
/// island props for hydration.
///
/// The no-JS refresh is kept (unlike `/pads`'s armed state): with scripting off
/// there is no stream, so a periodic reload is the only way the binding table
/// picks up a preset edit made from another surface. The page says in a
/// `<noscript>` block that the echo itself needs JavaScript — a refresh cannot
/// substitute for a live feed, and pretending otherwise would be the dead grid
/// this page exists not to be.
pub(crate) fn render_check(page: &EmbeddedPage, payload: &CheckPayload) -> PageOutput {
    let slots = build_slots(&page.module, payload);
    let prefix = body_prefix(payload, "/check");
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — button check",
        route_pattern: "/check",
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
    use std::collections::BTreeMap;

    /// The island source, compiled IN so the cross-language guards below cannot
    /// silently stop reading anything: move or rename the file and this crate
    /// fails to build.
    const CHECK_ISLAND_TS: &str = include_str!("../../../studio-ui/src/CheckIsland.ts");
    const CHECK_TS: &str = include_str!("../../../studio-ui/src/check.ts");

    /// The rendered document with the `__ksx-payload` data block removed.
    ///
    /// Every field of the payload is embedded verbatim as JSON for the client
    /// to hydrate from, so a naive `html.contains(sentence)` passes for any
    /// sentence in the PAYLOAD whether or not the page renders it. Assertions
    /// about what a reader sees go through this.
    fn rendered(html: &str) -> String {
        let Some(start) = html.find("<script id=\"__ksx-payload\"") else {
            return html.to_owned();
        };
        let end = html[start..]
            .find("</script>")
            .map_or(html.len(), |at| start + at + "</script>".len());
        format!("{}{}", &html[..start], &html[end..])
    }

    fn slot(number: u8, bindings: &[(&str, &[&str])]) -> ksx_api::MapperSlot {
        ksx_api::MapperSlot {
            number,
            persona: "xbox360".into(),
            persona_label: "Xbox 360".into(),
            preset: format!("IPAC P{number}"),
            keyboard: "IPAC".into(),
            bindings: bindings
                .iter()
                .map(|(f, keys)| {
                    (
                        (*f).to_owned(),
                        keys.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>(),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
            backup: None,
            turbo: BTreeMap::new(),
            macros_off: false,
        }
    }

    fn cabinet() -> CheckPayload {
        payload(
            ksx_api::MapperSnapshot {
                generated_at: "2026-08-08 12:00:00 UTC".into(),
                source: r#"slots of profile "Steam" (games.toml)"#.into(),
                config_root: r"C:\Users\v\.ksx".into(),
                slots: vec![
                    slot(1, &[("A", &["G"]), ("dpad.up", &["Up"]), ("B", &[])]),
                    slot(2, &[("A", &["G"]), ("dpad.up", &["W"])]),
                ],
                profile: Some("Steam".into()),
            },
            SessionView {
                reachable: true,
                running: true,
                line: "running (2 slots)".into(),
                profile: Some("Steam".into()),
            },
        )
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load("/check").expect("the /check route is embedded");
        assert_eq!(page.module.header.version, 2);
    }

    #[test]
    fn the_check_head_is_complete() {
        let page = EmbeddedPage::load("/check").unwrap();
        assert_complete_head("/check", &render_check(&page, &cabinet()).html);
    }

    /// **The fan-out, server-side.** One key on two slots is TWO chips, each
    /// naming its own slot — which is what makes "four pads glowing from one
    /// keystroke" possible at all, because the client lights chips by
    /// (`data-slot`, `data-control`) and cannot light a slot that has no chip.
    ///
    /// Catches a seam that emitted one chip per CONTROL NAME rather than per
    /// (slot, control): the page would have looked correct on a one-slot
    /// cabinet and shown P1 lighting alone on a four-slot one — the exact
    /// demo, silently broken.
    #[test]
    fn one_key_on_two_slots_renders_two_chips_each_naming_its_slot() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = rendered(&render_check(&page, &cabinet()).html);
        for slot in ["1", "2"] {
            assert!(
                html.contains(&format!(r#"data-slot="{slot}""#)),
                "slot {slot} has no chip: {html}"
            );
        }
        assert_eq!(
            html.matches(r#"data-control="A""#).count(),
            2,
            "A is on both slots, so it is two chips: {html}"
        );
        assert!(html.contains("P1"), "{html}");
        assert!(html.contains("P2"), "{html}");
    }

    /// **The roster is the BACKEND's, unbound controls included.**
    ///
    /// A control with no key is exactly the control somebody is standing at the
    /// cabinet trying to test, so it gets a chip and says `unbound` rather than
    /// being filtered out. Catches the version that rendered only bound
    /// controls: the one question the page exists to answer — "I pressed it and
    /// nothing happened" — became unanswerable, because the control was not on
    /// screen to be looked for.
    #[test]
    fn an_unbound_control_still_gets_a_chip_and_says_so() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = rendered(&render_check(&page, &cabinet()).html);
        assert!(
            html.contains(r#"data-control="B""#),
            "the unbound control is missing: {html}"
        );
        assert!(html.contains(UNBOUND), "{html}");
        // ...and a bound one shows the key that drives it, which is the
        // wiring-diagnostic half: press G, watch the chip that says G.
        assert!(html.contains(">G<"), "{html}");
    }

    /// **The server never claims the feed is live.**
    ///
    /// The stream is opened by the BROWSER, so at render time the server knows
    /// nothing about it. A paint that said "live" would be wrong for as long as
    /// the connection took and permanently wrong on a machine with no daemon —
    /// and this page's whole worth is that a dark grid means something.
    #[test]
    fn the_server_paint_never_asserts_a_live_feed() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = rendered(&render_check(&page, &cabinet()).html);
        assert!(html.contains("opening the live feed"), "{html}");
        for (name, value) in show_values(&cabinet()) {
            if name == "show:live" {
                assert!(!value, "the server cannot know the feed is up");
            }
        }
    }

    /// With no slots the page says why instead of rendering an empty grid that
    /// looks like a cabinet with nothing pressed.
    #[test]
    fn a_cabinet_with_no_slots_says_so_rather_than_showing_an_empty_grid() {
        let page = EmbeddedPage::load("/check").unwrap();
        let mut empty = cabinet();
        empty.mapper = ksx_api::MapperSnapshot::unavailable(
            "no slots are configured — `ksx slot assign` creates one",
        );
        let out = render_check(&page, &empty);
        let html = rendered(&out.html);
        assert!(html.contains("No slots to check"), "{html}");
        assert!(html.contains("ksx slot assign"), "{html}");
        assert!(
            !html.contains("data-control="),
            "no chips without slots: {html}"
        );
    }

    /// **With scripting off the page says the echo cannot work.**
    ///
    /// This is the page's single most important honest sentence: with no
    /// JavaScript there is no EventSource, so no chip can ever light, and a
    /// grid of dark chips is exactly what a WORKING check looks like while
    /// nobody is pressing anything. Catches the version that shipped the grid
    /// with no `<noscript>` at all — indistinguishable, on screen, from a
    /// cabinet whose panel is dead.
    #[test]
    fn with_no_javascript_the_page_says_the_echo_cannot_work() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = render_check(&page, &cabinet()).html;
        // Every <noscript> on the page, because `body_prefix` emits one of its
        // own (the meta refresh) before this one.
        let blocks: Vec<&str> = html
            .split("<noscript")
            .skip(1)
            .filter_map(|rest| rest.split("</noscript>").next())
            .collect();
        assert!(!blocks.is_empty(), "no noscript block at all: {html}");
        assert!(
            blocks
                .iter()
                .any(|b| (b.contains("JavaScript") || b.contains("scripting"))
                    && b.to_lowercase().contains("light")),
            "one noscript block must name what is missing AND what will not              happen because of it: {blocks:?}"
        );
    }

    /// The nav reaches every sibling page, and marks this one.
    #[test]
    fn the_nav_reaches_every_sibling_page() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = render_check(&page, &cabinet()).html;
        for route in ["/", "/map", "/pads", "/devices", "/profiles", "/setup"] {
            assert!(
                html.contains(&format!(r#"href="{route}""#)),
                "the nav lost {route}: {html}"
            );
        }
        assert!(html.contains(r#"aria-current="page""#), "{html}");
    }

    /// **The two-attribute contract, both sides.**
    ///
    /// `data-slot` and `data-control` are the whole seam between the
    /// server-rendered chips and the client's live echo. They are raw payload
    /// values on purpose — a composed `chip-1-dpad-up` would be a string
    /// spelled in Rust and again in TypeScript. This asserts both halves still
    /// speak the same two names, which is the one thing a Rust test can check
    /// about the other language.
    #[test]
    fn the_client_looks_chips_up_by_the_two_attributes_the_seam_renders() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = rendered(&render_check(&page, &cabinet()).html);
        for attr in ["data-slot", "data-control"] {
            assert!(html.contains(attr), "the seam stopped rendering {attr}");
            assert!(
                CHECK_ISLAND_TS.contains(&format!("\"{attr}\"")),
                "CheckIsland.ts stopped rendering {attr}"
            );
            assert!(
                CHECK_TS.contains(attr),
                "check.ts stopped looking chips up by {attr}"
            );
        }
    }

    /// **The echo does not go through the list signal.**
    ///
    /// Rewriting a ~100-item list sixty times a second would rebuild ~100 DOM
    /// nodes sixty times a second on the phone this page is for. `paint`
    /// toggles classes instead, and this pins that: the frame handler must not
    /// call the roster applier.
    #[test]
    fn the_live_echo_never_rewrites_the_roster() {
        let frame_handler = CHECK_TS
            .split("function paint(")
            .nth(1)
            .expect("paint() exists")
            .split("\nfunction ")
            .next()
            .expect("...and ends");
        assert!(
            !frame_handler.contains("applyCheck"),
            "the per-frame path must not rewrite the roster list: {frame_handler}"
        );
        assert!(
            frame_handler.contains("classList"),
            "the echo is a class toggle: {frame_handler}"
        );
    }

    /// Loss is REPORTED. Both counters have a place on the page and a distinct
    /// sentence — "the panel is dead" and "you are pressing the wrong
    /// keyboard" are different findings, and a page that showed one number for
    /// both would merge them.
    #[test]
    fn both_loss_counters_have_their_own_sentence_on_the_page() {
        assert!(
            CHECK_ISLAND_TS.contains("lossLine"),
            "no dropped-frame line"
        );
        assert!(
            CHECK_ISLAND_TS.contains("offPanelLine"),
            "no off-panel line"
        );
        assert!(
            CHECK_TS.contains("frame.dropped") && CHECK_TS.contains("frame.off_panel"),
            "the client must read BOTH counters off the frame"
        );
        assert!(
            CHECK_TS.contains("wrong keyboard") || CHECK_TS.contains("pressing the panel"),
            "the off-panel sentence must say what it means"
        );
    }

    /// The slot-table contract this seam depends on, both directions: every
    /// name the seam injects is one the island RENDERS, and every scalar the
    /// island renders is one the seam injects. Read
    /// [`crate::render::assert_island_slot_contract`] before touching this —
    /// "the slot exists" is not the check.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load("/check").unwrap();
        let module = page.module();
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&CheckPayload::default());
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
            [LIST_SLOT_KEYS, LIST_SLOT_CHIPS],
            "list slot names drifted between the compiler/CheckIsland.ts and the              LIST_SLOT_* constants; slots: {names:?}"
        );
        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> = show_values(&CheckPayload::default())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between CheckIsland.ts and show_values()"
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
                list_values(&CheckPayload::default())
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

    /// The payload the page embeds is the payload `/api/check` serves — one
    /// struct, one serializer, so the poller cannot disagree with the paint.
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let page = EmbeddedPage::load("/check").unwrap();
        let html = render_check(&page, &cabinet()).html;
        let start = html
            .find("<script id=\"__ksx-payload\"")
            .expect("the payload block");
        let body = html[start..]
            .split_once('>')
            .expect("an open tag")
            .1
            .split("</script>")
            .next()
            .expect("a close tag");
        let parsed: CheckPayload =
            serde_json::from_str(body).expect("the embedded block IS a CheckPayload");
        assert_eq!(parsed, cabinet());
    }
}
