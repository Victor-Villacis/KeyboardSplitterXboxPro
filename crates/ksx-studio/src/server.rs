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

use crate::control::{ControlSource, SessionView};
use crate::error::StudioError;
use crate::render::{render_status, Assets, EmbeddedPage};
use crate::snapshot::{StatusPayload, StatusSnapshot, StatusSource};

struct AppState {
    page: EmbeddedPage,
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
    let page = EmbeddedPage::load()?;
    let state = Arc::new(AppState {
        page,
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
