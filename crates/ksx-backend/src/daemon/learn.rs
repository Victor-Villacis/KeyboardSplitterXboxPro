//! The daemon's learn-key service: "press the panel key for P1 · A".
//!
//! Serves the pipe verbs `learn-key` / `learn-poll` / `learn-cancel`
//! (docs/CONTROL-SURFACE.md). The design constraint is the pipe itself: it
//! serves connections **sequentially**, and Studio polls `status` every 2 s —
//! so a learn must never park the pipe thread. `learn-key` therefore only
//! STARTS an observation (on its own short-lived thread) and returns
//! immediately; the caller polls `learn-poll` for the outcome. A second
//! `learn-key` supersedes the first (its generation goes stale and its result
//! is discarded); `learn-cancel` stops listening within one observer slice.
//!
//! The observation itself is `ksx_capture::observe_next_key` — a Raw Input
//! sink, injected via [`ObserveFn`] so the whole protocol is testable without
//! a keyboard. Constants follow PadForge's recorder (the earned numbers,
//! docs/research/padforge-code-audit.md §1.2): 10 s timeout, 33 ms observer
//! slices, wait-for-release re-baselining inside the observer. What PadForge
//! never had — a visible countdown — is why `learn-poll` reports
//! `remaining_ms` on every answer.
//!
//! **When it can hear anything at all** is gated by the caller (pipe.rs):
//! only while NO session is running — a running session's captured keyboards
//! are suppressed below win32k, where a Raw Input sink hears nothing. That
//! gate lives in the pipe handler so this service stays a dumb recorder.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// PadForge's `TimeoutSeconds = 10`, adopted verbatim.
pub const LEARN_TIMEOUT: Duration = Duration::from_secs(10);

/// The injectable observer: block up to `timeout` (honouring the cancel flag)
/// for one key press, and report `(device instance path, Key name)`.
pub type ObserveFn = Arc<
    dyn Fn(Duration, Arc<AtomicBool>) -> Result<Option<(String, String)>, String> + Send + Sync,
>;

/// Where one learn attempt stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing has ever been asked.
    Idle,
    Listening,
    Hit {
        device: String,
        key: String,
    },
    TimedOut,
    Cancelled,
    Failed(String),
}

impl Phase {
    fn word(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Hit { .. } => "hit",
            Self::TimedOut => "timeout",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }
}

struct Learn {
    generation: u64,
    phase: Phase,
    deadline: Option<Instant>,
    cancel: Arc<AtomicBool>,
}

/// One per daemon; shared with the pipe thread.
#[derive(Clone)]
pub struct LearnService {
    state: Arc<Mutex<Learn>>,
    observe: ObserveFn,
    /// Set by [`LearnService::refusing`]: this service will never listen, and
    /// says why on every verb. `None` is the ordinary recorder.
    refusal: Option<&'static str>,
}

impl LearnService {
    pub fn new(observe: ObserveFn) -> Self {
        Self {
            state: Arc::new(Mutex::new(Learn {
                generation: 0,
                phase: Phase::Idle,
                deadline: None,
                cancel: Arc::new(AtomicBool::new(false)),
            })),
            observe,
            refusal: None,
        }
    }

    /// A service that never listens and says so, in words, on every verb.
    ///
    /// For a dispatch that must not be able to learn at all — the cabinet's
    /// (`crate::cabinet::daemon_sink`), because learning a key exists to fill
    /// in a binding and that is AUTHORING. It has to refuse **synchronously**:
    /// an injected observer that returns `Err` does not, because
    /// [`Self::start`] answers "listening" first and the error only surfaces
    /// on a later poll — a lock that opens for ten seconds is not a lock.
    ///
    /// Gated on `cabinet` because that is its only caller. Left ungated it is
    /// dead code in the default and `studio` builds — a warning that only
    /// appears when `cabinet` is OFF, so neither `--all-features` clippy nor a
    /// cabinet-only build can see it. Building every feature combination is
    /// what catches this class.
    #[cfg(feature = "cabinet")]
    pub fn refusing(reason: &'static str) -> Self {
        Self {
            refusal: Some(reason),
            ..Self::new(Arc::new(|_, _| Ok(None)))
        }
    }

    /// The one answer a refusing service ever gives.
    fn refused(&self) -> Option<serde_json::Value> {
        let reason = self.refusal?;
        Some(serde_json::json!({
            "ok": false,
            "state": "unavailable",
            "generation": 0,
            "remaining_ms": serde_json::Value::Null,
            "device": serde_json::Value::Null,
            "key": serde_json::Value::Null,
            "error": reason,
        }))
    }

    /// The real observer: `ksx_capture::observe_next_key`, key reported by its
    /// preset spelling.
    #[cfg(windows)]
    pub fn with_rawinput() -> Self {
        Self::new(Arc::new(|timeout, cancel| {
            ksx_capture::observe_next_key(timeout, &cancel)
                .map(|hit| hit.map(|k| (k.instance_path, k.key.name().to_owned())))
                .map_err(|e| e.to_string())
        }))
    }

    /// Start (or supersede) a learn. Returns the poll snapshot for the fresh
    /// generation; never blocks beyond a mutex.
    pub fn start(&self) -> serde_json::Value {
        if let Some(refused) = self.refused() {
            return refused;
        }
        let (generation, cancel) = {
            let mut learn = self.state.lock().expect("learn state poisoned");
            // Supersede: the old observer stops within a slice; its result,
            // if any, is generation-stale and gets discarded.
            learn.cancel.store(true, Ordering::SeqCst);
            learn.generation += 1;
            learn.phase = Phase::Listening;
            learn.deadline = Some(Instant::now() + LEARN_TIMEOUT);
            learn.cancel = Arc::new(AtomicBool::new(false));
            (learn.generation, learn.cancel.clone())
        };

        let state = self.state.clone();
        let observe = self.observe.clone();
        let spawned = std::thread::Builder::new()
            .name("ksx-learn".into())
            .spawn(move || {
                let outcome = observe(LEARN_TIMEOUT, cancel.clone());
                let mut learn = state.lock().expect("learn state poisoned");
                if learn.generation != generation {
                    return; // superseded — a fresh learn owns the state now
                }
                learn.phase = match outcome {
                    Ok(Some((device, key))) => Phase::Hit { device, key },
                    Ok(None) if cancel.load(Ordering::SeqCst) => Phase::Cancelled,
                    Ok(None) => Phase::TimedOut,
                    Err(error) => Phase::Failed(error),
                };
            });
        if let Err(err) = spawned {
            let mut learn = self.state.lock().expect("learn state poisoned");
            learn.phase = Phase::Failed(format!("could not spawn the learn thread: {err}"));
        }
        self.poll()
    }

    /// Snapshot for `learn-poll` (and the tail of `learn-key`).
    pub fn poll(&self) -> serde_json::Value {
        if let Some(refused) = self.refused() {
            return refused;
        }
        let learn = self.state.lock().expect("learn state poisoned");
        let remaining_ms = match (&learn.phase, learn.deadline) {
            (Phase::Listening, Some(deadline)) => Some(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64,
            ),
            _ => None,
        };
        let (device, key) = match &learn.phase {
            Phase::Hit { device, key } => (Some(device.clone()), Some(key.clone())),
            _ => (None, None),
        };
        let error = match &learn.phase {
            Phase::Failed(error) => Some(error.clone()),
            _ => None,
        };
        serde_json::json!({
            "ok": true,
            "state": learn.phase.word(),
            "generation": learn.generation,
            "remaining_ms": remaining_ms,
            "device": device,
            "key": key,
            "error": error,
        })
    }

    /// `learn-cancel`: stop listening. Idempotent; a hit that already landed
    /// stays a hit (the caller has not read it yet).
    pub fn cancel(&self) -> serde_json::Value {
        if let Some(refused) = self.refused() {
            return refused;
        }
        {
            let mut learn = self.state.lock().expect("learn state poisoned");
            learn.cancel.store(true, Ordering::SeqCst);
            if learn.phase == Phase::Listening {
                learn.phase = Phase::Cancelled;
                learn.deadline = None;
            }
        }
        self.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    type Outcome = Result<Option<(String, String)>, String>;

    /// Book-keeping the supersede test synchronizes on: how many observers
    /// ever entered, and how many are live right now.
    #[derive(Default)]
    struct ObserverCount {
        entered: std::sync::atomic::AtomicUsize,
        active: std::sync::atomic::AtomicUsize,
    }

    /// A scripted observer: every observe call reads one shared channel,
    /// blocking until the test feeds an outcome (or the call's cancel flag
    /// flips, which returns `Ok(None)` exactly like the real Raw Input
    /// observer). The counters let a test wait until a superseded observer
    /// has actually exited before feeding its successor.
    fn scripted() -> (LearnService, mpsc::Sender<Outcome>, Arc<ObserverCount>) {
        let (tx, rx) = mpsc::channel::<Outcome>();
        let rx = Arc::new(Mutex::new(rx));
        let count = Arc::new(ObserverCount::default());
        let closure_count = count.clone();
        let service = LearnService::new(Arc::new(move |_timeout, cancel| {
            closure_count.entered.fetch_add(1, Ordering::SeqCst);
            closure_count.active.fetch_add(1, Ordering::SeqCst);
            let outcome = loop {
                if cancel.load(Ordering::SeqCst) {
                    break Ok(None);
                }
                if let Ok(fed) = rx.lock().unwrap().recv_timeout(Duration::from_millis(2)) {
                    break fed;
                }
            };
            closure_count.active.fetch_sub(1, Ordering::SeqCst);
            outcome
        }));
        (service, tx, count)
    }

    fn wait_for_state(service: &LearnService, want: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = service.poll();
            if snap["state"] == want {
                return snap;
            }
            assert!(Instant::now() < deadline, "never reached {want}: {snap}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn idle_until_asked() {
        let (service, _tx, _count) = scripted();
        let snap = service.poll();
        assert_eq!(snap["state"], "idle");
        assert_eq!(snap["remaining_ms"], serde_json::Value::Null);
    }

    #[test]
    fn start_listens_with_a_countdown_then_reports_the_hit() {
        let (service, tx, _count) = scripted();
        let snap = service.start();
        assert_eq!(snap["state"], "listening");
        let remaining = snap["remaining_ms"].as_u64().expect("countdown");
        assert!(remaining <= 10_000, "{snap}");
        assert!(remaining > 8_000, "fresh learn ≈ full 10 s: {snap}");

        tx.send(Ok(Some((r"HID\VID_D209\1".into(), "G".into()))))
            .unwrap();
        let snap = wait_for_state(&service, "hit");
        assert_eq!(snap["device"], r"HID\VID_D209\1");
        assert_eq!(snap["key"], "G");
    }

    #[test]
    fn a_timeout_is_reported_as_timeout() {
        let (service, tx, _count) = scripted();
        service.start();
        tx.send(Ok(None)).unwrap();
        wait_for_state(&service, "timeout");
    }

    #[test]
    fn cancel_stops_a_listening_learn() {
        let (service, _tx, _count) = scripted();
        service.start();
        let snap = service.cancel();
        assert_eq!(snap["state"], "cancelled");
        assert_eq!(snap["remaining_ms"], serde_json::Value::Null);
        // Idempotent.
        assert_eq!(service.cancel()["state"], "cancelled");
    }

    #[test]
    fn a_second_learn_supersedes_the_first() {
        let (service, tx, count) = scripted();
        let first = service.start();
        let second = service.start();
        assert!(second["generation"].as_u64() > first["generation"].as_u64());

        // start() cancelled the first observer; wait until it has actually
        // exited (its Ok(None) is generation-stale and must be discarded,
        // leaving the SECOND generation still listening).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(count.entered.load(Ordering::SeqCst) == 2
            && count.active.load(Ordering::SeqCst) == 1)
        {
            assert!(Instant::now() < deadline, "first observer never exited");
            std::thread::sleep(Duration::from_millis(2));
        }
        let snap = service.poll();
        assert_eq!(
            snap["state"], "listening",
            "stale Ok(None) surfaced: {snap}"
        );

        // Now only the second observer is reading: feed it the hit.
        tx.send(Ok(Some(("fresh-device".into(), "G".into()))))
            .unwrap();
        let snap = wait_for_state(&service, "hit");
        assert_eq!(snap["device"], "fresh-device", "{snap}");
        assert_eq!(snap["key"], "G");
        assert_eq!(snap["generation"], second["generation"]);
    }

    /// Victor's regression (2026-08-05): a SECOND learn after a completed
    /// one must listen again and land its own hit — the daemon state machine
    /// must never wedge on the first generation's terminal phase.
    #[test]
    fn a_second_learn_after_a_hit_listens_and_hits_again() {
        let (service, tx, count) = scripted();

        // First full cycle.
        service.start();
        tx.send(Ok(Some(("dev-1".into(), "G".into())))).unwrap();
        wait_for_state(&service, "hit");

        // Second cycle: fresh listening state with a fresh countdown…
        let snap = service.start();
        assert_eq!(snap["state"], "listening", "{snap}");
        assert!(snap["remaining_ms"].as_u64().unwrap() > 8_000, "{snap}");
        assert_eq!(snap["key"], serde_json::Value::Null, "stale key leaked");

        // …whose observer is really running and lands its own hit.
        let deadline = Instant::now() + Duration::from_secs(5);
        while count.active.load(Ordering::SeqCst) != 1 {
            assert!(Instant::now() < deadline, "second observer never started");
            std::thread::sleep(Duration::from_millis(2));
        }
        tx.send(Ok(Some(("dev-2".into(), "F2".into())))).unwrap();
        let snap = wait_for_state(&service, "hit");
        assert_eq!(snap["device"], "dev-2");
        assert_eq!(snap["key"], "F2");

        // And a third start still works after that.
        assert_eq!(service.start()["state"], "listening");
        service.cancel();
    }

    #[test]
    fn an_observer_error_is_a_failed_state_with_the_reason() {
        let (service, tx, _count) = scripted();
        service.start();
        tx.send(Err("raw input sink exploded".into())).unwrap();
        let snap = wait_for_state(&service, "failed");
        assert_eq!(snap["error"], "raw input sink exploded");
    }
}
