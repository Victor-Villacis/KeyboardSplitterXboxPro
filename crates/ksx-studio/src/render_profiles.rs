//! The Profiles page's render seam — the third instance of the pattern
//! `render.rs` documents. Read that file's module docs first; nothing about
//! the mechanism is different here, only the data.
//!
//! What this page is for: the two writes a person could not previously make
//! without hand-editing TOML — a new games.toml profile, and a new preset from
//! an in-box template — plus the read that makes the first one honest.
//!
//! # The read that is the point
//!
//! `ksx_games::preflight` has always been able to say "that .exe is not
//! there". It ran at LAUNCH time, so a cabinet whose emulator moved looked
//! perfectly healthy in `ksx status`, in Studio's status page and in the
//! mapper's profile dropdown, right up to the moment someone pressed the
//! button and nothing happened. `MachineSource::profiles` runs the identical
//! check on the read side; this seam renders the result as a row that names
//! the path that is wrong, in an alarm card above everything else.
//!
//! A `steam://` URL is `launcher`, never `ok` — preflight cannot resolve it,
//! and a green badge would claim a check ksx did not make.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, PageConfig, PageOutput, RenderMode};

use crate::render::{body_prefix, with_icon_links, EmbeddedPage, PERSONALITY_CSS};
use crate::snapshot::ProfilesPayload;

/// List array slot names, binding-derived like every other page. The two
/// profile ROW lists share the `profileRows` signal — one carries a Switch
/// button, one does not — so the second occurrence gets the `#2` suffix, the
/// same shape the status page's two profile lists have carried since v4.
const LIST_SLOT_BROKEN: &str = "list:brokenRows:array";
const LIST_SLOT_PROFILES_LIVE: &str = "list:profileRows:array";
const LIST_SLOT_PROFILES_PLAIN: &str = "list:profileRows#2:array";
const LIST_SLOT_PRESET_OPTIONS: &str = "list:presetOptions:array";
const LIST_SLOT_PRESETS: &str = "list:presetRows:array";
const LIST_SLOT_TEMPLATES: &str = "list:templateRows:array";
const LIST_SLOT_TEMPLATE_OPTIONS: &str = "list:templateOptions:array";
const LIST_SLOT_NOTES: &str = "list:noteRows:array";

/// How many `createShow` pairs this page has; the layout test pins both the
/// count and every name.
const SHOW_COUNT: usize = 15;

#[cfg(test)]
const ISLAND_COMPONENT: &str = "ProfilesIsland";

/// Bare-named slots the seam deliberately never fills. EMPTY, and that is the
/// claim: every signal `ProfilesIsland.ts` binds to the DOM gets a server
/// value on every request.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots. EMPTY — every attribute value and every
/// text child on this page is either a named signal binding or static markup.
/// Note in particular that the two number inputs' `max` attributes are
/// BINDINGS (`maxSlots`, `maxPlayer`), not literals: `ksx_core::MAX_SLOTS` has
/// already been raised once, and `main.rs`'s `slot_arg` module exists because
/// three hardcoded copies of it did not move with it. The values come from
/// `ProfilesDerived`, so the SERVER and the browser read the same number —
/// which the previous revision did not do, and could not have, because the
/// island's ceiling was a compile-time string no poll could reach.
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

/// Scalar slot values, keyed by the signal names in ProfilesIsland.ts.
///
/// Everything but the flash comes out of [`ProfilesDerived`] verbatim. This
/// seam composes NO sentences and counts NO lists: it did, and the same lines
/// existed a second time in TypeScript, which is how the slot ceiling ended up
/// stale on one side only.
fn scalar_slots(view: &ProfilesPayload, flash: Option<&str>) -> serde_json::Value {
    let d = &view.view;
    serde_json::json!({
        "generatedAt": view.profiles.generated_at,
        "sessionLine": view.session.line,
        "flashLine": flash.unwrap_or(""),
        "daemonCmd": d.daemon_cmd,
        "gamesPath": view.profiles.games_path,
        "presetRoot": view.presets.config_root,
        "profilesSummary": d.profiles_summary,
        "brokenSummary": d.broken_summary,
        "presetsSummary": d.presets_summary,
        "templatesSummary": d.templates_summary,
        // Roster included — the island used to carry it as static copy, four
        // templates named while the registry ships six (SURFACES.md §1a).
        "templatesIntro": d.templates_intro,
        // The refusal sentences themselves. Empty when the read succeeded;
        // shown by `show:profilesUnreadable` / `show:presetsUnreadable`.
        "profilesError": view.profiles_error.clone().unwrap_or_default(),
        "presetsError": view.presets_error.clone().unwrap_or_default(),
        // Not literals: see ANONYMOUS_SLOTS.
        "maxSlots": d.max_slots.to_string(),
        "maxPlayer": d.max_player.to_string(),
    })
}

fn text(value: &str) -> SlotValue {
    SlotValue::Text(value.to_owned())
}

/// The list array payloads, keyed by their slot names. The two profile row
/// lists carry the same array — which one renders is decided by the show pair
/// around them (a Switch button only where a start could be accepted).
///
/// Every row was already composed by [`ProfilesDerived`]; this is the
/// `SlotValue` shim and nothing else.
fn list_values(view: &ProfilesPayload) -> [(&'static str, SlotValue); 8] {
    let d = &view.view;
    let broken_rows = SlotValue::array(
        d.broken_rows
            .iter()
            .map(|b| {
                SlotValue::object(vec![
                    ("title".to_owned(), text(&b.title)),
                    ("path".to_owned(), text(&b.path)),
                    ("verdict".to_owned(), text(&b.verdict)),
                ])
            })
            .collect(),
    );
    let rows = SlotValue::array(
        d.profile_rows
            .iter()
            .map(|g| {
                SlotValue::object(vec![
                    ("title".to_owned(), text(&g.title)),
                    ("path".to_owned(), text(&g.path)),
                    ("detail".to_owned(), text(&g.detail)),
                    ("verdict".to_owned(), text(&g.verdict)),
                    ("statecls".to_owned(), text(&g.statecls)),
                    ("statelabel".to_owned(), text(&g.statelabel)),
                ])
            })
            .collect(),
    );
    let preset_options = SlotValue::array(
        d.preset_options
            .iter()
            .map(|o| {
                SlotValue::object(vec![
                    ("value".to_owned(), text(&o.value)),
                    ("label".to_owned(), text(&o.label)),
                ])
            })
            .collect(),
    );
    let preset_rows = SlotValue::array(
        d.preset_rows
            .iter()
            .map(|r| {
                SlotValue::object(vec![
                    ("name".to_owned(), text(&r.name)),
                    ("detail".to_owned(), text(&r.detail)),
                    ("statecls".to_owned(), text(&r.statecls)),
                    ("statelabel".to_owned(), text(&r.statelabel)),
                ])
            })
            .collect(),
    );
    let template_rows = SlotValue::array(
        d.template_rows
            .iter()
            .map(|t| {
                SlotValue::object(vec![
                    ("id".to_owned(), text(&t.id)),
                    ("label".to_owned(), text(&t.label)),
                    ("detail".to_owned(), text(&t.detail)),
                    ("players".to_owned(), text(&t.players)),
                ])
            })
            .collect(),
    );
    let template_options = SlotValue::array(
        d.template_options
            .iter()
            .map(|o| {
                SlotValue::object(vec![
                    ("value".to_owned(), text(&o.value)),
                    ("label".to_owned(), text(&o.label)),
                ])
            })
            .collect(),
    );
    let notes = SlotValue::array(
        d.note_rows
            .iter()
            .map(|n| SlotValue::object(vec![("line".to_owned(), text(&n.line))]))
            .collect(),
    );
    [
        (LIST_SLOT_BROKEN, broken_rows),
        (LIST_SLOT_PROFILES_LIVE, rows.clone()),
        (LIST_SLOT_PROFILES_PLAIN, rows),
        (LIST_SLOT_PRESET_OPTIONS, preset_options),
        (LIST_SLOT_PRESETS, preset_rows),
        (LIST_SLOT_TEMPLATES, template_rows),
        (LIST_SLOT_TEMPLATE_OPTIONS, template_options),
        (LIST_SLOT_NOTES, notes),
    ]
}

/// Every show slot on this page, BY NAME, with the boolean the server wants.
///
/// All but the two flash branches are read straight off [`ProfilesDerived`] —
/// the flash is a per-request query parameter and belongs to the render, not
/// to the machine reads.
///
/// The two policies worth stating, both decided in `snapshot.rs`:
///
/// * the Switch button is offered ONLY when a start could actually be accepted
///   (`rowsLive`). Creating a profile or a preset is a plain disk write and
///   stays available with the daemon down — a first-run cabinet's exact case.
/// * a read that REFUSED gets `profilesUnreadable` / `presetsUnreadable`, and
///   those are mutually exclusive with the empty states. "No presets yet"
///   points the user at the template form below it; when the presets read is
///   the thing that failed, that form's `<select>` is empty too, so the only
///   route it offers cannot succeed.
fn show_values(view: &ProfilesPayload, flash: Option<&str>) -> [(&'static str, bool); SHOW_COUNT] {
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let d = &view.view;
    [
        ("show:pillRunning", d.pill_running),
        ("show:pillIdle", d.pill_idle),
        ("show:pillDown", d.pill_down),
        ("show:noDaemon", d.no_daemon),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        ("show:anyBroken", d.any_broken),
        ("show:rowsLive", d.rows_live),
        ("show:rowsPlain", d.rows_plain),
        ("show:profilesUnreadable", d.profiles_unreadable),
        ("show:canMakeProfile", d.can_make_profile),
        ("show:noPresetsYet", d.no_presets_yet),
        ("show:presetsUnreadable", d.presets_unreadable),
        ("show:canMakePreset", d.can_make_preset),
        ("show:anyNotes", d.any_notes),
    ]
}

/// Slot ids of every slot named `name`, in slot-table (== document) order.
/// Identical to `render.rs`'s and `render_map.rs`'s copies.
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
fn build_slots(module: &IrModule, view: &ProfilesPayload, flash: Option<&str>) -> SlotData {
    let scalars = scalar_slots(view, flash).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));
    for (name, value) in list_values(view) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    for (name, value) in show_values(view, flash) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, SlotValue::Bool(value));
        }
    }
    slots
}

/// Render the Profiles page for one payload: SSR slots for first paint, the
/// same data as the domain payload block for hydration.
/// The derived block is recomputed HERE rather than trusted from the caller,
/// so the SSR slots, the embedded payload block and whatever the 2 s poll will
/// later serve are one computation with three consumers. A caller that hands
/// in a stale or empty `view` cannot make the paint disagree with the data.
pub(crate) fn render_profiles(
    page: &EmbeddedPage,
    view: &ProfilesPayload,
    flash: Option<&str>,
) -> PageOutput {
    let payload = ProfilesPayload {
        flash: flash.map(str::to_owned),
        ..view.clone()
    }
    .derived();
    let slots = build_slots(&page.module, &payload, flash);
    let prefix = body_prefix(&payload, "/profiles");
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — profiles & presets",
        route_pattern: "/profiles",
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
    use ksx_api::{PresetRow, PresetsView, ProfileDetail, ProfilesView, TemplateRow};

    /// The reference cabinet, as of 2026-08-07: three profiles, one of which
    /// points at a mame.exe that is not there.
    fn sample() -> ProfilesPayload {
        ProfilesPayload {
            profiles: ProfilesView {
                generated_at: "2026-08-07 12:00:00 UTC".into(),
                config_root: "C:\\cfg\\ksx".into(),
                games_path: "C:\\cfg\\ksx\\games.toml".into(),
                profiles: vec![
                    ProfileDetail {
                        title: "Street Fighter".into(),
                        path: "C:\\games\\sf.exe".into(),
                        arguments: String::new(),
                        slots: 2,
                        presets: vec!["Arcade".into()],
                        state: "ok".into(),
                        verdict: "the program is there".into(),
                        broken_path: None,
                    },
                    ProfileDetail {
                        title: "MAME 4P".into(),
                        path: "D:\\emu\\mame\\mame.exe".into(),
                        arguments: String::new(),
                        slots: 4,
                        presets: vec!["P1".into(), "P2".into()],
                        state: "broken".into(),
                        verdict: "game profile 'MAME 4P' points at 'D:\\emu\\mame\\mame.exe', \
                                  which does not exist"
                            .into(),
                        broken_path: Some("D:\\emu\\mame\\mame.exe".into()),
                    },
                    ProfileDetail {
                        title: "Steam".into(),
                        path: "steam://rungameid/620".into(),
                        arguments: String::new(),
                        slots: 1,
                        presets: vec!["Arcade".into()],
                        state: "launcher".into(),
                        verdict: "handed to Steam; ksx cannot verify it ahead of time".into(),
                        broken_path: None,
                    },
                ],
                notes: Vec::new(),
            },
            presets: PresetsView {
                config_root: "C:\\cfg\\ksx\\presets".into(),
                presets: vec![
                    PresetRow {
                        name: "Arcade".into(),
                        bound: 25,
                        macros: 1,
                        protected: false,
                        source: "C:\\cfg\\ksx\\presets\\Arcade.toml".into(),
                    },
                    PresetRow {
                        name: "default".into(),
                        bound: 20,
                        macros: 0,
                        protected: true,
                        source: "default".into(),
                    },
                ],
                templates: vec![
                    TemplateRow {
                        id: "arcade-6button".into(),
                        label: "I-PAC / MAME six-button fighting panel (2 players)".into(),
                        detail: "A standard two-player, six-button arcade panel…".into(),
                        players: vec![1, 2],
                    },
                    TemplateRow {
                        id: "keyboard-2p".into(),
                        label: "Two players sharing ONE keyboard: WASD vs the arrows".into(),
                        detail: "Two people on one ordinary keyboard, no encoder…".into(),
                        players: vec![1, 2],
                    },
                ],
            },
            session: idle_session(),
            profiles_error: None,
            presets_error: None,
            notes: Vec::new(),
            flash: None,
            view: Default::default(),
        }
        .derived()
    }

    fn idle_session() -> SessionView {
        SessionView {
            reachable: true,
            running: false,
            line: "idle — daemon reachable".into(),
            profile: None,
        }
    }

    fn page() -> EmbeddedPage {
        EmbeddedPage::load("/profiles").expect("embedded profiles page must load")
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        assert_eq!(page().module.header.version, 2);
    }

    /// The slot-table contract, exactly as the status page pins its own: every
    /// scalar exists, the array slot NAMES are the ones the constants claim in
    /// document order, the `show:` set matches [`show_values`], and the island
    /// is the one the client registry activates. Then the real gate —
    /// injected == rendered, both ways.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = page();
        let module = &page.module;
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&ProfilesPayload::default(), None);
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
                LIST_SLOT_BROKEN,
                LIST_SLOT_PROFILES_LIVE,
                LIST_SLOT_PROFILES_PLAIN,
                LIST_SLOT_PRESET_OPTIONS,
                LIST_SLOT_PRESETS,
                LIST_SLOT_TEMPLATES,
                LIST_SLOT_TEMPLATE_OPTIONS,
                LIST_SLOT_NOTES,
            ],
            "list slot names drifted between ProfilesIsland.ts and the \
             LIST_SLOT_* constants; slots: {names:?}"
        );
        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> =
            show_values(&ProfilesPayload::default(), None)
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between ProfilesIsland.ts and show_values()"
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
                list_values(&ProfilesPayload::default())
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

    /// THE regression this page exists for: a profile whose .exe is gone is
    /// visible as broken, with the exact path, before anything is launched.
    #[test]
    fn a_broken_profile_names_the_path_that_is_wrong() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        assert!(out.html.contains("Broken profiles"), "{}", out.html);
        assert!(
            out.html
                .contains("1 profile points at a program that is not there:"),
            "{}",
            out.html
        );
        // The path is in the ALARM CARD ITSELF, not merely somewhere on the
        // page: "MAME 4P is broken" with the string printed two cards further
        // down is a second search, which is the state this page replaced.
        let card = out
            .html
            .split_once("Broken profiles")
            .and_then(|(_, rest)| rest.split_once("</section>"))
            .map(|(card, _)| card)
            .expect("the broken card");
        assert!(card.contains("MAME 4P"), "{card}");
        assert!(card.contains("D:\\emu\\mame\\mame.exe"), "{card}");
        assert!(card.contains("which does not exist"), "{card}");
        // …and the games.toml path, because "fix it" is not actionable
        // without the file it is in.
        assert!(card.contains("C:\\cfg\\ksx\\games.toml"), "{card}");
        // The healthy profile is not in the alarm card.
        assert!(!card.contains("Street Fighter"), "{card}");
    }

    /// A `steam://` profile is never green: preflight cannot resolve it, so
    /// claiming it works would be claiming a check that did not happen.
    ///
    /// Rewritten from three `assert_eq!(state_class("launcher"), "pill
    /// pill-idle")` lines, which restated the body of the function under test
    /// and could not fail unless someone edited both halves. This reads the
    /// RENDERED ROW instead: it fails against a `state_class` whose `launcher`
    /// arm falls through to the `_ => "pill pill-ok"` default — which is the
    /// one-character edit that would make ksx claim a check it never made.
    #[test]
    fn a_launcher_profile_is_neither_ok_nor_broken() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(out.html.contains("steam://rungameid/620"), "{}", out.html);
        assert!(out.html.contains("cannot verify it ahead of time"));

        // The Steam row, isolated: from its title to the end of its <li>.
        // Searched in the RENDERED body only — the `__ksx-payload` block above
        // it carries every one of these strings as data, which would make a
        // "does the markup say this" assertion pass on JSON.
        let row = ssr_body(&out.html)
            .split_once("Steam")
            .and_then(|(_, rest)| rest.split_once("</li>"))
            .map(|(row, _)| row.to_owned())
            .expect("the steam row");
        assert!(
            row.contains("pill-idle"),
            "a launcher row must carry the neutral pill: {row}"
        );
        assert!(
            !row.contains("pill-ok"),
            "a launcher row must NOT be green — preflight cannot resolve a \
             steam:// URL, so green claims a check ksx did not make: {row}"
        );
        // …and the healthy row still is green, so the assertion above is not
        // passing because nothing is ever green.
        let ok_row = ssr_body(&out.html)
            .split_once("Street Fighter")
            .and_then(|(_, rest)| rest.split_once("</li>"))
            .map(|(row, _)| row.to_owned())
            .expect("the sf row");
        assert!(ok_row.contains("pill-ok"), "{ok_row}");
    }

    /// Everything after the `__ksx-payload` block — the markup a browser with
    /// no JavaScript actually paints. The payload block is JSON and carries
    /// every displayed string as data, so a `contains` over the whole document
    /// proves only that the server SERIALIZED something.
    fn ssr_body(html: &str) -> &str {
        html.split_once(r#"<div id="app" data-forma-ssr"#)
            .map(|(_, body)| body)
            .expect("the SSR root")
    }

    /// With nothing broken, the alarm card is not rendered at all — a card
    /// that says "0 profiles are broken" is noise on every healthy cabinet.
    #[test]
    fn a_healthy_cabinet_shows_no_broken_card() {
        let mut view = sample();
        view.profiles.profiles.retain(|p| p.state != "broken");
        let out = render_profiles(&page(), &view, None);
        assert!(!out.html.contains("Broken profiles"), "{}", out.html);
    }

    /// The create forms are the answer to "I can't create a new profile", so
    /// their fields and their actions are pinned.
    #[test]
    fn both_create_forms_post_to_their_own_route_with_their_fields() {
        let out = render_profiles(&page(), &sample(), None);
        for needle in [
            r#"action="/profiles/new""#,
            r#"name="title""#,
            r#"name="path""#,
            r#"name="slots""#,
            r#"name="preset""#,
            r#"action="/profiles/preset/new""#,
            r#"name="template""#,
            r#"name="player""#,
            r#"action="/profiles/switch""#,
        ] {
            assert!(out.html.contains(needle), "missing {needle}: {}", out.html);
        }
        // Every form is a POST to a same-origin relative action — the guard's
        // mutating test is method-based, and forma's CSP carries
        // `form-action 'self'`.
        assert!(!out.html.contains(r#"action="http"#), "{}", out.html);
    }

    /// Task #14's template has to be reachable from a menu, which is the whole
    /// reason `LocalMachine::presets` stopped answering with an empty
    /// `templates` list.
    #[test]
    fn the_template_menu_offers_the_two_player_desktop_keyboard_layout() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(out.html.contains(r#"value="keyboard-2p""#), "{}", out.html);
        assert!(out.html.contains("WASD vs the arrows"));
    }

    /// The slot ceiling is `ksx_core::MAX_SLOTS` on BOTH sides of hydration.
    ///
    /// The previous version asserted only that the SSR HTML contained
    /// `max="16"`. It would have kept passing after a MAX_SLOTS raise while
    /// the hydrated page showed 16, because the number the browser used came
    /// from `createSignal("16")` in ProfilesIsland.ts — a literal `setMaxSlots`
    /// was never called with, that no payload field could correct, and that
    /// adoption writes into the DOM over the server's value. So it guarded the
    /// appearance of the attribute, not the number.
    ///
    /// This asserts the whole path: the attribute the server paints, AND the
    /// `view.max_slots` field the browser reads out of the embedded payload —
    /// the only place ProfilesIsland now gets it from. It fails against the
    /// broken version at the payload assertion (no such field existed), and it
    /// fails against any future one that reintroduces a client-side literal,
    /// because a literal cannot equal a raised MAX_SLOTS.
    #[test]
    fn the_slot_ceiling_is_the_real_max_slots_on_both_sides() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(
            out.html
                .contains(&format!(r#"max="{}""#, ksx_core::MAX_SLOTS)),
            "the SSR paint must carry the real ceiling: {}",
            out.html
        );
        let embedded = embedded_payload(&out.html);
        assert_eq!(
            embedded.pointer("/view/max_slots"),
            Some(&serde_json::json!(ksx_core::MAX_SLOTS)),
            "the ceiling the BROWSER uses arrives in the payload, or the \
             client is back to a compile-time literal: {}",
            out.html
        );
        // The same for the preset form's player ceiling, which was the
        // literal "4" whether or not the selected template had four blocks.
        assert_eq!(
            embedded.pointer("/view/max_player"),
            Some(&serde_json::json!(2)),
            "the player ceiling must come from the offered templates (the \
             sample's widest is 2), not from a literal: {}",
            out.html
        );
    }

    /// The `__ksx-payload` block, parsed.
    fn embedded_payload(html: &str) -> serde_json::Value {
        let block = html
            .split_once(r#"<script id="__ksx-payload" type="application/json">"#)
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(body, _)| body.to_owned())
            .expect("payload block");
        serde_json::from_str(&block.replace("\\u003c", "<")).expect("payload parses")
    }

    /// With the daemon down, Switch is not offered at all — the plain row list
    /// renders instead. A dead button rendered as live is the one thing this
    /// page must not do; creating still works, because a disk write needs no
    /// daemon and a first-run cabinet has none.
    #[test]
    fn switch_is_withheld_with_no_daemon_but_create_is_not() {
        let mut view = sample();
        view.session = SessionView::unreachable("no daemon answered the control channel");
        let out = render_profiles(&page(), &view, None);
        assert!(
            !out.html.contains(r#"action="/profiles/switch""#),
            "{}",
            out.html
        );
        assert!(out.html.contains(r#"action="/profiles/new""#));
        assert!(out.html.contains(crate::render::NO_DAEMON_HEADLINE));
    }

    /// The flash is attacker-writable (it arrives from a query string) and is
    /// rendered escaped, on this page like every other.
    #[test]
    fn a_hostile_flash_is_escaped() {
        let out = render_profiles(
            &page(),
            &sample(),
            Some(r#"error: <script>alert("x")</script>"#),
        );
        assert!(!out.html.contains("<script>alert"), "{}", out.html);
        assert!(out.html.contains("&lt;script&gt;"), "{}", out.html);
    }

    /// One struct, one serializer: the block the page embeds is the shape
    /// `GET /api/profiles` serves.
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let out = render_profiles(&page(), &sample(), None);
        let block = out
            .html
            .split_once(r#"<script id="__ksx-payload" type="application/json">"#)
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(body, _)| body.to_owned())
            .expect("payload block");
        let embedded: serde_json::Value =
            serde_json::from_str(&block.replace("\\u003c", "<")).expect("payload parses");
        let served = serde_json::to_value(ProfilesPayload {
            flash: None,
            ..sample()
        })
        .unwrap();
        assert_eq!(embedded, served);
    }

    /// The nav is static markup per island, so a sibling page is invisible
    /// until every island lists it. Pin this page's own links.
    #[test]
    fn the_nav_reaches_both_siblings() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(out.html.contains(r#"href="/""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/map""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/pads""#), "{}", out.html);
        assert!(out.html.contains(r#"aria-current="page""#), "{}", out.html);
    }

    #[test]
    fn icon_links_are_in_the_profiles_head() {
        let out = render_profiles(&page(), &sample(), None);
        crate::render::assert_icon_links_in_head("/profiles", &out.html);
    }

    /// A read that refused renders as a NOTE beside an empty list, never as an
    /// empty list on its own — "no profiles" and "games.toml is unreadable"
    /// are not the same sentence.
    #[test]
    fn a_refused_read_is_rendered_not_swallowed() {
        let view = ProfilesPayload {
            notes: vec!["games.toml could not be read: expected `=` at line 4".into()],
            ..ProfilesPayload::default()
        };
        let out = render_profiles(&page(), &view, None);
        assert!(
            out.html.contains("Notes from the config read"),
            "{}",
            out.html
        );
        assert!(out.html.contains("expected `=` at line 4"));
    }

    /// A REFUSED read must not render as an assertion of absence.
    ///
    /// This is the one that fails against the shipped version. There, the
    /// handler substituted `ProfilesView::default()` on `Err`, so the page
    /// printed "no profiles in games.toml" — a statement about the FILE'S
    /// CONTENTS — when nothing had been read at all, with the real reason
    /// buried in the last card. A user acts on those two sentences
    /// differently: one says "make a profile", the other says "your config is
    /// broken, go fix it".
    ///
    /// It is the same class of failure as the session that reported success
    /// while the arcade panel was dead because a WinUSB board had fallen back
    /// to Interception: the surface answered for a read it never completed.
    #[test]
    fn a_read_that_refused_never_says_there_is_nothing_here() {
        let view = ProfilesPayload {
            profiles_error: Some("games.toml could not be read: expected `=` at line 4".into()),
            presets_error: Some("the presets folder could not be read: access denied".into()),
            session: idle_session(),
            ..ProfilesPayload::default()
        };
        let out = render_profiles(&page(), &view, None);

        // NOT the count sentences. These are the exact strings the shipped
        // version printed for this payload.
        assert!(
            !out.html.contains("no profiles in games.toml"),
            "a failed read rendered as 'you have no profiles': {}",
            out.html
        );
        assert!(
            !out.html.contains("no presets on disk"),
            "a failed read rendered as 'you have no presets': {}",
            out.html
        );
        // …the refusal itself, in the card it is about.
        assert!(
            out.html.contains("could NOT be read"),
            "the failure must be stated where the list would have been: {}",
            out.html
        );
        assert!(out.html.contains("expected `=` at line 4"), "{}", out.html);
        assert!(out.html.contains("access denied"), "{}", out.html);
    }

    /// The presets half of the same failure, which was the worse one: a
    /// `PresetsView::default()` made `noPresetsYet` true, whose copy is "Make a
    /// preset from an in-box template below first" — and the template
    /// `<select>` is fed by the SAME read that just failed, so the one route
    /// out of the empty state could not succeed. A closed loop with a wrong
    /// sentence on it.
    ///
    /// Fails against the shipped version on the first assertion.
    #[test]
    fn a_failed_presets_read_does_not_point_at_a_form_that_cannot_work() {
        let view = ProfilesPayload {
            presets_error: Some("the presets folder could not be read: access denied".into()),
            session: idle_session(),
            ..ProfilesPayload::default()
        };
        let out = render_profiles(&page(), &view, None);
        assert!(
            !out.html
                .contains("Make a preset from an in-box template below"),
            "a failed presets read must not send the user to the template \
             form — its <select> is empty for the same reason: {}",
            out.html
        );
        assert!(
            !out.html.contains(r#"action="/profiles/preset/new""#),
            "the template form must be withheld when the read that fills it \
             refused: {}",
            out.html
        );
        assert!(
            !out.html.contains(r#"action="/profiles/new""#),
            "and so must the profile form, whose preset <select> is the same \
             list: {}",
            out.html
        );
        assert!(out.html.contains("access denied"), "{}", out.html);

        // The control: presets that genuinely ARE empty still get the
        // actionable empty state and the template form. Without this the
        // assertions above would pass by withholding everything always.
        let empty = ProfilesPayload {
            session: idle_session(),
            ..ProfilesPayload::default()
        };
        let out = render_profiles(&page(), &empty, None);
        assert!(
            out.html
                .contains("Make a preset from an in-box template below"),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"action="/profiles/preset/new""#),
            "{}",
            out.html
        );
    }

    /// The in-box templates are LISTED, with the panel note that identifies
    /// them and the player range the form's number field is bounded by.
    ///
    /// `templatesSummary` ended in a colon ("2 in-box templates:") with a form
    /// after it and no list, and `TemplateRow::detail` — the field ksx-api
    /// documents as "a template nobody can identify from a list is a template
    /// nobody uses" — was serialized on every request and rendered nowhere.
    #[test]
    fn the_templates_are_listed_with_their_panel_note_and_player_range() {
        let out = render_profiles(&page(), &sample(), None);
        // The RENDERED body, not the payload block: `detail` and `players`
        // were serialized on every request before this change too, so a
        // whole-document `contains` would have passed against the version
        // that rendered neither.
        let body = ssr_body(&out.html);
        assert!(body.contains("2 in-box templates:"), "{body}");
        assert!(
            body.contains("Two people on one ordinary keyboard, no encoder"),
            "the panel note is the point of the list: {body}"
        );
        assert!(body.contains("players 1–2"), "{body}");
    }

    /// The zero arm the summary did not have: it printed "0 in-box templates:"
    /// — a plural with a colon and nothing after it.
    #[test]
    fn a_template_list_of_zero_is_not_a_colon() {
        let empty = ProfilesPayload {
            session: idle_session(),
            ..ProfilesPayload::default()
        };
        let out = render_profiles(&page(), &empty, None);
        assert!(!out.html.contains("0 in-box templates:"), "{}", out.html);
        assert!(out.html.contains("no in-box templates"), "{}", out.html);
    }
}
