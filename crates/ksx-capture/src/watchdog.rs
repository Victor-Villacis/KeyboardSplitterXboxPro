//! Consumer-stall watchdog (risk review R7): if `try_send` into the bounded
//! event channel fails continuously for longer than the threshold, the capture
//! thread must flip itself to passthrough so captured keyboards reach the OS
//! instead of a black hole.
//!
//! Pure state machine with an injectable clock (`now_ms`) so the 500 ms policy
//! is unit-testable without sleeping.

/// Tracks continuous send failure. One-shot: once tripped it stays tripped —
/// recovery is a policy decision for the supervisor (M4), not the hot loop.
#[derive(Debug)]
pub struct Watchdog {
    threshold_ms: u64,
    failing_since_ms: Option<u64>,
    tripped: bool,
}

impl Watchdog {
    /// Spec'd trip threshold: consumer stalled for > 500 ms.
    pub const DEFAULT_THRESHOLD_MS: u64 = 500;

    pub fn new(threshold_ms: u64) -> Self {
        Self {
            threshold_ms,
            failing_since_ms: None,
            tripped: false,
        }
    }

    /// A send succeeded: the failure window resets (a tripped dog stays
    /// tripped).
    pub fn on_send_ok(&mut self) {
        self.failing_since_ms = None;
    }

    /// A `try_send` failed at `now_ms` (any monotonic millisecond clock).
    /// Returns `true` exactly once, at the moment the continuous-failure window
    /// exceeds the threshold — the caller must then force passthrough, flag
    /// health, and log loudly.
    #[must_use]
    pub fn on_send_failed(&mut self, now_ms: u64) -> bool {
        if self.tripped {
            return false;
        }
        match self.failing_since_ms {
            None => {
                self.failing_since_ms = Some(now_ms);
                false
            }
            Some(t0) => {
                if now_ms.saturating_sub(t0) > self.threshold_ms {
                    self.tripped = true;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn tripped(&self) -> bool {
        self.tripped
    }
}

impl Default for Watchdog {
    fn default() -> Self {
        Self::new(Self::DEFAULT_THRESHOLD_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_trip_below_threshold() {
        let mut wd = Watchdog::new(500);
        assert!(!wd.on_send_failed(0));
        assert!(!wd.on_send_failed(100));
        assert!(!wd.on_send_failed(500)); // exactly at threshold: not yet (> rule)
        assert!(!wd.tripped());
    }

    #[test]
    fn trips_once_after_continuous_failure() {
        let mut wd = Watchdog::new(500);
        assert!(!wd.on_send_failed(1_000));
        assert!(wd.on_send_failed(1_501), "must trip past the window");
        assert!(wd.tripped());
        // Never reports the trip twice.
        assert!(!wd.on_send_failed(2_000));
        assert!(wd.tripped());
    }

    #[test]
    fn success_resets_the_window() {
        let mut wd = Watchdog::new(500);
        assert!(!wd.on_send_failed(0));
        wd.on_send_ok();
        // Failure clock restarts: 501 ms after the ORIGINAL failure is fine.
        assert!(!wd.on_send_failed(501));
        assert!(!wd.tripped());
        // But another 501 ms of continuous failure trips.
        assert!(wd.on_send_failed(1_003));
    }

    #[test]
    fn tripped_stays_tripped_even_after_success() {
        let mut wd = Watchdog::new(500);
        let _ = wd.on_send_failed(0);
        assert!(wd.on_send_failed(501));
        wd.on_send_ok();
        assert!(wd.tripped(), "recovery is the supervisor's call, not ours");
    }

    #[test]
    fn re_arm_by_replacement_allows_a_second_trip() {
        // The backends' recovery pattern: when the supervisor re-enables
        // capture (SetCaptured) after a trip, the tripped dog is REPLACED with
        // a fresh one so stall protection can fire again — otherwise captured
        // keyboards would black-hole with no guard left.
        let mut wd = Watchdog::new(500);
        let _ = wd.on_send_failed(0);
        assert!(wd.on_send_failed(501));
        assert!(wd.tripped());
        wd = Watchdog::default();
        assert!(!wd.tripped());
        assert!(!wd.on_send_failed(1_000));
        assert!(wd.on_send_failed(1_501), "protection lives again");
    }

    #[test]
    fn non_monotonic_clock_does_not_underflow() {
        let mut wd = Watchdog::new(500);
        assert!(!wd.on_send_failed(1_000));
        assert!(!wd.on_send_failed(900)); // clock hiccup: saturates, no panic
        assert!(!wd.tripped());
    }
}
