//! **Studio's half of the live input feed: the daemon's stream, as Server-Sent
//! Events.**
//!
//! One route, `GET /api/live`, and one job: take the [`ksx_api::LiveStream`]
//! this process opens on `\\.\pipe\ksx-live` and hand it to a browser without
//! ever letting the browser's pace reach the machine that owns the keyboard.
//!
//! ```text
//!  \\.\pipe\ksx-live ──▶ spawn_blocking bridge ──▶ mpsc(1) ──▶ SSE ──▶ EventSource
//!   (one JSON line              blocking_send        the        text/event-
//!    per frame)                                   backpressure   stream
//!                                                    meeting
//!                                                     point
//! ```
//!
//! # Why SSE and not a WebSocket
//!
//! This stream is **one-directional** — the browser has nothing to say back;
//! every write it wants is already a POST to a route beside this one, going
//! through the same `ControlSource` verb the CLI uses. Given that, SSE wins on
//! every axis that matters here:
//!
//! - **It is HTTP.** Same origin, same `guard.rs` layer, same CSP
//!   (`connect-src 'self'`), same 303-and-flash world as every other route. A
//!   WebSocket upgrade is a second protocol with a second set of rules to get
//!   right, and it would need its own answer to the DNS-rebinding check the
//!   guard already applies to everything.
//! - **It reconnects by itself.** `EventSource` retries on its own, with a
//!   server-settable interval — which is exactly the behaviour wanted when the
//!   daemon is restarted under a page that is left open. A WebSocket client
//!   has to be taught to do that, in TypeScript, correctly, including the
//!   backoff.
//! - **It is far less code.** No upgrade handshake, no ping/pong, no close
//!   codes, no frame masking. The whole client is `new EventSource('/api/live')`
//!   plus an `onmessage`.
//!
//! The one thing a WebSocket would buy — a browser→daemon channel — is the one
//! thing that must NOT exist here: a surface that could push into the input
//! pipeline over a socket is a second control surface, and the whole
//! architecture is that every write is a verb (docs/SURFACES.md §1).
//!
//! If a future consumer genuinely needs to talk back (the E8 light bus is a
//! candidate — a lamp driver that wants to *set* an LED), that is a verb on
//! the control pipe, not a duplex socket bolted onto this.
//!
//! # Backpressure: one bounded slot, and it blocks
//!
//! The channel between the bridge thread and the response body is **capacity
//! one, and the bridge BLOCKS on it**. That is the whole design, and every
//! alternative was worse:
//!
//! - an unbounded queue grows without limit behind a tab that stopped reading
//!   — the thing this must never do;
//! - a bounded queue that *drops* would need its own counter, and then a frame
//!   would carry two different "you missed some" numbers from two different
//!   hops, which a reader has to add up correctly to get a true answer.
//!
//! Blocking gives one counter and keeps it true. A stalled browser fills the
//! TCP window → the body stops being polled → this channel stays full → the
//! bridge parks in `blocking_send` → it stops reading the pipe → the daemon's
//! writer parks in `WriteFile` → **the daemon's subscription queue fills and
//! the sink drops, counting into that subscriber's `dropped`** — which is
//! reported to this same consumer in the next frame that gets through
//! (`ksx-app/src/daemon/live_pipe.rs`). Every buffer in the chain is bounded,
//! nothing anywhere waits on the capture thread, and the number the page shows
//! is the number that is true.
//!
//! # And when there is no daemon
//!
//! The endpoint answers, cleanly and at once, with one `unavailable` event
//! carrying the [`ksx_api::codes::NO_CHANNEL`] refusal and its remedy — never
//! a hang, and never an empty stream that looks like a quiet panel. The
//! close-freeze bug (a read with no deadline on a UI thread) is the reason
//! that sentence is worth writing down; the pipe open is budgeted, and the
//! blocking read that follows it lives on a `spawn_blocking` thread that owns
//! nothing else.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use ksx_api::{LiveEnvelope, LiveSource};
use tokio::sync::mpsc;

/// The bridge's channel depth. One, deliberately — see the module docs.
const SLOT: usize = 1;

/// How long a browser waits before reconnecting after the stream ends.
///
/// Sent as SSE's `retry:` field, so it is the SERVER's decision rather than
/// each client's. Two seconds: fast enough that starting the daemon under an
/// open page feels immediate, slow enough that a machine with no daemon at all
/// is not dialling a missing pipe sixty times a minute.
const RETRY_MS: u64 = 2_000;

/// The SSE event name a frame arrives under. Named rather than default so the
/// island can bind one listener per kind and a future kind cannot be mistaken
/// for a frame.
const EVENT_FRAME: &str = "frame";

/// The SSE event name for "there is nothing to stream, and here is why".
const EVENT_UNAVAILABLE: &str = "unavailable";

/// `GET /api/live` — the whole endpoint.
///
/// Always 200 with a `text/event-stream` body, including when there is no
/// daemon: a browser's `EventSource` treats a non-200 as a fatal error and
/// stops retrying, so refusing with a status code would mean a page that had
/// to be reloaded by hand after starting the daemon. The refusal travels as an
/// event instead, which the page can *show*, and the retry keeps running.
pub fn stream(source: Arc<dyn LiveSource>) -> Response {
    let (tx, rx) = mpsc::channel::<String>(SLOT);

    // The pipe open blocks (briefly, and it is budgeted), and every read after
    // it blocks by design. Both belong on a thread that owns nothing else.
    tokio::task::spawn_blocking(move || bridge(source.as_ref(), &tx));

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            // A proxy or a browser that buffered this would turn a live feed
            // into a batch of stale frames delivered at once — which is worse
            // than no feed, because it looks like one.
            (header::CACHE_CONTROL, "no-store"),
            (header::CONNECTION, "keep-alive"),
            // nginx and friends buffer proxied responses by default; this is
            // the conventional opt-out and costs nothing when nobody is
            // listening for it.
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        axum::body::Body::from_stream(Frames(rx)),
    )
        .into_response()
}

/// The blocking half: open the stream, forward every frame, stop when either
/// end goes away.
fn bridge(source: &dyn LiveSource, tx: &mpsc::Sender<String>) {
    let mut stream = match source.open() {
        Ok(stream) => stream,
        Err(refusal) => {
            // One event, then the stream ends and the browser retries after
            // `RETRY_MS`. The page prints this sentence — it is the daemon's
            // own words, with the remedy attached.
            let _ = tx.blocking_send(unavailable_event(&refusal));
            return;
        }
    };
    loop {
        match stream.next_frame() {
            // BLOCKING SEND — the backpressure meeting point. An `Err` means
            // the receiver is gone: the tab closed, and this thread's job is
            // over. Returning drops the pipe handle, which ends the daemon's
            // viewer thread, which drops its subscription.
            Ok(envelope) => {
                if tx.blocking_send(frame_event(&envelope)).is_err() {
                    return;
                }
            }
            // Terminal. Say why before going: "the daemon closed it" and "the
            // frame did not parse" send a reader to different places.
            Err(refusal) => {
                let _ = tx.blocking_send(unavailable_event(&refusal));
                return;
            }
        }
    }
}

/// One SSE `frame` event.
///
/// The `data:` payload is the [`LiveEnvelope`] verbatim — the same JSON the
/// daemon wrote onto the pipe, re-serialized but not reshaped. Studio adds no
/// facts to this stream and removes none: it is a pipe, not a view.
fn frame_event(envelope: &LiveEnvelope) -> String {
    // A serialize failure here is unreachable (it round-tripped on the way in),
    // but "unreachable" is not "impossible", and a silently skipped frame is
    // the exact failure the whole stream exists to prevent. Say it instead.
    match serde_json::to_string(envelope) {
        Ok(json) => sse(EVENT_FRAME, &json),
        Err(err) => sse(
            EVENT_UNAVAILABLE,
            &serde_json::json!({
                "code": ksx_api::codes::PIPE_ERROR,
                "message": format!("a live frame could not be re-serialized: {err}"),
            })
            .to_string(),
        ),
    }
}

/// One SSE `unavailable` event: the refusal, as JSON, with its remedy.
fn unavailable_event(refusal: &ksx_api::Refusal) -> String {
    let body = serde_json::to_string(refusal).unwrap_or_else(|_| {
        // `Refusal` is three strings; this cannot fail. If it somehow does,
        // the page still gets a sentence rather than a dead stream.
        format!(
            r#"{{"code":"{}","message":"the live feed refused, and the refusal could not be encoded"}}"#,
            ksx_api::codes::PIPE_ERROR
        )
    });
    sse(EVENT_UNAVAILABLE, &body)
}

/// One SSE message: `retry`, `event`, `data`, blank line.
///
/// `retry` rides on every message rather than only the first, because a client
/// that reconnects starts a NEW response and would otherwise fall back to its
/// own default interval — which browsers do not agree on.
fn sse(event: &str, data: &str) -> String {
    debug_assert!(
        !data.contains('\n'),
        "SSE data is one line per field; a payload with a newline in it silently \
         becomes two fields: {data}"
    );
    format!("retry: {RETRY_MS}\nevent: {event}\ndata: {data}\n\n")
}

/// The response body: whatever the bridge has sent, as bytes.
///
/// Hand-written rather than pulling in `tokio-stream` for one adapter —
/// `Receiver::poll_recv` is public and this is the whole of it.
struct Frames(mpsc::Receiver<String>);

impl futures_core::Stream for Frames {
    type Item = Result<String, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|next| next.map(Ok))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_api::{codes, KeyHit, LiveFrame, LiveStream, Refusal};

    /// **A frame is one SSE message, and its payload is one line.**
    ///
    /// Catches the version that wrote the envelope with `serde_json::
    /// to_string_pretty`: every newline in the payload started a fresh `data:`
    /// field, so the browser reassembled a JSON document with literal newlines
    /// in it and `JSON.parse` threw on every single frame — a live feed that
    /// delivered nothing, over a connection that looked perfectly healthy.
    #[test]
    fn one_frame_is_one_event_with_one_data_line() {
        let envelope = LiveEnvelope {
            frame: LiveFrame {
                running: true,
                keys: vec![KeyHit {
                    key: "G".to_owned(),
                    device: r"HID\VID_D209".to_owned(),
                    alias: "IPAC P1".to_owned(),
                    down: true,
                }],
                ..LiveFrame::default()
            },
            unavailable: None,
        };
        let message = frame_event(&envelope);
        assert!(message.ends_with("\n\n"), "an SSE message ends blank");
        let data: Vec<&str> = message
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .collect();
        assert_eq!(data.len(), 1, "one data line, not one per pretty-print row");
        assert_eq!(
            serde_json::from_str::<LiveEnvelope>(data[0]).unwrap(),
            envelope,
            "what the browser parses is what the daemon sent"
        );
        assert!(message.contains(&format!("event: {EVENT_FRAME}\n")));
        assert!(
            message.contains(&format!("retry: {RETRY_MS}\n")),
            "every message carries the reconnect interval: {message}"
        );
    }

    /// **No daemon is an ANSWER, not a hang and not a dead stream.**
    ///
    /// The bridge must emit the refusal — code, sentence and remedy — and then
    /// end, so the browser retries on its own. Catches the version that
    /// returned early on `open()` failure with nothing sent: the page held an
    /// open `EventSource`, showed an empty grid, and looked exactly like a
    /// working feed on a panel nobody was touching.
    #[test]
    fn no_daemon_sends_the_refusal_and_ends() {
        struct NoDaemon;
        impl LiveSource for NoDaemon {
            fn open(&self) -> Result<Box<dyn LiveStream>, Refusal> {
                Err(Refusal::with_remedy(
                    codes::NO_CHANNEL,
                    "no ksx daemon is running, so there is no live input feed",
                    "ksx daemon",
                ))
            }
        }
        let (tx, mut rx) = mpsc::channel::<String>(SLOT);
        std::thread::spawn(move || bridge(&NoDaemon, &tx))
            .join()
            .unwrap();

        let message = rx.blocking_recv().expect("the refusal was sent");
        assert!(
            message.contains(&format!("event: {EVENT_UNAVAILABLE}\n")),
            "{message}"
        );
        let data = message
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("a data line");
        let refusal: Refusal = serde_json::from_str(data).unwrap();
        assert!(refusal.is_no_channel());
        assert_eq!(refusal.remedy.as_deref(), Some("ksx daemon"));
        assert!(
            rx.blocking_recv().is_none(),
            "and then the stream ends, so the browser retries"
        );
    }

    /// The bridge forwards frames unchanged and stops the moment the reader is
    /// gone — the lifetime that makes a closed tab cost the pipeline nothing.
    ///
    /// Catches a bridge that ignored the send error and kept reading: the
    /// daemon's viewer thread and its subscription would have stayed alive for
    /// as long as the daemon did, one per tab ever opened.
    #[test]
    fn the_bridge_stops_when_the_reader_goes_away() {
        struct Endless(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl LiveStream for Endless {
            fn next_frame(&mut self) -> Result<LiveEnvelope, Refusal> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LiveEnvelope::default())
            }
        }
        struct Source(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl LiveSource for Source {
            fn open(&self) -> Result<Box<dyn LiveStream>, Refusal> {
                Ok(Box::new(Endless(self.0.clone())))
            }
        }

        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel::<String>(SLOT);
        let source = Source(reads.clone());
        let worker = std::thread::spawn(move || bridge(&source, &tx));
        drop(rx);
        worker
            .join()
            .expect("the bridge returns rather than spinning");
        // A capacity-1 channel plus one in-flight send is the most that can be
        // read before the closed receiver is noticed. The number is not the
        // point; the bound is.
        let reads = reads.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            reads <= SLOT + 2,
            "the bridge read {reads} frames after the reader left"
        );
    }
}
