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
const SETUP_SHOW_COUNT: usize = 20;

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

/// How many slot numbers the "wire a slot" form offers.
///
/// Eight, not `ksx_core::MAX_SLOTS`: eight is the player count this project has
/// actually driven (four XInput + four DS4), and a dropdown of sixteen is a
/// worse answer than a config file for the cabinet that needs sixteen. A test
/// below pins it at or below `MAX_SLOTS`, so the menu can never offer a slot
/// the daemon would refuse.
const SLOT_CHOICES: u8 = 8;

/// The configuration in one line — the loudest thing on the page, and the
/// first. Mirrored by `SetupIsland.ts` `configSummary`.
fn config_summary(payload: &SetupPayload) -> String {
    let view = &payload.setup.view;
    // A provider that refused knows nothing about this machine, so it must not
    // claim there is no configuration — that sentence is advice ("import one"),
    // and it would be the wrong advice.
    if !payload.setup.available {
        return "The configuration could not be read.".to_owned();
    }
    if !view.config_exists {
        return "There is no configuration on this machine yet.".to_owned();
    }
    format!(
        "Configured — {} board(s), {} slot(s), {} preset(s).",
        view.devices.len(),
        view.slots.len(),
        view.presets.len()
    )
}

fn boards_line(count: usize) -> String {
    match count {
        0 => "no boards named yet".to_owned(),
        1 => "1 board named:".to_owned(),
        n => format!("{n} boards named:"),
    }
}

fn slots_line(count: usize) -> String {
    match count {
        0 => "no slots wired yet".to_owned(),
        1 => "1 slot wired:".to_owned(),
        n => format!("{n} slots wired:"),
    }
}

fn library_line(payload: &SetupPayload) -> String {
    let view = &payload.setup.view;
    format!(
        "{} preset(s) and {} game profile(s) on disk.",
        view.presets.len(),
        view.profiles.len()
    )
}

fn export_line(payload: &SetupPayload) -> String {
    let view = &payload.setup.view;
    format!(
        "One JSON file: settings, boards, slots, {} game profile(s) and {} preset(s).",
        view.profiles.len(),
        view.presets.len()
    )
}

/// The learner, as the sentence step 3 reads. Mirrored by `SetupIsland.ts`
/// `learnLine`.
fn learn_line(learn: &crate::control::LearnView) -> String {
    match learn.state.as_str() {
        "listening" => "Listening — press any button on the panel now.".to_owned(),
        "hit" => match learn.device.as_deref() {
            Some(device) => format!("Seen, on {device}."),
            None => "Seen.".to_owned(),
        },
        "unavailable" => learn
            .error
            .clone()
            .unwrap_or_else(|| "the daemon's listener is not available".to_owned()),
        _ if !learn.ok => learn
            .error
            .clone()
            .unwrap_or_else(|| "the daemon's listener is not available".to_owned()),
        _ => {
            "Nothing is listening. Start the listener, then press a button on the panel.".to_owned()
        }
    }
}

/// Scalar slot values, keyed by the signal names in `SetupIsland.ts`.
fn scalar_slots(payload: &SetupPayload, flash: Option<&str>) -> serde_json::Value {
    let view = &payload.setup.view;
    serde_json::json!({
        "generatedAt": if view.generated_at.is_empty() { "(no snapshot)" } else { &view.generated_at },
        "sessionLine": payload.session.line,
        "flashLine": flash.unwrap_or(""),
        "daemonCmd": daemon_command(&payload.session),
        "configLine": config_summary(payload),
        // SUPPORT DETAIL, and the page renders it as such. It is here because a
        // bug report needs it, not because anyone is meant to go there.
        "configRoot": if view.config_root.is_empty() { "(unknown)" } else { &view.config_root },
        "boardsSummary": boards_line(view.devices.len()),
        "slotsSummary": slots_line(view.slots.len()),
        "libraryLine": library_line(payload),
        "exportLine": export_line(payload),
        "proveLine": learn_line(&payload.learn),
        "proveKey": payload.learn.key.clone().unwrap_or_default(),
        "setupSource": payload.setup.source,
    })
}

/// The list array payloads, keyed by their (unique) slot names.
fn list_values(payload: &SetupPayload) -> [(&'static str, SlotValue); 7] {
    let view = &payload.setup.view;

    let steps = SlotValue::array(
        view.steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                SlotValue::object(vec![
                    ("badge".to_owned(), SlotValue::Text(format!("{}", i + 1))),
                    ("title".to_owned(), SlotValue::Text(step.title.clone())),
                    ("detail".to_owned(), SlotValue::Text(step.detail.clone())),
                    // Presentation, from the backend's decision — this seam
                    // never decides which step is next.
                    (
                        "cls".to_owned(),
                        SlotValue::Text(format!("step {}", step.state)),
                    ),
                ])
            })
            .collect(),
    );

    let slot_options = SlotValue::array(
        (1..=SLOT_CHOICES)
            .map(|n| {
                SlotValue::object(vec![
                    ("value".to_owned(), SlotValue::Text(n.to_string())),
                    ("label".to_owned(), SlotValue::Text(format!("Slot {n}"))),
                ])
            })
            .collect(),
    );

    let presets = SlotValue::array(
        view.presets
            .iter()
            .map(|name| SlotValue::object(vec![("text".to_owned(), SlotValue::Text(name.clone()))]))
            .collect(),
    );

    let profiles = SlotValue::array(
        view.profiles
            .iter()
            .map(|title| {
                SlotValue::object(vec![("text".to_owned(), SlotValue::Text(title.clone()))])
            })
            .collect(),
    );

    let devices = SlotValue::array(
        view.devices
            .iter()
            .map(|device| {
                SlotValue::object(vec![
                    ("title".to_owned(), SlotValue::Text(device.alias.clone())),
                    (
                        "detail".to_owned(),
                        SlotValue::Text(format!("{} · {}", device.backend, device.id)),
                    ),
                ])
            })
            .collect(),
    );

    let slots = SlotValue::array(
        view.slots
            .iter()
            .map(|slot| {
                SlotValue::object(vec![
                    (
                        "title".to_owned(),
                        SlotValue::Text(format!("Slot {} — {}", slot.number, slot.preset)),
                    ),
                    (
                        "detail".to_owned(),
                        SlotValue::Text(format!(
                            "{} · {} · {}",
                            slot.device, slot.persona, slot.source
                        )),
                    ),
                ])
            })
            .collect(),
    );

    let notes = SlotValue::array(
        view.notes
            .iter()
            .map(|note| SlotValue::object(vec![("text".to_owned(), SlotValue::Text(note.clone()))]))
            .collect(),
    );

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

/// Every show slot on this page, BY NAME, with the boolean the server wants.
///
/// The learner's four states are a partition: exactly one of `proveDown`,
/// `proveListening`, `proveHit`, `proveIdle` is true, so the panel always
/// offers precisely one control and never a dead button as live.
fn show_values(
    payload: &SetupPayload,
    flash: Option<&str>,
) -> [(&'static str, bool); SETUP_SHOW_COUNT] {
    let view = &payload.setup.view;
    let session = &payload.session;
    let learn = &payload.learn;
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let available = payload.setup.available;

    // "Can this page write a slot?" is two facts, and both have to be true: a
    // daemon to take the write, and a preset for the slot to point AT. A menu
    // with no options and a live button is the shape that makes a user think
    // they did something.
    let wireable = session.reachable && !view.presets.is_empty();

    let listener_down = !session.reachable || learn.state == "unavailable";
    let listening = !listener_down && learn.state == "listening";
    let hit = !listener_down && learn.state == "hit";

    [
        ("show:pillRunning", session.reachable && session.running),
        ("show:pillIdle", session.reachable && !session.running),
        ("show:pillDown", !session.reachable),
        ("show:noDaemon", !session.reachable),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        ("show:setupDown", !available),
        ("show:firstRun", available && !view.config_exists),
        ("show:configured", available && view.config_exists),
        ("show:canWire", wireable),
        ("show:cannotWire", !wireable),
        ("show:proveDown", listener_down),
        ("show:proveListening", listening),
        ("show:proveHit", hit),
        ("show:proveIdle", !listener_down && !listening && !hit),
        ("show:hasBoards", !view.devices.is_empty()),
        ("show:noBoards", view.devices.is_empty()),
        ("show:hasSlots", !view.slots.is_empty()),
        ("show:noSlots", view.slots.is_empty()),
        ("show:hasNotes", !view.notes.is_empty()),
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
    use crate::render::{assert_icon_links_in_head, assert_island_slot_contract, payload_json};
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

    fn configured() -> SetupPayload {
        SetupPayload {
            setup: SetupSnapshot::ready(configured_view()),
            session: idle_session(),
            learn: idle_learn(),
            flash: None,
        }
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
        }
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

    /// Wiring a slot is one backend verb and it BOUNCES the pads. The warning
    /// is on the page before the click, not in the flash after it.
    #[test]
    fn the_slot_form_warns_that_the_pads_replug_before_the_click() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), None);
        assert!(out.html.contains(r#"action="/setup/slot""#), "{}", out.html);
        assert!(out.html.contains("REPLUGS the pads"), "{}", out.html);
        // The preset menu offers what is on disk, and the slot menu never
        // offers a slot the daemon would refuse.
        assert!(out.html.contains("IPAC P1"), "{}", out.html);
        assert!(out.html.contains("Slot 1"), "{}", out.html);
        assert!(!out.html.contains("Slot 9"), "{}", out.html);
        // Where it lands is a choice, and config.toml is the default.
        assert!(
            out.html.contains("(this cabinet&#x27;s config)")
                || out.html.contains("(this cabinet's config)"),
            "{}",
            out.html
        );
        assert!(out.html.contains("Street Fighter"), "{}", out.html);
    }

    /// No preset on disk means nothing to point a slot at: the control renders
    /// disabled with the reason, never live with an empty menu.
    #[test]
    fn a_machine_with_no_presets_cannot_wire_a_slot_and_says_why() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &fresh(), None);
        assert!(
            !out.html.contains(r#"action="/setup/slot""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("it needs a preset to point at"),
            "{}",
            out.html
        );
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

    /// A provider that refuses says so, in words, instead of rendering an empty
    /// checklist that reads as "this machine has nothing configured" — the two
    /// states want opposite advice.
    #[test]
    fn a_refused_machine_provider_is_rendered_as_a_refusal() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let payload = SetupPayload {
            setup: SetupSnapshot::unavailable(
                "reading the first-run state is not available on this surface",
            ),
            session: idle_session(),
            learn: idle_learn(),
            flash: None,
        };
        let out = render_setup(&page, &payload, None);
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
        // Neither of the two config states may claim anything.
        assert!(
            !out.html
                .contains("There is no configuration on this machine yet."),
            "{}",
            out.html
        );
    }

    /// Every page links onward. The board step goes to `/devices` rather than
    /// duplicating it, and the nav reaches the other two screens.
    #[test]
    fn the_page_links_onward_instead_of_duplicating() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &configured(), None);
        assert!(out.html.contains(r#"href="/devices""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/map""#), "{}", out.html);
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
    fn icon_links_are_in_the_setup_head() {
        let out = render_setup(&EmbeddedPage::load("/setup").unwrap(), &configured(), None);
        assert_icon_links_in_head("/setup", &out.html);
    }

    #[test]
    fn render_survives_an_empty_payload() {
        let page = EmbeddedPage::load("/setup").unwrap();
        let out = render_setup(&page, &SetupPayload::default(), None);
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        assert!(out.html.contains("no boards named yet"), "{}", out.html);
        assert!(out.html.contains("no slots wired yet"), "{}", out.html);
    }

    /// The slot menu can never offer a slot number the daemon would refuse.
    ///
    /// `ksx-core` is a dev-dependency here on purpose (see Cargo.toml): the
    /// page knows no vocabulary at runtime, and the test reads the one true
    /// constant. A `const` block rather than a runtime assertion, so lowering
    /// `MAX_SLOTS` fails the BUILD of this test rather than one run of it.
    #[test]
    fn the_slot_menu_never_exceeds_what_the_backend_accepts() {
        const {
            assert!(
                SLOT_CHOICES <= ksx_core::MAX_SLOTS,
                "the wire-a-slot menu offers a slot number `slot-assign` refuses: \
                 lower SLOT_CHOICES to at most ksx_core::MAX_SLOTS"
            );
        }
    }

    /// The two summary lines the client re-derives per poll are the ones this
    /// seam renders for the first paint. `SetupIsland.ts` mirrors these; the
    /// wording is pinned here so a change on one side is a test failure rather
    /// than a page that flickers between two sentences every two seconds.
    #[test]
    fn the_summary_lines_are_the_ones_the_client_mirrors() {
        assert_eq!(boards_line(0), "no boards named yet");
        assert_eq!(boards_line(1), "1 board named:");
        assert_eq!(boards_line(3), "3 boards named:");
        assert_eq!(slots_line(0), "no slots wired yet");
        assert_eq!(slots_line(1), "1 slot wired:");
        assert_eq!(slots_line(4), "4 slots wired:");
        assert_eq!(
            config_summary(&fresh()),
            "There is no configuration on this machine yet."
        );
        assert_eq!(
            config_summary(&configured()),
            "Configured — 1 board(s), 1 slot(s), 2 preset(s)."
        );
        assert_eq!(
            learn_line(&idle_learn()),
            "Nothing is listening. Start the listener, then press a button on the panel."
        );
    }
}
