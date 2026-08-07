//! The render seam: embedded FMIR + per-request [`StatusSnapshot`] /
//! [`SessionView`] → HTML, with the same data emitted twice — slots for the
//! SSR first paint, the source payload for client hydration.
//!
//! # SSR slots for first paint, a payload block for hydration (and why both)
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
//! 2. **Emits the SAME data as the domain payload** — a [`StatusPayload`]
//!    JSON in the [`PAYLOAD_SCRIPT_ID`] script block (a non-executing
//!    `type="application/json"` data block, so the strict CSP is untouched).
//!    The client seeds its signals from it BEFORE adoption — dogfood ledger
//!    #5: adoption binds effects that immediately write signal state into the
//!    DOM, so plain hydration clobbers SSR values; seeding first is the one
//!    sanctioned live path. After adoption a 2 s poller rewrites the same
//!    signals from `GET /api/status`, which serves the identical
//!    [`StatusPayload`] shape (parity pinned by
//!    `the_payload_block_matches_the_api_payload_shape`).
//!
//!    Keeping BOTH emissions is deliberate: slots alone give a correct first
//!    paint but hydration would clobber it (ledger #5); the payload alone
//!    would require client rendering and break the no-JS baseline. The
//!    redundancy is the design, not an accident — same struct, same
//!    serializer, one derivation mirror (StatusIsland.ts) covered by tests on
//!    this side.
//!
//!    Since compiler 0.3.1 the walker ALSO emits `data-forma-props` on the
//!    island root, built from the island's `slot_ids` (ledger #8 closed — the
//!    `__forma_islands` impersonation this file used to hand-emit is gone).
//!    Those native props carry the rendered SLOT values; the block above
//!    carries the SOURCE payload the client's own model needs. Ledger #19.
//!
//! Three flavours of slot exist on this page:
//!
//! - **Scalars** — every `createSignal` in `studio-ui/src/StatusIsland.ts`
//!   becomes a slot named after the signal getter. Unique names, injected via
//!   [`SlotData::from_json`] (name-keyed, defaults preserved for misses).
//!   Compiler 0.3.1 walks island component files for signal scopes, so the
//!   twin re-declarations `StatusPage.ts` used to carry are gone (ledger #9).
//! - **Lists** — every `createList` becomes an Array slot. Since the v4
//!   lists read from named signals (`() => padTiles()`), compiler 0.2.0
//!   derives the slot name from the BINDING (`list:padTiles:array`) instead
//!   of the positional `list:#N:array` v3 lived with — reordering lists in
//!   the page no longer shifts names (ledger #3, mostly resolved for us).
//!   Injected by NAME; the `LIST_SLOT_*` constants pin the five names.
//! - **Shows** — every `createShow` becomes a Bool slot named after its
//!   CONDITION binding (`createShow(() => canStart(), …)` →
//!   `show:canStart`), so shows are injected by name like everything else
//!   (compiler 0.3.1; dogfood ledger #4 closed 2026-08-06 — the `SHOW_ORDER`
//!   positional array and its "append, never insert" rule are gone). The show
//!   pairs are what color state server-side (the server picks which
//!   statically-styled variant renders), and after hydration the same pairs
//!   flip live from client signals.
//!
//! `tests::embedded_ir_slot_layout_matches_the_seam` pins the exact list slot
//! NAMES (order included), the exact show slot NAMES, and the island table
//! including its non-empty `slot_ids` — a compiler bump that renames slots,
//! or a StatusIsland.ts edit that adds/renames lists or shows, is a test
//! failure, not a silently blank section. It then hands the whole slot table
//! to [`assert_island_slot_contract`], which asserts the thing a name-exists
//! check cannot: that every name the seam injects is one the ISLAND RENDERS,
//! and that every scalar the island renders is one the seam injects. Read that
//! function before touching this seam — "the slot exists" was the assertion
//! that let a dead slot through the whole gate on 2026-08-06.
//!
//! History: compiler 0.1.8 named EVERY list `list:array`, and this seam
//! resolved lists positionally too (a `LIST_ORDER` table, since deleted).
//! Per-instance slot naming was the upstream feature request this page
//! dogfooded (docs/ENHANCEMENTS.md E7 loop); fixed upstream in
//! `@getforma/compiler` 0.2.0, adopted 2026-08-05 — the E7 dogfood loop's
//! first closed cycle. Per-instance `createShow` naming (ledger #4) and
//! populated island `slot_ids` (ledger #8) landed in 0.3.1 and were adopted
//! 2026-08-06; the same release stopped extracting signals from the root
//! `*Page` file ONLY (ledger #9), which is why `StatusPage.ts` is now four
//! lines instead of thirty declarations.

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

/// The brand icons: `favicon.ico`, `favicon.svg`, `apple-touch-icon.png`.
/// Written by `tools/icongen` from the two master SVGs — see
/// `assets/brand/README.md`.
///
/// A SECOND embed, rooted at `brand/` rather than `assets/`, for one concrete
/// reason: `assets/` is the OUTPUT DIRECTORY of `studio-ui/build.mjs`, and
/// `@getforma/build` `rmSync`s it at the top of every run. Icons parked there
/// would survive exactly until the next UI rebuild and then 404, with nothing
/// in any build log to say why. The icons are not UI build output and they do
/// not belong in the UI build's scratch space.
#[derive(Embed)]
#[folder = "brand/"]
pub(crate) struct BrandAssets;

/// The `<head>` icon links, identical on both pages.
///
/// Three declarations because three consumers choose differently, and each
/// wants a different file:
///
/// - **`favicon.svg`** — the modern path; browsers that support SVG icons
///   prefer it over the `.ico`. It is the SIMPLIFIED master, not the
///   detailed one, because what a browser does with it is render it into a
///   16–32 px tab. Handing it the detailed art would throw away the whole
///   point of having two drawings.
/// - **`favicon.ico`** — the fallback, and what Windows itself reads when the
///   page is pinned or saved as a shortcut. No `sizes` attribute: the file
///   carries eight size-specific entries and the consumer reads the ICO
///   directory to choose. A hand-written list here could only ever go stale
///   against `tools/icongen`'s table.
/// - **`apple-touch-icon.png`** — 180 px, flattened onto the plate colour, so
///   iOS's home-screen mask has no transparent corners to composite black.
///
/// # Why this is spliced in rather than passed to `render_page`
///
/// forma-server 0.1.4's `PageConfig` has no head hook — `title`,
/// `config_script`, `body_class`, `personality_css`, `body_prefix` is the
/// whole list, and none of them reaches `<head>`'s link section. One
/// `</head>` insertion is a smaller and more honest workaround than forking
/// the template, and the tests below fail the moment upstream grows a real
/// hook or changes the markup — which is the point at which this should go
/// away.
const ICON_LINKS: &str = concat!(
    r#"<link rel="icon" href="/favicon.svg" type="image/svg+xml">"#,
    r#"<link rel="icon" href="/favicon.ico" type="image/x-icon">"#,
    r#"<link rel="apple-touch-icon" href="/apple-touch-icon.png">"#,
);

/// Splice [`ICON_LINKS`] into a rendered page's `<head>`.
///
/// A missing `</head>` costs the page its icons and nothing else, so it is a
/// warning rather than a panic: a status page that renders without a favicon
/// still tells the user whether their cabinet is working.
pub(crate) fn with_icon_links(mut out: PageOutput) -> PageOutput {
    match out.html.find("</head>") {
        Some(at) => out.html.insert_str(at, ICON_LINKS),
        None => tracing::warn!("rendered page has no </head>; ksx icon links not added"),
    }
    out
}

/// The oracle both page modules' tests use: every icon link is present, and
/// every one of them is INSIDE `<head>`.
///
/// The inside-`<head>` half is the half that earns its keep. The links are
/// spliced in after forma-server has rendered, so an upstream template change
/// that moved or renamed `</head>` would drop three `<link>` elements into
/// the body — where they do nothing whatsoever, and where the page looks
/// completely normal.
#[cfg(test)]
pub(crate) fn assert_icon_links_in_head(route: &str, html: &str) {
    let head_end = html
        .find("</head>")
        .unwrap_or_else(|| panic!("{route}: rendered page has no </head>"));
    for link in [
        r#"<link rel="icon" href="/favicon.svg" type="image/svg+xml">"#,
        r#"<link rel="icon" href="/favicon.ico" type="image/x-icon">"#,
        r#"<link rel="apple-touch-icon" href="/apple-touch-icon.png">"#,
    ] {
        let at = html
            .find(link)
            .unwrap_or_else(|| panic!("{route}: missing icon link {link}"));
        assert!(at < head_end, "{route}: {link} landed outside <head>");
    }
}

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
/// screen — hydrated on load. Its name is the `activateIslands` registry key
/// in `studio-ui/src/status.ts`; forma-ir stamps the id on the SSR root as
/// `data-forma-island` and hangs the native props off the same element. The
/// layout test pins both. Test-only since 2026-08-06: the id used to key the
/// hand-emitted `__forma_islands` props object (ledger #8's workaround), and
/// nothing in the request path needs it now that forma-ir emits the props.
#[cfg(test)]
const ISLAND_ID: u16 = 0;
#[cfg(test)]
const ISLAND_COMPONENT: &str = "StatusIsland";

/// How many `createShow` pairs this page has. All state COLOR on this SSR
/// page is done with show pairs — the server picks which statically-styled
/// variant renders — so the list is long; the layout test pins both the count
/// and every name.
///
/// Compiler 0.3.1 names show slots after their CONDITION binding
/// (`createShow(() => pillIdle(), …)` → `show:pillIdle`), so shows are
/// name-addressable exactly like lists and scalars. The `SHOW_ORDER`
/// positional array this file carried since v1 — the mapping whose only guard
/// was a count assertion, and which renumbered itself whenever a show was
/// inserted mid-document — is GONE (dogfood ledger #4/#14, adopted
/// 2026-08-06). [`show_values`] now yields `(slot name, value)` pairs, so
/// document order is documentation, not contract.
const SHOW_COUNT: usize = 17;

/// Bare-named slots this page renders and the seam deliberately never fills.
/// EMPTY here, and that is the claim: every signal `StatusIsland.ts` binds to
/// the DOM gets a server value on every request. See
/// [`assert_island_slot_contract`] for what an unlisted one would cost.
#[cfg(test)]
const CLIENT_ONLY_SLOTS: [&str; 0] = [];

/// Anonymous (`attr:`/`text:`) slots this page compiles to. EMPTY, and that is
/// the strongest form of ledger #10/#20's guard: every attribute value and
/// every text child on the status page is either a named signal binding or
/// static markup — nothing renders from a default the seam cannot reach.
#[cfg(test)]
const ANONYMOUS_SLOTS: [&str; 0] = [];

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
///
/// This is a HAND COPY of two tokens, so it drifts silently — and it had:
/// before the Street Fighter palette pass these read `#0b0e14`/`#dbe2ef`
/// while studio.css had moved to `#0a0d13`/`#e3e9f4`, i.e. the first paint
/// was a *different colour* from the stylesheet that replaced it. Nothing
/// could catch that, because a wrong anti-flash colour looks like a flash.
/// `ksx-studio/tests/contrast.rs` now parses studio.css and pins both.
/// `pub(crate)` so a THIRD page reuses this copy instead of minting another
/// one. `render_map.rs` keeps a byte-identical copy from before the rule was
/// worth stating, and `tests/contrast.rs` pins both against the `--bg`/`--text`
/// tokens — but a copy that cannot drift is better than a copy that is checked,
/// and the check only knows about the two files it names.
pub(crate) const PERSONALITY_CSS: &str = "body{background:#120c1c;color:#f0ebe0;margin:0}\
@media (prefers-color-scheme:light){body{background:#f6f3ee;color:#1c1428}}";

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
        // There used to be a `forma_server::check_ir_compatibility(&module)`
        // call here. 0.2.0 removed the function, and re-implementing it would
        // have been re-implementing dead code: `IrModule::parse` above starts
        // with `IrHeader::parse`, which rejects a version mismatch outright
        // (`IrError::UnsupportedVersion`, forma-ir format.rs). Any module that
        // reached the old call had ALREADY passed the identical comparison, so
        // it could only ever return `Ok`.
        //
        // That was true in 0.1.4 as well — the guard is in both releases — so
        // nothing is lost here, and a mismatched IR still fails, one line
        // earlier, through `StudioError::Ir`.

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
    let options = SlotValue::array(
        snap.profiles
            .iter()
            .map(|g| {
                SlotValue::object(vec![("title".to_owned(), SlotValue::Text(g.title.clone()))])
            })
            .collect(),
    );
    let pads = SlotValue::array(
        snap.pads
            .iter()
            .enumerate()
            .map(|(i, p)| {
                SlotValue::object(vec![
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
    let ghosts = SlotValue::array(
        (snap.pads.len()..PAD_TILE_FLOOR)
            .map(|i| {
                SlotValue::object(vec![(
                    "slot".to_owned(),
                    SlotValue::Text(format!("P{}", i + 1)),
                )])
            })
            .collect(),
    );
    let profiles = SlotValue::array(
        snap.profiles
            .iter()
            .map(|g| {
                SlotValue::object(vec![
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

/// Every show slot on this page, BY NAME, with the boolean the server wants
/// in it. The session-controls policy is unchanged: exactly one of "start",
/// "stop" or "daemon down" is true, so the panel always says something and
/// never offers a dead button as live. The same rule colors the header pill,
/// and every status pill is a pair where exactly one side renders.
///
/// The names are the compiler's (`show:<condition getter>`), so they are the
/// signal names in `StatusIsland.ts` — rename a signal there and the layout
/// test names the missing slot instead of some panel quietly rendering its
/// neighbour's boolean. The comments carry what the old SHOW_ORDER labels
/// said; nothing here depends on their order any more.
fn show_values(
    snap: &StatusSnapshot,
    session: &SessionView,
    flash: Option<&str>,
) -> [(&'static str, bool); SHOW_COUNT] {
    let flash_err = flash.is_some_and(|f| f.starts_with("error"));
    let can_start = session.reachable && !session.running;
    let running = session.reachable && session.running;
    [
        ("show:pillRunning", running),
        ("show:pillIdle", can_start),
        ("show:pillDown", !session.reachable),
        // FIX 1: the unmissable banner, first child of <main> on BOTH pages.
        ("show:noDaemon", !session.reachable),
        ("show:flashOk", flash.is_some() && !flash_err),
        ("show:flashError", flash_err),
        ("show:canStart", can_start),
        ("show:canStop", running),
        ("show:daemonDown", !session.reachable),
        // profile rows: with Start buttons / inert.
        ("show:rowsLive", can_start),
        ("show:rowsPlain", !can_start),
        ("show:vigemOk", vigem_ok(snap)),
        ("show:vigemWarn", !vigem_ok(snap)),
        ("show:icptBorrowed", interception_installed(snap)),
        ("show:icptAbsent", !interception_installed(snap)),
        ("show:autostartOn", autostart_on(snap)),
        ("show:autostartOff", !autostart_on(snap)),
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
    // Shows BY NAME since compiler 0.3.1 (ledger #4 closed): a show whose
    // signal was renamed degrades to its authored default (false — nothing
    // renders) instead of silently taking the next show's boolean, which is
    // what the old positional zip did.
    for (name, value) in show_values(snap, session, flash) {
        if let Some(id) = named_slot_ids(module, name).into_iter().next() {
            slots.set(id, SlotValue::Bool(value));
        }
    }
    slots
}

/// The id of the DOMAIN PAYLOAD data block — ksx's own channel, not Forma's.
///
/// History (dogfood ledger #8, adopted 2026-08-06): this block used to be
/// `__forma_islands` and used to carry `{"0": <payload>}`, because compiler
/// 0.2.0 registered islands with EMPTY `slot_ids`, which meant forma-ir's
/// walker never emitted `data-forma-props` and the islands protocol's
/// script-tag shared-props path was the only way to get server data to the
/// client. Compiler 0.3.1 populates `slot_ids`, so the walker emits
/// `data-forma-props` ITSELF and `loadIslandProps` prefers it — the
/// impersonation is deleted, along with the island-id keying it needed.
///
/// What is left is a genuinely different thing, which is why it kept a block
/// and got a ksx name: native island props carry the RENDERED SLOT VALUES
/// (`vigemLine`, `show:canStart`, `list:padTiles:array`, …). The clients own
/// an editing model over the SOURCE payload — `map.ts` keeps `lastPayload`
/// and derives conflicts, macro drafts and undo from it — and no slot carries
/// that. See ledger #19.
pub(crate) const PAYLOAD_SCRIPT_ID: &str = "__ksx-payload";

/// The slot contract both pages' layout tests assert, in one place because it
/// is one invariant: **a name the seam injects must be the name the island
/// actually renders, and a name the island renders must be one the seam
/// injects.**
///
/// Existence is NOT the check, and finding that out cost an evening. Compiler
/// 0.3.1 keeps every slot name on a page unique by SUFFIXING collisions
/// (`signal-scope.ts` `uniqueName`), so a signal declared in two scopes — the
/// ledger #9 twin shape, one careless `createSignal` in `*Page.ts` — mints the
/// UNSUFFIXED name for the DEAD declaration and pushes the rendered one to
/// `#2`. `names.contains("vigemLine")` still passes, [`SlotData::from_json`]
/// still reports the value set, and the page renders its compile-time default
/// forever. The compiler says so on one build line among the thirteen the
/// ledger tells you not to chase; nothing else noticed, and every test here
/// stayed green (verified 2026-08-06 by resurrecting one twin and running the
/// whole gate).
///
/// The island's `slot_ids` ARE the render set — that is what makes them the
/// right oracle, and the reason ledger #8's fix is load-bearing for more than
/// `data-forma-props`.
///
/// - `injected` — every slot name the seam addresses by name (scalars, lists,
///   shows). Each must resolve to exactly ONE slot, and that slot must be in
///   the island's `slot_ids`.
/// - `client_only` — bare-named slots the seam deliberately never fills,
///   because the value only exists after an interaction the server has not
///   had. Every one is a documented exception; anything else rendered by the
///   island and not injected is the #10 failure mode (a field that silently
///   shows its authored default).
/// - `anonymous` — `attr:`/`text:` slots, the compiler's name for a binding it
///   could not name after a signal. These can NEVER be injected: they render
///   their compile-time default and nothing else. Pinned as an exact set (a
///   new one is a review, not a surprise) and asserted non-empty, which is the
///   guard against ledger #10/#20(a) — an attribute whose value silently
///   vanishes.
#[cfg(test)]
pub(crate) fn assert_island_slot_contract(
    module: &IrModule,
    injected: &[&str],
    client_only: &[&str],
    anonymous: &[&str],
) {
    use std::collections::BTreeSet;

    let entries = module.slots.entries();
    let name_of = |id: u16| -> &str {
        entries
            .iter()
            .find(|e| e.slot_id == id)
            .and_then(|e| module.strings.get(e.name_str_idx).ok())
            .unwrap_or("<unknown>")
    };
    let islands = module.islands.entries();
    assert_eq!(islands.len(), 1, "expected exactly one island");
    let rendered: BTreeSet<u16> = islands[0].slot_ids.iter().copied().collect();

    for name in injected {
        let ids = named_slot_ids(module, name);
        assert_eq!(
            ids.len(),
            1,
            "slot '{name}' occurs {} times — the seam injects the FIRST, so \
             the others can never be filled",
            ids.len()
        );
        assert!(
            rendered.contains(&ids[0]),
            "slot '{name}' (id {}) is injected server-side but the island \
             never renders it — the compiler gave the rendered binding a \
             different name (look for '{name}#2' in the build log). Injection \
             will silently succeed and the page will show the authored \
             default.",
            ids[0]
        );
    }

    // The converse: every bare-named slot the island renders is either
    // injected or a documented client-only exception.
    let rendered_bare: BTreeSet<&str> = rendered
        .iter()
        .map(|id| name_of(*id))
        .filter(|n| !n.contains(':'))
        .collect();
    let accounted: BTreeSet<&str> = injected
        .iter()
        .copied()
        .filter(|n| !n.contains(':'))
        .chain(client_only.iter().copied())
        .collect();
    assert_eq!(
        rendered_bare, accounted,
        "the island renders scalar slots the seam does not inject (or the \
         seam claims ones it does not render). An un-injected scalar renders \
         its authored default on every request, forever, with no error — add \
         it to the seam, or name it in the client-only list with a reason."
    );

    // Anonymous slots: un-injectable by construction.
    let anon_found: BTreeSet<&str> = entries
        .iter()
        .filter_map(|e| module.strings.get(e.name_str_idx).ok())
        .filter(|n| n.starts_with("attr:") || n.starts_with("text:"))
        .collect();
    let anon_expected: BTreeSet<&str> = anonymous.iter().copied().collect();
    assert_eq!(
        anon_found, anon_expected,
        "the anonymous-slot set changed. These are bindings the compiler \
         could not name after a signal, so the seam can never inject them — \
         they render their COMPILE-TIME DEFAULT and nothing else. A new one \
         is fine only if that default is the finished text (a concatenation \
         of literals folds; anything reading a signal does not). Check the \
         default, then pin it here."
    );
    for name in &anon_found {
        let entry = entries
            .iter()
            .find(|e| module.strings.get(e.name_str_idx).is_ok_and(|n| n == *name))
            .expect("just enumerated");
        assert!(
            !entry.default_bytes.is_empty(),
            "anonymous slot '{name}' has an EMPTY default, so it renders as \
             nothing — this is ledger #10/#20(a) exactly: an attribute with no \
             value, or a child with no text, and no warning anywhere"
        );
    }
}

/// The domain payload as a JSON data block body. `<` is JSON-escaped so a
/// hostile snapshot line can never close the `<script>` data block early —
/// inside JSON, `<` only occurs in strings, where `<` is equivalent.
pub(crate) fn payload_json<T: serde::Serialize>(payload: &T) -> String {
    serde_json::to_value(payload)
        .unwrap_or(serde_json::Value::Null)
        .to_string()
        .replace('<', "\\u003c")
}

/// Everything that precedes `#app`: the no-JS fallback refresh and the domain
/// payload block.
///
/// - The `<noscript>` meta refresh targets `refresh_url` WITHOUT any query
///   string: a flash arrives via /?flash=… (post-redirect), shows for one
///   cycle, and the next no-JS refresh lands on a clean URL. (With JS the
///   poller keeps the page live and the entry clears the flash + URL itself.)
/// - The payload block is `type="application/json"` — a data block, never
///   executed, outside the CSP's script-src entirely; the entries read it by
///   id ([`PAYLOAD_SCRIPT_ID`]).
pub(crate) fn body_prefix<T: serde::Serialize>(payload: &T, refresh_url: &str) -> String {
    format!(
        "<noscript><meta http-equiv=\"refresh\" content=\"{REFRESH_SECS}; url={refresh_url}\"></noscript>\
         <script id=\"{PAYLOAD_SCRIPT_ID}\" type=\"application/json\">{}</script>",
        payload_json(payload)
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
    with_icon_links(render_page(&PageConfig {
        title: "ksx Studio — cabinet status",
        route_pattern: "/",
        manifest: &page.manifest,
        config_script: None,
        // The SAFE server-data path (serde_json-escaped, surfaced as
        // window.__FORMA_CONFIG__), as opposed to config_script, which is a raw
        // trusted escape hatch. ksx injects its island props through the FMIR
        // slot table instead, so neither is used — but the distinction matters
        // the day anything server-derived does need to reach the client.
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
    /// `LIST_SLOT_*` constants claim (order included), the `show:` slots are
    /// exactly the ones [`show_values`] addresses BY NAME (ledger #4, adopted
    /// 2026-08-06 — this replaced a bare count assertion, which was all a
    /// positional seam could be guarded by), and the island table is the one
    /// island the client registry activates, carrying real `slot_ids` (ledger
    /// #8, adopted the same day — the assertion used to be
    /// `slot_ids.is_empty()`, a tripwire written to fail exactly here).
    /// Fails when StatusIsland.ts, the compiler's naming scheme, or this file
    /// drift.
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
        // Shows, BY NAME and as a SET: every show slot the page compiles to
        // is addressed by the seam, and every name the seam addresses exists.
        // Order is deliberately not asserted — that is the whole point of
        // ledger #4 being closed.
        let ir_shows: std::collections::BTreeSet<&str> = names
            .iter()
            .copied()
            .filter(|n| n.starts_with("show:"))
            .collect();
        let seam_shows: std::collections::BTreeSet<&str> =
            show_values(&StatusSnapshot::default(), &SessionView::default(), None)
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        assert_eq!(
            ir_shows, seam_shows,
            "show slots drifted between StatusIsland.ts and show_values()"
        );
        assert_eq!(
            ir_shows.len(),
            SHOW_COUNT,
            "SHOW_COUNT is stale; slots: {names:?}"
        );
        // The island table: exactly one island, the whole screen, hydrated
        // on load, named for the activateIslands registry key in status.ts.
        // Compiler 0.3.1 populates `slot_ids` (ledger #8 closed), so
        // forma-ir's walker emits `data-forma-props` itself — the reason the
        // hand-built `__forma_islands` block is gone from this file.
        let islands = module.islands.entries();
        assert_eq!(islands.len(), 1, "expected exactly one island");
        assert_eq!(islands[0].id, ISLAND_ID);
        assert_eq!(
            module.strings.get(islands[0].name_str_idx).unwrap(),
            ISLAND_COMPONENT
        );
        assert!(
            !islands[0].slot_ids.is_empty(),
            "island slot_ids are empty — compiler regressed to 0.2.0 \
             behaviour and native data-forma-props will not be emitted"
        );
        // …and the slot contract itself: injected == rendered, both ways.
        // See [`assert_island_slot_contract`] for why "the name exists" is
        // not the check.
        let injected: Vec<&str> = scalars
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .chain(
                list_values(&StatusSnapshot::default())
                    .iter()
                    .map(|(n, _)| *n),
            )
            .chain(seam_shows.iter().copied())
            .collect();
        assert_island_slot_contract(module, &injected, &CLIENT_ONLY_SLOTS, &ANONYMOUS_SLOTS);
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

    /// The island shape: the SSR walker stamps the island attributes on the
    /// page root, emits NATIVE `data-forma-props` from the island's slot_ids
    /// (ledger #8, adopted 2026-08-06), the ksx payload block carries the
    /// source payload beside it, and the client bundle loads via a NONCE'd
    /// module script (the strict CSP allows nothing else). The anti-flash
    /// personality CSS rides the same nonce.
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
        // Native island props, carrying the SSR slot values themselves — the
        // whole point of ledger #8 being closed.
        //
        // WHICH CHANNEL they arrive in is forma's choice, not ours, and it
        // changed at forma-ir 0.2.0: props whose JSON exceeds
        // `INLINE_PROPS_MAX_BYTES` (1 KiB) spill from the inline
        // `data-forma-props` attribute into the shared `__forma_islands`
        // block. That is upstream acting on ksx's own finding #22 — inline
        // props were 32.5% of the /map response and could not be switched off.
        // This page's props are far over the ceiling, so it uses the block.
        //
        // So the assertion is on the CONTRACT rather than the mechanism:
        // exactly one channel, carrying the values. `loadIslandProps` reads
        // the attribute first and falls back to the block, and the walker
        // emits only one, so precedence never has two answers.
        let inline = out.html.contains("data-forma-props=\"");
        let shared = out.html.contains(r#"<script id="__forma_islands""#);
        assert!(
            inline ^ shared,
            "props must travel in EXACTLY one channel (inline={inline}, \
             shared={shared}): {}",
            out.html
        );
        // Values, in whichever channel — attribute-escaped inline, plain JSON
        // in the block.
        assert!(
            out.html
                .contains(r#""vigemLine":"installed — service running"#)
                || out
                    .html
                    .contains("&quot;vigemLine&quot;:&quot;installed — service running"),
            "props must carry the injected slot VALUES: {}",
            out.html
        );
        assert!(
            out.html.contains(r#""show:pillIdle":true"#)
                || out.html.contains("&quot;show:pillIdle&quot;:true"),
            "props must carry the named show booleans: {}",
            out.html
        );
        // The ksx payload data block (non-executing, CSP-exempt) — the SOURCE
        // payload the client's own model reads (ledger #19).
        assert!(
            out.html
                .contains(r#"<script id="__ksx-payload" type="application/json">"#),
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
                r#"<style nonce="{nonce}">body{{background:#120c1c"#
            )),
            "personality css must carry the CSP nonce: {}",
            out.html
        );
    }

    /// Ledger #5's contract, server side: the payload block IS the
    /// /api/status payload — one struct, one serializer, so the signals the
    /// client seeds before adoption and the ones the poller overwrites can
    /// never see different shapes. (The poller itself only runs in a
    /// browser; visual confirmation stays a manual step.)
    #[test]
    fn the_payload_block_matches_the_api_payload_shape() {
        let payload = StatusPayload {
            snapshot: sample(),
            session: idle_session(),
            flash: None,
        };
        let json = payload_json(&payload);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("payload parse");
        assert_eq!(
            parsed,
            serde_json::to_value(&payload).unwrap(),
            "the payload block must be byte-compatible with /api/status"
        );
        // And the rendered page embeds exactly that block.
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &payload.snapshot, &payload.session, None);
        assert!(out.html.contains(&json), "{}", out.html);
    }

    /// The payload block is a data block inside HTML: a hostile snapshot line
    /// must not be able to close the script element early.
    #[test]
    fn the_payload_block_cannot_break_out_of_its_script() {
        let mut snap = sample();
        snap.vigem = "</script><script>alert(1)</script>".into();
        let payload = StatusPayload {
            snapshot: snap,
            session: idle_session(),
            flash: Some("</script>".into()),
        };
        let json = payload_json(&payload);
        assert!(!json.contains('<'), "unescaped '<' in payload: {json}");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("still valid JSON");
        assert_eq!(
            parsed["snapshot"]["vigem"],
            serde_json::json!("</script><script>alert(1)</script>"),
            "escaping must be lossless"
        );
        // The NATIVE props path escapes too, which is forma-ir's job — pin
        // that a hostile line cannot break out of whichever channel carries
        // it. This page's props exceed INLINE_PROPS_MAX_BYTES so they land in
        // the shared block, where the hazard is `</script` rather than a
        // stray quote; forma-ir 0.2.0's changelog calls that escaping "now
        // total". Both channels are checked so a payload that shrinks below
        // the ceiling does not quietly skip the assertion.
        let page = EmbeddedPage::load("/").unwrap();
        let out = render_status(&page, &payload.snapshot, &payload.session, None);
        let props = out
            .html
            .split("data-forma-props=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .or_else(|| {
                out.html
                    .split(r#"<script id="__forma_islands" type="application/json">"#)
                    .nth(1)
                    .and_then(|s| s.split("</script>").next())
            })
            .expect("island props in one channel or the other");
        assert!(
            !props.contains("<script") && !props.contains("</script"),
            "island props must be escaped for their channel: {props}"
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

    /// The brand embed exists AND is the same bytes `tools/icongen` wrote.
    ///
    /// The byte comparison is the part that earns its keep. `favicon.ico` is
    /// a COPY of `assets/brand/dist/ksx.ico` — the shell, the installer and
    /// this page each read their own copy — and a copy is exactly the thing
    /// that goes stale silently. Regenerate the brand, forget to re-run the
    /// tool, and Studio wears last month's mark forever with no error
    /// anywhere. Here it is a test failure with the fix in the message.
    #[test]
    fn brand_embed_carries_the_trio() {
        for name in ["favicon.ico", "favicon.svg", "apple-touch-icon.png"] {
            assert!(
                BrandAssets::get(name).is_some(),
                "{name} missing from crates/ksx-studio/brand/ — \
                 run: cargo run --manifest-path tools/icongen/Cargo.toml --release"
            );
        }

        let embedded = BrandAssets::get("favicon.ico").unwrap();
        let canonical = include_bytes!("../../../assets/brand/dist/ksx.ico");
        assert_eq!(
            embedded.data.as_ref(),
            canonical.as_slice(),
            "crates/ksx-studio/brand/favicon.ico has drifted from \
             assets/brand/dist/ksx.ico — re-run tools/icongen"
        );

        let svg = BrandAssets::get("favicon.svg").unwrap();
        let master = include_bytes!("../../../assets/brand/ksx-simple.svg");
        assert_eq!(
            svg.data.as_ref(),
            master.as_slice(),
            "favicon.svg has drifted from the SIMPLIFIED master — re-run \
             tools/icongen. (It is the simplified art on purpose: a browser \
             renders an SVG icon into a 16-32 px tab.)"
        );
    }

    /// The status page declares all three icons, inside `<head>`. The mapper
    /// runs the same oracle from its own module.
    #[test]
    fn icon_links_are_in_the_status_head() {
        let out = render_status(
            &EmbeddedPage::load("/").unwrap(),
            &sample(),
            &idle_session(),
            None,
        );
        assert_icon_links_in_head("/", &out.html);
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
