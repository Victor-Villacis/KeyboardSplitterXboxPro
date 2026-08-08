//! The blocking axum server around the render seam.
//!
//! GET / renders the page (SSR + island props); GET /api/status serves the
//! same [`StatusPayload`] as JSON for the island's 2 s poller (same-origin
//! only — the CSP's `connect-src 'self'` is exactly what permits the fetch).
//! The three POST routes each perform one [`ControlSource`] verb and
//! 303-redirect back to /, carrying the outcome in a `flash` query parameter
//! — plain HTML forms remain the baseline (`form-action 'self'`), which the
//! client optionally upgrades to fetch-submits that read the redirect's
//! flash without a reload.
//!
//! v15 adds `/pads`, and it takes its facts from a THIRD provider:
//! [`ksx_api::MachineSource`], beside the existing status and control ones.
//! Neither of those could answer it — `StatusSource` reads the config store
//! and `ControlSource` the daemon pipe, and a ViGEm bus is neither. Its two
//! verbs keep the CLI's consent shape rather than inventing one: a spawn is
//! bounded (it unplugs itself, because a page has no Ctrl+C) and a prune is a
//! DRY RUN unless the form carries `confirm=yes`.
//!
//! v9 gives the MAPPER the same baseline: `/map/*` are form-encoded twins of
//! the `/api/*` mapper verbs (bind, clear, restore, clear-all, pause), each
//! calling the identical [`ControlSource`] method and 303-ing back to
//! `/map?slot=N&flash=…`. With JavaScript the island intercepts the submit
//! and reports through its toast stack instead; with JavaScript off the page
//! is still fully operable, which is the whole point of the shape.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::control::{BindOutcome, BindRequest, ControlSource, SessionView};
use crate::error::StudioError;
use crate::render::{render_status, Assets, BrandAssets, EmbeddedPage};
use crate::render_check::render_check;
use crate::render_devices::render_devices;
use crate::render_map::render_map;
use crate::render_setup::render_setup;
use crate::render_start::render_start;
use crate::snapshot::{
    CheckPayload, DevicesPayload, MapPayload, PadsPayload, ProfilesPayload, SetupPayload,
    SetupSnapshot, StartPayload, StatusPayload, StatusSnapshot, StatusSource,
};

struct AppState {
    page: EmbeddedPage,
    map_page: EmbeddedPage,
    check_page: EmbeddedPage,
    pads_page: EmbeddedPage,
    devices_page: EmbeddedPage,
    profiles_page: EmbeddedPage,
    setup_page: EmbeddedPage,
    start_page: EmbeddedPage,
    source: Box<dyn StatusSource>,
    control: Box<dyn ControlSource>,
    /// The MACHINE reads and writes that are not a `DaemonCommand`: the
    /// device enumeration and the two `[[device]]` writes behind `/devices`,
    /// the ViGEm bus report and the two pad verbs behind `/pads`, the
    /// preflighted profile list, the preset list with its templates, the
    /// two creates behind `/profiles`, and the config in and out plus the
    /// first-run state behind `/setup`. A THIRD provider rather than more
    /// methods on the other two, because that is the split `ksx-api` already
    /// draws — status is what the box looks like, control is what the daemon
    /// can be told, and this is what is on the machine itself (the USB tree,
    /// the config store, the bus device), readable while the pipe is dead,
    /// exactly as the read-only mapper is.
    machine: Box<dyn ksx_api::MachineSource>,
    /// The FOURTH provider: the live input feed
    /// (`ksx_api::LiveSource` over the `ksx-live` named pipe).
    ///
    /// Not a method on any of the other three, because it is not their shape.
    /// Status is a point-in-time snapshot, control is one verb per call and
    /// machine is what is on this box — all three are *questions with an
    /// answer*. This is a subscription that outlives any answer, on its own
    /// channel, for a reason [`ksx_api::LiveSource`] spells out: the control
    /// pipe serves connections one at a time, so a stream on it would take the
    /// daemon's whole control surface down with the tab.
    ///
    /// `Arc`, not `Box`, because every SSE connection hands a handle to its own
    /// blocking bridge thread.
    live: Arc<dyn ksx_api::LiveSource>,
}

/// Serve the page until the process is killed (Ctrl+C included — no
/// graceful-shutdown plumbing).
///
/// Blocking on purpose: the caller owns no runtime, and this function keeps
/// its own multi-threaded tokio runtime at NORMAL priority, fully isolated
/// from anything time-critical (docs/ENHANCEMENTS.md E7 "own runtime, normal
/// priority").
///
/// Refuses any non-loopback `bind` before a socket exists.
pub fn serve(
    bind: SocketAddr,
    source: Box<dyn StatusSource>,
    control: Box<dyn ControlSource>,
    machine: Box<dyn ksx_api::MachineSource>,
    live: Arc<dyn ksx_api::LiveSource>,
) -> Result<(), StudioError> {
    if !bind.ip().is_loopback() {
        return Err(StudioError::NonLoopbackBind { bind });
    }
    let page = EmbeddedPage::load("/")?;
    let mapper = EmbeddedPage::load("/map")?;
    let check = EmbeddedPage::load("/check")?;
    let pads = EmbeddedPage::load("/pads")?;
    let devices = EmbeddedPage::load("/devices")?;
    let profiles = EmbeddedPage::load("/profiles")?;
    let setup = EmbeddedPage::load("/setup")?;
    let start = EmbeddedPage::load("/start")?;
    let state = Arc::new(AppState {
        page,
        map_page: mapper,
        check_page: check,
        pads_page: pads,
        devices_page: devices,
        profiles_page: profiles,
        setup_page: setup,
        start_page: start,
        source,
        control,
        machine,
        live,
    });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("ksx-studio")
        .enable_io()
        .enable_time()
        .build()
        .map_err(StudioError::Runtime)?;

    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .map_err(|source| StudioError::Bind { bind, source })?;
        let app = Router::new()
            .route("/", get(status_page))
            .route("/api/status", get(api_status))
            .route("/session/start", post(session_start))
            .route("/session/stop", post(session_stop))
            .route("/config/reload", post(config_reload))
            // The mapper (v5): page + poll payload + the learn/bind verbs,
            // each a thin wrapper over one ControlSource method (= one pipe
            // verb — no GUI-only code paths).
            .route("/map", get(map_page))
            .route("/api/map", get(api_map))
            .route("/api/learn", get(api_learn_poll))
            .route("/api/learn/start", post(api_learn_start))
            .route("/api/learn/cancel", post(api_learn_cancel))
            .route("/api/bind", post(api_bind))
            // v10: write a control's WHOLE key list (many keys → one
            // control). The island computes the new set from the payload it
            // is already polling — add = ∪ {k}, per-key ✕ = ∖ {k} — and posts
            // it here; `bind_keys` is what knows how to spell that on the
            // wire. Not a new daemon verb: today it composes the same `map`
            // the button beside it uses.
            .route("/api/bind/keys", post(api_bind_keys))
            // v11: the macro editor's SAVE — one whole `[macros.<name>]`
            // table per call, through `ControlSource::save_macro` (= the
            // daemon's `map-macro` verb, = the `ksx macro` CLI's writer). No
            // GUI-only path, and no second macro schema: the steps on the wire
            // are the MacroStepView rows the read side already served.
            .route("/api/macro/save", post(api_macro_save))
            .route("/api/preset/restore", post(api_preset_restore))
            .route("/api/preset/clear-all", post(api_preset_clear_all))
            // The mapper's own session controls (FIX 0): "Pause emulation &
            // map" and "Resume emulation" are the SAME ControlSource verbs
            // the status page's forms use — one pipe verb each, no GUI-only
            // path — served as JSON so the mapper never navigates away and
            // loses the user's place.
            .route("/api/session/stop", post(api_session_stop))
            .route("/api/session/start", post(api_session_start))
            // v9 — the NO-JAVASCRIPT write path. Same verbs, same
            // ControlSource methods as the /api/* routes beside them; the
            // only difference is the wire shape (form-encoded in, 303 out
            // instead of JSON in and JSON back). A browser with scripting
            // switched off can bind, clear, restore and pause with these,
            // and the island fetch-enhances them on top (map.ts) so a page
            // WITH scripting never navigates.
            .route("/map/bind", post(map_form_bind))
            // v10 — MANY KEYS → ONE CONTROL, without JavaScript. Same row
            // form, same key picker, two more submits: `add` appends the
            // picked key to the control's list, `key/remove` takes just that
            // one off it. Both are read-modify-write on the key SET and land
            // through `ControlSource::bind_keys` — no new daemon verb.
            .route("/map/add", post(map_form_add))
            .route("/map/key/remove", post(map_form_remove_key))
            .route("/map/clear", post(map_form_clear))
            .route("/map/turbo", post(map_form_turbo))
            .route("/map/preset/restore", post(map_form_restore))
            .route("/map/preset/clear-all", post(map_form_clear_all))
            .route("/map/session/stop", post(map_form_session_stop))
            // v15 — the PADS page: what is on the ViGEm bus, a bounded pad
            // test, and the prune that clears ghosts. Three routes, one
            // `MachineSource` verb each, and the arming step is a GET
            // (`/pads?confirm=1`) because showing someone what a destructive
            // button will remove must not itself be a POST.
            // BUILD C — the button check, one click from the mapper. Two
            // routes and no verbs: the page is a READ of the slot roster, and
            // the lighting-up arrives on /api/live beside it rather than
            // through either of these. Nothing here writes, so nothing here
            // needs the guard's mutating arm — the Host check still covers it,
            // which is what stops a rebound origin watching the panel.
            .route("/check", get(check_page))
            .route("/api/check", get(api_check))
            // ── THE LIVE FEED ─────────────────────────────────────────────
            // One route, and it is the keystone the button check stands on:
            // the daemon's input fan-out as Server-Sent Events. Read-only and
            // one-directional by construction — the browser has nothing to say
            // back, and everything it might want to say is already a verb on a
            // route above. `crate::live` carries why SSE and not a WebSocket,
            // and how a stalled tab is made to cost the pipeline nothing.
            .route("/api/live", get(api_live))
            .route("/pads", get(pads_page))
            .route("/api/pads", get(api_pads))
            .route("/pads/spawn", post(pads_form_spawn))
            .route("/pads/prune", post(pads_form_prune))
            // v17: the DEVICE PICKER — `ksx device scan` as a page, plus the
            // two writes it exists for. Read is `MachineSource::device_scan`
            // (boards, not devnodes); the writes are `device_pick` and
            // `device_remove`, which are the CLI's own plan/apply pair and
            // need no daemon. Claiming is NOT here and never will be: it needs
            // elevation, which docs/SURFACES.md §3 marks "never" for the
            // browser — the page prints the command instead.
            .route("/devices", get(devices_page))
            .route("/api/devices", get(api_devices))
            .route("/devices/pick", post(devices_form_pick))
            .route("/devices/remove", post(devices_form_remove))
            // PROFILES & PRESETS. The read is `MachineSource::profiles`
            // (games.toml with `ksx_games::preflight` already run, so a
            // profile whose .exe moved is a broken ROW instead of a cabinet
            // that does nothing when the button is pressed) plus
            // `MachineSource::presets`. The three writes are one backend verb
            // each: `profile_new`, `preset_new`, and — for "switch to this" —
            // the SAME `ControlSource::start` the status page's forms post,
            // 303-ing back here so the user keeps their place, exactly as
            // `/map/session/stop` reuses `stop`.
            .route("/profiles", get(profiles_page))
            .route("/api/profiles", get(api_profiles))
            .route("/profiles/new", post(profiles_form_new))
            .route("/profiles/switch", post(profiles_form_switch))
            .route("/profiles/preset/new", post(profiles_form_preset_new))
            // ── /setup — the CONFIG FIRST, and the first run ───────────────
            // Two verbs a person sees (Export, Import) and three steps, each
            // one backend verb. No route here takes a filesystem path, in or
            // out: the export IS the bytes and the import IS the document, so
            // nothing on this page ever asks anyone to name a file.
            .route("/setup", get(setup_screen))
            .route("/api/setup", get(api_setup))
            // A GET on purpose: it writes nothing, and `guard.rs` decides what
            // to police by METHOD — a read wearing a POST would be a lie the
            // guard then has to work around. The Host check still covers it.
            .route("/setup/export.json", get(setup_export))
            // DRY RUN unless the form's "write it" box is ticked
            // (`ksx_api::ImportRequest::apply`), which is the CLI's consent
            // shape and not a web-only ceremony.
            //
            // The one route with its own body limit. axum's default is 2 MB,
            // and a whole cabinet — config, every games.toml profile and every
            // preset in one interop document — can exceed it; the cost of the
            // default was a bare 413 with no way back to the page. 8 MB is
            // roomy for a configuration and still a bound, and the handler
            // turns the rejection into a flashed sentence either way.
            .route(
                "/setup/import",
                post(setup_form_import)
                    .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024)),
            )
            // Step 2: one `ControlSource::assign_slot` — the same pipe verb
            // `ksx slot assign` performs. It BOUNCES the pads, which the page
            // says before the click, not after it.
            .route("/setup/slot", post(setup_form_slot))
            // Step 3: the daemon's own learner, the two verbs the mapper
            // already uses. The page renders `learn_poll` per request, so this
            // step works with scripting switched off.
            .route("/setup/prove", post(setup_form_prove))
            .route("/setup/prove/cancel", post(setup_form_prove_cancel))
            // ── /start — THE FIRST RUN (docs/FIRST-RUN.md moments 4–7) ─────
            //
            // A new page rather than a rebuilt `/setup`, and the split is the
            // contract, not the layout: every `/setup` step reads config.toml
            // and writes to it, and NOTHING here touches a file until
            // `/start/save`. One screen holding both rules would be a screen
            // where the user cannot tell which controls commit — which is the
            // whole thing staging exists to fix (`render_start.rs` has the
            // longer version).
            //
            // Nine routes. The six `stage-*` ones are ONE `ControlSource` verb
            // each and reach nothing outside the daemon's own memory — no file,
            // no driver, no session — which is what makes exploring free.
            // `/start/save` is one config write (the same shape as
            // `/setup/slot`) and `/start/play` starts a session from a plan
            // built in memory. There is no route that claims a board: §3 marks
            // that "never" for the browser, and a first-run flow is the last
            // place to make an exception.
            //
            // The RESCAN is deliberately not here. It is a link back to
            // `/start`, because re-reading the machine writes nothing and a
            // read wearing a POST is a lie the guard then has to work around
            // (the same argument `/setup/export.json` makes).
            .route("/start", get(start_page))
            .route("/api/start", get(api_start))
            .route("/start/device", post(start_form_device))
            .route("/start/controller", post(start_form_controller))
            // Moment 6 IN THE STAGE: dress a staged controller in one of
            // ksx's in-box layouts. One `stage-edit`, so it reaches nothing
            // outside the daemon's memory — the bindings a first-run user
            // needs arrive without a file write and without the mapper, which
            // edits FILES and could therefore never have been step 3 of a flow
            // that has not saved anything yet.
            .route("/start/controller/layout", post(start_form_layout))
            .route("/start/controller/remove", post(start_form_remove))
            .route("/start/blocking", post(start_form_blocking))
            .route("/start/discard", post(start_form_discard))
            .route("/start/save", post(start_form_save))
            .route("/start/play", post(start_form_play))
            // Canon helper: correct no-cache + Service-Worker-Allowed
            // headers for free (replaced a hand-rolled handler).
            .route("/sw.js", get(forma_server::sw::serve_sw::<Assets>))
            .route(
                "/_assets/{filename}",
                get(forma_server::assets::serve_asset::<Assets>),
            )
            // The brand icons, at the ROOT paths their consumers hard-code.
            // Not under `/_assets/`: a browser asks for `/favicon.ico` with
            // no prompting from the markup, and iOS probes
            // `/apple-touch-icon.png` the same way — a link tag pointing
            // elsewhere is an optimisation on top of those defaults, never a
            // replacement for them.
            .route("/favicon.ico", get(favicon_ico))
            .route("/favicon.svg", get(favicon_svg))
            .route("/apple-touch-icon.png", get(apple_touch_icon))
            // ONE layer over every route, not a check per handler. The routes
            // above arrived in three separate milestones and the mapper alone
            // contributed eight form endpoints; a guard you have to remember to
            // add to each new one is a guard that will be missing from the
            // ninth. See `crate::guard` for what this refuses and why an
            // absent `Origin` is deliberately allowed through.
            .layer(axum::middleware::from_fn(move |req, next| {
                crate::guard::same_origin(bind, req, next)
            }))
            .with_state(state);
        tracing::info!(%bind, "ksx Studio listening");
        axum::serve(listener, app).await.map_err(StudioError::Serve)
    })
}

#[derive(Deserialize)]
struct PageQuery {
    flash: Option<String>,
}

// DELETED at forma-server 0.2.0: `relax_style_src`.
//
// Dogfood ledger #13 — a nonce in `style-src` makes browsers drop
// `'unsafe-inline'` semantics per spec, so every inline `style=""` ATTRIBUTE
// was ignored; forma's own compiled bindings emit those (the mapper's zone
// geometry, the countdown bar's width), and all 25 hit zones collapsed into a
// pile at the stage's top-left. The workaround rewrote the directive to
// `style-src 'self' 'unsafe-inline'`.
//
// 0.2.0 fixes it properly: the policy now carries
// `style-src-attr 'unsafe-inline'` alongside a still-nonce-locked `style-src`,
// so attribute styles apply while `<style>` blocks and stylesheets keep their
// nonce. Nothing to work around.
//
// Keeping it would have been actively harmful, and invisibly so. It matched
// `directive.starts_with("style-src")`, which catches `style-src-attr` too —
// so it would have DESTROYED the new directive and stripped the nonce off the
// real one, leaving a strictly weaker policy than upstream ships. The page
// would have rendered perfectly throughout, because the relaxed `style-src` it
// substituted still permits the inline styles the mapper needs. A workaround
// that silently outlives its bug is worse than the bug.

/// One fresh (snapshot, session) pair. Collectors hit the registry, the
/// SCM, schtasks.exe and the daemon pipe — blocking work, kept off the
/// async workers.
async fn collect(state: &Arc<AppState>) -> (StatusSnapshot, SessionView) {
    let snap_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        (snap_state.source.snapshot(), snap_state.control.session())
    })
    .await
    .unwrap_or_else(|_| {
        (
            StatusSnapshot::degraded("status collection panicked"),
            SessionView::unreachable("status collection panicked"),
        )
    })
}

async fn status_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let (snap, session) = collect(&state).await;

    let flash = query.flash.as_deref().filter(|f| !f.trim().is_empty());
    let out = render_status(&state.page, &snap, &session, flash);
    // No HTTP `Refresh` header any more: it would reload the page for JS
    // users too, defeating the island poller. The no-JS fallback is the
    // <noscript> meta refresh render.rs emits.
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The island poller's endpoint: the SAME [`StatusPayload`] shape the page
/// embeds as island props (parity unit-tested in render.rs). `flash` is
/// always null here — a poll is not an action.
///
/// A loopback bind and no CORS headers keep this response body private from a
/// cross-origin `fetch`, and the page's `connect-src 'self'` is what allows its
/// own. Note what that does NOT cover: a cross-origin form POST needs no CORS
/// permission at all, and a rebound DNS name is same-origin by the browser's
/// reckoning. Both are handled in [`crate::guard`], not here.
async fn api_status(State(state): State<Arc<AppState>>) -> Response {
    let (snapshot, session) = collect(&state).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(StatusPayload {
            snapshot,
            session,
            flash: None,
        }),
    )
        .into_response()
}

/// One fresh mapper payload. Blocking work (config store reads + up to two
/// pipe requests) off the async workers, like [`collect`].
async fn collect_map(
    state: &Arc<AppState>,
    selected: Option<u8>,
    macro_selected: Option<String>,
) -> MapPayload {
    let map_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let mapper = map_state.source.mapper();
        let session = map_state.control.session();
        let learn = map_state.control.learn_poll();
        let selected = selected
            .filter(|n| mapper.slots.iter().any(|s| s.number == *n))
            .or_else(|| mapper.slots.first().map(|s| s.number))
            .unwrap_or(0);
        // v11: the macro editor reads ONE preset — the selected slot's, since
        // that is the pad whose controls are the grid's columns.
        let macros = match mapper.slots.iter().find(|s| s.number == selected) {
            Some(slot) => map_state.source.macros(&slot.preset),
            None => crate::snapshot::MacroSnapshot::unavailable(
                "no slot is selected, so there is no preset to read macros from",
            ),
        };
        MapPayload {
            mapper,
            session,
            learn,
            selected,
            macros,
            macro_selected: macro_selected.unwrap_or_default(),
        }
    })
    .await
    .unwrap_or_else(|_| MapPayload {
        mapper: crate::snapshot::MapperSnapshot::unavailable("mapper collection panicked"),
        session: SessionView::unreachable("mapper collection panicked"),
        learn: crate::control::LearnView::unavailable("mapper collection panicked"),
        selected: 0,
        macros: crate::snapshot::MacroSnapshot::unavailable("mapper collection panicked"),
        macro_selected: String::new(),
    })
}

#[derive(Deserialize)]
struct MapQuery {
    slot: Option<u8>,
    /// v11: which `[macros.<name>]` table the macro editor paints. The tabs
    /// are anchors, so this is how a page with no JavaScript walks a preset's
    /// macros — exactly like `slot=` walks its slots.
    #[serde(rename = "macro")]
    macro_name: Option<String>,
    /// v9: the outcome of the no-JS form POST that redirected here. Same
    /// post-redirect-get channel `/` has always used for its session forms.
    flash: Option<String>,
}

async fn map_page(State(state): State<Arc<AppState>>, Query(query): Query<MapQuery>) -> Response {
    let payload = collect_map(&state, query.slot, query.macro_name.clone()).await;
    let flash = query.flash.as_deref().filter(|f| !f.trim().is_empty());
    let out = render_map(&state.map_page, &payload, flash);
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The mapper poller's endpoint — the same [`MapPayload`] shape the /map page
/// embeds as island props (parity unit-tested in render_map.rs).
async fn api_map(State(state): State<Arc<AppState>>, Query(query): Query<MapQuery>) -> Response {
    let payload = collect_map(&state, query.slot, query.macro_name.clone()).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

/// One blocking ControlSource call → JSON, shared by the learn/bind routes.
async fn control_json<T, F>(state: Arc<AppState>, call: F) -> Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(&dyn ControlSource) -> T + Send + 'static,
{
    let value = tokio::task::spawn_blocking(move || call(state.control.as_ref())).await;
    match value {
        Ok(value) => (
            [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
            axum::Json(value),
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "the control call panicked",
        )
            .into_response(),
    }
}

async fn api_learn_poll(State(state): State<Arc<AppState>>) -> Response {
    control_json(state, |control| control.learn_poll()).await
}

async fn api_learn_start(State(state): State<Arc<AppState>>) -> Response {
    control_json(state, |control| control.learn_start()).await
}

async fn api_learn_cancel(State(state): State<Arc<AppState>>) -> Response {
    control_json(state, |control| control.learn_cancel()).await
}

async fn api_bind(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<BindRequest>,
) -> Response {
    control_json(state, move |control| control.bind(&request)).await
}

/// POST /api/bind/keys — the JSON twin of `/map/add` + `/map/key/remove`.
/// The caller sends the FULL key list it wants the control to hold; what the
/// island computed and what a form computed therefore go through the same
/// [`ControlSource::bind_keys`], which is the one place that knows what the
/// daemon can express.
#[derive(Deserialize)]
struct BindKeysRequest {
    preset: String,
    function: String,
    /// Empty = clear the control. There is no null-vs-empty distinction here:
    /// "hold no keys" and "be unbound" are the same state.
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    reload: bool,
    /// AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3). Absent leaves the control's
    /// existing rate alone — "Add another key" must not switch an auto-fire
    /// off — `0` clears it, `n` sets it.
    #[serde(default)]
    turbo_hz: Option<u32>,
}

async fn api_bind_keys(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<BindKeysRequest>,
) -> Response {
    control_json(state, move |control| {
        control.bind_keys(
            &request.preset,
            &request.function,
            &request.keys,
            request.force,
            request.reload,
            request.turbo_hz,
        )
    })
    .await
}

/// POST /api/macro/save — write (or delete) one whole `[macros.<name>]` table.
///
/// `reload` is forced on, exactly like the restore route: the daemon only
/// applies to a session that is actually RUNNING, and a macro body is a
/// binding change — it changes no slot, persona or device, so the session
/// hot-swaps it with the pads left plugged instead of bouncing them.
///
/// Feedback parity with every other write on this page: `message` is the
/// toast, `problems` are the refusal's rows, `warnings` are the advisories a
/// successful save still has to say out loud (a step below the sampling
/// floor), and `backup` names the restore point this edit left — which the
/// mapper's existing "Restore backup from …" (`latest-backup`) undoes.
async fn api_macro_save(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<crate::control::MacroWrite>,
) -> Response {
    let request = crate::control::MacroWrite {
        reload: true,
        ..request
    };
    control_json(state, move |control| control.save_macro(&request)).await
}

#[derive(Deserialize)]
struct RestoreRequest {
    preset: String,
    /// One of [`crate::control::RESTORE_MODES`] — validated here so a typo is
    /// a 200-with-error the page can flash, not a daemon round-trip.
    mode: String,
}

/// POST /api/preset/restore — the mapper's three restore destinations, one
/// pipe `map-restore` per call (reload always requested: the daemon only
/// applies to a RUNNING session). Answers `{ok, message}` / `{ok:false,
/// error}`; the daemon's message already names what was written and what was
/// backed up first.
async fn api_preset_restore(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<RestoreRequest>,
) -> Response {
    let Some(mode) = crate::control::RestoreMode::parse(&request.mode) else {
        return (
            [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
            axum::Json(serde_json::json!({
                "ok": false,
                "error": format!(
                    "unknown restore mode \"{}\" ({})",
                    request.mode,
                    crate::control::RESTORE_MODES.join(" | ")
                ),
            })),
        )
            .into_response();
    };
    control_json(state, move |control| {
        match control.restore(&request.preset, mode) {
            Ok(message) => serde_json::json!({ "ok": true, "message": message }),
            Err(refusal) => serde_json::json!({ "ok": false, "error": refusal.message }),
        }
    })
    .await
}

#[derive(Deserialize)]
struct PresetRequest {
    preset: String,
}

/// POST /api/preset/clear-all — unbind every function of one preset. One pipe
/// `map-clear-all`; the daemon takes a timestamped backup first, so the page's
/// confirm can promise a road home and mean it.
async fn api_preset_clear_all(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<PresetRequest>,
) -> Response {
    control_json(state, move |control| {
        match control.clear_all(&request.preset) {
            Ok(message) => serde_json::json!({ "ok": true, "message": message }),
            Err(refusal) => serde_json::json!({ "ok": false, "error": refusal.message }),
        }
    })
    .await
}

#[derive(Deserialize)]
struct SessionRequest {
    /// `None`/empty = whatever the daemon is already configured with.
    profile: Option<String>,
}

/// POST /api/session/stop — "Pause emulation & map".
async fn api_session_stop(State(state): State<Arc<AppState>>) -> Response {
    control_json(state, |control| match control.stop() {
        Ok(message) => serde_json::json!({ "ok": true, "message": message }),
        Err(refusal) => serde_json::json!({ "ok": false, "error": refusal.message }),
    })
    .await
}

/// POST /api/session/start — "Resume emulation", with the profile the mapper
/// remembered when it paused, so the cabinet comes back to the same game.
async fn api_session_start(
    State(state): State<Arc<AppState>>,
    axum::Json(request): axum::Json<SessionRequest>,
) -> Response {
    let profile = request
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    control_json(state, move |control| {
        match control.start(profile.as_deref()) {
            Ok(message) => serde_json::json!({ "ok": true, "message": message }),
            Err(refusal) => serde_json::json!({ "ok": false, "error": refusal.message }),
        }
    })
    .await
}

// ── v9: the no-JavaScript mapper forms ─────────────────────────────────────
// Everything below is the SAME ControlSource verb the /api/* route above it
// calls — no new daemon surface, no second writer. What differs is only the
// wire shape a browser without scripting can produce: an
// `application/x-www-form-urlencoded` body in, a 303 to
// `/map?slot=N&flash=…` out. The flash is the page's whole feedback channel
// when there is no toast stack to write to.
//
// A form body names a SLOT NUMBER, never a preset: the server resolves one
// from the other against the config it just read, so a hand-made POST can
// only ever address a slot this cabinet actually has.

#[derive(Deserialize)]
struct MapSlotForm {
    slot: Option<u8>,
}

#[derive(Deserialize)]
struct MapBindForm {
    slot: Option<u8>,
    function: String,
    /// The `<select name="key">` value. The empty placeholder means "nothing
    /// picked" — an honest refusal, never a silent clear (that is what the
    /// Clear button beside it is for).
    #[serde(default)]
    key: Option<String>,
    /// The panel's checkbox. Present at all = ticked (HTML omits an unchecked
    /// box entirely), which is this path's answer to a cross-slot refusal.
    #[serde(default)]
    force: Option<String>,
    /// The row's turbo box. Blank leaves the rate alone; `0` clears it. Only
    /// the "Turbo" submit reads it — Bind/Add/Remove/Clear all send it too
    /// (one form, several verbs), and each one decides for itself.
    #[serde(default)]
    turbo_hz: Option<String>,
}

#[derive(Deserialize)]
struct MapRestoreForm {
    slot: Option<u8>,
    mode: String,
}

/// 303 back to the mapper, carrying the outcome as the flash. Errors are
/// flashed exactly like successes — the no-JS page must never fail silently.
fn map_redirect(slot: u8, outcome: Result<String, String>) -> Response {
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/map?slot={slot}&flash={}", urlencode(&flash))).into_response()
}

/// Run one slot-scoped verb for the slot a form named. Blocking work (a
/// config read plus one pipe request) off the async workers, like [`act`].
///
/// The whole SLOT is handed to the verb, not just its preset name, because
/// v10's add/remove-one are read-modify-write on the control's key list: the
/// same config read that resolves the preset already carries the bindings the
/// edit has to be computed against, so nothing reads the file twice and no
/// form has to be trusted with a key list it made up.
async fn map_act_slot<F>(state: Arc<AppState>, slot: Option<u8>, verb: F) -> Response
where
    F: FnOnce(&dyn ControlSource, &crate::snapshot::MapperSlot) -> Result<String, String>
        + Send
        + 'static,
{
    let (number, outcome) = tokio::task::spawn_blocking(move || {
        let mapper = state.source.mapper();
        let chosen = slot
            .and_then(|n| mapper.slots.iter().find(|s| s.number == n))
            .or_else(|| mapper.slots.first());
        match chosen {
            Some(slot) => (slot.number, verb(state.control.as_ref(), slot)),
            None => (0, Err(format!("nothing to map — {}", mapper.source))),
        }
    })
    .await
    .unwrap_or_else(|_| (0, Err("the control call panicked".to_owned())));
    map_redirect(number, outcome)
}

/// [`map_act_slot`] for the verbs that need nothing but the preset name.
async fn map_act<F>(state: Arc<AppState>, slot: Option<u8>, verb: F) -> Response
where
    F: FnOnce(&dyn ControlSource, &str) -> Result<String, String> + Send + 'static,
{
    map_act_slot(state, slot, move |control, slot| {
        verb(control, &slot.preset)
    })
    .await
}

/// One [`BindOutcome`] as the sentence a page with no JavaScript reads.
///
/// A cross-slot refusal names the other slot AND the way to say yes to it:
/// the learn flow asks with a Replace dialog, a form asks with the panel's
/// checkbox. Either way the answer is never "nothing happened".
fn bind_flash(function: &str, key: Option<&str>, outcome: BindOutcome) -> Result<String, String> {
    if outcome.ok {
        let mut line = match key {
            Some(key) => format!("{function} is now {key}"),
            None => format!("{function} is now unbound"),
        };
        if !outcome.also_drives.is_empty() {
            line.push_str(&format!(
                " — that key also drives {}",
                outcome.also_drives.join(", ")
            ));
        }
        line.push('.');
        return Ok(line);
    }
    Err(bind_refusal(function, key, outcome))
}

/// The refusal half of every write on this page, in one voice: a cross-slot
/// conflict names the other slot AND the checkbox that says yes to it, and
/// anything else quotes the daemon (or [`crate::control::multi_key_refusal`],
/// which already explains itself).
fn bind_refusal(function: &str, key: Option<&str>, outcome: BindOutcome) -> String {
    if outcome.code.as_deref() == Some("conflict") && !outcome.conflicts.is_empty() {
        let named = key.unwrap_or("that key");
        let who: Vec<String> = outcome
            .conflicts
            .iter()
            .map(|c| c.describe(named))
            .collect();
        return format!(
            "{function} was not changed: {} — tick \"let this key drive another slot's \
             control too\" in the Bind by name panel and submit again",
            who.join("; ")
        );
    }
    format!(
        "{function} was not changed: {}",
        outcome
            .error
            .unwrap_or_else(|| "the daemon refused the write".to_owned())
    )
}

/// One key-SET write as the sentence a page with no JavaScript reads. It
/// reports the control's whole list, because that is the thing that changed —
/// and it says out loud that the keys are alternatives, which is the fact a
/// row of two key tags does not carry on its own.
fn keys_flash(
    function: &str,
    key: Option<&str>,
    after: &[String],
    outcome: BindOutcome,
) -> Result<String, String> {
    if !outcome.ok {
        return Err(bind_refusal(function, key, outcome));
    }
    let mut line = match after {
        [] => format!("{function} is now unbound"),
        [one] => format!("{function} is now {one}"),
        _ => format!(
            "{function} now has {} — any one of them presses it",
            after.join(" · ")
        ),
    };
    if !outcome.also_drives.is_empty() {
        line.push_str(&format!(
            " — that key also drives {}",
            outcome.also_drives.join(", ")
        ));
    }
    line.push('.');
    Ok(line)
}

/// The key a form picked, or the refusal that names what to do instead.
/// Shared by every route that needs one, so "I forgot to pick a key" is the
/// same sentence everywhere.
fn picked_key(form: &MapBindForm, function: &str, verb: &str) -> Result<String, String> {
    form.key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            format!("no key picked for {function} — choose one from the list before \"{verb}\"")
        })
}

/// POST /map/add — ADD the picked key to what the control already has,
/// instead of replacing it (MAME-style OR-chaining: either key presses the
/// control, docs/INPUT-TRANSFORMS.md §1a). Read-modify-write on the key list
/// the config read already carries; the whole set goes to `bind_keys`.
async fn map_form_add(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapBindForm>,
) -> Response {
    let function = form.function.trim().to_owned();
    let force = form.force.is_some();
    let key = match picked_key(&form, &function, "Add") {
        Ok(key) => key,
        Err(refusal) => return map_redirect(form.slot.unwrap_or(0), Err(refusal)),
    };
    map_act_slot(state, form.slot, move |control, slot| {
        let current = slot.bindings.get(&function).cloned().unwrap_or_default();
        let next = crate::control::with_key(&current, &key);
        if next.len() == current.len() {
            return Ok(format!(
                "{function} already has {key} — nothing to add (it has {}).",
                current.join(" · ")
            ));
        }
        let outcome = control.bind_keys(&slot.preset, &function, &next, force, true, None);
        keys_flash(&function, Some(&key), &next, outcome)
    })
    .await
}

/// POST /map/key/remove — take ONE key off a control and leave the others.
/// The no-JS twin of the legend chips' per-key ✕: the row's key picker says
/// WHICH key goes, so removing one of several never needs JavaScript.
async fn map_form_remove_key(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapBindForm>,
) -> Response {
    let function = form.function.trim().to_owned();
    let key = match picked_key(&form, &function, "Remove key") {
        Ok(key) => key,
        Err(refusal) => return map_redirect(form.slot.unwrap_or(0), Err(refusal)),
    };
    map_act_slot(state, form.slot, move |control, slot| {
        let current = slot.bindings.get(&function).cloned().unwrap_or_default();
        let next = crate::control::without_key(&current, &key);
        if next.len() == current.len() {
            return Err(format!(
                "{function} was not changed: it is not bound to {key}{}",
                if current.is_empty() {
                    " (it is unbound)".to_owned()
                } else {
                    format!(" (it has {})", current.join(" · "))
                }
            ));
        }
        let outcome = control.bind_keys(&slot.preset, &function, &next, false, true, None);
        // The removed key is named in the sentence, because "A is now S" on
        // its own does not say what just left.
        keys_flash(&function, Some(&key), &next, outcome)
            .map(|line| format!("{key} removed. {line}"))
    })
    .await
}

/// POST /map/bind — the form twin of `/api/bind`.
async fn map_form_bind(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapBindForm>,
) -> Response {
    let function = form.function.trim().to_owned();
    let key = form
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    let force = form.force.is_some();
    if key.is_none() {
        return map_redirect(
            form.slot.unwrap_or(0),
            Err(format!(
                "no key picked for {function} — choose one from the list (\"Clear\" is how \
                 you unbind it)"
            )),
        );
    }
    map_act(state, form.slot, move |control, preset| {
        let request = BindRequest {
            preset: preset.to_owned(),
            function: function.clone(),
            key: key.clone(),
            force,
            // A binding-only edit is hot-swapped into a running session — the
            // pads stay plugged in (ksx-app `apply_bindings`).
            reload: true,
        };
        bind_flash(&function, key.as_deref(), control.bind(&request))
    })
    .await
}

/// POST /map/clear — the same `map` verb with a null key, which is exactly
/// what `ksx map --clear` writes. No second unbind path.
async fn map_form_clear(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapBindForm>,
) -> Response {
    let function = form.function.trim().to_owned();
    map_act(state, form.slot, move |control, preset| {
        let request = BindRequest {
            preset: preset.to_owned(),
            function: function.clone(),
            key: None,
            force: false,
            reload: true,
        };
        bind_flash(&function, None, control.bind(&request))
    })
    .await
}

/// POST /map/turbo — set (or clear) a control's AUTO-FIRE rate
/// (docs/INPUT-TRANSFORMS.md §3), the no-JS twin of the learn modal's Turbo
/// row.
///
/// It writes through the SAME `bind_keys` every other row verb uses, with the
/// control's CURRENT key list: turbo is a property of the control, so setting
/// it is a re-write of that control with one more field, not a second writer.
/// A blank box is a refusal rather than a silent clear — `0` is how you say
/// "off", exactly as it is on the CLI.
async fn map_form_turbo(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapBindForm>,
) -> Response {
    let function = form.function.trim().to_owned();
    let raw = form.turbo_hz.as_deref().map(str::trim).unwrap_or("");
    let hz = match raw.parse::<u32>() {
        Ok(hz) => hz,
        Err(_) => {
            return map_redirect(
                form.slot.unwrap_or(0),
                Err(format!(
                    "no turbo rate given for {function} — type a number of presses a second                      into the box (0 turns auto-fire off)"
                )),
            )
        }
    };
    map_act_slot(state, form.slot, move |control, slot| {
        let current = slot.bindings.get(&function).cloned().unwrap_or_default();
        if current.is_empty() && hz > 0 {
            return Err(format!(
                "{function} has no keys, so there is nothing to auto-fire — bind a key first"
            ));
        }
        let outcome = control.bind_keys(&slot.preset, &function, &current, false, true, Some(hz));
        if !outcome.ok {
            return Err(bind_refusal(&function, None, outcome));
        }
        Ok(match (hz, outcome.turbo_effective_hz) {
            (0, _) => format!("{function} no longer auto-fires."),
            (asked, Some(effective)) if effective != asked => format!(
                "{function} auto-fires at about {effective} Hz — {asked} Hz was asked for, but                  a press AND a release must each survive a 60 Hz poll."
            ),
            (asked, _) => format!("{function} auto-fires at {asked} Hz."),
        })
    })
    .await
}

/// POST /map/preset/restore — the form twin of `/api/preset/restore`, same
/// three destinations, same validation before any daemon round trip.
async fn map_form_restore(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapRestoreForm>,
) -> Response {
    let mode = form.mode.trim().to_owned();
    let Some(mode) = crate::control::RestoreMode::parse(&mode) else {
        return map_redirect(
            form.slot.unwrap_or(0),
            Err(format!(
                "unknown restore mode \"{mode}\" ({})",
                crate::control::RESTORE_MODES.join(" | ")
            )),
        );
    };
    map_act(state, form.slot, move |control, preset| {
        control.restore(preset, mode).map_err(flash_of)
    })
    .await
}

/// POST /map/preset/clear-all — the form twin of `/api/preset/clear-all`.
async fn map_form_clear_all(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapSlotForm>,
) -> Response {
    map_act(state, form.slot, move |control, preset| {
        control.clear_all(preset).map_err(flash_of)
    })
    .await
}

/// POST /map/session/stop — "Pause emulation & map" without JavaScript. The
/// same `stop` verb the status page's form posts; it just comes back to /map
/// so the user keeps their place. (There is no form twin for Resume: the
/// resume bar is client-only state — this page having paused something — so
/// a no-JS page never shows it. `/` starts a session back up.)
async fn map_form_session_stop(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapSlotForm>,
) -> Response {
    let slot = form.slot.unwrap_or(0);
    let outcome = tokio::task::spawn_blocking(move || state.control.stop().map_err(flash_of))
        .await
        .unwrap_or_else(|_| Err("the control call panicked".to_owned()));
    map_redirect(slot, outcome)
}

// ---------------------------------------------------------------------------
// /devices — enumerate, pick, remove
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DevicesQuery {
    flash: Option<String>,
}

/// One fresh device scan + session view.
///
/// Blocking on both halves and then some: this walks the whole USB tree,
/// re-reads config.toml and games.toml, and dials the daemon pipe for the
/// session line. Never cached, for the reason `sources.rs` gives — a board
/// that was unplugged ten minutes ago must stop being offered.
///
/// A panic here renders a page that SAYS the scan failed rather than a 500,
/// the same contract `collect` and `collect_map` keep: a dead-end error page
/// stops the refresh loop, and the user is then looking at a browser error
/// instead of at their cabinet.
async fn collect_devices(state: &Arc<AppState>) -> DevicesPayload {
    let scan_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let session = scan_state.control.session();
        match scan_state.machine.device_scan() {
            Ok(scan) => DevicesPayload {
                scan,
                session,
                unavailable: String::new(),
                flash: None,
            },
            Err(refusal) => DevicesPayload {
                scan: ksx_api::DeviceScanView::default(),
                session,
                // The refusal's own sentence, plus its way out. This is the
                // one place the remedy is NOT dropped: it is going onto a
                // page, not into a 300-character flash, and "run `ksx
                // devices`" is the whole value of the message.
                unavailable: match &refusal.remedy {
                    Some(remedy) => format!("{} — {remedy}", refusal.message),
                    None => refusal.message.clone(),
                },
                flash: None,
            },
        }
    })
    .await
    .unwrap_or_else(|_| DevicesPayload {
        scan: ksx_api::DeviceScanView::default(),
        session: SessionView::unreachable("the device scan panicked"),
        unavailable: "the device scan panicked — nothing below is a reading of this machine"
            .to_owned(),
        flash: None,
    })
}

// ── /setup: the config first, and the first run ────────────────────────────
//
// Two verbs a user sees. EXPORT hands back a file; IMPORT takes a document.
// Neither takes a path — `ksx_api::MachineSource::{config_export,
// config_import}` are in-memory on purpose, so no screen has to put a
// filesystem in front of someone who asked for their configuration.
//
// Three steps, each ONE backend verb, and each independently resumable: none of
// them is a wizard step, so there is no half-written state to come back to.
// Step 1 belongs to `/devices` and is a link. Steps 2 and 3 are the POSTs
// below.

/// One fresh setup payload. The machine read hits the config store and the two
/// control calls hit the daemon pipe — blocking work, kept off the async
/// workers exactly like [`collect`].
async fn collect_setup(state: &Arc<AppState>) -> SetupPayload {
    let setup_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let setup = match setup_state.machine.setup_state() {
            Ok(view) => SetupSnapshot::ready(view),
            // A refusal is a FACT to render, not a blank page: "this build has
            // no machine provider" and "this machine has nothing configured"
            // want opposite advice.
            Err(refusal) => SetupSnapshot::unavailable(&refusal.message),
        };
        SetupPayload {
            setup,
            session: setup_state.control.session(),
            // Step 3's whole read. Doing it here rather than in client code is
            // what makes "press a button and watch it land" work with
            // scripting off — the <noscript> refresh repaints the key.
            learn: setup_state.control.learn_poll(),
            flash: None,
            ..SetupPayload::default()
        }
        // The page's sentences and its show booleans, composed from the three
        // reads above (snapshot.rs). Composed HERE means the poller's JSON and
        // the server paint carry the identical words — the client derives
        // none of them.
        .composed()
    })
    .await
    .unwrap_or_else(|_| {
        SetupPayload {
            setup: SetupSnapshot::unavailable("the setup collection panicked"),
            session: SessionView::unreachable("the setup collection panicked"),
            learn: crate::control::LearnView::unavailable("the setup collection panicked"),
            flash: None,
            ..SetupPayload::default()
        }
        .composed()
    })
}

async fn devices_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DevicesQuery>,
) -> Response {
    let mut payload = collect_devices(&state).await;
    let flash = query
        .flash
        .as_deref()
        .filter(|f| !f.trim().is_empty())
        .map(str::to_owned);
    payload.flash = flash.clone();
    let out = render_devices(&state.devices_page, &payload, flash.as_deref());
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

async fn setup_screen(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let payload = collect_setup(&state).await;
    let flash = query.flash.as_deref().filter(|f| !f.trim().is_empty());
    let out = render_setup(&state.setup_page, &payload, flash);
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The poller's endpoint: the SAME [`DevicesPayload`] shape the page embeds
/// (parity unit-tested in render_devices.rs). `flash` is always null — a poll
/// is not an action.
async fn api_devices(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_devices(&state).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

/// The setup poller's endpoint — the same [`SetupPayload`] the page embeds
/// (parity pinned in render_setup.rs).
async fn api_setup(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_setup(&state).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

#[derive(Deserialize)]
struct PickForm {
    /// The interface the row's hidden field carries — an instance path. The
    /// backend's own resolver accepts an alias or a unique substring too, and
    /// this posts whatever the user's row held, so the two verbs cannot
    /// disagree about what an argument means.
    query: String,
    /// The name `[[slot]]` entries will use. Blank means "derive one from the
    /// board", exactly like the absent `--alias` flag — a web form always
    /// submits the field, so the emptiness has to survive to the writer as
    /// `None` rather than as an empty alias it would then refuse.
    alias: Option<String>,
}

#[derive(Deserialize)]
struct RemoveForm {
    alias: String,
    /// The row's checkbox. Present at all = ticked (HTML omits an unchecked
    /// box entirely) — the same shape the mapper's cross-slot panel uses.
    #[serde(default)]
    force: Option<String>,
}

/// 303 back to the picker, carrying the outcome as the flash.
///
/// Errors flash exactly like successes: this page's whole job is deciding
/// which board drives which slot, and a Remove that silently did nothing is
/// how someone ends up debugging a cabinet that was never changed.
fn devices_redirect(outcome: Result<String, String>) -> Response {
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/devices?flash={}", urlencode(&flash))).into_response()
}

/// POST /devices/pick — write one `[[device]]` entry.
///
/// One backend verb, and deliberately the typed one: `device_edit::pick` (the
/// CLI entry point) routes refusals through a function that calls
/// `std::process::exit`, which would take the whole server down on a mistyped
/// alias. `MachineSource::device_pick` is the same plan/apply pair with a
/// `Refusal` on the error arm.
async fn devices_form_pick(
    State(state): State<Arc<AppState>>,
    Form(form): Form<PickForm>,
) -> Response {
    let spec = ksx_api::DevicePickSpec {
        query: form.query,
        alias: form.alias,
    };
    let outcome = tokio::task::spawn_blocking(move || {
        state
            .machine
            .device_pick(&spec)
            .map(|view| view.summary)
            .map_err(flash_of)
    })
    .await
    .unwrap_or_else(|_| Err("the device pick panicked".to_owned()));
    devices_redirect(outcome)
}

/// POST /devices/remove — delete one `[[device]]` entry.
///
/// The narrowest of ksx's three removals, and the page says so beside the
/// button. `RemoveOutcome`'s summary is what flashes, and it carries the one
/// fact that surprises people: deleting the entry did not release a claimed
/// board.
async fn devices_form_remove(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RemoveForm>,
) -> Response {
    let spec = ksx_api::DeviceRemoveSpec {
        alias: form.alias,
        force: form.force.is_some(),
    };
    let outcome = tokio::task::spawn_blocking(move || {
        state
            .machine
            .device_remove(&spec)
            .map(|view| view.summary)
            .map_err(flash_of)
    })
    .await
    .unwrap_or_else(|_| Err("the device removal panicked".to_owned()));
    devices_redirect(outcome)
}

// ── /start: the first run (docs/FIRST-RUN.md moments 4–7) ──────────────────

/// One fresh first-run payload: the staged setup, the device enumeration, the
/// presets on disk and whether a pad can be plugged at all.
///
/// Four reads with four failure modes, kept apart all the way to the page —
/// `SURFACES.md` §1b. A dead daemon must not read as "you have staged
/// nothing", a refused enumeration must not read as "you have no keyboards",
/// an unreadable presets folder must not read as "nothing would be replaced",
/// and a driver check that did not answer must not read as a working bus. Each
/// degrades to the honest value its own type provides
/// (`StagedSetupView::unreachable`, `DeviceScanView::default`, a non-empty
/// `presets_error`, `PadBusView::unreadable`) and never to `Default::default()`.
///
/// Never cached, and that is `FIRST-RUN.md` §5's visible-rescan requirement
/// met by construction: a user who plugs a keyboard in while this page is open
/// sees it at the next 2 s poll, without knowing a scan exists.
async fn collect_start(state: &Arc<AppState>) -> StartPayload {
    let start_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let staged = start_state.control.staged();
        let session = start_state.control.session();
        let (scan, unavailable) = match start_state.machine.device_scan() {
            Ok(scan) => (scan, String::new()),
            Err(refusal) => (ksx_api::DeviceScanView::default(), flash_of(refusal)),
        };
        let (presets, presets_error) = match start_state.machine.presets() {
            Ok(view) => (view.presets, String::new()),
            Err(refusal) => (Vec::new(), flash_of(refusal)),
        };
        // A refusal becomes the UNREADABLE view, never the default one — and
        // `PadBusView`'s default is itself unreadable, so neither path can
        // paint a healthy bus onto a machine nobody looked at.
        let pad_bus = start_state
            .machine
            .pad_bus()
            .unwrap_or_else(|refusal| ksx_api::PadBusView::unreadable(flash_of(refusal)));
        StartPayload {
            staged,
            scan,
            session,
            pad_bus,
            unavailable,
            presets,
            presets_error,
            flash: None,
            ..StartPayload::default()
        }
        .composed()
    })
    .await
    .unwrap_or_else(|_| {
        StartPayload {
            staged: ksx_api::StagedSetupView::unreachable("the first-run collection panicked"),
            scan: ksx_api::DeviceScanView::default(),
            session: SessionView::unreachable("the first-run collection panicked"),
            pad_bus: ksx_api::PadBusView::unreadable("the first-run collection panicked"),
            unavailable: "the device scan panicked — nothing below is a reading of this machine"
                .to_owned(),
            presets_error: "the preset read panicked".to_owned(),
            ..StartPayload::default()
        }
        .composed()
    })
}

async fn start_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let mut payload = collect_start(&state).await;
    let flash = query
        .flash
        .as_deref()
        .filter(|f| !f.trim().is_empty())
        .map(str::to_owned);
    payload.flash = flash.clone();
    let out = render_start(&state.start_page, &payload, flash.as_deref());
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The poller's endpoint — the SAME [`StartPayload`] the page embeds (parity
/// pinned in render_start.rs). `flash` is always null: a poll is not an action.
async fn api_start(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_start(&state).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

#[derive(Deserialize)]
struct StartDeviceForm {
    /// The `ksx_core::DeviceSelector` the row carried
    /// (`ksx_api::BoardRow::selector`, served). **Never a path anybody typed**
    /// — `FIRST-RUN.md` §6 forbids asking, and the page has no text input.
    selector: String,
    alias: String,
    label: String,
}

#[derive(Deserialize)]
struct StartControllerForm {
    persona: String,
    /// The preset name, from `StagedSetupView::next_preset`. Served rather than
    /// typed, because it becomes a file name.
    preset: String,
    /// The in-box layout it starts from — a `TemplateRow::id` off the SERVED
    /// roster (`StagedSetupView::layouts`), never a name anybody typed.
    ///
    /// Optional on the wire because a form without the field is a legal thing
    /// for a client to send, and the backend has one honest answer for it: a
    /// controller that binds nothing, which `commit()` then refuses by name.
    #[serde(default)]
    layout: Option<String>,
}

#[derive(Deserialize)]
struct StartLayoutForm {
    number: u8,
    layout: String,
}

#[derive(Deserialize)]
struct StartSlotForm {
    number: u8,
}

#[derive(Deserialize)]
struct StartBlockingForm {
    blocking: String,
}

/// 303 back to the first-run page, carrying the outcome as the flash.
///
/// Refusals flash exactly like successes and carry ksx-core's own sentence:
/// this flow is four decisions deep and a click that silently did nothing is
/// how somebody ends up pressing Play on a setup they think has two controllers
/// in it.
fn start_redirect(outcome: Result<String, String>) -> Response {
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/start?flash={}", urlencode(&flash))).into_response()
}

/// Run one staging edit off the async workers (the pipe client blocks) and
/// 303 back.
///
/// Every one of these touches ONE value in the daemon and nothing else — no
/// file, no driver, no session. That is `FIRST-RUN.md` §2, and it is why this
/// helper has no confirm step, no backup and no dry run: there is nothing to
/// undo, because there is nothing to have done.
async fn stage_edit(state: Arc<AppState>, edit: ksx_api::StageEdit) -> Response {
    let outcome = tokio::task::spawn_blocking(move || {
        let outcome = state.control.stage_edit(&edit);
        if outcome.ok {
            Ok(outcome.headline())
        } else {
            Err(outcome.headline())
        }
    })
    .await
    .unwrap_or_else(|_| Err("the staging edit panicked".to_owned()));
    start_redirect(outcome)
}

/// POST /start/device — moment 4. Replaces any earlier choice, freely.
async fn start_form_device(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartDeviceForm>,
) -> Response {
    stage_edit(
        state,
        ksx_api::StageEdit::ChooseDevice {
            selector: form.selector,
            alias: form.alias,
            label: form.label,
        },
    )
    .await
}

/// POST /start/controller — moment 5. `number: None` so the backend picks the
/// lowest free slot: a first-run user must never be asked for a slot number.
async fn start_form_controller(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartControllerForm>,
) -> Response {
    stage_edit(
        state,
        ksx_api::StageEdit::AddSlot {
            number: None,
            persona: form.persona,
            preset: form.preset,
            layout: form.layout,
        },
    )
    .await
}

/// POST /start/controller/layout — moment 6's menu half.
///
/// The surface names a layout; ksx-core builds the preset. Nothing here writes
/// a file: the bindings land in the staged slot, which is what makes "map it"
/// a step this flow can actually perform before anything has been saved. The
/// player block follows the slot number, so nobody is asked what a player block
/// is.
async fn start_form_layout(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartLayoutForm>,
) -> Response {
    stage_edit(
        state,
        ksx_api::StageEdit::SetLayout {
            number: form.number,
            layout: form.layout,
            player: None,
        },
    )
    .await
}

/// POST /start/controller/remove — moment 5's other half. Free and complete.
async fn start_form_remove(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartSlotForm>,
) -> Response {
    stage_edit(
        state,
        ksx_api::StageEdit::RemoveSlot {
            number: form.number,
        },
    )
    .await
}

/// POST /start/blocking — moment 6's one question (`FIRST-RUN.md` §3).
async fn start_form_blocking(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartBlockingForm>,
) -> Response {
    stage_edit(
        state,
        ksx_api::StageEdit::SetBlocking {
            blocking: form.blocking,
        },
    )
    .await
}

/// POST /start/discard — "Start over". §2 requires that it always works.
async fn start_form_discard(State(state): State<Arc<AppState>>) -> Response {
    stage_edit(state, ksx_api::StageEdit::Discard).await
}

/// POST /start/save — moment 7, half one. **One config write.**
///
/// The same shape `/setup/slot` and `/devices/pick` use: a backend verb that
/// takes a timestamped backup and hands the I/O to the store's atomic save.
/// It starts nothing — `Committed::message` says so in words, because "saved"
/// and "playing" are the two states this flow must never let anyone confuse.
async fn start_form_save(State(state): State<Arc<AppState>>) -> Response {
    let outcome = tokio::task::spawn_blocking(move || {
        let outcome = state.control.stage_commit();
        if outcome.ok {
            Ok(outcome.headline())
        } else {
            Err(outcome.headline())
        }
    })
    .await
    .unwrap_or_else(|_| Err("the save panicked".to_owned()));
    start_redirect(outcome)
}

/// POST /start/play — moment 7, half two. **Starts a session and writes
/// nothing.**
///
/// Separate from Save on purpose (§2: "saving and playing are separate acts"),
/// and not a flag on it: a combined button would make the two indistinguishable
/// at the moment a user is deciding whether to commit to anything at all. The
/// plan is built in the daemon from the staged value with no file read
/// (`ksx-app`'s `stage::plan`), so a session that starts here means exactly
/// what the screen showed.
async fn start_form_play(State(state): State<Arc<AppState>>) -> Response {
    let outcome = tokio::task::spawn_blocking(move || {
        let outcome = state.control.stage_play();
        if outcome.ok {
            Ok(outcome.headline())
        } else {
            Err(outcome.headline())
        }
    })
    .await
    .unwrap_or_else(|_| Err("the staged start panicked".to_owned()));
    start_redirect(outcome)
}

/// 303 back to /setup with the outcome as the flash. Errors flash too — this
/// page must never fail silently, and its no-JS path has nowhere else to look.
fn setup_redirect(outcome: Result<String, String>) -> Response {
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/setup?flash={}", urlencode(&flash))).into_response()
}

/// Comma-separated form words → the `what` list the api verbs take. Empty means
/// "whatever the document carries" / "the whole root", which is what both verbs
/// already document.
fn what_words(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Deserialize)]
struct ExportQuery {
    /// `config,games,presets` — absent means the whole root.
    what: Option<String>,
}

/// GET /setup/export.json — the configuration as a download.
///
/// A GET because it writes nothing (see the route comment). The response is the
/// document itself with a `Content-Disposition`, which is what makes an
/// ordinary `<a download>` work with scripting switched off — no blob, no
/// clipboard, no path to type.
async fn setup_export(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ExportQuery>,
) -> Response {
    let what = what_words(query.what.as_deref());
    let outcome = tokio::task::spawn_blocking(move || {
        state
            .machine
            .config_export(&ksx_api::ExportRequest { what })
    })
    .await
    .unwrap_or_else(|_| {
        Err(ksx_api::Refusal::new(
            ksx_api::codes::REFUSED,
            "the export panicked",
        ))
    });

    let export = match outcome {
        Ok(export) => export,
        // Back to the page with the reason, rather than a bare error body: the
        // user clicked a link on a page, so the page is where the answer goes.
        Err(refusal) => return setup_redirect(Err(refusal.message)),
    };

    let disposition = format!("attachment; filename=\"{}\"", export.filename);
    let mut response = export.document.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Every field optional on purpose. A missing one is a REFUSAL WITH A SENTENCE
/// (303 + `?flash=error: …`), not axum's 422 — this page's whole feedback
/// channel with scripting off is the flash, and a bare status page would
/// dead-end the user with nothing to read.
#[derive(Deserialize)]
struct ImportForm {
    #[serde(default)]
    document: Option<String>,
    #[serde(default)]
    what: Option<String>,
    /// The "write it" box. Present at all = ticked (HTML omits an unchecked box
    /// entirely), so an absent field is a DRY RUN — which is the consent shape
    /// `ksx config import` has always had, arriving here for free.
    #[serde(default)]
    apply: Option<String>,
    #[serde(default)]
    force: Option<String>,
}

/// POST /setup/import — one `MachineSource::config_import`.
///
/// The report is structured; the flash is one line (`urlencode` caps at 300
/// characters). What the line carries is chosen rather than truncated: a
/// refusal names the FIRST fault and how many more there are, because the
/// commonest way an import fails is a document that will not validate, and
/// telling the owner of this page to go and run a CLI to read a list the page
/// is already holding is the dead end this screen exists to remove.
///
/// The extractor is a `Result` on purpose. Every refusal on this route is a
/// flashed sentence — that is the whole feedback channel with scripting off —
/// so an over-large paste or a wrong content type has to arrive as one too,
/// rather than as axum's bare 413/415 with no way back to the page.
async fn setup_form_import(
    State(state): State<Arc<AppState>>,
    form: Result<Form<ImportForm>, axum::extract::rejection::FormRejection>,
) -> Response {
    let Ok(Form(form)) = form else {
        return setup_redirect(Err(
            "that document could not be read — it may be larger than this page accepts \
             (8 MB). Import it with `ksx config import <file>` instead"
                .to_owned(),
        ));
    };
    let request = ksx_api::ImportRequest {
        document: form.document.unwrap_or_default(),
        what: what_words(form.what.as_deref()),
        apply: form.apply.is_some(),
        force: form.force.is_some(),
    };
    if request.document.trim().is_empty() {
        return setup_redirect(Err(
            "nothing to import — paste a configuration into the box first".to_owned(),
        ));
    }
    let outcome = tokio::task::spawn_blocking(move || state.machine.config_import(&request))
        .await
        .unwrap_or_else(|_| {
            Err(ksx_api::Refusal::new(
                ksx_api::codes::REFUSED,
                "the import panicked",
            ))
        });
    setup_redirect(match outcome {
        Ok(report) if report.ok => Ok(import_flash(&report)),
        Ok(report) => Err(import_flash(&report)),
        Err(refusal) => Err(refusal.message),
    })
}

/// One [`ksx_api::ImportReport`] as the sentence this page flashes.
///
/// The backend composes the fact and names no control (`onboard::import`); each
/// surface adds its own. Here that is two things the report cannot know: the
/// label on THIS page's consent box, and the first of the faults it is holding.
fn import_flash(report: &ksx_api::ImportReport) -> String {
    let mut line = report.summary.clone();
    if let Some(first) = report.faults.first() {
        line.push_str(&format!(" First: {first}"));
        let rest = report.faults.len() - 1;
        if rest > 0 {
            line.push_str(&format!(" (+{rest} more)"));
        }
    } else if report.ok && !report.applied {
        // A clean dry run. The backend said what it WOULD do and that nothing
        // was written; "write it" is the name of the box on this page and
        // nowhere else.
        line.push_str(" Tick \"write it\" and import again to apply.");
    }
    line
}

#[derive(Deserialize)]
struct SetupSlotForm {
    /// Optional so a malformed post is a flashed refusal rather than a 422 —
    /// same rule as [`ImportForm`].
    #[serde(default)]
    slot: Option<u8>,
    #[serde(default)]
    preset: Option<String>,
    /// The `<select>`'s "(this cabinet's config)" sentinel is the empty string:
    /// no profile, so `config.toml`'s `[[slot]]` list.
    #[serde(default)]
    profile: Option<String>,
    /// The persona `<select>`, whose "(leave it as it is)" sentinel is the
    /// empty string. Blank never means `xbox360`: it means the form was not
    /// asked about the persona, and the slot keeps whatever it presents itself
    /// as today. See [`ksx_api::SlotAssignRequest::persona`].
    #[serde(default)]
    persona: Option<String>,
}

/// POST /setup/slot — step 2, one `ControlSource::assign_slot` (pipe
/// `slot-assign`, the same verb `ksx slot assign` performs).
///
/// `reload` is asked for, and unlike every other reload on this protocol it is
/// a BOUNCE: the pads replug. The page says so above the button, because after
/// the click is too late to be told.
async fn setup_form_slot(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetupSlotForm>,
) -> Response {
    let preset = form.preset.unwrap_or_default().trim().to_owned();
    if preset.is_empty() {
        return setup_redirect(Err(
            "no preset picked — a slot has to point at one".to_owned()
        ));
    }
    let Some(slot) = form.slot else {
        return setup_redirect(Err(
            "no slot picked — choose which player this preset is for".to_owned(),
        ));
    };
    let request = ksx_api::SlotAssignRequest {
        slot,
        preset: Some(preset),
        profile: form
            .profile
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
        // Verbatim, blank dropped — the persona NAME is the backend's to
        // parse and to refuse. A page that validated it here would be the
        // second copy of `Persona::FromStr` docs/SURFACES.md §1 forbids, and
        // it would go stale against ksx-core silently.
        persona: form
            .persona
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
        reload: true,
    };
    let outcome = tokio::task::spawn_blocking(move || state.control.assign_slot(&request))
        .await
        .unwrap_or_else(|_| {
            crate::control::SlotOutcome::failed(
                "the control call panicked",
                "run `ksx slot assign --slot N --preset NAME`",
            )
        });
    setup_redirect(slot_flash(outcome))
}

/// One [`crate::control::SlotOutcome`] as the sentence this page flashes.
///
/// **The canonical formatters, not a second reconstruction.**
/// [`ksx_api::SlotOutcome::headline`] is the designated one-line renderer and
/// is what the cabinet (`ksx-cabinet/src/app.rs`) and the daemon
/// (`ksx-app/src/daemon/pipe.rs`) print; `refusal()` is the matching one for
/// the error arm, and it carries the CODE and the remedy that a hand-built
/// `unwrap_or` throws away.
///
/// This used to rebuild the sentence from flags — `message` then `if restarted
/// { push(" The pads replugged.") }` — which is the exact re-derivation
/// `control.rs::a_slot_outcome_prints_what_the_daemon_said_rather_than_re_deriving_it`
/// forbids by name. It went wrong three ways: it double-named the bounce when
/// the daemon's own sentence already named it, it lost slot/preset/`unchanged`
/// when there was no sentence, and it had no way to say "nothing was running,
/// so nothing had to restart" — which is the state this page's own wire form is
/// offered in, because it turns on `reachable`, not `running`.
fn slot_flash(outcome: crate::control::SlotOutcome) -> Result<String, String> {
    match outcome.refusal() {
        Some(refusal) => Err(refusal.message),
        None => Ok(outcome.headline()),
    }
}

/// POST /setup/prove — step 3, `ControlSource::learn_start` (pipe `learn-key`).
///
/// The daemon's own learner, unchanged: the mapper's "press a key" dialog is
/// the same two verbs. Nothing new is listening to a keyboard here.
async fn setup_form_prove(State(state): State<Arc<AppState>>) -> Response {
    let outcome = tokio::task::spawn_blocking(move || state.control.learn_start())
        .await
        .unwrap_or_else(|_| crate::control::LearnView::unavailable("the control call panicked"));
    setup_redirect(learn_flash(
        outcome,
        "Listening — press a button on the panel.",
    ))
}

/// POST /setup/prove/cancel — `ControlSource::learn_cancel`.
async fn setup_form_prove_cancel(State(state): State<Arc<AppState>>) -> Response {
    let outcome = tokio::task::spawn_blocking(move || state.control.learn_cancel())
        .await
        .unwrap_or_else(|_| crate::control::LearnView::unavailable("the control call panicked"));
    setup_redirect(learn_flash(outcome, "Stopped listening."))
}

fn learn_flash(view: crate::control::LearnView, done: &str) -> Result<String, String> {
    match view.refusal() {
        Some(refusal) => Err(refusal.message),
        None => Ok(done.to_owned()),
    }
}

// ---------------------------------------------------------------------------
// /pads — the ViGEm bus, a bounded pad test, and the prune (v15)
// ---------------------------------------------------------------------------
//
// Every fact and every refusal on this page arrives from ONE
// `ksx_api::MachineSource` call. Nothing here counts pads, decides whether a
// bus restart is allowed, or works out how many of eight Xbox pads a game
// could actually read — those are backend decisions with backend tests, and a
// second copy of any of them living in a web page is the failure
// docs/SURFACES.md §1 names by example.

#[derive(Deserialize)]
struct PadsQuery {
    /// `1` ARMS the prune: the page re-reads the plan and renders every pad it
    /// would remove, beside a real submit. Deliberately a GET — showing
    /// someone what a destructive button will destroy must not itself change
    /// anything, so a reload, a bookmark or a back button are all harmless.
    confirm: Option<String>,
    flash: Option<String>,
}

/// One fresh pads payload. `MachineSource::pads_view` enumerates devnodes and
/// reads XInput — blocking work, kept off the async workers like [`collect`]
/// and [`collect_map`].
///
/// **The session is read ONCE and fed to the view.** It appears twice on this
/// screen — the header pill, and the spawn panel's refusal — and two separate
/// pipe round-trips inside one render are two separate points in time: a
/// session that starts between them paints "idle" beside "a session is
/// running", or offers a Spawn button the verb will refuse.
async fn collect_pads(state: &Arc<AppState>, confirm: bool) -> PadsPayload {
    let pads_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let session = pads_state.control.session();
        match pads_state.machine.pads_view(session.running) {
            Ok(pads) => PadsPayload {
                pads,
                session,
                confirm,
                unavailable: None,
                flash: None,
            },
            // A provider that cannot answer renders a banner, never a 500 and
            // never an empty pad list that reads as "your bus is clean" — and
            // `unreadable` is what makes the difference, because a
            // `PadsView::default()` here would render four empty pad tiles and
            // a devnode line asserting ViGEmBus is not installed, about a bus
            // ksx never managed to look at.
            Err(refusal) => {
                let reason = match refusal.remedy.as_deref() {
                    Some(remedy) => format!("{} — {remedy}", refusal.message),
                    None => refusal.message,
                };
                PadsPayload {
                    pads: ksx_api::PadsView::unreadable(&reason),
                    session,
                    confirm,
                    unavailable: Some(reason),
                    flash: None,
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| PadsPayload {
        pads: ksx_api::PadsView::unreadable("the pad collection panicked"),
        session: SessionView::unreachable("pad collection panicked"),
        confirm,
        unavailable: Some("the pad collection panicked".to_owned()),
        flash: None,
    })
}

async fn pads_page(State(state): State<Arc<AppState>>, Query(query): Query<PadsQuery>) -> Response {
    let mut payload = collect_pads(&state, query.confirm.as_deref() == Some("1")).await;
    let flash = query.flash.as_deref().filter(|f| !f.trim().is_empty());
    payload.flash = flash.map(str::to_owned);
    let out = crate::render_pads::render_pads(&state.pads_page, &payload);
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The pads poller's endpoint — the same [`PadsPayload`] the page embeds.
///
/// `confirm` is false here whatever the page is showing: a 2 s poll is not a
/// user arming a destructive action, and letting a poll re-arm it would mean
/// the confirm panel could reappear after the user had walked away from it.
async fn api_pads(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_pads(&state, false).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

/// One fresh [`CheckPayload`]: the slot roster and the session, on a blocking
/// worker like every other collector read.
///
/// Deliberately the SAME `StatusSource::mapper()` the mapper page calls. The
/// button check's control roster is a preset's binding table — there is no
/// second read of it and no second shape for it, so a preset edit made on
/// /map is on /check at the next roster poll with nothing to keep in step.
async fn collect_check(state: &Arc<AppState>) -> CheckPayload {
    let check_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        crate::render_check::payload(check_state.source.mapper(), check_state.control.session())
    })
    .await
    .unwrap_or_else(|_| {
        crate::render_check::payload(
            ksx_api::MapperSnapshot::unavailable("reading the slots panicked"),
            SessionView::unreachable("reading the slots panicked"),
        )
    })
}

/// `GET /check` — BUILD C, the button check (docs/MAPPER-UX.md).
///
/// The document is the binding table, read from disk; the lighting-up is
/// `/api/live` beside it. Both halves are needed and only one of them is here
/// — see `crate::render_check` for why that split IS the page.
async fn check_page(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_check(&state).await;
    let out = render_check(&state.check_page, &payload);
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The roster poller's endpoint — the same [`CheckPayload`] the /check page
/// embeds as island props (parity unit-tested in render_check.rs).
///
/// Polled every few SECONDS, not at display rate: this is the structure, and
/// the structure only changes when somebody edits a preset.
async fn api_check(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_check(&state).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

/// `GET /api/live` — the daemon's input fan-out, as Server-Sent Events.
///
/// The one endpoint on this server that is a SUBSCRIPTION rather than an
/// answer. It never touches the other three providers: it hands the request's
/// own bridge thread a handle to [`AppState::live`] and gets out of the way.
/// `crate::live` holds the design — why SSE, why the channel is one slot deep
/// and blocking, and why "no daemon" is a 200 with a refusal event rather than
/// a status code.
async fn api_live(State(state): State<Arc<AppState>>) -> Response {
    crate::live::stream(Arc::clone(&state.live))
}

/// 303 back to /pads, carrying the outcome as the flash. Errors flash exactly
/// like successes — a page with no JavaScript must never fail silently.
fn pads_redirect(outcome: Result<String, String>) -> Response {
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/pads?flash={}", urlencode(&flash))).into_response()
}

/// Run one machine verb off the async workers, then 303 back to /pads.
async fn pads_act<F>(state: Arc<AppState>, verb: F) -> Response
where
    F: FnOnce(&dyn ksx_api::MachineSource) -> Result<String, String> + Send + 'static,
{
    let outcome = tokio::task::spawn_blocking(move || verb(state.machine.as_ref()))
        .await
        .unwrap_or_else(|_| Err("the machine call panicked".to_owned()));
    pads_redirect(outcome)
}

#[derive(Deserialize)]
struct SpawnForm {
    #[serde(default)]
    count: u8,
    #[serde(default)]
    persona: String,
    #[serde(default)]
    hold_secs: u64,
}

/// POST /pads/spawn — `ksx pads --count N --persona P`, bounded.
///
/// No validation here, on purpose: an absent field arrives as 0/"" and the
/// backend plan refuses it in words ("pad count must be 1..=16, got 0"). A
/// second copy of the bounds in this handler is a second thing to keep in step
/// with `ksx_core::MAX_SLOTS`.
async fn pads_form_spawn(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SpawnForm>,
) -> Response {
    pads_act(state, move |machine| {
        machine
            .pads(&ksx_api::PadsSpawnSpec {
                count: form.count,
                persona: form.persona,
                hold_secs: form.hold_secs,
            })
            .map_err(flash_of)
    })
    .await
}

#[derive(Deserialize)]
struct PruneForm {
    /// The armed panel's hidden field, and the only thing that turns this from
    /// a dry run into a bus restart.
    ///
    /// Its absence is not an error: `pads_prune(false)` is the CLI's own dry
    /// run, so a POST that did not come from the confirm screen answers with
    /// what WOULD happen instead of doing it. The guard stops another site;
    /// this is what stops a stray submit from this one.
    #[serde(default)]
    confirm: Option<String>,
}

/// POST /pads/prune — `ksx pads --prune`, with `--yes` spelled `confirm=yes`.
async fn pads_form_prune(
    State(state): State<Arc<AppState>>,
    Form(form): Form<PruneForm>,
) -> Response {
    let confirm = form.confirm.as_deref() == Some("yes");
    pads_act(state, move |machine| {
        machine.pads_prune(confirm).map_err(flash_of)
    })
    .await
}

#[derive(Deserialize)]
struct StartForm {
    profile: Option<String>,
}

async fn session_start(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartForm>,
) -> Response {
    // "" is the dropdown's "(config default)" sentinel — no override.
    let profile = form
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    act(state, move |control| {
        control.start(profile.as_deref()).map_err(flash_of)
    })
    .await
}

async fn session_stop(State(state): State<Arc<AppState>>) -> Response {
    act(state, |control| control.stop().map_err(flash_of)).await
}

async fn config_reload(State(state): State<Arc<AppState>>) -> Response {
    act(state, |control| control.reload().map_err(flash_of)).await
}

// ---------------------------------------------------------------------------
// /profiles — the games.toml profiles and the presets, with the two creates
// ---------------------------------------------------------------------------

/// One fresh profiles payload. Both machine reads hit the config store, which
/// blocks; kept off the async workers like [`collect`] and [`collect_map`].
///
/// A read that REFUSES is recorded as a REFUSAL, not as an empty view.
///
/// This was the review finding, and it is this project's signature bug: on
/// `Err` the handler substituted `ProfilesView::default()`, so the page said
/// "no profiles in games.toml" at the top and buried "games.toml could not be
/// read: …" in the last card. The presets side was worse — a `PresetsView`
/// default made `noPresetsYet` true, whose copy sends the user to a template
/// form whose `<select>` is ALSO empty, so the one route offered could not
/// succeed. "I could not read this" and "there is nothing here" are different
/// sentences, and a user acts on them differently.
///
/// So the refusal lands in a typed field ([`ProfilesPayload::profiles_error`])
/// that every derived line branches on, AND in the notes, which is where a
/// warning from a read that SUCCEEDED goes.
async fn collect_profiles(state: &Arc<AppState>) -> ProfilesPayload {
    let read_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let session = read_state.control.session();
        let mut notes = Vec::new();
        let mut profiles_error = None;
        let mut presets_error = None;
        let profiles = match read_state.machine.profiles() {
            Ok(view) => {
                notes.extend(view.notes.iter().cloned());
                view
            }
            Err(refusal) => {
                notes.push(refusal.message.clone());
                if let Some(remedy) = &refusal.remedy {
                    notes.push(remedy.clone());
                }
                // The same message+remedy join a refused ACTION flashes: this
                // string replaces the list the user came for, so dropping the
                // way out would leave the one card with nowhere to send them.
                profiles_error = Some(flash_of(refusal));
                ksx_api::ProfilesView::default()
            }
        };
        let presets = match read_state.machine.presets() {
            Ok(view) => view,
            Err(refusal) => {
                notes.push(refusal.message.clone());
                if let Some(remedy) = &refusal.remedy {
                    notes.push(remedy.clone());
                }
                presets_error = Some(flash_of(refusal));
                ksx_api::PresetsView::default()
            }
        };
        // Both reads discover the config root independently, so a machine with
        // no config root refuses BOTH with the identical sentence. Printing it
        // twice reads as two problems, and the client keys its note list by the
        // line — duplicate keys are a reconcile hazard as well as a lie.
        // `Vec::dedup` would not do it: the duplicates are not adjacent (the
        // first read contributes a message AND its remedy before the second
        // read's message arrives).
        let mut seen = std::collections::BTreeSet::new();
        notes.retain(|line| seen.insert(line.clone()));
        ProfilesPayload {
            profiles,
            presets,
            session,
            profiles_error,
            presets_error,
            notes,
            flash: None,
            view: Default::default(),
        }
        .derived()
    })
    .await
    .unwrap_or_else(|_| {
        // A panicked read is a FAILED read, not an empty machine — the same
        // distinction, arriving by a different door.
        ProfilesPayload {
            session: SessionView::unreachable("profile collection panicked"),
            profiles_error: Some("profile collection panicked".to_owned()),
            presets_error: Some("profile collection panicked".to_owned()),
            notes: vec!["profile collection panicked".to_owned()],
            ..ProfilesPayload::default()
        }
        .derived()
    })
}

async fn profiles_page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let payload = collect_profiles(&state).await;
    let flash = query.flash.as_deref().filter(|f| !f.trim().is_empty());
    let out = crate::render_profiles::render_profiles(&state.profiles_page, &payload, flash);
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&out.csp)
                    .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The Profiles poller's endpoint — the same [`ProfilesPayload`] the page
/// embeds (parity unit-tested in render_profiles.rs).
async fn api_profiles(State(state): State<Arc<AppState>>) -> Response {
    let payload = collect_profiles(&state).await;
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        axum::Json(payload),
    )
        .into_response()
}

/// 303 back to /profiles, carrying the outcome as the flash. Errors flash
/// exactly like successes — the no-JS page must never fail silently.
fn profiles_redirect(outcome: Result<String, String>) -> Response {
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/profiles?flash={}", urlencode(&flash))).into_response()
}

/// Run one [`ksx_api::MachineSource`] verb off the async workers, then 303
/// back to /profiles. The [`act`] of this page.
async fn machine_act<F>(state: Arc<AppState>, verb: F) -> Response
where
    F: FnOnce(&dyn ksx_api::MachineSource) -> Result<String, String> + Send + 'static,
{
    let outcome = tokio::task::spawn_blocking(move || verb(state.machine.as_ref()))
        .await
        .unwrap_or_else(|_| Err("the machine call panicked".to_owned()));
    profiles_redirect(outcome)
}

/// Every field defaults, deliberately — including the two that are required —
/// and every field is a `String`, which is the same decision twice.
///
/// Not laxity: an extraction FAILURE is a 422 with no `Location`, and the
/// island's fetch-submit reads its outcome out of the redirect's `?flash=`
/// (`profiles.ts`). A 422 therefore arrives as `flash = null`, which the page
/// renders as nothing at all — the exact silent failure every route here is
/// written to avoid. Defaulting hands an empty spec to the planner instead,
/// which refuses in words and 303s like everything else.
///
/// The number fields were `Option<u8>` and that was HALF the fix, which is
/// worse than none because it looks finished. `#[serde(default)]` covers an
/// ABSENT key; a browser sends `slots=` — present, empty — the moment a user
/// clears a non-`required` `<input type="number">`, and serde_urlencoded
/// answers "cannot parse integer from empty string". Straight back to the 422
/// with no `Location`, and a button that does nothing at all. So the wire type
/// is a string all the way in and [`number_field`] does the parsing, where a
/// bad value is a worded refusal on a 303 like every other refusal here.
#[derive(Deserialize)]
struct NewProfileForm {
    #[serde(default)]
    title: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    arguments: String,
    /// The form's `<input type="number">`, as text. Empty falls through to the
    /// planner's own refusal rather than a silent default: "how many players"
    /// is not a question this layer may answer on the user's behalf.
    #[serde(default)]
    slots: String,
    #[serde(default)]
    preset: String,
}

/// One numeric form field, parsed HERE instead of by the extractor.
///
/// `Ok(None)` means "the user left it blank" — the caller decides whether that
/// is a default or a refusal. `Err` is the sentence to flash: a 303 with words
/// on it, never a 422 the page cannot read.
fn number_field(raw: &str, field: &str) -> Result<Option<u8>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<u8>().map(Some).map_err(|_| {
        format!("\"{raw}\" is not a number this form can use for {field} — type a whole number")
    })
}

/// POST /profiles/new — the verb that did not exist.
///
/// `ksx setup` writes INTO a profile and bails when the title is absent;
/// `ksx config import` replaces the whole file. Neither creates one, which is
/// why "I can't create a new profile" was a true statement about every surface
/// ksx had. One `MachineSource::profile_new` call, one plan, one write with a
/// timestamped backup.
async fn profiles_form_new(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewProfileForm>,
) -> Response {
    // Blank stays 0, which the planner refuses by name ("a profile hands out
    // 1..=MAX_SLOTS slots") — this layer does not pick a player count.
    let slots = match number_field(&form.slots, "slots") {
        Ok(value) => value.unwrap_or(0),
        Err(message) => return profiles_redirect(Err(message)),
    };
    machine_act(state, move |machine| {
        machine
            .profile_new(&ksx_api::NewProfile {
                title: form.title,
                path: form.path,
                arguments: form.arguments,
                slots,
                preset: form.preset,
            })
            .map_err(flash_of)
    })
    .await
}

/// Defaulted, and string-typed, for the same reasons as [`NewProfileForm`].
#[derive(Deserialize)]
struct NewPresetForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    template: String,
    #[serde(default)]
    player: String,
}

/// POST /profiles/preset/new — `ksx preset new`, through the same writer.
///
/// No `force` field, deliberately: overwriting a preset is destructive and the
/// consent shape for it is the CLI's `--force`. A web form that could clobber
/// a 25-binding mapping because a name collided is not a form this page wants.
/// That makes the refusal load-bearing — it is the ONLY thing standing between
/// "that name is taken" and a user with nowhere to go — which is why
/// [`flash_of`] carries `Refusal::remedy`, and why the Presets card no longer
/// claims that only the built-ins are protected.
async fn profiles_form_preset_new(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewPresetForm>,
) -> Response {
    // Blank means player 1 — every template has a first block, and the field
    // is labelled as the multi-player exception rather than a required answer.
    let player = match number_field(&form.player, "the player block") {
        Ok(value) => value.unwrap_or(1),
        Err(message) => return profiles_redirect(Err(message)),
    };
    machine_act(state, move |machine| {
        machine
            .preset_new(&ksx_api::NewPreset {
                name: form.name,
                template: form.template,
                player,
                force: false,
            })
            .map_err(flash_of)
    })
    .await
}

/// POST /profiles/switch — start a session under one profile.
///
/// The SAME `ControlSource::start` the status page's forms post and the tray
/// enqueues; the only difference is that this one comes back to /profiles, so
/// the user keeps their place. Exactly the shape `/map/session/stop` already
/// has. There is no second "switch profile" verb and there must not be.
async fn profiles_form_switch(
    State(state): State<Arc<AppState>>,
    Form(form): Form<StartForm>,
) -> Response {
    let profile = form
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    let outcome = tokio::task::spawn_blocking(move || {
        state.control.start(profile.as_deref()).map_err(flash_of)
    })
    .await
    .unwrap_or_else(|_| Err("the control call panicked".to_owned()));
    profiles_redirect(outcome)
}

/// One [`ksx_api::Refusal`] as the sentence a page flashes — message AND
/// remedy.
///
/// It used to drop the remedy, justified by "the page already has a place for
/// it: the no-daemon banner, which prints the exact `ksx daemon` command".
/// That is true of the CONTROL refusals this function was written for, and
/// false of every machine verb the Profiles page added, which is where review
/// caught it. `preset-exists` is the sharpest case: the message names the
/// preset it protected, and the remedy — "--force overwrites it (a timestamped
/// backup is taken first)" — is the only path forward that exists anywhere on
/// that page. Same for `unknown-template` ("`ksx preset list --templates`
/// names the ones that ship") and `NoSuchPreset` ("`ksx preset new …` makes
/// one"). A refusal with its way out deleted is just an error message.
///
/// One line still, joined with an em dash: `urlencode` caps the flash at 300
/// characters and the page renders it in a wrapping `<p>`.
fn flash_of(refusal: ksx_api::Refusal) -> String {
    match refusal.remedy {
        Some(remedy) => format!("{} — {remedy}", refusal.message),
        None => refusal.message,
    }
}

/// Run one control verb off the async workers (the pipe client blocks), then
/// 303 back to / with the outcome as the flash. Errors are flashed too —
/// never a silent failure, never an error page dead-ending the refresh loop.
async fn act<F>(state: Arc<AppState>, verb: F) -> Response
where
    F: FnOnce(&dyn ControlSource) -> Result<String, String> + Send + 'static,
{
    let outcome = tokio::task::spawn_blocking(move || verb(state.control.as_ref()))
        .await
        .unwrap_or_else(|_| Err("the control call panicked".to_owned()));
    let flash = match outcome {
        Ok(message) => message,
        Err(error) => format!("error: {error}"),
    };
    Redirect::to(&format!("/?flash={}", urlencode(&flash))).into_response()
}

/// One embedded brand icon, with a real content type.
///
/// `forma_server::assets::serve_asset` is deliberately NOT reused for these.
/// Two of its behaviours are right for hashed build output and wrong here:
///
/// - its `mime_for` knows js/css/woff2/wasm/json/html/svg and answers
///   `application/octet-stream` for everything else, so the `.ico` and the
///   `.png` would arrive as downloads rather than as icons;
/// - it stamps `Cache-Control: public, max-age=31536000, immutable`, which is
///   correct for `studio.485e0edb.css` and actively harmful for a file called
///   `favicon.ico` — regenerate the brand and every browser that ever loaded
///   the page keeps the old mark for a year.
///
/// A day is the compromise: browsers cache favicons aggressively anyway, and
/// a cabinet's Studio page is opened from a bookmark most days.
fn brand(name: &str, mime: &'static str) -> Response {
    let Some(file) = BrandAssets::get(name) else {
        // Only reachable if the crate was compiled without
        // `crates/ksx-studio/brand/`, which `brand_embed_carries_the_trio`
        // turns into a test failure rather than a 404 nobody sees.
        tracing::error!(name, "brand asset missing from the embed");
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        file.data.to_vec(),
    )
        .into_response()
}

async fn favicon_ico() -> Response {
    brand("favicon.ico", "image/x-icon")
}

async fn favicon_svg() -> Response {
    brand("favicon.svg", "image/svg+xml")
}

async fn apple_touch_icon() -> Response {
    brand("apple-touch-icon.png", "image/png")
}

/// Query-string percent-encoding (RFC 3986 unreserved set kept literal).
/// Local, tiny, and total — not worth a dependency.
fn urlencode(text: &str) -> String {
    // The flash is a one-line human sentence; cap it (on a char boundary, so
    // the encoded query decodes as valid UTF-8) so a pathological daemon
    // error cannot mint an absurd URL.
    let mut out = String::new();
    let mut utf8 = [0u8; 4];
    for c in text.chars().take(300) {
        for byte in c.encode_utf8(&mut utf8).bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char);
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ledger #13, closed upstream: ksx ships forma's CSP verbatim now.
    ///
    /// This replaces a test that pinned the OLD workaround. It asserts the
    /// three properties that made deleting it safe, against the header a real
    /// request produces — not against a hand-written string, because the point
    /// is that upstream's policy is what reaches the browser.
    #[test]
    fn the_served_csp_is_forma_own_and_permits_style_attributes() {
        let csp = forma_server::build_csp_header(&forma_server::generate_csp_nonce());

        // 1. The reason the workaround existed. `style-src-attr 'unsafe-inline'`
        //    is what lets the mapper's compiled `style:` bindings apply — the
        //    25 hit zones' geometry, and the countdown bar's width.
        assert!(
            csp.contains("style-src-attr 'unsafe-inline'"),
            "without this every inline style=\"\" is dropped and the mapper's zones \
             collapse into a pile: {csp}"
        );

        // 2. What the workaround cost, and no longer does. A nonce in style-src
        //    is what makes `<style>` blocks and stylesheets attacker-proof;
        //    relax_style_src stripped it to buy attribute styles.
        assert!(
            csp.contains("style-src 'nonce-"),
            "style-src must stay nonce-locked — attribute styles are permitted by \
             style-src-attr, not by weakening this: {csp}"
        );

        // 3. The trap that made keeping the workaround dangerous rather than
        //    merely redundant. It matched `starts_with("style-src")`, so it
        //    caught style-src-attr too — destroying the new directive AND
        //    stripping the nonce, while the page still rendered perfectly.
        assert_eq!(
            csp.split(';')
                .filter(|d| d.trim().starts_with("style-src"))
                .count(),
            2,
            "two directives share the style-src prefix; any prefix-matching \
             rewrite of this header silently mangles one of them: {csp}"
        );
        assert!(csp.contains("script-src 'nonce-"), "{csp}");
    }

    struct NullSource;
    impl StatusSource for NullSource {
        fn snapshot(&self) -> StatusSnapshot {
            StatusSnapshot::default()
        }
    }

    struct NullControl;
    impl ControlSource for NullControl {
        fn session(&self) -> SessionView {
            SessionView::unreachable("test")
        }
        fn start(&self, _profile: Option<&str>) -> Result<String, ksx_api::Refusal> {
            Err(ksx_api::Refusal::new(ksx_api::codes::REFUSED, "test"))
        }
        fn stop(&self) -> Result<String, ksx_api::Refusal> {
            Err(ksx_api::Refusal::new(ksx_api::codes::REFUSED, "test"))
        }
        fn reload(&self) -> Result<String, ksx_api::Refusal> {
            Err(ksx_api::Refusal::new(ksx_api::codes::REFUSED, "test"))
        }
    }

    /// Every method defaulted: the trait refuses in words and names the CLI
    /// verb that works, which is the honest provider for a test that never
    /// gets as far as binding.
    struct NullMachine;
    impl ksx_api::MachineSource for NullMachine {}

    /// Rule C: no code path may open a non-loopback listener. The refusal
    /// happens before any socket exists.
    #[test]
    fn serve_refuses_non_loopback_binds() {
        for addr in ["0.0.0.0:4460", "192.168.1.10:4460", "[::]:4460"] {
            let bind: SocketAddr = addr.parse().unwrap();
            let err = serve(
                bind,
                Box::new(NullSource),
                Box::new(NullControl),
                Box::new(NullMachine),
                Arc::new(ksx_api::NoLiveSource::new("no live feed in this test")),
            )
            .unwrap_err();
            assert!(
                matches!(err, StudioError::NonLoopbackBind { .. }),
                "{addr}: {err}"
            );
        }
    }

    /// The flash round-trips through a URL: encoding must cover everything a
    /// daemon error message can contain, and the length cap must hold.
    #[test]
    fn urlencode_is_query_safe_and_capped() {
        assert_eq!(
            urlencode("started (4 slot(s))"),
            "started%20%284%20slot%28s%29%29"
        );
        assert_eq!(urlencode("a&b=c?d#e"), "a%26b%3Dc%3Fd%23e");
        assert_eq!(urlencode("naïve"), "na%C3%AFve");
        assert_eq!(urlencode(&"x".repeat(1000)).len(), 300, "capped");
    }
}
