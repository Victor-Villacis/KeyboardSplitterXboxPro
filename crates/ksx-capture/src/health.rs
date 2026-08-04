//! Lock-free health flags a capture thread publishes and the app/CLI reads.
//!
//! The handle is grabbed from the backend *before* `run` consumes it and stays
//! valid for the life of the process — atomics only, so the capture hot path
//! can set flags without locks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
struct Inner {
    reboot_required: AtomicBool,
    watchdog_tripped: AtomicBool,
    dropped_events: AtomicU64,
    panicked: AtomicBool,
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
        let snap = observer.snapshot();
        assert!(snap.reboot_required);
        assert!(snap.watchdog_tripped);
        assert_eq!(snap.dropped_events, 5);
        assert!(snap.panicked);
    }
}
