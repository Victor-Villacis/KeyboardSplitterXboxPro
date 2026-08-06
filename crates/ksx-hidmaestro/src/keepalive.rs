//! Idle dedup with a keepalive — why 16 ms, and not "some small number".
//!
//! Identical frames may be skipped: the driver *latches* state, so republishing
//! an unchanged frame at 1 kHz is pure waste. But three separate driver
//! watchdogs bound how long `SeqNo` is allowed to sit still
//! (`padforge-code-audit.md` §3.3), and each one fails differently:
//!
//! 1. **500 ms with no event signal** → the driver recycles its handles.
//! 2. **>250 event signals with no `SeqNo` advance** → same recycle, counted in
//!    wakes rather than time.
//! 3. **>500 consecutive unchanged-`SeqNo` reads** → the GIP companion tears
//!    down the mapping and **zeroes XInput state**. This is the dangerous one:
//!    the count is driven by *reader* rate (the companion's 8 ms pump plus every
//!    `XUSB_GET_STATE` a game issues), not by wall time, so a heavy consumer mix
//!    burns through 500 reads far faster than any timer would predict.
//!
//! Watchdog 3 is what makes the obvious answer wrong. PadForge first tried a
//! 250 ms keepalive and saw **one-frame releases of held buttons** under heavy
//! consumer mixes: 250 ms of silence was under the time limits but over the
//! read count, XInput state got zeroed, and a held button blinked.
//!
//! [`KEEPALIVE`] = 16 ms is derived from watchdog 3: republishing every 16 ms
//! tolerates a reader doing ~31k reads/s before it can accumulate 500 unchanged
//! reads (500 / 0.016 s), which is an order of magnitude past any real consumer
//! mix. It still elides ~94% of idle submits at a 1 kHz tick (15 of every 16).
//!
//! **Changes always submit same-tick.** The keepalive is a floor on publication
//! rate, never a ceiling on latency: nothing in this module can delay a frame
//! that differs from the last one.

use std::time::{Duration, Instant};

/// The idle republication interval. Derived above — do not "tune" it without
/// re-deriving it from the watchdogs.
pub const KEEPALIVE: Duration = Duration::from_millis(16);

/// Watchdog 1: no event signal for this long recycles driver handles.
pub const WATCHDOG_HANDLE_RECYCLE: Duration = Duration::from_millis(500);
/// Watchdog 2: event signals without a `SeqNo` advance before a recycle.
pub const WATCHDOG_STALE_WAKES: u32 = 250;
/// Watchdog 3: consecutive unchanged-`SeqNo` reads before the GIP companion
/// zeroes XInput state.
pub const WATCHDOG_UNCHANGED_READS: u32 = 500;

/// What the cadence decided for this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publish {
    /// The frame differs from the last published one — go now.
    Changed,
    /// Identical frame, but the keepalive is due.
    Keepalive,
    /// Identical frame and the keepalive is not due — skip the submit entirely.
    Skip,
}

impl Publish {
    pub fn should_publish(self) -> bool {
        !matches!(self, Publish::Skip)
    }
}

/// Per-pad idle dedup state.
///
/// Holds the last published frame bytes inline so "changed?" is a byte compare
/// with no allocation and no hashing (a hash would trade a certain answer for a
/// probabilistic one to save nothing).
pub struct Cadence<const N: usize> {
    last_frame: [u8; N],
    /// `None` until the first publish — the first frame of a pad's life is
    /// always a change, even if it happens to be all zeroes.
    last_publish: Option<Instant>,
}

impl<const N: usize> Default for Cadence<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Cadence<N> {
    pub const fn new() -> Self {
        Self {
            last_frame: [0; N],
            last_publish: None,
        }
    }

    /// Decides what to do with `frame` at `now`, **without** mutating state —
    /// so a caller that fails to publish does not poison the cadence.
    pub fn decide(&self, frame: &[u8; N], now: Instant) -> Publish {
        match self.last_publish {
            None => Publish::Changed,
            Some(last) => {
                if *frame != self.last_frame {
                    Publish::Changed
                } else if now.duration_since(last) >= KEEPALIVE {
                    Publish::Keepalive
                } else {
                    Publish::Skip
                }
            }
        }
    }

    /// Records that `frame` really went out at `now`.
    pub fn note_published(&mut self, frame: &[u8; N], now: Instant) {
        self.last_frame = *frame;
        self.last_publish = Some(now);
    }

    /// Convenience for the submit path: decide, and if it says go, record.
    pub fn take(&mut self, frame: &[u8; N], now: Instant) -> Publish {
        let decision = self.decide(frame, now);
        if decision.should_publish() {
            self.note_published(frame, now);
        }
        decision
    }

    pub fn last_publish(&self) -> Option<Instant> {
        self.last_publish
    }
}

/// The reader rate (reads/second) at which a given keepalive interval would
/// still stay under [`WATCHDOG_UNCHANGED_READS`].
///
/// Exists so the 16 ms constant can be *checked* rather than trusted.
pub fn tolerated_reads_per_second(keepalive: Duration) -> f64 {
    f64::from(WATCHDOG_UNCHANGED_READS) / keepalive.as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 8;

    #[test]
    fn a_change_publishes_on_the_same_tick_always() {
        let mut c = Cadence::<N>::new();
        let t0 = Instant::now();
        assert_eq!(c.take(&[0; N], t0), Publish::Changed, "first frame ever");
        // One microsecond later, well inside the keepalive window: a changed
        // frame still goes out immediately. The keepalive is a floor, not a
        // rate limit.
        let t1 = t0 + Duration::from_micros(1);
        assert_eq!(c.take(&[1; N], t1), Publish::Changed);
        assert_eq!(c.take(&[2; N], t1), Publish::Changed);
    }

    #[test]
    fn identical_frames_are_skipped_until_the_keepalive_is_due() {
        let mut c = Cadence::<N>::new();
        let t0 = Instant::now();
        c.take(&[5; N], t0);
        for ms in 1..16 {
            assert_eq!(
                c.take(&[5; N], t0 + Duration::from_millis(ms)),
                Publish::Skip,
                "{ms} ms"
            );
        }
        assert_eq!(c.take(&[5; N], t0 + KEEPALIVE), Publish::Keepalive);
    }

    /// The measured claim from the audit, as arithmetic: at a 1 kHz tick with a
    /// perfectly idle pad, 16 ms dedup cuts submits by ~94%.
    #[test]
    fn idle_dedup_elides_about_94_percent_of_submits_at_1khz() {
        let mut c = Cadence::<N>::new();
        let t0 = Instant::now();
        let mut published = 0;
        for tick in 0..1000u32 {
            if c.take(&[7; N], t0 + Duration::from_millis(u64::from(tick)))
                .should_publish()
            {
                published += 1;
            }
        }
        // 1 initial + one per 16 ms.
        assert_eq!(published, 1 + 1000 / 16);
        let elided = 1.0 - f64::from(published) / 1000.0;
        assert!(
            (0.93..0.95).contains(&elided),
            "expected ~94% elided, got {elided:.3}"
        );
    }

    /// Why 16 and not 250: the read-count watchdog, not a timer.
    #[test]
    fn sixteen_ms_survives_a_read_rate_that_250ms_does_not() {
        // The audit's number: 16 ms tolerates ~31k reads/s.
        let tolerated = tolerated_reads_per_second(KEEPALIVE);
        assert!(
            (31_000.0..31_500.0).contains(&tolerated),
            "expected ~31k reads/s, got {tolerated:.0}"
        );
        // The interval PadForge tried first, and the button-blink it caused: at
        // 250 ms a consumer mix doing only 2k reads/s already blows the
        // 500-unchanged-read budget and the companion zeroes XInput state.
        let quarter_second = tolerated_reads_per_second(Duration::from_millis(250));
        assert_eq!(quarter_second, 2000.0);
        assert!(tolerated > quarter_second * 15.0);
    }

    /// The keepalive must also clear the two *time*-based watchdogs with room
    /// to spare — it is derived from the strictest of the three, so this is a
    /// sanity check that the strictest really is watchdog 3.
    #[test]
    fn the_keepalive_clears_the_time_based_watchdogs_by_a_wide_margin() {
        assert!(KEEPALIVE * 30 < WATCHDOG_HANDLE_RECYCLE);
        // Watchdog 2 counts wakes: at one publish per keepalive, an idle pad
        // can never accumulate 250 signals without an advance, because every
        // signal we cause also advances SeqNo. So the binding constraint is
        // watchdog 3, which is the strictest of the three — the fact the 16 ms
        // derivation rests on.
        assert!(
            tolerated_reads_per_second(KEEPALIVE)
                > f64::from(WATCHDOG_STALE_WAKES) / KEEPALIVE.as_secs_f64(),
            "watchdog 3 must bind before watchdog 2"
        );
    }

    #[test]
    fn deciding_does_not_mutate_so_a_failed_submit_cannot_poison_the_cadence() {
        let mut c = Cadence::<N>::new();
        let t0 = Instant::now();
        c.take(&[1; N], t0);
        // Pretend the publish failed: we called decide, not take.
        let t1 = t0 + KEEPALIVE;
        assert_eq!(c.decide(&[1; N], t1), Publish::Keepalive);
        assert_eq!(c.decide(&[1; N], t1), Publish::Keepalive, "still due");
        assert_eq!(c.last_publish(), Some(t0));
        // Only note_published moves the window.
        c.note_published(&[1; N], t1);
        assert_eq!(c.decide(&[1; N], t1), Publish::Skip);
    }

    #[test]
    fn an_all_zero_first_frame_still_publishes() {
        // A neutral pad that never moves must still exist on the wire; the
        // "unchanged" comparison starts from "nothing published", not from a
        // zeroed buffer that happens to match.
        let mut c = Cadence::<N>::new();
        assert_eq!(c.take(&[0; N], Instant::now()), Publish::Changed);
    }
}
