//! Lock-free health flags a capture thread publishes and the app/CLI reads.
//!
//! The handle is grabbed from the backend *before* `run` consumes it and stays
//! valid for the life of the process — atomics only, so the capture hot path
//! can set flags without locks.
//!
//! # Latches, and why a reader needs a baseline
//!
//! Every flag here is **write-only**: a backend sets it, nothing ever clears it.
//! That is right for the state it describes (a stall that happened, happened)
//! and it was harmless while a [`HealthHandle`] lived exactly as long as one
//! session — the M3–M5 shape, where `supervise()` built the backend, read its
//! flags, and dropped it.
//!
//! M6 broke that assumption. The daemon claims its WinUSB panel **once**, for
//! its whole lifetime, so the health state behind it now outlives every session
//! that borrows it ([`crate::backend::Handles`], `ksx-app`'s `daemon::panel`).
//! A reader that treats these latches as "what happened to me" therefore reads
//! somebody else's history: one watchdog trip in the first game would end every
//! later game before it translated a keystroke, blaming a stall that was already
//! over.
//!
//! [`HealthView`] is the fix, and it is a *reader*-side one — the flags stay
//! exactly as latched and as cheap as they were, and the panel keeps its one
//! shared health state. A session takes a baseline at its start and reports
//! [`CaptureHealth::since`] it. See that method for which values are
//! session-scoped and which are cumulative on purpose.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
struct Inner {
    reboot_required: AtomicBool,
    watchdog_tripped: AtomicBool,
    dropped_events: AtomicU64,
    panicked: AtomicBool,
    inject_failures: AtomicU64,
}

/// Cloneable, thread-safe view of a capture backend's health.
#[derive(Clone, Debug, Default)]
pub struct HealthHandle(Arc<Inner>);

/// Point-in-time snapshot of [`HealthHandle`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureHealth {
    /// Interception slot exhaustion detected (keyboard slot climbed or left the
    /// 1..=10 budget): the affected keyboard is dead to the driver until the
    /// machine reboots. Surfaced loudly — the legacy app went silently deaf.
    pub reboot_required: bool,
    /// The consumer stalled long enough that the backend force-flipped itself
    /// to passthrough so keystrokes reach the OS instead of a black hole.
    pub watchdog_tripped: bool,
    /// Events dropped because the bounded channel was full at `try_send`.
    pub dropped_events: u64,
    /// The capture loop panicked (caught; filter was reset by the drop guard).
    pub panicked: bool,
    /// Synthetic key events the OS refused, on a backend whose passthrough is
    /// injection rather than a re-send (M6 WinUSB — see
    /// [`crate::inject`]). Nonzero means keystrokes that were supposed to
    /// reach Windows did not: a claimed board going quiet has a cause, and
    /// this is it.
    pub inject_failures: u64,
}

impl CaptureHealth {
    /// This snapshot as seen by a reader that started at `baseline`.
    ///
    /// The split is deliberate, and it is a statement about what each value
    /// *describes* rather than about convenience:
    ///
    /// | value | scope | why |
    /// |---|---|---|
    /// | `watchdog_tripped` | **session** | a stall is an event in one pipeline. The consumer that stalled is gone with the session it belonged to; the next one deserves to be judged on its own. |
    /// | `panicked` | **session** | same, and the drop guard already reset the filter when it happened. |
    /// | `dropped_events` | **session** | "the engine could not keep up" is a fact about this run's engine. |
    /// | `inject_failures` | **session** | ditto for this run's injection. |
    /// | `reboot_required` | **cumulative** | Interception slot exhaustion is a fact about the **machine**, not about a session: the affected board stays dead to the driver until Windows restarts. A tray tooltip that forgot it after one Stop would be lying, so this one is carried forward on purpose. |
    ///
    /// Counters saturate rather than wrap: a baseline can never be above a
    /// later snapshot in practice (the counters only grow), and a reader that
    /// somehow got one must report zero, not `u64::MAX`.
    pub fn since(&self, baseline: &CaptureHealth) -> CaptureHealth {
        CaptureHealth {
            // Cumulative by design — see the table above.
            reboot_required: self.reboot_required,
            watchdog_tripped: self.watchdog_tripped && !baseline.watchdog_tripped,
            dropped_events: self.dropped_events.saturating_sub(baseline.dropped_events),
            panicked: self.panicked && !baseline.panicked,
            inject_failures: self
                .inject_failures
                .saturating_sub(baseline.inject_failures),
        }
    }
}

/// A session-scoped reader over health state that may outlive the session.
///
/// Construct one at session start and read [`HealthView::snapshot`] instead of
/// [`HealthHandle::snapshot`]: everything already latched belongs to whatever
/// came before, and only what happens from here on is this session's. When the
/// handle really is fresh (`ksx run`, which builds its own backend per run) the
/// baseline is all-zero and the view is indistinguishable from the handle.
///
/// [`HealthView::cumulative`] is still there for the readers that legitimately
/// want the whole history — `ksx monitor`, and anything reporting the state of
/// the machine rather than of a session.
#[derive(Clone, Debug)]
pub struct HealthView {
    handle: HealthHandle,
    baseline: CaptureHealth,
}

impl HealthView {
    /// Take the baseline **now**. Call this before the session's capture thread
    /// can publish anything, so nothing of its own is swallowed by the
    /// baseline.
    pub fn new(handle: HealthHandle) -> Self {
        let baseline = handle.snapshot();
        Self { handle, baseline }
    }

    /// What has happened since this view was created (plus the values that are
    /// cumulative on purpose — see [`CaptureHealth::since`]).
    pub fn snapshot(&self) -> CaptureHealth {
        self.handle.snapshot().since(&self.baseline)
    }

    /// The unfiltered state of the underlying handle, history and all.
    pub fn cumulative(&self) -> CaptureHealth {
        self.handle.snapshot()
    }

    /// What was already latched when this view was taken.
    pub fn baseline(&self) -> CaptureHealth {
        self.baseline
    }
}

impl HealthHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> CaptureHealth {
        CaptureHealth {
            reboot_required: self.0.reboot_required.load(Ordering::Relaxed),
            watchdog_tripped: self.0.watchdog_tripped.load(Ordering::Relaxed),
            dropped_events: self.0.dropped_events.load(Ordering::Relaxed),
            panicked: self.0.panicked.load(Ordering::Relaxed),
            inject_failures: self.0.inject_failures.load(Ordering::Relaxed),
        }
    }

    // The setters are public because a `CaptureBackend` may live in another
    // crate (the M6 WinUSB backend) or in a test harness, and a backend that
    // cannot publish its own health is a backend whose stalls are invisible.
    // Nothing outside a backend should ever call them.

    /// Backend-facing: slot exhaustion detected (reboot required).
    pub fn set_reboot_required(&self) {
        self.0.reboot_required.store(true, Ordering::Relaxed);
    }

    /// Backend-facing: the stall watchdog fired and passthrough was forced.
    pub fn set_watchdog_tripped(&self) {
        self.0.watchdog_tripped.store(true, Ordering::Relaxed);
    }

    /// Backend-facing: `n` events were dropped by a full event channel.
    pub fn add_dropped(&self, n: u64) {
        self.0.dropped_events.fetch_add(n, Ordering::Relaxed);
    }

    /// Backend-facing: the capture loop panicked (filters already reset).
    pub fn set_panicked(&self) {
        self.0.panicked.store(true, Ordering::Relaxed);
    }

    /// Backend-facing: `n` synthetic key events were refused by the OS.
    pub fn add_inject_failures(&self, n: u64) {
        self.0.inject_failures.fetch_add(n, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_flags_across_clones() {
        let h = HealthHandle::new();
        let observer = h.clone();
        assert_eq!(observer.snapshot(), CaptureHealth::default());
        h.set_reboot_required();
        h.add_dropped(3);
        h.add_dropped(2);
        h.set_watchdog_tripped();
        h.set_panicked();
        h.add_inject_failures(4);
        let snap = observer.snapshot();
        assert!(snap.reboot_required);
        assert!(snap.watchdog_tripped);
        assert_eq!(snap.dropped_events, 5);
        assert!(snap.panicked);
        assert_eq!(snap.inject_failures, 4);
    }

    /// The R1 regression, at the level it is caused: a handle that outlives a
    /// session hands the next one a fatal flag it did not earn.
    #[test]
    fn a_view_taken_after_a_trip_reports_a_healthy_session() {
        let h = HealthHandle::new();
        // Session 1 stalls and panics on the way out.
        h.set_watchdog_tripped();
        h.set_panicked();
        h.add_dropped(9);
        h.add_inject_failures(3);

        // Session 2 starts on the SAME handle — the daemon still holds its claim.
        let session2 = HealthView::new(h.clone());
        let snap = session2.snapshot();
        assert!(
            !snap.watchdog_tripped,
            "session 1's stall must not end session 2 before it translates a key"
        );
        assert!(!snap.panicked, "session 1's panic is not session 2's");
        assert_eq!(snap.dropped_events, 0, "session 2 dropped nothing yet");
        assert_eq!(snap.inject_failures, 0);

        // ...and session 2's own trouble is still fully visible.
        h.add_dropped(2);
        h.add_inject_failures(1);
        let snap = session2.snapshot();
        assert_eq!(snap.dropped_events, 2);
        assert_eq!(snap.inject_failures, 1);
        assert_eq!(
            session2.cumulative().dropped_events,
            11,
            "the handle itself keeps the whole history"
        );
    }

    /// Slot exhaustion is a fact about the machine, not about a session: it
    /// stays true across the baseline on purpose, because the board really is
    /// dead to the driver until Windows restarts.
    #[test]
    fn reboot_required_is_cumulative_across_sessions_by_design() {
        let h = HealthHandle::new();
        h.set_reboot_required();
        let session2 = HealthView::new(h.clone());
        assert!(
            session2.snapshot().reboot_required,
            "a tray that forgets REBOOT REQUIRED after one Stop is lying"
        );
        assert!(session2.baseline().reboot_required);
    }

    /// A fresh handle (every `ksx run`) must behave exactly as it did before
    /// views existed.
    #[test]
    fn a_view_over_a_fresh_handle_is_the_handle() {
        let h = HealthHandle::new();
        let view = HealthView::new(h.clone());
        assert_eq!(view.snapshot(), CaptureHealth::default());
        h.set_watchdog_tripped();
        h.add_dropped(4);
        assert_eq!(view.snapshot(), h.snapshot());
    }
}
