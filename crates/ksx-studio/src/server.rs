//! The blocking axum server around the render seam.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::error::StudioError;
use crate::render::{render_status, Assets, EmbeddedPage};
use crate::snapshot::{StatusSnapshot, StatusSource};

struct AppState {
    page: EmbeddedPage,
    source: Box<dyn StatusSource>,
}

/// Serve the status page until the process is killed (Ctrl+C included — no
/// graceful-shutdown plumbing in the skeleton).
///
/// Blocking on purpose: the caller owns no runtime, and this function keeps
/// its own multi-threaded tokio runtime at NORMAL priority, fully isolated
/// from anything time-critical (docs/ENHANCEMENTS.md E7 "own runtime, normal
/// priority").
///
/// Refuses any non-loopback `bind` before a socket exists.
pub fn serve(bind: SocketAddr, source: Box<dyn StatusSource>) -> Result<(), StudioError> {
    if !bind.ip().is_loopback() {
        return Err(StudioError::NonLoopbackBind { bind });
    }
    let page = EmbeddedPage::load()?;
    let state = Arc::new(AppState { page, source });

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
            .route("/sw.js", get(service_worker))
            .route(
                "/_assets/{filename}",
                get(forma_server::assets::serve_asset::<Assets>),
            )
            .with_state(state);
        tracing::info!(%bind, "ksx Studio listening");
        axum::serve(listener, app).await.map_err(StudioError::Serve)
    })
}

async fn status_page(State(state): State<Arc<AppState>>) -> Response {
    // Collectors hit the registry, the SCM and schtasks.exe — blocking work,
    // kept off the async workers.
    let snap_state = Arc::clone(&state);
    let snap = tokio::task::spawn_blocking(move || snap_state.source.snapshot())
        .await
        .unwrap_or_else(|_| StatusSnapshot::degraded("status collection panicked"));

    let out = render_status(&state.page, &snap);
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
            // Belt to the body's meta-refresh braces; both are JS-free.
            (
                HeaderName::from_static("refresh"),
                HeaderValue::from_static("2"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        out.html,
    )
        .into_response()
}

/// The page's (nonce'd, CSP-clean) inline script always registers /sw.js;
/// serve the build's real service worker rather than 404 every 2 seconds.
async fn service_worker() -> Response {
    match forma_server::assets::asset_bytes::<Assets>("sw.js") {
        Some(bytes) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            )],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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

    /// Rule C: no code path may open a non-loopback listener. The refusal
    /// happens before any socket exists.
    #[test]
    fn serve_refuses_non_loopback_binds() {
        for addr in ["0.0.0.0:4460", "192.168.1.10:4460", "[::]:4460"] {
            let bind: SocketAddr = addr.parse().unwrap();
            let err = serve(bind, Box::new(NullSource)).unwrap_err();
            assert!(
                matches!(err, StudioError::NonLoopbackBind { .. }),
                "{addr}: {err}"
            );
        }
    }
}
