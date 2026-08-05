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

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::control::{BindRequest, ControlSource, SessionView};
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
            .route("/api/preset/restore", post(api_preset_restore))
            .route("/api/preset/clear-all", post(api_preset_clear_all))
            // The mapper's own session controls (FIX 0): "Pause emulation &
            // map" and "Resume emulation" are the SAME ControlSource verbs
            // the status page's forms use — one pipe verb each, no GUI-only
            // path — served as JSON so the mapper never navigates away and
            // loses the user's place.
            .route("/api/session/stop", post(api_session_stop))
            .route("/api/session/start", post(api_session_start))
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
async fn collect_map(state: &Arc<AppState>, selected: Option<u8>) -> MapPayload {
    let map_state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let mapper = map_state.source.mapper();
        let session = map_state.control.session();
        let learn = map_state.control.learn_poll();
        let selected = selected
            .filter(|n| mapper.slots.iter().any(|s| s.number == *n))
            .or_else(|| mapper.slots.first().map(|s| s.number))
            .unwrap_or(0);
        MapPayload {
            mapper,
            session,
            learn,
            selected,
        }
    })
    .await
    .unwrap_or_else(|_| MapPayload {
        mapper: crate::snapshot::MapperSnapshot::unavailable("mapper collection panicked"),
        session: SessionView::unreachable("mapper collection panicked"),
        learn: crate::control::LearnView::unavailable("mapper collection panicked"),
        selected: 0,
    })
}

#[derive(Deserialize)]
struct MapQuery {
    slot: Option<u8>,
}

async fn map_page(State(state): State<Arc<AppState>>, Query(query): Query<MapQuery>) -> Response {
    let payload = collect_map(&state, query.slot).await;
    let out = render_map(&state.map_page, &payload);
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
    let payload = collect_map(&state, query.slot).await;
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
