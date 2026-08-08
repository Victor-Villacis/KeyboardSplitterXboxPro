//! **The live feed's channel: `\\.\pipe\ksx-live`.**
//!
//! [`crate::feed`] is the fan-out; this is the door it goes out of for a
//! surface that is not in this process. The cabinet window does not use it —
//! it runs inside the daemon and subscribes to the [`crate::feed::LiveSink`]
//! directly. Studio is a **separate process** reached by URL, so its button
//! check needs the same stream one hop further out, and this is that hop.
//!
//! ```text
//!  [engine thread] ─ LiveSink::key/::pad ─┐  bounded, lossy, try_send
//!                                         ▼
//!                                  [LiveSink fan-out]
//!                       ┌─────────────────┴──────────────────┐
//!                       ▼                                    ▼
//!            cabinet (in-process)              live pipe, one thread per
//!            LiveSubscription                  connection: subscribe, poll
//!                                              at ~60 Hz, write one JSON
//!                                              line per frame
//!                                                            │
//!                                              \\.\pipe\ksx-live (outbound)
//!                                                            ▼
//!                                              [ksx studio] → SSE → browser
//! ```
//!
//! # Why this is a second pipe and not a verb on the control pipe
//!
//! The full argument is on [`ksx_api::LiveSource`], where every client can
//! read it. The one that decided it, from this side: **the control pipe serves
//! connections sequentially, on one thread.** A connection held open to carry
//! frames would hold that thread for as long as the tab was open, and `status`
//! / `start` / `stop` would never be answered again. One browser tab would
//! take the daemon's whole control surface down with it.
//!
//! So this channel is the opposite shape on purpose: **one thread per
//! connection**, each owning its own subscription for the connection's life.
//! The two pipes also fail independently, which is the property that matters
//! at a cabinet — a wedged live feed must never stop you pressing Stop.
//!
//! # Backpressure, end to end, with exactly one counter
//!
//! Every buffer in the chain is bounded and the whole chain is *blocking*
//! after the first hop, which is what lets one number stay true:
//!
//! - engine → subscription: `try_send` on a bounded queue. Full means the
//!   event is dropped and this subscriber's `dropped` counter moves. **This is
//!   the only lossy hop, and it is the one that reports.** The engine never
//!   waits (`crate::feed`, property 2).
//! - subscription → pipe: this module's per-connection thread polls and
//!   blocks in `WriteFile`. A stalled reader parks THIS thread, whose
//!   subscription queue then fills, so the loss is counted at the hop above
//!   and reported to this same consumer in the next frame that gets through.
//! - pipe → Studio → browser: Studio's bridge blocks too, for the same reason
//!   and with the same effect.
//!
//! A tab that stops reading therefore grows no queue anywhere, slows nothing,
//! and is TOLD what it missed. That last part is the whole point: a button
//! check that silently skipped the press you just made is worse than one that
//! says it did.
//!
//! # Lifetime
//!
//! Closing the tab drops the SSE stream → Studio's bridge thread ends → the
//! pipe handle closes → this thread's `WriteFile` fails → the thread returns →
//! the `LiveSubscription` drops → `unsubscribe`. With the last viewer gone the
//! sink's gate closes and the pipeline is back to paying one relaxed atomic
//! load per event. The chain has no step that needs anyone to remember it.

use std::time::{Duration, Instant};

use ksx_api::{LiveEnvelope, LiveFeed};

/// How often a connection wakes to fold and publish — display rate.
///
/// The producer publishes every transition and this only decides how often
/// they are *folded* (`crate::feed`, "coalescing is the CONSUMER's job"), so a
/// tap shorter than a tick is invisible in `down` and unmistakable in `hit`
/// rather than lost.
const TICK: Duration = Duration::from_millis(16);

/// A frame goes out at least this often even when nothing has happened.
///
/// Not a nicety: a consumer cannot tell "quiet panel" from "wedged daemon" by
/// waiting, and its read has no deadline of its own (see
/// [`ksx_api::LiveStream`]). The heartbeat is what makes staleness *visible*
/// on the far side — a page can say "last frame 12 s ago" instead of showing a
/// grid that looks fine and is dead.
const KEEPALIVE: Duration = Duration::from_secs(2);

/// How many surfaces may watch at once.
///
/// Each viewer costs a thread and a 1024-slot queue (~16 KB). Same-user trust
/// means this is not a security boundary — it is a bound, so a page that
/// reconnects in a loop cannot accumulate threads inside the process that owns
/// the keyboard. Refused viewers are told the number, not dropped silently.
const MAX_VIEWERS: usize = 8;

/// The words a refused viewer reads. Stated with the limit in it so the answer
/// is actionable rather than mysterious.
fn too_many_viewers() -> String {
    format!(
        "the live feed already has {MAX_VIEWERS} viewers — close a Studio tab \
         or a cabinet window and reload"
    )
}

/// One connection's publisher: poll the feed, decide whether this tick has
/// anything to say, and produce the line to write.
///
/// Split out from the socket so the *policy* — what is worth a frame, when a
/// heartbeat is due, when a state change must go out at once — is testable
/// without a pipe, without Windows and without sleeping.
pub(crate) struct Pump {
    /// When the last line went out. `None` = nothing has yet, so the first
    /// tick always publishes: a page that has just connected must be told the
    /// state immediately, not after a keepalive.
    last_sent: Option<Instant>,
    /// The `running` flag as last published.
    last_running: Option<bool>,
    /// The `unavailable` sentence as last published.
    last_reason: Option<Option<String>>,
}

impl Pump {
    pub(crate) fn new() -> Self {
        Self {
            last_sent: None,
            last_running: None,
            last_reason: None,
        }
    }

    /// One tick: `Some(line)` to write (newline included), `None` to stay
    /// quiet.
    ///
    /// `now` is a parameter rather than a `Instant::now()` call so the
    /// keepalive can be tested in microseconds.
    pub(crate) fn tick(&mut self, feed: &mut dyn LiveFeed, now: Instant) -> Option<String> {
        let reason = feed.unavailable();
        let frame = feed.poll();

        let state_moved =
            self.last_running != Some(frame.running) || self.last_reason.as_ref() != Some(&reason);
        let heartbeat_due = self
            .last_sent
            .is_none_or(|sent| now.duration_since(sent) >= KEEPALIVE);

        if !has_news(&frame) && !state_moved && !heartbeat_due {
            // Nothing is lost by staying quiet: `poll` only returned an EMPTY
            // frame, and both of its reset-on-read counters (`dropped`,
            // `off_panel`) are zero — a nonzero one is news by the line above.
            return None;
        }

        self.last_running = Some(frame.running);
        self.last_reason = Some(reason.clone());
        self.last_sent = Some(now);
        let envelope = LiveEnvelope {
            frame,
            unavailable: reason,
        };
        match serde_json::to_string(&envelope) {
            Ok(mut line) => {
                line.push('\n');
                Some(line)
            }
            // Unreachable in practice — every field is a number, a bool or a
            // String — but a silently skipped frame is exactly the failure this
            // whole module exists to make impossible, so it is said out loud.
            Err(err) => {
                tracing::error!(%err, "a live frame could not be serialized; it was skipped");
                None
            }
        }
    }
}

/// Does this frame say anything at all?
///
/// The two counters are in here deliberately: `dropped` and `off_panel` are
/// reset by the read that produced them, so a frame carrying nothing but a
/// drop count is still news — skipping it would throw the count away, and the
/// count is the honest half of a lossy stream.
fn has_news(frame: &ksx_api::LiveFrame) -> bool {
    !frame.slots.is_empty()
        || !frame.keys.is_empty()
        || !frame.feedback.is_empty()
        || frame.dropped > 0
        || frame.off_panel > 0
}

/// `[[device]] id` → friendly name, for the consumer-side naming
/// (`LiveSubscription::set_aliases`).
///
/// A function rather than a table because it is read **per connection, on the
/// connection's own thread**: the engine thread that publishes key hits must
/// not read a config file, and a viewer that connects after a config edit must
/// see the new names without the daemon restarting.
pub type AliasFn = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

/// Everything a live connection needs. One struct so the server, the tests and
/// any future consumer share one wiring point — the same shape
/// [`super::pipe::PipeDeps`] has, for the same reason.
pub struct LiveDeps {
    pub feed: crate::feed::LiveSink,
    pub aliases: AliasFn,
}

// ---------------------------------------------------------------------------
// Server — Win32 named pipe, plain threads. No async runtime anywhere, exactly
// like the control pipe (E7 rule A).
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod server {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_OUTBOUND;

    use crate::daemon::pipe::server::Instance;

    /// Serve `name` until the process exits. Returns immediately.
    ///
    /// A name that cannot be owned is logged and **not fatal**: a daemon whose
    /// live feed is unavailable is still a daemon that splits a keyboard. The
    /// consequence lands where it can be seen — a surface that dials the pipe
    /// gets `no-channel` with a remedy, rather than a page that looks live and
    /// is not.
    pub fn spawn(name: String, deps: LiveDeps) {
        let result = std::thread::Builder::new()
            .name("ksx-live-pipe".into())
            .spawn(move || serve(&name, Arc::new(deps)));
        if let Err(err) = result {
            tracing::error!("could not spawn the live-feed pipe thread: {err}");
        }
    }

    fn serve(name: &str, deps: Arc<LiveDeps>) {
        let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut instance = match Instance::create_with(&wide_name, true, PIPE_ACCESS_OUTBOUND) {
            Ok(instance) => instance,
            Err(code) => {
                tracing::error!(
                    "live feed pipe {name} unavailable (WinError {code}); \
                     is another ksx daemon already running?"
                );
                return;
            }
        };
        // Threads currently serving a viewer. Not the sink's subscriber count:
        // that one also counts the in-process cabinet window, which is not a
        // pipe client and must not use up a pipe client's place.
        let viewers = Arc::new(AtomicUsize::new(0));
        tracing::info!("live feed listening on {name}");
        loop {
            if !instance.connect() {
                // A failed accept on a healthy handle is transient; recreate
                // rather than spin on it. Same policy as the control pipe.
                drop(instance);
                match Instance::create_with(&wide_name, false, PIPE_ACCESS_OUTBOUND) {
                    Ok(fresh) => instance = fresh,
                    Err(code) => {
                        tracing::error!("live feed pipe died (WinError {code})");
                        return;
                    }
                }
                continue;
            }
            // The NEXT instance exists before this connection is handed off,
            // so a viewer arriving mid-handoff queues on it instead of finding
            // no pipe at all.
            let next = Instance::create_with(&wide_name, false, PIPE_ACCESS_OUTBOUND);
            hand_off(instance, &deps, &viewers);
            match next {
                Ok(fresh) => instance = fresh,
                Err(code) => {
                    tracing::error!("live feed pipe died (WinError {code})");
                    return;
                }
            }
        }
    }

    /// Give one accepted connection its own thread — or refuse it in words.
    fn hand_off(instance: Instance, deps: &Arc<LiveDeps>, viewers: &Arc<AtomicUsize>) {
        if viewers.load(Ordering::Relaxed) >= MAX_VIEWERS {
            // Refused, not dropped: the viewer reads one envelope saying why
            // and can put that sentence on screen. A connection that simply
            // closed would look like "the daemon is gone", which is a
            // different problem with a different fix.
            let envelope = LiveEnvelope::unreachable(too_many_viewers());
            if let Ok(mut line) = serde_json::to_string(&envelope) {
                line.push('\n');
                instance.write_all(line.as_bytes());
            }
            instance.finish();
            tracing::warn!("a live-feed viewer was refused: already at {MAX_VIEWERS}");
            return;
        }
        viewers.fetch_add(1, Ordering::Relaxed);
        let deps = Arc::clone(deps);
        let seat = Arc::clone(viewers);
        let spawned = std::thread::Builder::new()
            .name("ksx-live-viewer".into())
            .spawn(move || {
                stream(instance, &deps);
                seat.fetch_sub(1, Ordering::Relaxed);
            });
        if spawned.is_err() {
            // The count has to come back or the seat is lost forever.
            viewers.fetch_sub(1, Ordering::Relaxed);
            tracing::error!("could not spawn a live-feed viewer thread");
        }
    }

    /// One viewer, start to finish. Owns the connection and the subscription;
    /// returning drops both.
    fn stream(instance: Instance, deps: &LiveDeps) {
        let mut subscription = deps.feed.subscribe();
        // Read on THIS thread, never the engine's, and per connection so a
        // viewer that opens after a config edit sees the new names.
        subscription.set_aliases((deps.aliases)());
        tracing::info!(
            subscribers = deps.feed.subscriber_count(),
            "live feed: a viewer connected"
        );
        let mut pump = Pump::new();
        loop {
            if let Some(line) = pump.tick(&mut subscription, Instant::now()) {
                // BLOCKING, on purpose. A stalled reader parks this thread,
                // its subscription queue fills, and the sink counts the loss
                // for this consumer — which is the number the next frame
                // through carries. See the module docs.
                if !instance.write_all(line.as_bytes()) {
                    break;
                }
            }
            std::thread::sleep(TICK);
        }
        instance.finish();
        // `subscription` drops here: unsubscribed, and with the last viewer
        // gone the sink's gate closes again.
        drop(subscription);
        tracing::info!(
            subscribers = deps.feed.subscriber_count(),
            "live feed: a viewer went away"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_api::{KeyHit, LiveFrame, SlotLive};

    /// A feed under test: hand it frames, it hands them out one per poll.
    struct Scripted {
        frames: std::collections::VecDeque<LiveFrame>,
        reason: Option<String>,
    }

    impl Scripted {
        fn quiet(reason: Option<&str>) -> Self {
            Self {
                frames: std::collections::VecDeque::new(),
                reason: reason.map(str::to_owned),
            }
        }

        fn push(&mut self, frame: LiveFrame) {
            self.frames.push_back(frame);
        }
    }

    impl LiveFeed for Scripted {
        fn poll(&mut self) -> LiveFrame {
            self.frames.pop_front().unwrap_or_default()
        }

        fn unavailable(&self) -> Option<String> {
            self.reason.clone()
        }
    }

    fn parse(line: &str) -> LiveEnvelope {
        assert!(line.ends_with('\n'), "one frame is one LINE: {line:?}");
        assert_eq!(line.matches('\n').count(), 1, "and only one: {line:?}");
        serde_json::from_str(line.trim()).expect("a frame the client can read")
    }

    /// **A page that has just connected is told the state at once.**
    ///
    /// Catches the version that published only when `has_news`: open the button
    /// check on an idle cabinet and it showed nothing — no "start emulation"
    /// line, no anything — until somebody pressed something, which is the one
    /// moment a page is being read to find out WHY nothing is happening.
    #[test]
    fn the_first_tick_always_publishes_even_with_nothing_to_say() {
        let mut feed = Scripted::quiet(Some("no session is running"));
        let mut pump = Pump::new();
        let line = pump
            .tick(&mut feed, Instant::now())
            .expect("the first tick always speaks");
        let envelope = parse(&line);
        assert!(!envelope.frame.running);
        assert_eq!(
            envelope.unavailable.as_deref(),
            Some("no session is running"),
            "the REASON crosses the wire, it is not re-derived from running:false"
        );
    }

    /// A quiet panel does not fill the pipe with empty frames — and does not
    /// go silent either. The heartbeat is what lets a consumer tell "nothing
    /// is being pressed" from "the daemon is wedged", since its own read has
    /// no deadline.
    #[test]
    fn a_quiet_feed_publishes_only_a_keepalive() {
        let start = Instant::now();
        let mut feed = Scripted::quiet(None);
        let mut pump = Pump::new();
        assert!(pump.tick(&mut feed, start).is_some(), "the first one");

        assert!(pump
            .tick(&mut feed, start + Duration::from_millis(16))
            .is_none());
        assert!(pump.tick(&mut feed, start + KEEPALIVE / 2).is_none());
        assert!(
            pump.tick(&mut feed, start + KEEPALIVE).is_some(),
            "the heartbeat is due"
        );
        // ...and the clock restarts from the frame that just went out.
        assert!(pump
            .tick(&mut feed, start + KEEPALIVE + Duration::from_millis(16))
            .is_none());
    }

    /// A press publishes on the tick it arrives, not on the next heartbeat.
    #[test]
    fn a_key_hit_publishes_immediately() {
        let start = Instant::now();
        let mut feed = Scripted::quiet(None);
        let mut pump = Pump::new();
        pump.tick(&mut feed, start);

        feed.push(LiveFrame {
            running: true,
            keys: vec![KeyHit {
                key: "G".to_owned(),
                device: r"HID\VID_D209".to_owned(),
                alias: "IPAC P1".to_owned(),
                down: true,
            }],
            slots: vec![SlotLive {
                slot: 1,
                hit: vec!["A".to_owned()],
                ..SlotLive::default()
            }],
            ..LiveFrame::default()
        });
        let line = pump
            .tick(&mut feed, start + Duration::from_millis(16))
            .expect("a press is news");
        let envelope = parse(&line);
        assert_eq!(envelope.frame.keys[0].key, "G");
        assert!(envelope.frame.slot(1).expect("slot 1").was_hit("A"));
    }

    /// **A frame whose only content is a drop count must still be sent.**
    ///
    /// `dropped` and `off_panel` are reset by the read that produced them, so
    /// a pump that treated "no keys, no slots" as "nothing to say" would throw
    /// the count away — and the consumer would never learn it had missed
    /// anything. That is precisely the silent loss this stream is built to
    /// avoid; against the version that skipped on `slots.is_empty() &&
    /// keys.is_empty()` alone, this fails.
    #[test]
    fn a_frame_carrying_only_a_drop_count_is_still_news() {
        let start = Instant::now();
        let mut feed = Scripted::quiet(None);
        let mut pump = Pump::new();
        pump.tick(&mut feed, start);

        feed.push(LiveFrame {
            dropped: 17,
            ..LiveFrame::default()
        });
        let line = pump
            .tick(&mut feed, start + Duration::from_millis(16))
            .expect("a loss report is news");
        assert_eq!(parse(&line).frame.dropped, 17);

        // ...and the same for keys from a board bound to no slot: "the panel is
        // dead" and "you are pressing the wrong keyboard" stay two sentences.
        feed.push(LiveFrame {
            off_panel: 3,
            ..LiveFrame::default()
        });
        let line = pump
            .tick(&mut feed, start + Duration::from_millis(32))
            .expect("an off-panel report is news");
        assert_eq!(parse(&line).frame.off_panel, 3);
    }

    /// A session starting or ending reaches a watching page on the next tick,
    /// not up to a keepalive later — and so does a change of REASON.
    #[test]
    fn a_state_change_publishes_without_waiting_for_the_heartbeat() {
        let start = Instant::now();
        let mut feed = Scripted::quiet(Some("no session is running"));
        let mut pump = Pump::new();
        pump.tick(&mut feed, start);
        assert!(pump
            .tick(&mut feed, start + Duration::from_millis(16))
            .is_none());

        // The session starts: `running` flips and the reason goes away.
        feed.reason = None;
        feed.push(LiveFrame {
            running: true,
            ..LiveFrame::default()
        });
        let line = pump
            .tick(&mut feed, start + Duration::from_millis(32))
            .expect("a session start is news");
        let envelope = parse(&line);
        assert!(envelope.frame.running);
        assert!(envelope.unavailable.is_none());

        // The session ends: the reason comes back, and it is worth a frame
        // even though the empty frame beside it is not.
        feed.reason = Some("no session is running".to_owned());
        let line = pump
            .tick(&mut feed, start + Duration::from_millis(48))
            .expect("a session end is news");
        assert_eq!(
            parse(&line).unavailable.as_deref(),
            Some("no session is running")
        );
    }

    /// The refusal a ninth viewer reads names the limit and what to do, rather
    /// than the connection simply closing — which would be indistinguishable
    /// from "the daemon is gone".
    #[test]
    fn a_refused_viewer_is_told_the_limit_and_the_way_out() {
        let words = too_many_viewers();
        assert!(words.contains(&MAX_VIEWERS.to_string()), "{words}");
        assert!(words.contains("close"), "{words}");
        let envelope = LiveEnvelope::unreachable(words);
        assert!(envelope.unavailable.is_some());
        assert!(!envelope.frame.running, "a refused viewer sees no session");
    }
}
