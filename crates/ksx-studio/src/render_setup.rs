//! The `/setup` render seam: embedded FMIR + per-request [`SetupPayload`] →
//! HTML, with the same data emitted twice (SSR slots for first paint, the
//! source payload for hydration). Read `render.rs`'s module docs for the
//! mechanism — everything below is what is particular to this page.
//!
//! # The config comes first, and it has exactly two verbs
//!
//! The owner's words, and they are the whole brief: *"the config should be
//! first"* and *"why do we show the config root, we talked about this being
//! seamless — only import and export etc."*
//!
//! So the first card on this page is the configuration, and the two things you
//! do to a configuration are **Export** (download one file) and **Import**
//! (paste one back). A filesystem path appears exactly once, in
//! [`smallprint`-styled] text at the bottom of the inventory, where a bug
//! report can quote it. It is never a control, never the subject of a sentence
//! and never the thing you operate.
//!
//! [`smallprint`-styled]: ../../../studio-ui/src/studio.css
//!
//! # The checklist is rendered, not computed
//!
//! `stepRows` comes straight off `ksx_api::SetupView::steps`, which ksx-app's
//! `onboard::plan_steps` decides — pure, and unit-tested there against every
//! combination of three counts. docs/SURFACES.md §1: a surface may not hold
//! logic another surface would need, and "which step is next" is exactly that
//! (the cabinet's own first-run screen will want the same answer). What this
//! file does with the steps is pick a CSS class and a number, which is
//! presentation and nothing else.
//!
//! # …and so is every sentence, and every show flag
//!
//! Same rule, applied to the rest of the page after a review found six display
//! strings and the whole show-flag algebra hand-mirrored between this file and
//! `SetupIsland.ts`. They are composed ONCE now, in `snapshot.rs`
//! ([`crate::snapshot::SetupLines`] and [`crate::snapshot::SetupFlags`]),
//! travel in the payload, and both the SSR paint and the client's two-second
//! poll read the same fields. Two implementations of one sentence in two
//! languages drift silently; there is now one.
//!
//! The slot MENU obeys the same rule and is the sharpest case:
//! `SetupView::max_slots` carries `ksx_core::MAX_SLOTS`, so the form offers the
//! slots the daemon accepts rather than a number this file picked.
//!
//! # A refused read is not an empty machine
//!
//! `SetupSnapshot::available` gates the entire inventory and the entire
//! checklist, not just the headline. A provider that refused knows nothing
//! about this machine: "I could not read this" and "there is nothing here" are
//! different sentences, and the user acts on them differently — the second one
//! is advice ("import a config", "go and name your board") and it would be the
//! wrong advice. `show:stepsKnown` is the checklist's gate, every `has*`/`no*`
//! inventory flag folds the same fact in, and `show:setupDown` /
//! `show:stepsUnknown` render the refusal where the claims would have been.
//!
//! # MERGE DEPENDENCY: step 1 links to `/devices`, which this branch does not serve
//!
//! Stated here rather than left implicit. The board step is a link to the
//! devices screen (task #22, `wt/ui-devices`) because a board is picked and
//! named in ONE place; this branch adds no such route, so on its own the link
//! is a 404. It must land in the same merge as the devices page. Until it does,
//! the step-1 card names `ksx device pick` beside the button — the same shape
//! step 3 uses for a missing listener — so the step is never a dead end.
//!
//! # Three steps, three backend verbs
//!
//! | step | verb | route |
//! |---|---|---|
//! | find the board, name it | — (the devices screen owns it) | link to `/devices` |
//! | wire a slot | `ControlSource::assign_slot` | `POST /setup/slot` |
//! | prove a button lights | `ControlSource::learn_start` / `learn_poll` | `POST /setup/prove` |
//!
//! Each is resumable because none of them is a wizard: every one reads the
//! configuration as it stands and writes one thing. Abandon the page half way
//! and what landed is a named board, or a wired slot — both of which are
//! complete, valid configurations on their own. There is no half-written state
//! to resume INTO, which is the only version of "resumable" worth having.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{body_prefix, daemon_command, with_icon_links, EmbeddedPage, PERSONALITY_CSS};
use crate::snapshot::SetupPayload;

/// List array slot names, binding-derived (compiler 0.2.0+). The layout test
/// pins this exact set, in this order.
const LIST_SLOT_STEPS: &str = "list:stepRows:array";
const LIST_SLOT_SLOT_OPTIONS: &str = "list:slotOptions:array";
const LIST_SLOT_PRESET_OPTIONS: &str = "list:presetOptions:array";
const LIST_SLOT_PROFILE_OPTIONS: &str = "list:profileOptions:array";
const LIST_SLOT_DEVICES: &str = "list:deviceRows:array";
const LIST_SLOT_SLOTS: &str = "list:slotRows:array";
const LIST_SLOT_NOTES: &str = "list:noteRows:array";

/// How many `createShow` pairs this page has; pinned by the layout test
/// alongside every name.
const SETUP_SHOW_COUNT: usize = 22;

#[cfg(test)]
const ISLAND_COMPONENT: &str = "SetupIsland";

/// Bare-named slots this page renders and the seam deliberately never fills.
/// EMPTY: every signal `SetupIsland.ts` binds to the DOM gets a server value on
/// every request, including the learner's — which is what makes step 3 work
/// with JavaScript switched off (the `<noscript>` refresh re-reads
/// `learn_poll` and repaints the key).
#[cfg(test)]
const SETUP_CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots this page compiles to. EMPTY, and that is
/// the claim: every attribute value and every text child on this page is
/// either a named signal binding, a list member read, or static markup.
#[cfg(test)]
const SETUP_ANONYMOUS_SLOTS: [&str; 0] = [];

/// Scalar slot values, keyed by the signal names in `SetupIsland.ts`.
///
/// Every SENTENCE here comes off [`SetupPayload::lines`], which
/// `snapshot.rs` composed — this seam picks a slot name and a CSS class and
/// derives nothing. `SetupIsland.ts` reads the identical fields off the same
/// payload, so the SSR paint and the two-second poll cannot disagree: there is
/// one implementation of the wording, not two.
fn scalar_slots(payload: &SetupPayload, flash: Option<&str>) -> serde_json::Value {
    let view = &payload.setup.view;
    let lines = &payload.lines;
    serde_json::json!({
        "generatedAt": if view.generated_at.is_empty() { "(no snapshot)" } else { &view.generated_at },
        "sessionLine": payload.session.line,
        "flashLine": flash.unwrap_or(""),
        "daemonCmd": daemon_command(&payload.session),
        "configLine": lines.config,
        // SUPPORT DETAIL, and the page renders it as such. It is here because a
        // bug report needs it, not because anyone is meant to go there.
        "configRoot": if view.config_root.is_empty() { "(unknown)" } else { &view.config_root },
        "boardsSummary": lines.boards,
        "slotsSummary": lines.slots,
        "libraryLine": lines.library,
        "exportLine": lines.export,
        "proveLine": lines.prove,
        "proveKey": payload.learn.key.clone().unwrap_or_default(),
        "setupSource": payload.setup.source,
        "wireBlocked": lines.wire_blocked,
        "proveBlocked": lines.prove_blocked,
        "wireWarning": lines.wire_warning,
    })
}

/// The list array payloads, keyed by their (unique) slot names.
///
/// VERBATIM from [`crate::snapshot::SetupRows`] — this seam formats nothing.
/// The row sentences ("Slot 3 — IPAC P1", "interception · usb:…") and the
/// slot-menu ceiling used to be composed here and AGAIN in `SetupIsland.ts`,
/// which is docs/SURFACES.md §1 drift with a runtime cost: the two copies
/// could disagree between the SSR paint and the first poll. Now both read the
/// rows `SetupPayload::composed` filled, so they cannot.
fn list_values(payload: &SetupPayload) -> [(&'static str, SlotValue); 7] {
    let rows = &payload.rows;

    let steps = SlotValue::array(
        rows.steps
            .iter()
            .map(|step| {
                SlotValue::object(vec![
                    ("badge".to_owned(), SlotValue::Text(step.badge.clone())),
                    ("title".to_owned(), SlotValue::Text(step.title.clone())),
                    ("detail".to_owned(), SlotValue::Text(step.detail.clone())),
                    ("cls".to_owned(), SlotValue::Text(step.cls.clone())),
                ])
            })
            .collect(),
    );

    let slot_options = SlotValue::array(
        rows.slot_options
            .iter()
            .map(|option| {
                SlotValue::object(vec![
                    ("value".to_owned(), SlotValue::Text(option.value.clone())),
                    ("label".to_owned(), SlotValue::Text(option.label.clone())),
                ])
            })
            .collect(),
    );

    let text_rows = |rows: &[crate::snapshot::SetupTextRowView]| {
        SlotValue::array(
            rows.iter()
                .map(|row| {
                    SlotValue::object(vec![("text".to_owned(), SlotValue::Text(row.text.clone()))])
                })
                .collect(),
        )
    };
    let presets = text_rows(&rows.preset_options);
    let profiles = text_rows(&rows.profile_options);
    let notes = text_rows(&rows.notes);

    let pair_rows = |rows: &[crate::snapshot::SetupPairRowView]| {
        SlotValue::array(
            rows.iter()
                .map(|row| {
                    SlotValue::object(vec![
                        ("title".to_owned(), SlotValue::Text(row.title.clone())),
                        ("detail".to_owned(), SlotValue::Text(row.detail.clone())),
                    ])
                })
                .collect(),
        )
    };
    let devices = pair_rows(&rows.devices);
    let slots = pair_rows(&rows.slots);

    [
        (LIST_SLOT_STEPS, steps),
        (LIST_SLOT_SLOT_OPTIONS, slot_options),
        (LIST_SLOT_PRESET_OPTIONS, presets),
        (LIST_SLOT_PROFILE_OPTIONS, profiles),
        (LIST_SLOT_DEVICES, devices),
        (LIST_SLOT_SLOTS, slots),
        (LIST_SLOT_NOTES, notes),
    ]
}

/// Every show slot on this page, BY NAME, mapped to the boolean
/// [`crate::snapshot::SetupFlags`] already decided.
///
/// The mapping is the only thing here: slot name → field. Nothing on this line
/// is a decision, which is the point — `SetupIsland.ts` assigns the SAME
/// fields into its signals, so the learner partition and the "is this readable"
/// gate cannot be true on one side of the seam and false on the other.
///
/// The two flash booleans are the exception and are computed here, because a
/// flash is not a fact about the machine: it arrives on the query string, shows
/// once, and the client clears it on a timer.
fn show_values(
    payload: &SetupPayload,
    flash: Option<&str>,
) -> [(&'static str, bool); SETUP_SHOW_COUNT] {
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let f = &payload.flags;

    [
        ("show:pillRunning", f.pill_running),
        ("show:pillIdle", f.pill_idle),
        ("show:pillDown", f.pill_down),
        ("show:noDaemon", f.no_daemon),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        ("show:setupDown", f.setup_down),
        // Two slot names, one fact. A signal used in two places on a page
        // compiles to two slots with the SAME name, and this seam can only
        // fill the first — so the checklist's gate gets its own signal in the
        // island rather than reusing the card's.
        ("show:stepsKnown", f.setup_known),
        ("show:stepsUnknown", f.setup_down),
        ("show:firstRun", f.first_run),
        ("show:configured", f.configured),
        ("show:canWire", f.can_wire),
        ("show:cannotWire", f.cannot_wire),
        ("show:proveDown", f.prove_down),
        ("show:proveListening", f.prove_listening),
        ("show:proveHit", f.prove_hit),
        ("show:proveIdle", f.prove_idle),
        ("show:hasBoards", f.has_boards),
        ("show:noBoards", f.no_boards),
        ("show:hasSlots", f.has_slots),
        ("show:noSlots", f.no_slots),
        ("show:hasNotes", f.has_notes),
    ]
}

/// Slot ids of every slot named `name`, in slot-table (== document) order.
/// Identical to `render.rs`'s; copied for the same reason `render_map.rs`
/// copies it — the alternative is exporting a helper whose only caller is a
/// sibling page module.
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
fn build_slots(module: &IrModule, payload: &SetupPayload, flash: Option<&str>) -> SlotData {
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

/// Render `/setup` for one payload. `flash` is the outcome of the POST that
/// redirected here — the same post-redirect-get channel the other two pages
/// use.
pub(crate) fn render_setup(
    page: &EmbeddedPage,
    payload: &SetupPayload,
    flash: Option<&str>,
) -> PageOutput {
    // Re-derive the sentences and the show booleans from the facts in hand.
    // Both halves are derived data (snapshot.rs), and doing it HERE is what
    // makes it impossible to render a payload whose wording was composed
    // against different facts — including one a test mutated field by field.
    let payload = &payload.clone().composed();
    let slots = build_slots(&page.module, payload, flash);
    let prefix = body_prefix(payload, "/setup");
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — setup",
        route_pattern: "/setup",
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
    use crate::control::{LearnView, SessionView};
    use crate::render::{assert_complete_head, assert_island_slot_contract, payload_json};
    use crate::snapshot::SetupSnapshot;
    use ksx_api::{setup_states, setup_steps, SetupDeviceRow, SetupSlotRow, SetupStep, SetupView};

    fn steps() -> Vec<SetupStep> {
        vec![
            SetupStep {
                id: setup_steps::BOARD.into(),
                title: "Find your board and name it".into(),
                detail: "One board is named.".into(),
                state: setup_states::DONE.into(),
            },
            SetupStep {
                id: setup_steps::SLOT.into(),
                title: "Wire a slot".into(),
                detail: "No slot is wired yet.".into(),
                state: setup_states::NOW.into(),
            },
            SetupStep {
                id: setup_steps::PROVE.into(),
                title: "Press a button and watch it land".into(),
                detail: "Once a board is named and a slot is wired…".into(),
                state: setup_states::LATER.into(),
            },
        ]
    }

    fn configured_view() -> SetupView {
        SetupView {
            generated_at: "2026-08-07 12:00:00 UTC".into(),
            config_root: "C:\\cfg\\ksx".into(),
            config_exists: true,
            devices: vec![SetupDeviceRow {
                alias: "P1 board".into(),
                id: "usb:d209:0430:00".into(),
                backend: "interception".into(),
            }],
            slots: vec![SetupSlotRow {
                number: 1,
                device: "P1 board".into(),
                preset: "IPAC P1".into(),
                persona: "Xbox 360 pad".into(),
                source: "config.toml".into(),
            }],
            presets: vec!["IPAC P1".into(), "default".into()],
            profiles: vec!["Street Fighter".into()],
            steps: steps(),
            notes: Vec::new(),
            ..SetupView::default()
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

    fn idle_learn() -> LearnView {
        LearnView {
            ok: true,
            state: "idle".into(),
            remaining_ms: None,
            device: None,
            key: None,
            error: None,
        }
    }

    /// A daemon with a session up — the state the pad-bounce warning is true
    /// of.
    fn running_session() -> SessionView {
        SessionView {
            reachable: true,
            running: true,
            line: "running — 4 pad(s)".into(),
            profile: Some("Street Fighter".into()),
        }
    }

    fn configured() -> SetupPayload {
        SetupPayload {
            setup: SetupSnapshot::ready(configured_view()),
            session: idle_session(),
            learn: idle_learn(),
            flash: None,
            ..SetupPayload::default()
        }
        .composed()
    }

    fn fresh() -> SetupPayload {
        SetupPayload {
            setup: SetupSnapshot::ready(SetupView {
                generated_at: "2026-08-07 12:00:00 UTC".into(),
                config_root: "C:\\cfg\\ksx".into(),
                config_exists: false,
                steps: vec![SetupStep {
                    id: setup_steps::BOARD.into(),
                    title: "Find your board and name it".into(),
                    detail: "Nothing is named yet.".into(),
                    state: setup_states::NOW.into(),
                }],
                ..SetupView::default()
            }),
            session: idle_session(),
            learn: idle_learn(),
            flash: None,
            ..SetupPayload::default()
        }
        .composed()
    }

    /// The state this whole page's honesty rule is about: the machine provider
    /// REFUSED. Nothing was read, so nothing may be claimed.
    fn refused() -> SetupPayload {
        SetupPayload {
            setup: SetupSnapshot::unavailable(
                "reading the first-run state is not available on this surface",
            ),
            session: idle_session(),
            learn: idle_learn(),
            flash: None,
            ..SetupPayload::default()
        }
        .composed()
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load("/setup").expect("embedded setup page must load");
        assert_eq!(page.module().header.version, 2);
    }

    /// The slot-table contract: every scalar the seam injects exists, the list
    /// names are exactly the `LIST_SLOT_*` set in order, the `show:` names are
    /// exactly what [`show_values`] addresses, and — the assertion that
    /// matters — every injected name is one the ISLAND RENDERS. See
    /// `render.rs::assert_island_slot_contract` for the evening that cost.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let module = page.module();
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let empty = SetupPayload::default();
        let scalars = scalar_slots(&empty, None);
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
                LIST_SLOT_STEPS,
                LIST_SLOT_SLOT_OPTIONS,
                LIST_SLOT_PRESET_OPTIONS,
                LIST_SLOT_PROFILE_OPTIONS,
                LIST_SLOT_DEVICES,
                LIST_SLOT_SLOTS,
                LIST_SLOT_NOTES,
            ],
            "list slot names drifted between SetupIsland.ts and the LIST_SLOT_* \
             constants; slots: {names:?}"
        );

        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> = show_values(&empty, None)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between SetupIsland.ts and show_values()"
        );
        assert_eq!(
            ir_shows.len(),
            SETUP_SHOW_COUNT,
            "SETUP_SHOW_COUNT is stale; slots: {names:?}"
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
            .chain(list_values(&empty).iter().map(|(n, _)| *n))
            .chain(seam_shows.iter().copied())
            .collect();
        assert_island_slot_contract(
            module,
            &injected,
            &SETUP_CLIENT_ONLY_SLOTS,
            &SETUP_ANONYMOUS_SLOTS,
        );
    }

    /// The brief, as a test. A path may appear on this page, but never as the
    /// interface: not in a heading, not as a link, not as the label of a
    /// control — only inside the support line at the bottom.
    #[test]
    fn the_config_root_is_small_print_and_never_a_control() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), None);

        // It IS there — a bug report needs it.
        assert!(out.html.contains("C:\\cfg\\ksx"), "{}", out.html);
        // …inside the support line, and after both verbs.
        let at = out.html.find("C:\\cfg\\ksx").expect("config root rendered");
        let smallprint = out
            .html
            .find(r#"class="smallprint""#)
            .expect("the support line must exist");
        assert!(
            smallprint < at,
            "the config root must live inside the small print, not above it"
        );
        let export = out
            .html
            .find(r#"href="/setup/export.json""#)
            .expect("Export is one of the two verbs");
        let import = out
            .html
            .find(r#"action="/setup/import""#)
            .expect("Import is the other");
        assert!(
            export < at && import < at,
            "both verbs must come before the path: export at {export}, import at \
             {import}, path at {at}"
        );
        // And it is never an anchor, a form action or an input value.
        for hostile in [
            r#"href="C:\cfg"#,
            r#"action="C:\cfg"#,
            r#"value="C:\cfg"#,
            r#"<h2>C:\cfg"#,
        ] {
            assert!(!out.html.contains(hostile), "{hostile} in {}", out.html);
        }
    }

    /// The two verbs are on the page, reachable without JavaScript, and Export
    /// is a GET link while Import is a POST form — because one reads and one
    /// writes, and `guard.rs` decides what to police by METHOD.
    #[test]
    fn export_is_a_link_and_import_is_a_post_form() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), None);
        assert!(
            out.html.contains(r#"href="/setup/export.json""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("download"), "{}", out.html);
        assert!(
            out.html.contains(r#"method="post" action="/setup/import""#)
                || out.html.contains(r#"action="/setup/import""#),
            "{}",
            out.html
        );
        // Consent: the write box is UNTICKED in the markup. A page that shipped
        // it checked would turn a dry run into a rewrite of someone's cabinet.
        let form = out
            .html
            .split(r#"action="/setup/import""#)
            .nth(1)
            .expect("the import form");
        let form = &form[..form.find("</form>").expect("closed form")];
        assert!(form.contains(r#"name="apply""#), "{form}");
        assert!(
            !form.contains("checked"),
            "the apply box must ship unticked: {form}"
        );
        assert!(form.contains(r#"name="document""#), "{form}");
    }

    /// A fresh machine reads as a fresh machine: the checklist says find the
    /// board, the inventory says nothing is there, and the page still offers
    /// Import — which is the fastest route out of exactly this state.
    #[test]
    fn a_machine_with_no_config_is_told_so_and_offered_a_way_in() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &fresh(), None);
        assert!(
            out.html
                .contains("There is no configuration on this machine yet."),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("Find your board and name it"),
            "{}",
            out.html
        );
        assert!(out.html.contains(r#"class="step now""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/devices""#), "{}", out.html);
        assert!(out.html.contains("No board has a name yet"), "{}", out.html);
        assert!(
            out.html.contains(r#"action="/setup/import""#),
            "{}",
            out.html
        );
    }

    /// The checklist is the BACKEND's decision, rendered. Change the state a
    /// step arrives with and the class changes; nothing here re-derives it.
    #[test]
    fn step_state_comes_from_the_backend_not_from_this_seam() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), None);
        assert!(out.html.contains(r#"class="step done""#), "{}", out.html);
        assert!(out.html.contains(r#"class="step now""#), "{}", out.html);
        assert!(out.html.contains(r#"class="step later""#), "{}", out.html);

        // …and a state this build has never heard of still renders, as itself,
        // rather than being silently coerced to something that looks fine.
        let mut payload = configured();
        payload.setup.view.steps[0].state = "quarantined".to_owned();
        let out = render_setup(&page, &payload, None);
        assert!(
            out.html.contains(r#"class="step quarantined""#),
            "{}",
            out.html
        );
    }

    /// Wiring a slot is one backend verb, and against a RUNNING session it
    /// bounces the pads. The warning is on the page before the click, not in
    /// the flash after it — and it is about the session there actually is.
    ///
    /// The second half fails against the version that shipped: the warning was
    /// unconditional markup, and the form is offered whenever the daemon is
    /// REACHABLE (not running), so an idle cabinet was told its four
    /// controllers were about to vanish by a write that replugs nothing.
    #[test]
    fn the_slot_form_warns_that_the_pads_replug_before_the_click() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let mut running = configured();
        running.session = running_session();
        let out = render_setup(&page, &running, None);
        assert!(out.html.contains(r#"action="/setup/slot""#), "{}", out.html);
        assert!(out.html.contains("REPLUGS the pads"), "{}", out.html);
        // The preset menu offers what is on disk.
        assert!(out.html.contains("IPAC P1"), "{}", out.html);
        assert!(out.html.contains("Slot 1"), "{}", out.html);
        // Where it lands is a choice, and config.toml is the default.
        assert!(
            out.html.contains("(this cabinet&#x27;s config)")
                || out.html.contains("(this cabinet's config)"),
            "{}",
            out.html
        );
        assert!(out.html.contains("Street Fighter"), "{}", out.html);

        // Reachable but idle: the form is still offered, and the sentence
        // above it must not claim a bounce that will not happen.
        let out = render_setup(&page, &configured(), None);
        assert!(out.html.contains(r#"action="/setup/slot""#), "{}", out.html);
        assert!(
            !out.html.contains("REPLUGS the pads"),
            "an idle daemon was warned its pads would replug: {}",
            out.html
        );
        assert!(out.html.contains("Nothing is running"), "{}", out.html);
    }

    /// The slot menu offers every slot the BACKEND accepts — the ceiling it
    /// serves on `SetupView::max_slots`, which is `ksx_core::MAX_SLOTS`.
    ///
    /// This is the test that fails against the shipped version. That one held
    /// `const SLOT_CHOICES: u8 = 8` here and again in `SetupIsland.ts`, and
    /// guarded it with `assert!(SLOT_CHOICES <= MAX_SLOTS)` — an assertion that
    /// can only ever catch an OVER-offer, so it stayed green forever while the
    /// page hid slots 9-16 that `ksx slot assign` accepts and that this page's
    /// own inventory would list.
    ///
    /// `ksx-core` is a dev-dependency here on purpose (see Cargo.toml): the
    /// page knows no vocabulary at runtime, and the test reads the one true
    /// constant.
    #[test]
    fn the_slot_menu_offers_every_slot_the_backend_accepts() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let mut payload = configured();
        payload.session = running_session();
        let out = render_setup(&page, &payload, None);

        for n in 1..=ksx_core::MAX_SLOTS {
            assert!(
                out.html.contains(&format!(">Slot {n}<")),
                "the menu skips slot {n}, which the daemon accepts: {}",
                out.html
            );
        }
        // …and not one past it.
        let past = u16::from(ksx_core::MAX_SLOTS) + 1;
        assert!(
            !out.html.contains(&format!(">Slot {past}<")),
            "the menu offers slot {past}, which `slot-assign` refuses: {}",
            out.html
        );

        // The number is the BACKEND's, not this file's: serve a smaller
        // ceiling and the menu shrinks with it, with nothing recompiled.
        payload.setup.view.max_slots = 3;
        let out = render_setup(&page, &payload, None);
        assert!(out.html.contains(">Slot 3<"), "{}", out.html);
        assert!(
            !out.html.contains(">Slot 4<"),
            "the menu ignored the ceiling the backend served: {}",
            out.html
        );
    }

    /// A disabled control renders the reason that is TRUE — not a sentence
    /// covering every reason it could have been.
    ///
    /// The version that shipped had one static paragraph ("wiring a slot is a
    /// daemon write, and it needs a preset to point at. Start the daemon, and
    /// import or create a preset first") for a condition that is two facts
    /// ANDed. It fired for both single-cause states, so half the time it told
    /// the user to fix something that was not broken. This test renders each
    /// cause on its own and would fail against that version at the first
    /// `assert_ne!` — the two sentences were identical.
    #[test]
    fn a_disabled_wire_control_names_the_cause_that_actually_fired() {
        let page = EmbeddedPage::load("/setup").unwrap();

        // Daemon UP, no preset on disk.
        let out = render_setup(&page, &fresh(), None);
        assert!(
            !out.html.contains(r#"action="/setup/slot""#),
            "no live control with nothing to point it at: {}",
            out.html
        );
        assert!(
            out.html.contains("a slot points at a preset"),
            "{}",
            out.html
        );
        assert!(
            !out.html.to_lowercase().contains("start the daemon"),
            "told a user whose daemon is running to start it: {}",
            out.html
        );

        // Daemon DOWN, presets on disk.
        let mut dead = configured();
        dead.session = SessionView::unreachable("no daemon control channel");
        let out = render_setup(&page, &dead, None);
        assert!(
            !out.html.contains(r#"action="/setup/slot""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("no daemon is running"), "{}", out.html);
        assert!(
            !out.html.contains("preset new"),
            "told a user with \"IPAC P1\" on disk to go and create a preset: {}",
            out.html
        );

        // The two are different sentences, which is the whole finding.
        let a = disabled_reason(&render_setup(&page, &fresh(), None).html);
        let b = disabled_reason(&render_setup(&page, &dead, None).html);
        assert_ne!(a, b, "one sentence covered both causes");
    }

    /// The rendered text of the disabled-wire paragraph, for comparing two
    /// renders. `class="controls off"` is the wrapper the island gives it.
    fn disabled_reason(html: &str) -> String {
        let at = html
            .find(r#"class="controls off""#)
            .expect("a disabled control renders its reason");
        let tail = &html[at..];
        let warn = tail.find(r#"class="warn""#).expect("the reason paragraph");
        let rest = &tail[warn..];
        rest[..rest.find("</p>").expect("closed paragraph")].to_owned()
    }

    /// Step 3 works with JavaScript off: the learner's state is SSR'd, so the
    /// `<noscript>` refresh alone is enough to see a press land.
    #[test]
    fn the_learner_state_is_server_rendered_for_the_no_js_path() {
        let page = EmbeddedPage::load("/setup").unwrap();

        let out = render_setup(&page, &configured(), None);
        assert!(out.html.contains("Nothing is listening"), "{}", out.html);
        assert!(
            out.html.contains(r#"action="/setup/prove""#),
            "{}",
            out.html
        );

        let mut listening = configured();
        listening.learn.state = "listening".into();
        let out = render_setup(&page, &listening, None);
        assert!(
            out.html.contains("press any button on the panel"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"action="/setup/prove/cancel""#),
            "{}",
            out.html
        );

        let mut hit = configured();
        hit.learn.state = "hit".into();
        hit.learn.key = Some("LeftShift".into());
        hit.learn.device = Some("HID\\VID_D209&PID_0430".into());
        let out = render_setup(&page, &hit, None);
        assert!(out.html.contains("LeftShift"), "{}", out.html);
        assert!(
            out.html.contains("HID\\VID_D209&amp;PID_0430"),
            "{}",
            out.html
        );

        // …and the no-JS refresh that re-reads it is really emitted.
        assert!(
            out.html.contains(
                r#"<noscript><meta http-equiv="refresh" content="5; url=/setup"></noscript>"#
            ),
            "{}",
            out.html
        );
    }

    /// Exactly one of the learner's four panels renders, for every state the
    /// daemon can report — including one it cannot, which must fall to "idle"
    /// rather than to nothing.
    #[test]
    fn the_learner_panels_are_a_partition() {
        for (state, reachable) in [
            ("idle", true),
            ("listening", true),
            ("hit", true),
            ("unavailable", true),
            ("something-new", true),
            ("listening", false),
        ] {
            let mut payload = configured();
            payload.learn.state = state.into();
            payload.session.reachable = reachable;
            let payload = payload.composed();
            let shows = show_values(&payload, None);
            let live = shows
                .iter()
                .filter(|(name, on)| {
                    *on && matches!(
                        *name,
                        "show:proveIdle"
                            | "show:proveListening"
                            | "show:proveHit"
                            | "show:proveDown"
                    )
                })
                .count();
            assert_eq!(live, 1, "{state}/{reachable} lit {live} learner panels");
        }
    }

    /// **A refused read must not render as an assertion of absence.**
    ///
    /// The page a provider-refusal produces says it could not read this
    /// machine, and says nothing else about it. Not "no boards named yet", not
    /// "No slot is wired yet — step 2 above is how one gets wired", not "0
    /// preset(s) and 0 game profile(s) on disk", and above all not a promise
    /// about the contents of a download whose source it just failed to read.
    ///
    /// Every string below rendered on the version that shipped, where only the
    /// headline carried the `available` guard — so this test fails against it
    /// six times over. It also rendered the checklist card, heading and all,
    /// with zero step rows.
    #[test]
    fn a_refused_machine_provider_is_rendered_as_a_refusal() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &refused(), None);

        // It SAYS so, and it says why.
        assert!(
            out.html.contains("The configuration could not be read"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("not available on this surface"),
            "{}",
            out.html
        );

        // …and it asserts nothing about a machine nothing was read from.
        for claim in [
            "There is no configuration on this machine yet.",
            "no boards named yet",
            "No board has a name yet",
            "no slots wired yet",
            "No slot is wired yet",
            "0 preset(s) and 0 game profile(s) on disk.",
            "One JSON file: settings, boards, slots, 0 game profile(s) and 0 preset(s).",
            "Three steps, in order.",
        ] {
            assert!(
                !out.html.contains(claim),
                "a refused read still claimed {claim:?}: {}",
                out.html
            );
        }

        // The two states are visibly different pages, which is the point: an
        // empty machine that WAS read says all of the above, loudly.
        let empty_but_read = SetupPayload {
            setup: SetupSnapshot::ready(SetupView::default()),
            session: idle_session(),
            learn: idle_learn(),
            flash: None,
            ..SetupPayload::default()
        };
        let read = render_setup(&page, &empty_but_read, None);
        assert!(read.html.contains("no boards named yet"), "{}", read.html);
        assert!(read.html.contains("No slot is wired yet"), "{}", read.html);
        assert!(
            !read.html.contains("The configuration could not be read"),
            "a successful read of an empty machine is not a failure: {}",
            read.html
        );

        // And the checklist card is not rendered empty — the failure state has
        // its own words where the steps would be.
        assert!(
            out.html.contains("cannot say which step is next"),
            "{}",
            out.html
        );
        assert!(
            !out.html.contains(r#"class="step now""#),
            "a refused read rendered a checklist row: {}",
            out.html
        );
    }

    /// Every page links onward. The board step goes to `/devices` rather than
    /// duplicating it, and the nav reaches the other two screens.
    ///
    /// `/devices` is task #22 and is NOT served by this crate yet (see the
    /// module's MERGE DEPENDENCY note), so the step also names the shell verb
    /// that does the same job — the same shape step 3 uses for a missing
    /// listener. Without it, the first thing a fresh cabinet clicks is a bare
    /// 404 with no way back.
    #[test]
    fn the_page_links_onward_instead_of_duplicating() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), None);
        assert!(out.html.contains(r#"href="/devices""#), "{}", out.html);
        assert!(out.html.contains("ksx device pick"), "{}", out.html);
        assert!(out.html.contains(r#"href="/""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/map""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/pads""#), "{}", out.html);
        assert!(
            out.html.contains(r#"class="navlink on" href="/setup""#),
            "the current route must be marked: {}",
            out.html
        );
    }

    /// Ledger #5's contract: the payload block IS the /api/setup payload.
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let payload = configured();
        let json = payload_json(&payload);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("payload parse");
        assert_eq!(
            parsed,
            serde_json::to_value(&payload).unwrap(),
            "the payload block must be byte-compatible with /api/setup"
        );
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &payload, None);
        assert!(out.html.contains(&json), "{}", out.html);
    }

    /// The flash arrives from a query parameter — attacker-writable — and a
    /// pasted document is arbitrary text. Neither may become markup.
    #[test]
    fn hostile_text_is_escaped_not_injected() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), Some("<script>alert(1)</script>"));
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );

        let mut payload = configured();
        payload.setup.view.config_root = "<script>alert(2)</script>".into();
        payload.setup.view.devices[0].alias = "<img src=x onerror=alert(3)>".into();
        payload.setup.view.notes = vec!["</script><script>alert(4)</script>".into()];
        let out = render_setup(&page, &payload, None);
        assert!(
            !out.html.contains("<script>alert(2)</script>"),
            "{}",
            out.html
        );
        assert!(!out.html.contains("<img src=x onerror"), "{}", out.html);
        assert!(
            !out.html.contains("<script>alert(4)</script>"),
            "{}",
            out.html
        );
    }

    #[test]
    fn the_setup_head_is_complete() {
        let out = render_setup(&EmbeddedPage::load("/setup").unwrap(), &configured(), None);
        assert_complete_head("/setup", &out.html);
    }

    /// `SetupPayload::default()` is the DEFAULTED payload, which is a refused
    /// read (`available: false`) — so the page it renders is the refusal, not
    /// an empty cabinet. That distinction is the finding this page exists
    /// under; the default must not be the one payload that quietly breaks it.
    #[test]
    fn render_survives_an_empty_payload() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &SetupPayload::default(), None);
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        assert!(
            out.html.contains("The configuration could not be read"),
            "{}",
            out.html
        );
        assert!(!out.html.contains("no boards named yet"), "{}", out.html);
        assert!(!out.html.contains("no slots wired yet"), "{}", out.html);
    }

    /// **Every sentence on this page comes from the payload, so there is only
    /// one implementation of it.**
    ///
    /// This replaces a test whose docstring claimed to pin the Rust and the
    /// TypeScript together and whose body called six Rust functions — change
    /// any string in `SetupIsland.ts` and the whole workspace stayed green
    /// while the page flipped wording on the first two-second poll. There is
    /// nothing to pin now: `snapshot.rs` composes the lines, this seam injects
    /// them, and the island reads the SAME fields off the SAME payload.
    ///
    /// So what is checked here is the property that makes that true — the
    /// rendered HTML contains the payload's own lines, verbatim. A seam that
    /// went back to deriving one of them fails here.
    #[test]
    fn every_sentence_on_the_page_is_the_payload_s_own() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let mut payload = configured();
        payload.session = running_session();
        let payload = payload.composed();
        let out = render_setup(&page, &payload, None);

        let lines = &payload.lines;
        for (field, line) in [
            ("config", &lines.config),
            ("boards", &lines.boards),
            ("slots", &lines.slots),
            ("library", &lines.library),
            ("export", &lines.export),
            ("prove", &lines.prove),
            ("wire_warning", &lines.wire_warning),
        ] {
            assert!(
                !line.is_empty(),
                "{field} is empty for a configured machine"
            );
            assert!(
                out.html.contains(escape_text(line).as_str()),
                "the page did not render the payload's {field} line ({line:?}): {}",
                out.html
            );
        }
        // A wirable machine has no blocked reason to render, and says so by
        // saying nothing.
        assert_eq!(lines.wire_blocked, "");
        assert_eq!(lines.prove_blocked, "");

        // The payload the CLIENT reads carries them too — same struct, same
        // serializer, so the poll cannot repaint different words.
        let json = payload_json(&payload);
        assert!(json.contains(r#""config":"#), "{json}");
        assert!(out.html.contains(&json), "{}", out.html);
    }

    /// HTML-escape the few characters forma's text nodes escape, so a rendered
    /// sentence can be compared against its source.
    fn escape_text(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}
