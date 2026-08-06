//! The render seam: embedded FMIR + per-request [`StatusSnapshot`] /
//! [`SessionView`] → HTML, with the same data emitted twice — slots for the
//! SSR first paint, island props for client hydration.
//!
//! # v4: SSR slots for first paint, islands props for hydration (and why both)
//!
//! The page is one Forma ISLAND (`StatusIsland`, compiled between
//! ISLAND_START/ISLAND_END opcodes). Per request this seam:
//!
//! 1. **Injects slots** exactly as v3 did — the compiler declares NAMED slots
//!    in the FMIR slot table, [`SlotData`] is populated before the IR walk,
//!    and the walker renders the full page server-side. This is what the
//!    browser paints before (or without) any JavaScript — the no-JS
//!    experience is still the complete v3 page, plus a `<noscript>` meta
//!    refresh so it keeps updating.
//! 2. **Emits the SAME data as island props** — a [`StatusPayload`] JSON in
//!    the `__forma_islands` script block (the islands protocol's script-tag
//!    props mode; a non-executing `type="application/json"` data block, so
//!    the strict CSP is untouched). The client seeds its signals from it
//!    BEFORE adoption — dogfood ledger #5: adoption binds effects that
//!    immediately write signal state into the DOM, so plain hydration
//!    clobbers SSR values; islands-with-props is the one sanctioned live
//!    path. After adoption a 2 s poller rewrites the same signals from
//!    `GET /api/status`, which serves the identical [`StatusPayload`] shape
//!    (parity pinned by `island_props_match_the_api_payload_shape`).
//!
//!    Keeping BOTH emissions is deliberate: slots alone give a correct first
//!    paint but hydration would clobber it (ledger #5); props alone would
//!    require client rendering and break the no-JS baseline. The redundancy
//!    is the design, not an accident — same struct, same serializer, one
//!    derivation mirror (StatusIsland.ts) covered by tests on this side.
//!
//!    The props ride our own script block rather than a `data-forma-props`
//!    attribute because compiler 0.2.0 registers islands with EMPTY
//!    `slot_ids` — the walker's own props emission never fires (ledger #8).
//!
//! Three flavours of slot exist on this page:
//!
//! - **Scalars** — every `createSignal` in `studio-ui/src/StatusPage.ts`
//!   (the compile-time slot-table declaration; the runtime twin lives in
//!   `StatusIsland.ts`) becomes a slot named after the signal getter.
//!   Unique names, injected via [`SlotData::from_json`] (name-keyed,
//!   defaults preserved for misses).
//! - **Lists** — every `createList` becomes an Array slot. Since the v4
//!   lists read from named signals (`() => padTiles()`), compiler 0.2.0
//!   derives the slot name from the BINDING (`list:padTiles:array`) instead
//!   of the positional `list:#N:array` v3 lived with — reordering lists in
//!   the page no longer shifts names (ledger #3, mostly resolved for us).
//!   Injected by NAME; the `LIST_SLOT_*` constants pin the five names.
//! - **Shows** — every `createShow` still becomes a Bool slot named
//!   `show:createShow` — so shows remain the one POSITIONAL seam
//!   (slot-table order == emission order == document order). [`SHOW_ORDER`]
//!   documents that mapping; the show pairs are what color state server-side
//!   (the server picks which statically-styled variant renders), and after
//!   hydration the same pairs flip live from client signals.
//!
//! `tests::embedded_ir_slot_layout_matches_the_seam` pins the exact list slot
//! NAMES (order included), the show count, and the island table — a compiler
//! bump that renames slots, or a StatusIsland.ts edit that adds/reorders
//! lists or shows, is a test failure, not a silently blank section.
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
use crate::snapshot::{StatusPayload, StatusSnapshot};

/// The committed `studio-ui` build output (see the crate docs for the
/// regeneration command — Node is never needed to build or run ksx).
#[derive(Embed)]
#[folder = "assets/"]
pub(crate) struct Assets;

/// List array slot names, BINDING-derived since the v4 lists read from named
/// signals (`() => padTiles()` → `list:padTiles:array`); a signal source
/// used by several lists gets `#N` occurrence suffixes in document order
/// (the two profile-row lists share `profileRows`). Rename a list signal in
/// StatusIsland.ts and the layout test fails until these match again.
const LIST_SLOT_PROFILE_OPTIONS: &str = "list:profileOptions:array";
const LIST_SLOT_PADS: &str = "list:padTiles:array";
const LIST_SLOT_GHOST_PADS: &str = "list:ghostTiles:array";
const LIST_SLOT_PROFILES_LIVE: &str = "list:profileRows:array";
const LIST_SLOT_PROFILES_PLAIN: &str = "list:profileRows#2:array";

/// The island table this page compiles to: exactly one island — the whole
/// screen — hydrated on load. Its id keys the props in the
/// `__forma_islands` script block; its name is the `activateIslands`
/// registry key in `studio-ui/src/status.ts`. The layout test pins both.
pub(crate) const ISLAND_ID: u16 = 0;
#[cfg(test)]
const ISLAND_COMPONENT: &str = "StatusIsland";

/// `createShow` booleans did NOT gain unique names in compiler 0.2.0 — every
/// show is still `show:createShow`, so shows are the remaining positional
/// seam (slot-table order == document order in StatusPage.ts). All state
/// COLOR on this SSR page is done with show pairs — the server picks which
/// statically-styled variant renders — so the list is long; the layout test
/// pins the count.
const SHOW_SLOT_NAME: &str = "show:createShow";
const SHOW_ORDER: [&str; 17] = [
    "header pill: running",
    "header pill: idle",
    "header pill: no daemon",
    // FIX 1: the unmissable banner, first child of <main> on BOTH pages.
    "no-daemon banner (top of page)",
    "flash: success",
    "flash: error",
    "start controls",
    "stop controls",
    "daemon down controls",
    // v14 moved the plumbing panel BELOW the profiles list (it is tertiary
    // information and was reading as loud as the session), so the profile-row
    // pair now precedes the driver/autostart pills. Shows are positional
    // (ledger #4): this order IS the document order in StatusIsland.ts.
    "profile rows: with Start buttons",
    "profile rows: inert",
    "vigem: ok pill",
    "vigem: attention pill",
    "interception: borrowed-time pill",
    "interception: absent pill",
    "autostart: on pill",
    "autostart: off pill",
];
const SHOW_COUNT: usize = SHOW_ORDER.len();

/// Seconds between full-page refreshes for the NO-JS fallback only (v4): the
/// meta pragma now lives inside `<noscript>`, so browsers running the island
/// poller never reload. Was 2 s while the page was read-only; a page with a
/// dropdown must leave the no-JS user time to aim at it before the reload
/// closes it.
pub(crate) const REFRESH_SECS: u32 = 5;

/// Inline `<style nonce>` applied before the stylesheet arrives (canon
/// template's anti-flash trick): the body starts on the studio ground color
/// instead of flashing white. Values mirror `--bg`/`--text` in studio.css,
/// both schemes.
const PERSONALITY_CSS: &str = "body{background:#0b0e14;color:#dbe2ef;margin:0}\
@media (prefers-color-scheme:light){body{background:#f2f4f8;color:#1a2130}}";

/// The minimum number of pad tiles the signature card shows: live pads
/// first, then ghost outlines up to this floor (a 4-slot XInput cabinet at
/// rest still LOOKS like a 4-slot cabinet). More than four live pads simply
/// render more tiles — 8-player DS4 sessions show all eight.
const PAD_TILE_FLOOR: usize = 4;

/// Parsed once at server start; immutable afterwards.
pub(crate) struct EmbeddedPage {
    pub(crate) manifest: AssetManifest,
    pub(crate) module: IrModule,
}

impl EmbeddedPage {
    /// Load the embedded page for one manifest route (`"/"` = status,
    /// `"/map"` = mapper).
    pub(crate) fn load(route: &str) -> Result<Self, StudioError> {
        let manifest_json = Assets::get("manifest.json")
            .ok_or_else(|| StudioError::Asset("manifest.json missing from embed".into()))?;
        let manifest: AssetManifest = serde_json::from_slice(&manifest_json.data)
            .map_err(|e| StudioError::Asset(format!("manifest.json unparsable: {e}")))?;

        let ir_name = manifest
            .route(route)
            .and_then(|r| r.ir.clone())
            .ok_or_else(|| {
                StudioError::Asset(format!("manifest route '{route}' has no .ir entry"))
            })?;
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

/// The vendored controller art, served from the embed (`/_assets/...`).
/// Gamepad-Asset-Pack by AL2009man, MIT — see `studio-ui/art/README.md`; the
/// page footers carry the visible credit (pinned by tests on both pages).
pub(crate) const ART_XBOX: &str = "/_assets/pad-xbox.svg";
pub(crate) const ART_DS4: &str = "/_assets/pad-ds4.svg";

/// The exact command that starts a daemon for THIS machine's configuration.
///
/// The profile flag matters: on a cabinet whose slots live in games.toml,
/// plain `ksx daemon` refuses to start ("nothing to run"), so printing it as
/// the remedy would send the user in a circle. `SessionView::profile` carries
/// the title — from the pipe when the daemon answers, and from the config when
/// it does not, which is precisely the case this string exists for.
pub(crate) fn daemon_command(session: &SessionView) -> String {
    match session.profile.as_deref().map(str::trim) {
        Some(profile) if !profile.is_empty() => format!("ksx daemon --game \"{profile}\""),
        _ => "ksx daemon".to_owned(),
    }
}

/// The headline of the no-daemon banner, on both pages, word for word.
///
/// It is deliberately blunt about the SPLIT — read works, write does not —
/// because the failure Victor hit was a page that looked completely normal and
/// silently ignored every click.
///
/// The string itself lives in the TypeScript (`StatusIsland.ts` /
/// `MapIsland.ts`) because it is static markup, not injected data — so this is
/// the test oracle that keeps the two pages saying the same sentence, and is
/// compiled only for tests.
#[cfg(test)]
pub(crate) const NO_DAEMON_HEADLINE: &str =
    "No daemon — ksx Studio can see your config but cannot change anything.";

/// Pick the art for a persona LABEL ("PlayStation (DS4) pad") or persona id
/// ("playstation"). Anything un-PlayStation renders as the Xbox pad — the
/// cabinet's default persona.
pub(crate) fn art_for(persona: &str) -> &'static str {
    let lower = persona.to_ascii_lowercase();
    if lower.contains("playstation") || lower.contains("ds4") || lower.contains("ps4") {
        ART_DS4
    } else {
        ART_XBOX
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
        // FIX 1: the copyable remedy, with this machine's profile flag.
        "daemonCmd": daemon_command(session),
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
///
/// The two profile ROW lists carry the same array — which one renders is
/// decided by the show pair around them (Start buttons only when a start
/// could actually be accepted). The pad tiles get a server-computed player
/// number ("P1"…), and the ghost list pads the grid out to
/// [`PAD_TILE_FLOOR`].
fn list_values(snap: &StatusSnapshot) -> [(&'static str, SlotValue); 5] {
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
            .enumerate()
            .map(|(i, p)| {
                SlotValue::Object(vec![
                    ("player".to_owned(), SlotValue::Text(format!("P{}", i + 1))),
                    ("persona".to_owned(), SlotValue::Text(p.persona.clone())),
                    ("instance".to_owned(), SlotValue::Text(p.instance.clone())),
                    // Real controller art per persona (replaces the v3 hand
                    // silhouettes) + the tile's jump into the mapper.
                    (
                        "art".to_owned(),
                        SlotValue::Text(art_for(&p.persona).to_owned()),
                    ),
                    (
                        "maphref".to_owned(),
                        SlotValue::Text(format!("/map?slot={}", i + 1)),
                    ),
                ])
            })
            .collect(),
    );
    let ghosts = SlotValue::Array(
        (snap.pads.len()..PAD_TILE_FLOOR)
            .map(|i| {
                SlotValue::Object(vec![(
                    "slot".to_owned(),
                    SlotValue::Text(format!("P{}", i + 1)),
                )])
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
        (LIST_SLOT_GHOST_PADS, ghosts),
        (LIST_SLOT_PROFILES_LIVE, profiles.clone()),
        (LIST_SLOT_PROFILES_PLAIN, profiles),
    ]
}

/// Badge derivations from the presentation-shaped snapshot lines. The
/// snapshot contract deliberately ships composed sentences (ksx-app owns
/// the wording); these prefixes are the stable part of that wording and the
/// unit tests pin them. Anything unrecognized degrades to the WARN side —
/// a pill must never say OK about a line it does not understand.
fn vigem_ok(snap: &StatusSnapshot) -> bool {
    snap.vigem.starts_with("installed — service running")
}

fn interception_installed(snap: &StatusSnapshot) -> bool {
    snap.interception.starts_with("installed")
}

fn autostart_on(snap: &StatusSnapshot) -> bool {
    snap.autostart.starts_with("registered")
}

/// The show booleans, in [`SHOW_ORDER`]. The session-controls policy is
/// unchanged: exactly one of "start", "stop" or "daemon down" is true, so
/// the panel always says something and never offers a dead button as live.
/// The same rule colors the header pill, and every status pill is a pair
/// where exactly one side renders.
fn show_values(
    snap: &StatusSnapshot,
    session: &SessionView,
    flash: Option<&str>,
) -> [bool; SHOW_COUNT] {
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let can_start = session.reachable && !session.running;
    [
        session.reachable && session.running,
        can_start,
        !session.reachable,
        !session.reachable, // the top-of-page banner
        flash.is_some() && !flash_err,
        flash_err,
        can_start,
        session.reachable && session.running,
        !session.reachable,
        can_start,
        !can_start,
        vigem_ok(snap),
        !vigem_ok(snap),
        interception_installed(snap),
        !interception_installed(snap),
        autostart_on(snap),
        !autostart_on(snap),
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
        .zip(show_values(snap, session, flash))
    {
        slots.set(id, SlotValue::Bool(value));
    }
    slots
}

/// The island props JSON: `{"0": <payload>}`, keyed by [`ISLAND_ID`] the way
/// `loadIslandProps` expects shared props (both pages compile to a single
/// island with id 0 — each pins that in its layout test). `<` is JSON-escaped
/// so a hostile snapshot line can never close the `<script>` data block early
/// — inside JSON, `<` only occurs in strings, where `<` is equivalent.
pub(crate) fn island_props_json<T: serde::Serialize>(payload: &T) -> String {
    let mut by_island = serde_json::Map::new();
    by_island.insert(
        ISLAND_ID.to_string(),
        serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
    );
    serde_json::Value::Object(by_island)
        .to_string()
        .replace('<', "\\u003c")
}

/// Everything that precedes `#app`: the no-JS fallback refresh and the
/// island props block.
///
/// - The `<noscript>` meta refresh targets `refresh_url` WITHOUT any query
///   string: a flash arrives via /?flash=… (post-redirect), shows for one
///   cycle, and the next no-JS refresh lands on a clean URL. (With JS the
///   poller keeps the page live and the entry clears the flash + URL itself.)
/// - The props block is `type="application/json"` — a data block, never
///   executed, outside the CSP's script-src entirely; the client reads it by
///   id (`__forma_islands`, the islands protocol's script-tag props mode).
pub(crate) fn body_prefix<T: serde::Serialize>(payload: &T, refresh_url: &str) -> String {
    format!(
        "<noscript><meta http-equiv=\"refresh\" content=\"{REFRESH_SECS}; url={refresh_url}\"></noscript>\
         <script id=\"__forma_islands\" type=\"application/json\">{}</script>",
        island_props_json(payload)
    )
}

/// Render the page for one snapshot + session view: SSR slots for first
/// paint, the same data as island props for hydration (module docs).
/// Falling back to Phase 1 (an empty `#app`, client mount from defaults)
/// can only happen if the embedded IR is broken — which
/// `EmbeddedPage::load` already refused.
pub(crate) fn render_status(
    page: &EmbeddedPage,
    snap: &StatusSnapshot,
    session: &SessionView,
    flash: Option<&str>,
) -> PageOutput {
    let slots = build_slots(&page.module, snap, session, flash);
    let payload = StatusPayload {
        snapshot: snap.clone(),
        session: session.clone(),
        flash: flash.map(str::to_owned),
    };
    let prefix = body_prefix(&payload, "/");
    render_page(&PageConfig {
        title: "ksx Studio — cabinet status",
        route_pattern: "/",
        manifest: &page.manifest,
        config_script: None,
        body_class: None,
        personality_css: Some(PERSONALITY_CSS),
        body_prefix: Some(&prefix),
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
            profile: None,
        }
    }

    fn running_session() -> SessionView {
        SessionView {
            reachable: true,
            running: true,
            line: "running — Street Fighter — 4 pad(s)".into(),
            profile: Some("Street Fighter".into()),
        }
    }

    #[test]
    fn embedded_page_loads_and_ir_is_fmir_v2() {
        let page = EmbeddedPage::load("/").expect("embedded page must load");
        assert_eq!(page.module().header.version, 2);
        // The raw-bytes guard from the forma spike: FMIR magic + u16 LE 2.
        let ir_name = page.manifest.route("/").unwrap().ir.clone().unwrap();
        let bytes = Assets::get(&ir_name).unwrap().data;
        assert_eq!(&bytes[0..6], b"FMIR\x02\x00");
    }

    /// Pins the slot-table contract the seam depends on: every scalar signal
    /// name exists, the list array slot NAMES are exactly the ones the
    /// `LIST_SLOT_*` constants claim (order included), there are exactly
    /// as many `show:createShow` slots as [`SHOW_ORDER`] claims, and the
    /// island table is the one island the client registry activates. Fails
    /// when StatusPage.ts/StatusIsland.ts, the compiler's naming scheme, or
    /// this file drift.
    #[test]
    fn embedded_ir_slot_layout_matches_the_seam() {
        let page = EmbeddedPage::load("/").unwrap();
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
                LIST_SLOT_GHOST_PADS,
                LIST_SLOT_PROFILES_LIVE,
                LIST_SLOT_PROFILES_PLAIN
            ],
            "list slot names drifted between the compiler/StatusIsland.ts and \
             the LIST_SLOT_* constants; slots: {names:?}"
        );
        assert_eq!(
            named_slot_ids(module, SHOW_SLOT_NAME).len(),
            SHOW_ORDER.len(),
            "show count drifted between StatusIsland.ts and SHOW_ORDER; slots: {names:?}"
        );
        // The island table: exactly one island, the whole screen, hydrated
        // on load, named for the activateIslands registry key in status.ts.
        // Its `slot_ids` are EMPTY under compiler 0.2.0 (ledger #8) — the
        // reason props ride our own `__forma_islands` block, so if a
        // compiler bump starts populating them, this fails and the seam
        // gets to adopt the native props path.
        let islands = module.islands.entries();
        assert_eq!(islands.len(), 1, "expected exactly one island");
        assert_eq!(islands[0].id, ISLAND_ID);
        assert_eq!(
            module.strings.get(islands[0].name_str_idx).unwrap(),
            ISLAND_COMPONENT
        );
        assert!(
            islands[0].slot_ids.is_empty(),
            "compiler now populates island slot_ids — consider native \
             data-forma-props emission instead of the __forma_islands block"
        );
    }

    #[test]
    fn render_injects_real_snapshot_data_into_ssr_html() {
        let page = EmbeddedPage::load("/").unwrap();
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
        // The auto-refresh is the NO-JS fallback only (v4): the pragma still
        // targets "/" (flash-clearing) but lives inside <noscript>, so the
        // island poller never fights a reload.
        assert!(
            out.html
                .contains(r#"<noscript><meta http-equiv="refresh" content="5; url=/"></noscript>"#),
            "{}",
            out.html
        );
    }

    /// The v4 island shape: the SSR walker stamps the island attributes on
    /// the page root, the props block carries the payload, and the client
    /// bundle loads via a NONCE'd module script (the strict CSP allows
    /// nothing else). The anti-flash personality CSS rides the same nonce.
    #[test]
    fn render_emits_the_island_its_props_and_nonced_scripts() {
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        // Island attributes on the SSR root (walker-emitted).
        assert!(
            out.html
                .contains(r#"data-forma-island="0" data-forma-component="StatusIsland""#),
            "{}",
            out.html
        );
        assert!(out.html.contains(r#"data-forma-hydrate="load""#));
        // The props data block (non-executing, CSP-exempt).
        assert!(
            out.html
                .contains(r#"<script id="__forma_islands" type="application/json">"#),
            "{}",
            out.html
        );
        // The client bundle ships again, and its tag carries the CSP nonce.
        let nonce = out
            .csp
            .split("'nonce-")
            .nth(1)
            .and_then(|s| s.split('\'').next())
            .expect("csp carries a nonce");
        assert!(
            out.html.contains(&format!(
                r#"<script type="module" nonce="{nonce}" src="/_assets/"#
            )),
            "module script must carry the CSP nonce: {}",
            out.html
        );
        assert!(
            out.html.contains(&format!(
                r#"<style nonce="{nonce}">body{{background:#0b0e14"#
            )),
            "personality css must carry the CSP nonce: {}",
            out.html
        );
    }

    /// Ledger #5's contract, server side: the island props ARE the
    /// /api/status payload — one struct, one serializer, so the signals the
    /// client seeds before adoption and the ones the poller overwrites can
    /// never see different shapes. (The poller itself only runs in a
    /// browser; visual confirmation stays a manual step.)
    #[test]
    fn island_props_match_the_api_payload_shape() {
        let payload = StatusPayload {
            snapshot: sample(),
            session: idle_session(),
            flash: None,
        };
        let props = island_props_json(&payload);
        let parsed: serde_json::Value = serde_json::from_str(&props).expect("props parse");
        assert_eq!(
            parsed[ISLAND_ID.to_string()],
            serde_json::to_value(&payload).unwrap(),
            "island props must be byte-compatible with what /api/status serves"
        );
        // And the rendered page embeds exactly that block.
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &payload.snapshot, &payload.session, None);
        assert!(out.html.contains(&props), "{}", out.html);
    }

    /// The props block is a data block inside HTML: a hostile snapshot line
    /// must not be able to close the script element early.
    #[test]
    fn island_props_cannot_break_out_of_the_script_block() {
        let mut snap = sample();
        snap.vigem = "</script><script>alert(1)</script>".into();
        let payload = StatusPayload {
            snapshot: snap,
            session: idle_session(),
            flash: Some("</script>".into()),
        };
        let props = island_props_json(&payload);
        assert!(!props.contains('<'), "unescaped '<' in props: {props}");
        let parsed: serde_json::Value = serde_json::from_str(&props).expect("still valid JSON");
        assert_eq!(
            parsed[ISLAND_ID.to_string()]["snapshot"]["vigem"],
            serde_json::json!("</script><script>alert(1)</script>"),
            "escaping must be lossless"
        );
    }

    /// The signature card: live pads render as accent tiles with a player
    /// number, persona, the REAL controller art (v5: Gamepad-Asset-Pack
    /// renders replaced the v3 hand-drawn silhouettes) and a per-slot jump
    /// into the mapper; the grid is padded with ghost tiles up to the
    /// four-slot floor.
    #[test]
    fn pad_tiles_render_art_maplinks_and_ghosts_up_to_the_floor() {
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        // Two live pads…
        assert!(out.html.contains(r#"class="padtile live""#), "{}", out.html);
        assert!(out.html.contains(">P1<"), "{}", out.html);
        assert!(out.html.contains(">P2<"), "{}", out.html);
        assert!(out.html.contains("Xbox 360 pad"));
        // …with the vendored art per persona (P1 xbox, P2 playstation)…
        assert!(
            out.html.contains(r#"src="/_assets/pad-xbox.svg""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"src="/_assets/pad-ds4.svg""#),
            "{}",
            out.html
        );
        // …and a per-slot Map affordance into the mapper page.
        assert!(out.html.contains(r#"href="/map?slot=1""#), "{}", out.html);
        assert!(out.html.contains(r#"href="/map?slot=2""#), "{}", out.html);
        // …two ghosts to reach the floor of four…
        assert!(out.html.contains(r#"class="padtile ghost""#));
        assert!(out.html.contains(">P3<"), "{}", out.html);
        assert!(out.html.contains(">P4<"), "{}", out.html);
        assert!(!out.html.contains(">P5<"));
    }

    /// Both vendored art files are really embedded (rust-embed picks up
    /// assets/), and the footer carries the MIT attribution the vendoring
    /// promised (studio-ui/art/README.md).
    #[test]
    fn the_art_is_embedded_and_credited() {
        assert!(
            Assets::get("pad-xbox.svg").is_some(),
            "pad-xbox.svg missing from embed"
        );
        assert!(
            Assets::get("pad-ds4.svg").is_some(),
            "pad-ds4.svg missing from embed"
        );
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        assert!(
            out.html.contains("Gamepad-Asset-Pack (MIT) by AL2009man"),
            "{}",
            out.html
        );
        // And the header links into the mapper.
        assert!(out.html.contains(r#"href="/map""#), "{}", out.html);
    }

    #[test]
    fn art_for_maps_personas_to_the_vendored_files() {
        assert_eq!(art_for("Xbox 360 pad"), ART_XBOX);
        assert_eq!(art_for("xbox360"), ART_XBOX);
        assert_eq!(art_for("PlayStation (DS4) pad"), ART_DS4);
        assert_eq!(art_for("playstation"), ART_DS4);
        assert_eq!(art_for("something unknown"), ART_XBOX, "default persona");
    }

    /// Status pills: exactly one side of each pair renders. The sample
    /// snapshot is all-healthy except Interception, which is installed and
    /// therefore on borrowed time (amber), never a paragraph-only warning.
    #[test]
    fn status_pills_pick_exactly_one_side_per_pair() {
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        // Header pill: idle.
        assert!(
            out.html.contains(r#"class="pill pill-idle">idle<"#),
            "{}",
            out.html
        );
        assert!(!out.html.contains(r#"class="pill pill-run""#));
        // ViGEmBus healthy, Interception installed → borrowed time.
        assert!(out.html.contains(">OK<"), "{}", out.html);
        assert!(out.html.contains(">borrowed time<"), "{}", out.html);
        assert!(!out.html.contains(">attention<"));
        assert!(!out.html.contains(">absent<"));
        // Autostart registered → on.
        assert!(
            out.html.contains(r#"class="pill pill-ok">on<"#),
            "{}",
            out.html
        );
    }

    /// A degraded snapshot must not say OK about anything.
    #[test]
    fn a_degraded_snapshot_renders_warn_pills_not_ok() {
        let page = EmbeddedPage::load("/").unwrap();
        let snap = StatusSnapshot::degraded("collector panicked");
        let out = render_status(&page, &snap, &SessionView::default(), None);
        assert!(!out.html.contains(">OK<"), "{}", out.html);
        assert!(out.html.contains(">attention<"), "{}", out.html);
        assert!(out.html.contains(">absent<"), "{}", out.html);
    }

    /// Profile rows carry their own one-click Start form when a start could
    /// be accepted — the hidden input's value is the exact profile title the
    /// daemon will be asked for.
    #[test]
    fn profile_rows_get_start_buttons_only_when_startable() {
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &sample(), &idle_session(), None);
        assert!(
            out.html
                .contains(r#"name="profile" value="Street Fighter""#),
            "{}",
            out.html
        );
        // Running: rows render inert — no per-row forms, no start actions.
        let out = render_status(&page, &sample(), &running_session(), None);
        assert!(out.html.contains("Street Fighter"), "{}", out.html);
        assert!(!out
            .html
            .contains(r#"name="profile" value="Street Fighter""#));
    }

    /// Idle + reachable: the Start form renders (with the profiles as
    /// options), Stop does not, and no disabled-controls block appears.
    #[test]
    fn an_idle_reachable_daemon_renders_the_start_form_with_profile_options() {
        let page = EmbeddedPage::load("/").unwrap();
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
        let page = EmbeddedPage::load("/").unwrap();
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
        let page = EmbeddedPage::load("/").unwrap();
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
        let page = EmbeddedPage::load("/").unwrap();
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
        let page = EmbeddedPage::load("/").unwrap();
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
        let page = EmbeddedPage::load("/").unwrap();
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
        let page = EmbeddedPage::load("/").unwrap();
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
