//! The render seam: embedded FMIR + per-request [`StatusSnapshot`] → HTML.
//!
//! # Data-injection mechanism (and why)
//!
//! forma-server 0.1.4 supports true server-side prop injection: the compiler
//! declares NAMED slots in the FMIR slot table, and `forma_ir::slot::SlotData`
//! is populated by the handler before the IR walk. That is the mechanism used
//! here — no JSON island, no string templating.
//!
//! Two flavours of slot exist on this page:
//!
//! - **Scalars** — every `createSignal` in `studio-ui/src/StatusPage.ts`
//!   becomes a slot named after the signal getter. Unique names, injected via
//!   [`SlotData::from_json`] (name-keyed, defaults preserved for misses).
//! - **Lists** — every `createList` becomes a slot literally named
//!   `list:array`. The compiler (0.1.8) gives ALL lists that same name, so
//!   name-keyed injection cannot address more than one list per page; the
//!   seam resolves them **positionally** instead (slot-table order == emission
//!   order == document order). [`LIST_ORDER`] documents the mapping and
//!   `tests::embedded_ir_slot_layout_matches_the_seam` pins it — reordering
//!   lists in StatusPage.ts without updating both is a test failure, not a
//!   silent blank section.
//!
//! Upstream feature request this dogfoods (docs/ENHANCEMENTS.md E7 loop):
//! per-list slot naming (e.g. `list:<binding>`), so multi-list pages can be
//! injected by name alone.

use forma_ir::parser::IrModule;
use forma_ir::slot::{SlotData, SlotValue};
use forma_server::{render_page, AssetManifest, PageConfig, PageOutput, RenderMode};
use rust_embed::Embed;

use crate::error::StudioError;
use crate::snapshot::StatusSnapshot;

/// The committed `studio-ui` build output (see the crate docs for the
/// regeneration command — Node is never needed to build or run ksx).
#[derive(Embed)]
#[folder = "assets/"]
pub(crate) struct Assets;

/// The compiler names every `createList` array slot identically; lists are
/// therefore resolved by position. Document order in StatusPage.ts:
/// virtual pads first, then game profiles.
const LIST_SLOT_NAME: &str = "list:array";
const LIST_ORDER: [&str; 2] = ["pads", "profiles"];
const LIST_COUNT: usize = LIST_ORDER.len();

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
fn scalar_slots(snap: &StatusSnapshot) -> serde_json::Value {
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

/// The two list arrays, in [`LIST_ORDER`].
fn list_values(snap: &StatusSnapshot) -> [SlotValue; LIST_COUNT] {
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
    [pads, profiles]
}

/// Slot ids of every `list:array` entry, in slot-table (== document) order.
fn list_array_slot_ids(module: &IrModule) -> Vec<u16> {
    module
        .slots
        .entries()
        .iter()
        .filter(|e| {
            module
                .strings
                .get(e.name_str_idx)
                .is_ok_and(|name| name == LIST_SLOT_NAME)
        })
        .map(|e| e.slot_id)
        .collect()
}

/// Populate every server-injected slot from the snapshot.
fn build_slots(module: &IrModule, snap: &StatusSnapshot) -> SlotData {
    // Scalars by name; starts from IR defaults, so a renamed signal degrades
    // to its authored default ("not collected"), never to garbage.
    let scalars = scalar_slots(snap).to_string();
    let mut slots = SlotData::from_json(&scalars, module)
        .unwrap_or_else(|_| SlotData::new_from_defaults(&module.slots));

    // Lists by position (see module docs).
    let ids = list_array_slot_ids(module);
    for (id, value) in ids.into_iter().zip(list_values(snap)) {
        slots.set(id, value);
    }
    slots
}

/// Render the status page for one snapshot. Falling back to Phase 1 (an empty
/// `#app` with zero client JS, i.e. a blank page) can only happen if the
/// embedded IR is broken — which `EmbeddedPage::load` already refused.
pub(crate) fn render_status(page: &EmbeddedPage, snap: &StatusSnapshot) -> PageOutput {
    let slots = build_slots(&page.module, snap);
    render_page(&PageConfig {
        title: "ksx Studio — cabinet status",
        route_pattern: "/",
        manifest: &page.manifest,
        config_script: None,
        body_class: None,
        personality_css: None,
        // The 2-second auto-refresh. A meta pragma is processed wherever the
        // element is inserted (WHATWG "pragma directives"), head or body; the
        // server also sends an HTTP `Refresh: 2` header (server.rs) as belt
        // and braces. No JS, nothing for the hardcoded CSP to block.
        body_prefix: Some(r#"<meta http-equiv="refresh" content="2">"#),
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
    /// name exists, and there are exactly as many `list:array` slots as
    /// [`LIST_ORDER`] claims. Fails when StatusPage.ts and this file drift.
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

        let scalars = scalar_slots(&StatusSnapshot::default());
        for key in scalars.as_object().unwrap().keys() {
            assert!(
                names.contains(&key.as_str()),
                "scalar slot '{key}' missing from the embedded IR; slots: {names:?}"
            );
        }
        assert_eq!(
            list_array_slot_ids(module).len(),
            LIST_ORDER.len(),
            "list count drifted between StatusPage.ts and LIST_ORDER; slots: {names:?}"
        );
    }

    #[test]
    fn render_injects_real_snapshot_data_into_ssr_html() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(&page, &sample());
        // Phase 2 actually happened — not the Phase-1 empty-mount fallback.
        assert!(out.html.contains("data-forma-ssr"), "{}", out.html);
        // Scalars.
        assert!(out.html.contains("v1.21.442.0"), "{}", out.html);
        assert!(out.html.contains("keyboard filter active"));
        assert!(out.html.contains("yes"));
        assert!(out.html.contains("ksx.exe alive (pid 4242)"));
        assert!(out.html.contains("2026-08-04 12:00:00 UTC"));
        // Lists, both of them.
        assert!(out
            .html
            .contains("USB\\VID_045E&amp;PID_028E\\2&amp;AA&amp;0&amp;01"));
        assert!(out.html.contains("PlayStation (DS4) pad"));
        assert!(out.html.contains("Street Fighter"));
        assert!(out.html.contains("2 virtual pads exposed by the bus"));
        // The auto-refresh pragma and the no-client-JS shape.
        assert!(out
            .html
            .contains(r#"<meta http-equiv="refresh" content="2">"#));
        assert!(!out.html.contains("<script type=\"module\""));
    }

    #[test]
    fn render_survives_an_empty_snapshot() {
        let page = EmbeddedPage::load().unwrap();
        let out = render_status(&page, &StatusSnapshot::default());
        assert!(out.html.contains("data-forma-ssr"));
        assert!(out.html.contains("no virtual pads exposed by the bus"));
        assert!(out.html.contains("no profiles in games.toml"));
    }

    #[test]
    fn snapshot_html_is_escaped_not_injected() {
        let page = EmbeddedPage::load().unwrap();
        let mut snap = sample();
        snap.vigem = "<script>alert(1)</script>".into();
        let out = render_status(&page, &snap);
        assert!(
            !out.html.contains("<script>alert(1)</script>"),
            "{}",
            out.html
        );
    }
}
