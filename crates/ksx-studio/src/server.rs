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
//! v9 gives the MAPPER the same baseline: `/map/*` are form-encoded twins of
//! the `/api/*` mapper verbs (bind, clear, restore, clear-all, pause), each
//! calling the identical [`ControlSource`] method and 303-ing back to
//! `/map?slot=N&flash=…`. With JavaScript the island intercepts the submit
//! and reports through its toast stack instead; with JavaScript off the page
//! is still fully operable, which is the whole point of the shape.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::control::{BindOutcome, BindRequest, ControlSource, SessionView};
use crate::error::StudioError;
use crate::render::{render_status, Assets, EmbeddedPage};
use crate::render_map::render_map;
use crate::snapshot::{MapPayload, StatusPayload, StatusSnapshot, StatusSource};

struct AppState {
    page: EmbeddedPage,
    map_page: EmbeddedPage,
    source: Box<dyn StatusSource>,
    control: Box<dyn ControlSource>,
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
) -> Result<(), StudioError> {
    if !bind.ip().is_loopback() {
        return Err(StudioError::NonLoopbackBind { bind });
    }
    let page = EmbeddedPage::load("/")?;
    let mapper = EmbeddedPage::load("/map")?;
    let state = Arc::new(AppState {
        page,
        map_page: mapper,
        source,
        control,
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
            .route("/map/preset/restore", post(map_form_restore))
            .route("/map/preset/clear-all", post(map_form_clear_all))
            .route("/map/session/stop", post(map_form_session_stop))
            // Canon helper: correct no-cache + Service-Worker-Allowed
            // headers for free (replaced a hand-rolled handler).
            .route("/sw.js", get(forma_server::sw::serve_sw::<Assets>))
            .route(
                "/_assets/{filename}",
                get(forma_server::assets::serve_asset::<Assets>),
            )
            .with_state(state);
        tracing::info!(%bind, "ksx Studio listening");
        axum::serve(listener, app).await.map_err(StudioError::Serve)
    })
}

#[derive(Deserialize)]
struct PageQuery {
    flash: Option<String>,
}

/// Dogfood ledger #13: forma-server's CSP nonce-locks `style-src`, and per
/// the CSP spec a nonce (or hash) in that directive makes browsers drop
/// `'unsafe-inline'` semantics — every inline `style=""` ATTRIBUTE is then
/// ignored. But forma's own compiled bindings EMIT style attributes (the
/// mapper's zone geometry `style:` items, the countdown bar's `width:`),
/// so under the stock CSP all 25 hit zones collapse into a 2 px pile at the
/// stage's top-left corner. Until upstream grows an attribute story, rewrite
/// the directive to `style-src 'self' 'unsafe-inline'`: scripts stay
/// nonce-locked (the actual XSS boundary); inline style on a localhost-only,
/// fully-escaped page is the accepted trade-off. The personality `<style>`
/// keeps working — 'unsafe-inline' covers elements once the nonce is gone.
fn relax_style_src(csp: &str) -> String {
    csp.split(';')
        .map(str::trim)
        .map(|directive| {
            if directive.starts_with("style-src") {
                "style-src 'self' 'unsafe-inline'"
            } else {
                directive
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

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
                HeaderValue::from_str(&relax_style_src(&out.csp))
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
/// always null here — a poll is not an action. Loopback bind + no CORS
/// headers keep it same-origin; the page's `connect-src 'self'` is what
/// allows the fetch.
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
                HeaderValue::from_str(&relax_style_src(&out.csp))
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
    if !crate::control::RESTORE_MODES.contains(&request.mode.as_str()) {
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
    }
    control_json(state, move |control| {
        match control.restore(&request.preset, &request.mode) {
            Ok(message) => serde_json::json!({ "ok": true, "message": message }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
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
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
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
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
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
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
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
        let outcome = control.bind_keys(&slot.preset, &function, &next, force, true);
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
        let outcome = control.bind_keys(&slot.preset, &function, &next, false, true);
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

/// POST /map/preset/restore — the form twin of `/api/preset/restore`, same
/// three destinations, same validation before any daemon round trip.
async fn map_form_restore(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapRestoreForm>,
) -> Response {
    let mode = form.mode.trim().to_owned();
    if !crate::control::RESTORE_MODES.contains(&mode.as_str()) {
        return map_redirect(
            form.slot.unwrap_or(0),
            Err(format!(
                "unknown restore mode \"{mode}\" ({})",
                crate::control::RESTORE_MODES.join(" | ")
            )),
        );
    }
    map_act(state, form.slot, move |control, preset| {
        control.restore(preset, &mode)
    })
    .await
}

/// POST /map/preset/clear-all — the form twin of `/api/preset/clear-all`.
async fn map_form_clear_all(
    State(state): State<Arc<AppState>>,
    Form(form): Form<MapSlotForm>,
) -> Response {
    map_act(state, form.slot, move |control, preset| {
        control.clear_all(preset)
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
    let outcome = tokio::task::spawn_blocking(move || state.control.stop())
        .await
        .unwrap_or_else(|_| Err("the control call panicked".to_owned()));
    map_redirect(slot, outcome)
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
    act(state, move |control| control.start(profile.as_deref())).await
}

async fn session_stop(State(state): State<Arc<AppState>>) -> Response {
    act(state, |control| control.stop()).await
}

async fn config_reload(State(state): State<Arc<AppState>>) -> Response {
    act(state, |control| control.reload()).await
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

    /// Ledger #13: the nonce must LEAVE style-src (its presence makes
    /// browsers ignore 'unsafe-inline', killing every style="" attribute the
    /// compiled bindings emit), while script-src keeps its nonce untouched.
    #[test]
    fn relax_style_src_swaps_the_nonce_for_unsafe_inline_styles_only() {
        let stock = "default-src 'none'; script-src 'nonce-abc123' 'self'; \
                     style-src 'nonce-abc123' 'self'; connect-src 'self'";
        let relaxed = relax_style_src(stock);
        assert!(
            relaxed.contains("style-src 'self' 'unsafe-inline'"),
            "{relaxed}"
        );
        assert!(
            !relaxed.contains("style-src 'nonce-"),
            "a style-src nonce would disable 'unsafe-inline': {relaxed}"
        );
        assert!(
            relaxed.contains("script-src 'nonce-abc123' 'self'"),
            "scripts must stay nonce-locked: {relaxed}"
        );
        assert!(relaxed.contains("default-src 'none'"), "{relaxed}");
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
        fn start(&self, _profile: Option<&str>) -> Result<String, String> {
            Err("test".into())
        }
        fn stop(&self) -> Result<String, String> {
            Err("test".into())
        }
        fn reload(&self) -> Result<String, String> {
            Err("test".into())
        }
    }

    /// Rule C: no code path may open a non-loopback listener. The refusal
    /// happens before any socket exists.
    #[test]
    fn serve_refuses_non_loopback_binds() {
        for addr in ["0.0.0.0:4460", "192.168.1.10:4460", "[::]:4460"] {
            let bind: SocketAddr = addr.parse().unwrap();
            let err = serve(bind, Box::new(NullSource), Box::new(NullControl)).unwrap_err();
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
