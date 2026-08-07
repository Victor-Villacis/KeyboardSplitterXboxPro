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
const LIST_SLOT_TEMPLATE_OPTIONS: &str = "list:templateOptions:array";
const LIST_SLOT_NOTES: &str = "list:noteRows:array";

/// How many `createShow` pairs this page has; the layout test pins both the
/// count and every name.
const SHOW_COUNT: usize = 12;

#[cfg(test)]
const ISLAND_COMPONENT: &str = "ProfilesIsland";

/// Bare-named slots the seam deliberately never fills. EMPTY, and that is the
/// claim: every signal `ProfilesIsland.ts` binds to the DOM gets a server
/// value on every request.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots. EMPTY — every attribute value and every
/// text child on this page is either a named signal binding or static markup.
/// Note in particular that the slot-count input's `max` is a BINDING
/// (`maxSlots`), not a literal `"16"`: `ksx_core::MAX_SLOTS` has already been
/// raised once, and `main.rs`'s `slot_arg` module exists because three
/// hardcoded copies of it did not move with it.
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

/// Scalar slot values, keyed by the signal names in ProfilesIsland.ts.
fn scalar_slots(view: &ProfilesPayload, flash: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "generatedAt": view.profiles.generated_at,
        "sessionLine": view.session.line,
        "flashLine": flash.unwrap_or(""),
        "daemonCmd": crate::render::daemon_command(&view.session),
        "gamesPath": view.profiles.games_path,
        "presetRoot": view.presets.config_root,
        "profilesSummary": profiles_summary(view.profiles.profiles.len()),
        "brokenSummary": broken_summary(broken(view).count()),
        "presetsSummary": presets_summary(view.presets.presets.len()),
        "templatesSummary": templates_summary(view.presets.templates.len()),
        // Not a literal: see ANONYMOUS_SLOTS.
        "maxSlots": ksx_api::MAX_SLOTS.to_string(),
    })
}

fn profiles_summary(count: usize) -> String {
    match count {
        0 => "no profiles in games.toml".to_owned(),
        1 => "1 profile in games.toml:".to_owned(),
        n => format!("{n} profiles in games.toml:"),
    }
}

fn broken_summary(count: usize) -> String {
    match count {
        1 => "1 profile points at a program that is not there:".to_owned(),
        n => format!("{n} profiles point at a program that is not there:"),
    }
}

fn presets_summary(count: usize) -> String {
    match count {
        0 => "no presets on disk".to_owned(),
        1 => "1 preset on disk:".to_owned(),
        n => format!("{n} presets on disk:"),
    }
}

fn templates_summary(count: usize) -> String {
    match count {
        1 => "1 in-box template:".to_owned(),
        n => format!("{n} in-box templates:"),
    }
}

/// The profiles whose program is not there. The word "broken" is the
/// provider's (`ProfileDetail::state`), not a re-derivation here — deciding
/// what counts as broken in a render seam is exactly the thing
/// docs/SURFACES.md §1 forbids.
fn broken(view: &ProfilesPayload) -> impl Iterator<Item = &ksx_api::ProfileDetail> {
    view.profiles
        .profiles
        .iter()
        .filter(|p| p.state == "broken")
}

/// A profile's state as the pill class that carries it. `launcher` gets the
/// neutral pill rather than the OK one, for the reason the module docs give.
fn state_class(state: &str) -> &'static str {
    match state {
        "broken" => "pill pill-warn",
        "launcher" => "pill pill-idle",
        _ => "pill pill-ok",
    }
}

fn profile_detail_line(p: &ksx_api::ProfileDetail) -> String {
    let slots = match p.slots {
        1 => "1 slot".to_owned(),
        n => format!("{n} slots"),
    };
    if p.presets.is_empty() {
        format!("{slots} — no preset named")
    } else {
        format!("{slots} on {}", p.presets.join(", "))
    }
}

fn preset_detail_line(p: &ksx_api::PresetRow) -> String {
    let controls = match p.bound {
        1 => "1 control".to_owned(),
        n => format!("{n} controls"),
    };
    let macros = match p.macros {
        0 => String::new(),
        n => format!(", {n} macro(s)"),
    };
    format!("{controls}{macros} — {}", p.source)
}

fn text(value: &str) -> SlotValue {
    SlotValue::Text(value.to_owned())
}

/// The list array payloads, keyed by their slot names. The two profile row
/// lists carry the same array — which one renders is decided by the show pair
/// around them (a Switch button only where a start could be accepted).
fn list_values(view: &ProfilesPayload) -> [(&'static str, SlotValue); 7] {
    let broken_rows = SlotValue::array(
        broken(view)
            .map(|p| {
                SlotValue::object(vec![
                    ("title".to_owned(), text(&p.title)),
                    // `broken_path` is the provider's answer to "which string
                    // is wrong"; falling back to `path` keeps the row honest
                    // for the empty-path case, where there IS no bad path.
                    (
                        "path".to_owned(),
                        text(p.broken_path.as_deref().unwrap_or(&p.path)),
                    ),
                    ("verdict".to_owned(), text(&p.verdict)),
                ])
            })
            .collect(),
    );
    let rows = SlotValue::array(
        view.profiles
            .profiles
            .iter()
            .map(|p| {
                SlotValue::object(vec![
                    ("title".to_owned(), text(&p.title)),
                    ("path".to_owned(), text(&p.path)),
                    ("detail".to_owned(), text(&profile_detail_line(p))),
                    ("verdict".to_owned(), text(&p.verdict)),
                    ("statecls".to_owned(), text(state_class(&p.state))),
                    ("statelabel".to_owned(), text(&p.state)),
                ])
            })
            .collect(),
    );
    let preset_options = SlotValue::array(
        view.presets
            .presets
            .iter()
            .map(|p| {
                SlotValue::object(vec![
                    ("value".to_owned(), text(&p.name)),
                    ("label".to_owned(), text(&p.name)),
                ])
            })
            .collect(),
    );
    let preset_rows = SlotValue::array(
        view.presets
            .presets
            .iter()
            .map(|p| {
                SlotValue::object(vec![
                    ("name".to_owned(), text(&p.name)),
                    ("detail".to_owned(), text(&preset_detail_line(p))),
                    (
                        "statecls".to_owned(),
                        text(if p.protected {
                            "pill pill-idle"
                        } else {
                            "pill pill-ok"
                        }),
                    ),
                    (
                        "statelabel".to_owned(),
                        text(if p.protected { "built-in" } else { "yours" }),
                    ),
                ])
            })
            .collect(),
    );
    let template_options = SlotValue::array(
        view.presets
            .templates
            .iter()
            .map(|t| {
                SlotValue::object(vec![
                    ("value".to_owned(), text(&t.id)),
                    ("label".to_owned(), text(&format!("{} — {}", t.id, t.label))),
                ])
            })
            .collect(),
    );
    let notes = SlotValue::array(
        view.notes
            .iter()
            .map(|line| SlotValue::object(vec![("line".to_owned(), text(line))]))
            .collect(),
    );
    [
        (LIST_SLOT_BROKEN, broken_rows),
        (LIST_SLOT_PROFILES_LIVE, rows.clone()),
        (LIST_SLOT_PROFILES_PLAIN, rows),
        (LIST_SLOT_PRESET_OPTIONS, preset_options),
        (LIST_SLOT_PRESETS, preset_rows),
        (LIST_SLOT_TEMPLATE_OPTIONS, template_options),
        (LIST_SLOT_NOTES, notes),
    ]
}

/// Every show slot on this page, BY NAME, with the boolean the server wants.
///
/// The one policy worth stating: the Switch button is offered ONLY when a
/// start could actually be accepted (`rowsLive`). Creating a profile or a
/// preset is a plain disk write and stays available with the daemon down —
/// which is exactly the case a first-run cabinet is in.
fn show_values(view: &ProfilesPayload, flash: Option<&str>) -> [(&'static str, bool); SHOW_COUNT] {
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let can_start = view.session.reachable && !view.session.running;
    let has_presets = !view.presets.presets.is_empty();
    [
        (
            "show:pillRunning",
            view.session.reachable && view.session.running,
        ),
        ("show:pillIdle", can_start),
        ("show:pillDown", !view.session.reachable),
        ("show:noDaemon", !view.session.reachable),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        ("show:anyBroken", broken(view).next().is_some()),
        ("show:rowsLive", can_start),
        ("show:rowsPlain", !can_start),
        ("show:canMakeProfile", has_presets),
        ("show:noPresetsYet", !has_presets),
        ("show:anyNotes", !view.notes.is_empty()),
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
pub(crate) fn render_profiles(
    page: &EmbeddedPage,
    view: &ProfilesPayload,
    flash: Option<&str>,
) -> PageOutput {
    let slots = build_slots(&page.module, view, flash);
    let payload = ProfilesPayload {
        flash: flash.map(str::to_owned),
        ..view.clone()
    };
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
            notes: Vec::new(),
            flash: None,
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
    #[test]
    fn a_launcher_profile_is_neither_ok_nor_broken() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(out.html.contains("steam://rungameid/620"), "{}", out.html);
        assert!(out.html.contains("cannot verify it ahead of time"));
        assert_eq!(state_class("launcher"), "pill pill-idle");
        assert_eq!(state_class("ok"), "pill pill-ok");
        assert_eq!(state_class("broken"), "pill pill-warn");
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

    /// The slot-count input's ceiling is `ksx_core::MAX_SLOTS`, injected —
    /// not a literal that a raise would leave behind (`main.rs` `slot_arg`).
    #[test]
    fn the_slot_ceiling_is_the_real_max_slots() {
        let out = render_profiles(&page(), &sample(), None);
        assert!(
            out.html
                .contains(&format!(r#"max="{}""#, ksx_core::MAX_SLOTS)),
            "{}",
            out.html
        );
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
}
