//! The render seam: embedded FMIR + per-request [`StatusSnapshot`] /
//! [`SessionView`] → HTML.
//!
//! # Data-injection mechanism (and why)
//!
//! forma-server 0.1.4 supports true server-side prop injection: the compiler
//! declares NAMED slots in the FMIR slot table, and `forma_ir::slot::SlotData`
//! is populated by the handler before the IR walk. That is the mechanism used
//! here — no JSON island, no string templating.
//!
//! Three flavours of slot exist on this page:
//!
//! - **Scalars** — every `createSignal` in `studio-ui/src/StatusPage.ts`
//!   becomes a slot named after the signal getter. Unique names, injected via
//!   [`SlotData::from_json`] (name-keyed, defaults preserved for misses).
//! - **Lists** — every `createList` becomes an Array slot with a unique
//!   per-instance name: `list:#N:array`, N numbering the list instances in
//!   document order (compiler 0.2.0). Injected by NAME, like the scalars;
//!   the `LIST_SLOT_*` constants pin the three names this page has.
//! - **Shows** — every `createShow` still becomes a Bool slot named
//!   `show:createShow` — 0.2.0 named lists uniquely but not shows — so shows
//!   remain the one POSITIONAL seam (slot-table order == emission order ==
//!   document order). [`SHOW_ORDER`] documents that mapping; the shows are
//!   what let an SSR-only page render the session controls conditionally
//!   (enabled / running / visibly-disabled) with zero client JS.
//!
//! `tests::embedded_ir_slot_layout_matches_the_seam` pins the exact list slot
//! NAMES (order included) and the show count — a compiler bump that renames
//! slots, or a StatusPage.ts edit that adds/reorders lists or shows, is a
//! test failure, not a silently blank section.
//!
//! History: compiler 0.1.8 named EVERY list `list:array`, and this seam
//! resolved lists positionally too (a `LIST_ORDER` table, since deleted).
//! Per-instance slot naming was the upstream feature request this page
//! dogfooded (docs/ENHANCEMENTS.md E7 loop); fixed upstream in
//! `@getforma/compiler` 0.2.0, adopted 2026-08-05 — the E7 dogfood loop's
//! first closed cycle. Per-instance `createShow` naming is the remaining ask.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, AssetManifest, PageConfig, PageOutput, RenderMode};
use rust_embed::Embed;

use crate::control::SessionView;
use crate::error::StudioError;
use crate::snapshot::StatusSnapshot;

/// The committed `studio-ui` build output (see the crate docs for the
/// regeneration command — Node is never needed to build or run ksx).
#[derive(Embed)]
#[folder = "assets/"]
pub(crate) struct Assets;

/// Since 0.2.0 the compiler names each `createList` array slot uniquely:
/// `list:#N:array`, N counting list instances in document order. These pin
/// the names of the three lists in StatusPage.ts — adding or reordering
/// lists there shifts the numbering, and the layout test fails until the
/// constants are updated to match.
const LIST_SLOT_PROFILE_OPTIONS: &str = "list:#1:array";
const LIST_SLOT_PADS: &str = "list:#2:array";
const LIST_SLOT_PROFILES: &str = "list:#3:array";

/// `createShow` booleans did NOT gain unique names in compiler 0.2.0 — every
/// show is still `show:createShow`, so shows are the remaining positional
/// seam. Document order in StatusPage.ts: the flash line, the Start form,
/// the Stop/Reload forms, the disabled controls + explanation shown when no
/// daemon control channel answers.
const SHOW_SLOT_NAME: &str = "show:createShow";
const SHOW_ORDER: [&str; 4] = ["flash", "start controls", "stop controls", "daemon down"];
const SHOW_COUNT: usize = SHOW_ORDER.len();

/// Seconds between full-page refreshes (meta pragma + HTTP `Refresh`). Was
/// 2 s while the page was read-only; a page with a dropdown must leave the
/// user time to aim at it before the reload closes it.
pub(crate) const REFRESH_SECS: u32 = 5;

/// Parsed once at server start; immutable afterwards.
pub(crate) struct EmbeddedPage {
    manifest: AssetManifest,
    module: IrModule,
}

impl EmbeddedPage {
    pub(crate) fn load() -> Result<Self, StudioError> {
        let manifest_json = Assets::get("manifest.json")
            .ok_or_else(|| StudioError::Asset("manifest.json missing from embed".into()))?;
        let manifest: AssetManifest = serde_json::from_slice(&manifest_json.data)
            .map_err(|e| StudioError::Asset(format!("manifest.json unparsable: {e}")))?;

        let ir_name = manifest
            .route("/")
            .and_then(|r| r.ir.clone())
            .ok_or_else(|| StudioError::Asset("manifest route '/' has no .ir entry".into()))?;
        let ir_bytes = Assets::get(&ir_name)
            .ok_or_else(|| StudioError::Asset(format!("{ir_name} missing from embed")))?;

        let module = IrModule::parse(&ir_bytes.data)
            .map_err(|e| StudioError::Ir(format!("{ir_name}: {e}")))?;
        forma_server::check_ir_compatibility(&module)
            .map_err(|e| StudioError::Ir(format!("{ir_name}: {e}")))?;

        Ok(Self { manifest, module })
    }

    #[cfg(test)]
    pub(crate) fn module(&self) -> &IrModule {
        &self.module
    }
}

/// Scalar slot values, keyed by the signal names in StatusPage.ts.
fn scalar_slots(
    snap: &StatusSnapshot,
    session: &SessionView,
    flash: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "generatedAt": snap.generated_at,
        "vigemLine": snap.vigem,
        "interceptionLine": snap.interception,
        "daemonYesNo": if snap.daemon_running { "yes" } else { "no" },
        "daemonDetail": snap.daemon_detail,
        "autostartLine": snap.autostart,
        "padsSummary": pads_summary(snap),
        "profilesSummary": profiles_summary(snap),
        "configRoot": snap.config_root,
        "sessionLine": session.line,
        "flashLine": flash.unwrap_or(""),
    })
}

fn pads_summary(snap: &StatusSnapshot) -> String {
    match snap.pads.len() {
        0 => "no virtual pads exposed by the bus".to_owned(),
        1 => "1 virtual pad exposed by the bus:".to_owned(),
        n => format!("{n} virtual pads exposed by the bus:"),
    }
}

fn profiles_summary(snap: &StatusSnapshot) -> String {
    match snap.profiles.len() {
        0 => "no profiles in games.toml".to_owned(),
        1 => "1 profile in games.toml:".to_owned(),
        n => format!("{n} profiles in games.toml:"),
    }
}

/// The list array payloads, keyed by their (unique) slot names.
fn list_values(snap: &StatusSnapshot) -> [(&'static str, SlotValue); 3] {
    let options = SlotValue::Array(
        snap.profiles
            .iter()
            .map(|g| {
                SlotValue::Object(vec![("title".to_owned(), SlotValue::Text(g.title.clone()))])
            })
            .collect(),
    );
    let pads = SlotValue::Array(
        snap.pads
            .iter()
            .map(|p| {
                SlotValue::Object(vec![
                    ("persona".to_owned(), SlotValue::Text(p.persona.clone())),
                    ("instance".to_owned(), SlotValue::Text(p.instance.clone())),
                ])
            })
            .collect(),
    );
    let profiles = SlotValue::Array(
        snap.profiles
            .iter()
            .map(|g| {
                SlotValue::Object(vec![
                    ("title".to_owned(), SlotValue::Text(g.title.clone())),
                    ("detail".to_owned(), SlotValue::Text(g.detail.clone())),
                ])
            })
            .collect(),
    );
    [
        (LIST_SLOT_PROFILE_OPTIONS, options),
        (LIST_SLOT_PADS, pads),
        (LIST_SLOT_PROFILES, profiles),
    ]
}

/// The show booleans, in [`SHOW_ORDER`]. This is the whole conditional-UI
/// policy: exactly one of "start", "stop" or "daemon down" is true, so the
/// panel always says something and never offers a dead button as live.
fn show_values(session: &SessionView, flash: Option<&str>) -> [bool; SHOW_COUNT] {
    [
        flash.is_some(),
        session.reachable && !session.running,
        session.reachable && session.running,
        !session.reachable,
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
fn build_slots(
    module: &IrModule,
    snap: &StatusSnapshot,
    session: &SessionView,
    flash: Option<&str>,
) -> SlotData {
    // Scalars by name; starts from IR defaults, so a renamed signal degrades
    // to its authored default ("not collected"), never to garbage.
    let scalars = scalar_slots(snap, session, flash).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));

    // Lists by name (unique since compiler 0.2.0). A rename upstream
    // degrades to the authored default (an empty list) — which is exactly
    // what the layout test exists to catch before it ships.
    for (name, value) in list_values(snap) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, value);
        }
    }
    // Shows by position — still all named `show:createShow` (module docs).
    for (id, value) in named_slot_ids(module, SHOW_SLOT_NAME)
        .into_iter()
        .zip(show_values(session, flash))
    {
        slots.set(id, SlotValue::Bool(value));
    }
    slots
}

/// Render the page for one snapshot + session view. Falling back to Phase 1
/// (an empty `#app` with zero client JS, i.e. a blank page) can only happen
/// if the embedded IR is broken — which `EmbeddedPage::load` already refused.
pub(crate) fn render_status(
    page: &EmbeddedPage,
    snap: &StatusSnapshot,
    session: &SessionView,
    flash: Option<&str>,
) -> PageOutput {
    let slots = build_slots(&page.module, snap, session, flash);
    let refresh = format!(r#"<meta http-equiv="refresh" content="{REFRESH_SECS}">"#);
    render_page(&PageConfig {
        title: "ksx Studio — cabinet status",
        route_pattern: "/",
        manifest: &page.manifest,
        config_script: None,
        body_class: None,
        personality_css: None,
        // The auto-refresh. A meta pragma is processed wherever the element
        // is inserted (WHATWG "pragma directives"), head or body; the server
        // also sends an HTTP `Refresh` header (server.rs) as belt and braces.
        // No JS, nothing for the hardcoded CSP to block.
        body_prefix: Some(&refresh),
        render_mode: RenderMode::Phase2SsrReconcile,
        ir_module: Some(&page.module),
        slots: Some(&slots),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{PadRow, ProfileRow};

    fn sample() -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "2026-08-04 12:00:00 UTC".into(),
            vigem: "installed — service running — driver v1.21.442.0".into(),
            interception: "installed — keyboard filter active".into(),
            daemon_running: true,
            daemon_detail: "ksx.exe alive (pid 4242)".into(),
            autostart: "registered — ksx daemon".into(),
            pads: vec![
                PadRow {
                    persona: "Xbox 360 pad".into(),
                    instance: "USB\\VID_045E&PID_028E\\2&AA&0&01".into(),
                },
                PadRow {
                    persona: "PlayStation (DS4) pad".into(),
                    instance: "USB\\VID_054C&PID_05C4\\2&AA&0&02".into(),
                },
            ],
            profiles: vec![ProfileRow {
                title: "Street Fighter".into(),
                detail: "C:\\games\\sf.exe — 2 slots".into(),
            }],
            config_root: "C:\\cfg\\ksx".into(),
        }
    }

    fn idle_session() -> SessionView {
        SessionView {
            reachable: true,
            running: false,
            line: "idle — daemon reachable".into(),
        }
    }

    fn running_session() -> SessionView {
        SessionView {
            reachable: true,
            running: true,
            line: "running — Street Fighter — 4 pad(s)".into(),
        }
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load().expect("embedded page must load");
        assert_eq!(page.module().header.version, 2);
        // The raw-bytes guard from the forma spike: FMIR magic + u16 LE 2.
        let ir_name = page.manifest.route("/").unwrap().ir.clone().unwrap();
        let bytes = Assets::get(&ir_name).unwrap().data;
        assert_eq!(&bytes[0..6], b"FMIR\x02\x00");
    }

    /// Pins the slot-table contract the seam depends on: every scalar signal
    /// name exists, the `list:#N:array` slot NAMES are exactly the ones the
    /// `LIST_SLOT_*` constants claim (order included), and there are exactly
    /// as many `show:createShow` slots as [`SHOW_ORDER`] claims. Fails when
    /// StatusPage.ts, the compiler's naming scheme, or this file drift.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load().unwrap();
        let module = page.module();
        let names: Vec<&str> = module
            .slots
            .entries()
            .iter()
            .filter_map(|e| module.strings.get(e.name_str_idx).ok())
            .collect();

        let scalars = scalar_slots(&StatusSnapshot::default(), &SessionView::default(), None);
        for key in scalars.as_object().unwrap().keys() {
            assert!(
                names.contains(&key.as_str()),
                "scalar slot '{key}' missing from the embedded IR; slots: {names:?}"
            );
        }
        // Every list array slot in the IR, in slot-table order, must be one
        // the seam injects by name — no extras, no misses, no renames.
        let array_slots: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("list:") && n.ends_with(":array"))
            .collect();
        assert_eq!(
            array_slots,
            [
                LIST_SLOT_PROFILE_OPTIONS,
                LIST_SLOT_PADS,
                LIST_SLOT_PROFILES
            ],
            "list slot names drifted between the compiler/StatusPage.ts and \
             the LIST_SLOT_* constants; slots: {names:?}"
        );
        assert_eq!(
            named_slot_ids(module, SHOW_SLOT_NAME).len(),
            SHOW_ORDER.len(),
            "show count drifted between StatusPage.ts and SHOW_ORDER; slots: {names:?}"
        );
    }

    #[test]
    fn render_injects_real_snapshot_data_into_ssr_html() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        // Phase 2 actually happened — not the Phase-1 empty-mount fallback.
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        // Scalars.
        assert!(out.html.contains("v1.21.442.0"), "{}", out.html);
        assert!(out.html.contains("keyboard filter active"));
        assert!(out.html.contains("yes"));
        assert!(out.html.contains("ksx.exe alive (pid 4242)"));
        assert!(out.html.contains("2026-08-04 12:00:00 UTC"));
        // Lists, all of them.
        assert!(out
            .html
            .contains("USB\\VID_045E&amp;PID_028E\\2&amp;AA&amp;0&amp;01"));
        assert!(out.html.contains("PlayStation (DS4) pad"));
        assert!(out.html.contains("Street Fighter"));
        assert!(out.html.contains("2 virtual pads exposed by the bus"));
        // The auto-refresh pragma and the no-client-JS shape.
        assert!(out
            .html
            .contains(r#"<meta http-equiv="refresh" content="5">"#));
        assert!(!out.html.contains("<script type=\"module\""));
    }

    /// Idle + reachable: the Start form renders (with the profiles as
    /// options), Stop does not, and no disabled-controls block appears.
    #[test]
    fn an_idle_reachable_daemon_renders_the_start_form_with_profile_options() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        assert!(out.html.contains("idle — daemon reachable"), "{}", out.html);
        assert!(
            out.html.contains(r#"action="/session/start""#),
            "{}",
            out.html
        );
        assert!(out.html.contains("(config default)"));
        // The reconcile markers sit inside the <option> tags, so assert on
        // the select's inner text: an option's submitted value IS its text
        // content (comments excluded), which is what /session/start receives.
        let select_start = out.html.find(r#"name="profile""#).expect("select");
        let select = &out.html[select_start..];
        let select = &select[..select.find("</select>").expect("closed select")];
        assert!(
            select.contains("Street Fighter"),
            "profile options must come from the snapshot's profiles: {select}"
        );
        assert!(!out.html.contains(r#"action="/session/stop""#));
        assert!(!out.html.contains("controls disabled"));
    }

    /// Running: Stop + Reload render, Start does not.
    #[test]
    fn a_running_session_renders_stop_and_reload() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(&page, &sample(), &running_session(), None);
        assert!(out.html.contains("running — Street Fighter — 4 pad(s)"));
        assert!(out.html.contains(r#"action="/session/stop""#));
        assert!(out.html.contains(r#"action="/config/reload""#));
        assert!(!out.html.contains(r#"action="/session/start""#));
    }

    /// No control channel: every control renders DISABLED with the reason —
    /// visible, inert, honest. No live form may appear.
    #[test]
    fn an_unreachable_daemon_renders_disabled_controls_with_the_reason() {
        let page = EmbeddedPage::load().unwrap();
        let session = SessionView::unreachable("no daemon control channel");
        let out = render_status(&page, &sample(), &session, None);
        assert!(
            out.html.contains("no daemon control channel"),
            "{}",
            out.html
        );
        assert!(out.html.contains("controls disabled"), "{}", out.html);
        assert!(out.html.contains("`ksx daemon`"));
        assert!(out.html.contains("disabled"));
        assert!(!out.html.contains(r#"action="/session/start""#));
        assert!(!out.html.contains(r#"action="/session/stop""#));
        assert!(!out.html.contains(r#"action="/config/reload""#));
    }

    #[test]
    fn a_flash_message_renders_only_when_present() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(
            &page,
            &sample(),
            &idle_session(),
            Some("error: already running"),
        );
        assert!(out.html.contains("error: already running"), "{}", out.html);
        let out = render_status(&page, &sample(), &idle_session(), None);
        assert!(!out.html.contains(r#"class="flash""#), "{}", out.html);
    }

    /// The flash arrives from a query parameter — attacker-writable — and
    /// must be escaped like everything else.
    #[test]
    fn a_hostile_flash_is_escaped_not_injected() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(
            &page,
            &sample(),
            &idle_session(),
            Some("<script>alert(1)</script>"),
        );
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );
    }

    #[test]
    fn render_survives_an_empty_snapshot() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(
            &page,
            &StatusSnapshot::default(),
            &SessionView::default(),
            None,
        );
        assert!(out.html.contains("data-forma-ssr"));
        assert!(out.html.contains("no virtual pads exposed by the bus"));
        assert!(out.html.contains("no profiles in games.toml"));
    }

    #[test]
    fn snapshot_html_is_escaped_not_injected() {
        let page = EmbeddedPage::load().unwrap();
        let mut snap = sample();
        snap.vigem = "<script>alert(1)</script>".into();
        let out = render_status(&page, &snap, &idle_session(), None);
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );
    }
}
