//! The /pads render seam: embedded FMIR + one [`PadsPayload`] → HTML.
//!
//! Same four-part slot seam as [`crate::render`] and [`crate::render_map`] —
//! scalars, lists, shows, and the layout test that pins all three — so read
//! `render.rs`'s module docs for the protocol. What is worth writing down here
//! is what this seam deliberately does NOT do.
//!
//! **Nothing on this page is decided here.** The pad summary, the XInput
//! ceiling paragraph, the prune plan's prose and its `kind`, and the label on
//! every single `<option>` all arrive composed from
//! `ksx_api::MachineSource::pads_view`. This file turns them into slot values
//! and picks which of two statically-styled variants renders. It owns four
//! format helpers ([`bus_line`], [`owners_line`], [`elevation_line`],
//! [`confirm_line`]) and they are wording over facts the provider supplied,
//! the same class of thing `render.rs`'s `pads_summary` is — mirrored in
//! `studio-ui/src/PadsIsland.ts` and pinned by the unit tests below.
//!
//! The reason that boundary is drawn hard here rather than loosely: the number
//! four is not a fact about a web page. `ksx pads --count 8 --persona xbox360`
//! plugs eight pads and Windows hands four of them to nobody (open task #16),
//! and the page's whole job is to say so BEFORE the button is pressed. A page
//! that worked that out for itself would be a second answer to a question the
//! CLI already answers, and docs/SURFACES.md §1 names that exact failure.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{art_for, body_prefix, with_icon_links, EmbeddedPage, PERSONALITY_CSS};
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
const SHOW_COUNT: usize = 13;

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

/// The ViGEmBus devnode, or why there is not one.
///
/// `None` is not an error and must not read like one: a machine that never
/// installed ViGEmBus has no devnode and also has no pads, which is a
/// perfectly healthy state for a cabinet that has not been set up yet.
fn bus_line(bus: Option<&str>) -> String {
    match bus {
        Some(id) => id.to_owned(),
        None => "none present — ViGEmBus is not installed, or its devnode has gone".to_owned(),
    }
}

/// Who is holding the pads, with the heuristic's limit stated rather than
/// hidden — the collector matches known splitter process NAMES, so a
/// third-party ViGEm feeder is invisible to it and "no owner" would be a
/// stronger claim than the evidence supports.
fn owners_line(owners: &[String]) -> String {
    if owners.is_empty() {
        return "no known splitter process is alive — a third-party ViGEm feeder would be \
                invisible here"
            .to_owned();
    }
    owners.join(", ")
}

/// Whether the prune can work from here, said before the click rather than
/// after the refusal.
fn elevation_line(elevated: Option<bool>) -> String {
    match elevated {
        Some(true) => "ksx Studio is running elevated — it can restart the bus itself.".to_owned(),
        Some(false) => "ksx Studio is NOT running elevated, and ksx never self-elevates — this \
                        prune will be refused. Run the command below from an elevated prompt \
                        instead."
            .to_owned(),
        None => "whether ksx Studio is elevated could not be determined; if the prune is \
                 refused, run the command below from an elevated prompt."
            .to_owned(),
    }
}

/// The confirm panel's lead. Says "every pad listed here", because a bus
/// restart cannot remove one pad and keep the others and a user about to press
/// this needs that to be the sentence, not a footnote.
fn confirm_line(count: usize) -> String {
    format!(
        "This removes {count} pad(s) by restarting the ViGEmBus devnode. Every pad listed here \
         goes, at once:"
    )
}

/// Scalar slot values, keyed by the signal names in PadsIsland.ts.
fn scalar_slots(payload: &PadsPayload) -> serde_json::Value {
    let view = &payload.pads;
    serde_json::json!({
        "generatedAt": view.generated_at,
        "padsSummary": view.summary,
        "busLine": bus_line(view.bus_instance_id.as_deref()),
        "ownersLine": owners_line(&view.owners),
        "xinputLine": view.xinput_line,
        "sessionLine": payload.session.line,
        "pruneDetail": view.prune.detail,
        "pruneCommand": view.prune.command.clone().unwrap_or_default(),
        "elevationLine": elevation_line(view.elevated),
        "spawnNote": view.spawn.note,
        "spawnRefusal": view.spawn.refused.clone().unwrap_or_default(),
        "confirmLine": confirm_line(view.prune.count),
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
    let ghosts = SlotValue::array(
        (view.pads.len()..PAD_TILE_FLOOR)
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
        ("show:pruneIdle", !restart),
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
    // flash shows for one cycle and then the URL is clean, and — the reason
    // this matters more here than on the other pages — `?confirm=1` is DROPPED
    // by the same rule, so a browser left open on the armed prune panel
    // disarms itself instead of sitting on a loaded destructive button.
    let prefix = body_prefix(payload, "/pads");
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
    use crate::render::assert_icon_links_in_head;

    fn view() -> ksx_api::PadsView {
        ksx_api::PadsView {
            generated_at: "2026-08-07 12:00:00 UTC".into(),
            summary: "2 virtual pads on the ViGEm bus:".into(),
            bus_instance_id: Some(r"ROOT\SYSTEM\0002".into()),
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
            session_running: false,
            xinput_ceiling: 4,
            xinput_in_use: 1,
            xinput_line: "Windows exposes exactly 4 XInput slots".into(),
            elevated: Some(false),
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
                        label: "8 pads — only 3 readable, 5 invisible to games (XInput)".into(),
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
        assert_icon_links_in_head("/pads", &out.html);
        assert!(
            out.html.contains(
                r#"<noscript><meta http-equiv="refresh" content="5; url=/pads"></noscript>"#
            ),
            "{}",
            out.html
        );
    }

    /// **Task #16, on the page.** The whole reason this page exists in the
    /// shape it does: the consequence of asking for more pads than Windows can
    /// expose has to be readable BEFORE the button is pressed. Two channels
    /// carry it, and both are rendered server-side so a browser with no
    /// JavaScript gets them too.
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
        // 2. The option itself. A dropdown entry reading "8" is a lie on a
        //    four-slot machine; this is the same click with the consequence
        //    attached to it.
        assert!(
            out.html
                .contains("8 pads — only 3 readable, 5 invisible to games"),
            "the count option must carry its own warning: {}",
            out.html
        );
        // …and the submit is a real form, so the warning is not JS-only.
        assert!(out.html.contains(r#"action="/pads/spawn""#), "{}", out.html);
        assert!(out.html.contains(r#"name="count""#), "{}", out.html);
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

    /// Arming is stateless: `confirm` cannot survive into the payload the API
    /// serves, so a poll can never re-arm a panel the user walked away from.
    /// (The route enforces it; this pins the payload half.)
    #[test]
    fn the_confirm_flag_defaults_to_disarmed() {
        assert!(!PadsPayload::default().confirm);
    }

    /// The provider could not answer. An empty pad list reads as "your bus is
    /// clean", which is the one thing this page must never say by accident.
    #[test]
    fn an_unavailable_provider_says_so_instead_of_showing_an_empty_bus() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(
            &page,
            &PadsPayload {
                pads: ksx_api::PadsView::default(),
                session: idle_session(),
                confirm: false,
                unavailable: Some("the pad collection panicked".into()),
                flash: None,
            },
        );
        assert!(
            out.html.contains("ksx could not read the ViGEm bus"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("the pad collection panicked"),
            "{}",
            out.html
        );
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

    /// The nav must reach every screen from this one, or a page is a dead end.
    #[test]
    fn the_nav_reaches_the_other_screens() {
        let page = EmbeddedPage::load("/pads").unwrap();
        let out = render_pads(&page, &payload());
        assert!(out.html.contains(r#"href="/""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/map""#), "{}", out.html);
        assert!(
            out.html.contains(r#"aria-current="page""#),
            "the current screen must be marked: {}",
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

    /// The four format helpers this file owns — the wording mirrored in
    /// PadsIsland.ts. Each one has a case that must not read like an error.
    #[test]
    fn the_format_helpers_stay_honest_about_absence() {
        assert_eq!(bus_line(Some("ROOT\\X")), "ROOT\\X");
        assert!(bus_line(None).contains("not installed"));

        assert_eq!(owners_line(&["a".to_owned(), "b".to_owned()]), "a, b");
        assert!(
            owners_line(&[]).contains("no known splitter"),
            "never claim there is no owner — the collector matches process NAMES"
        );

        assert!(elevation_line(Some(true)).contains("is running elevated"));
        assert!(elevation_line(Some(false)).contains("NOT running elevated"));
        assert!(elevation_line(None).contains("could not be determined"));

        assert!(confirm_line(15).contains("15 pad(s)"));
        assert!(confirm_line(15).contains("at once"));
    }
}
