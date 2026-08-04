//! Capture→submit latency instrumentation.
//!
//! Every [`ksx_core::KeyEvent`] carries `t`, stamped by the capture backend the
//! moment the driver handed the stroke over (QPC ticks on Windows). The engine
//! forwards that stamp with each [`ksx_core::PadDelta`], and the output thread
//! records `now - t` into an HDR histogram right after the ViGEm submit. That
//! window is the whole pipeline: driver receive → decision → channel → engine →
//! channel → IOCTL.
//!
//! Budget: **p99 < 1 ms** (docs/ARCHITECTURE.md rule 5). The histogram is a
//! permanent debug feature — `ksx run` prints p50/p99/max at shutdown and
//! `ksx run --latency` prints a rolling summary every 5 s.
//!
//! The clock is injected so the pipeline is testable off-Windows and with mock
//! backends whose `t` is not a real clock at all.

use hdrhistogram::Histogram;

/// The p99 budget from docs/ARCHITECTURE.md rule 5, in microseconds.
pub const BUDGET_P99_US: u64 = 1_000;

/// Monotonic tick source paired with its tick rate, matching whatever unit the
/// capture backend stamps `KeyEvent::t` in.
#[derive(Clone, Copy)]
pub struct Clock {
    pub now: fn() -> u64,
    pub ticks_per_sec: u64,
}

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clock")
            .field("ticks_per_sec", &self.ticks_per_sec)
            .finish_non_exhaustive()
    }
}

impl Clock {
    /// The real clock: `QueryPerformanceCounter`, the same source
    /// `InterceptionBackend` stamps events with.
    #[cfg(windows)]
    pub fn qpc() -> Self {
        Self {
            now: qpc_now,
            ticks_per_sec: qpc_frequency(),
        }
    }

    /// Nanoseconds since first use. The fallback on non-Windows and the clock
    /// tests use; meaningless against a mock backend's synthetic `t`, which is
    /// exactly why [`LatencyRecorder::record_since`] saturates instead of
    /// panicking.
    #[cfg(any(test, not(windows)))]
    pub fn monotonic_nanos() -> Self {
        Self {
            now: monotonic_nanos,
            ticks_per_sec: 1_000_000_000,
        }
    }

    /// The clock `ksx run` uses on this platform.
    pub fn platform() -> Self {
        #[cfg(windows)]
        {
            Self::qpc()
        }
        #[cfg(not(windows))]
        {
            Self::monotonic_nanos()
        }
    }

    fn ticks_to_micros(&self, ticks: u64) -> u64 {
        if self.ticks_per_sec == 0 {
            return 0;
        }
        (u128::from(ticks) * 1_000_000 / u128::from(self.ticks_per_sec)) as u64
    }
}

#[cfg(windows)]
fn qpc_now() -> u64 {
    let mut t: i64 = 0;
    // SAFETY: out-pointer to a stack i64; QPC cannot fail on XP+.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
    t as u64
}

#[cfg(windows)]
fn qpc_frequency() -> u64 {
    let mut f: i64 = 0;
    // SAFETY: out-pointer to a stack i64; fixed at boot, cannot fail on XP+.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut f) };
    if f <= 0 {
        // Documented impossible on XP+; degrade to "latency unmeasurable"
        // rather than dividing by zero.
        1
    } else {
        f as u64
    }
}

#[cfg(any(test, not(windows)))]
fn monotonic_nanos() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// One-line latency verdict, cheap to copy and to serialize.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LatencySummary {
    pub count: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

impl LatencySummary {
    /// `true` when there is enough data AND p99 is inside the budget.
    pub fn within_budget(&self) -> bool {
        self.count > 0 && self.p99_us < BUDGET_P99_US
    }

    pub fn render(&self) -> String {
        if self.count == 0 {
            return "latency: no pad updates were submitted (nothing to measure)".to_owned();
        }
        let verdict = if self.within_budget() {
            "[OK]  "
        } else {
            "[WARN]"
        };
        format!(
            "latency capture→submit over {} update(s): p50 {} us, p99 {} us, max {} us  \
             {verdict} budget p99 < {} us",
            self.count, self.p50_us, self.p99_us, self.max_us, BUDGET_P99_US
        )
    }

    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "updates": self.count,
            "p50_us": self.p50_us,
            "p99_us": self.p99_us,
            "max_us": self.max_us,
            "budget_p99_us": BUDGET_P99_US,
            "within_budget": self.within_budget(),
        })
    }
}

/// Owns the histogram. Lives on the output thread; never shared.
pub struct LatencyRecorder {
    hist: Histogram<u64>,
    clock: Clock,
}

impl LatencyRecorder {
    pub fn new(clock: Clock) -> Self {
        // 3 significant figures: ~0.1% relative error, a few KB of counters.
        let mut hist = Histogram::<u64>::new(3).expect("3 is a valid significant-figure count");
        hist.auto(true);
        Self { hist, clock }
    }

    /// Record one capture→submit interval. Saturating: a backend whose `t` is
    /// not this clock's ticks (the mock) yields a nonsense-but-finite sample
    /// instead of an underflow panic.
    pub fn record_since(&mut self, t_capture: u64) {
        let ticks = (self.clock.now)().saturating_sub(t_capture);
        let micros = self.clock.ticks_to_micros(ticks);
        if let Err(err) = self.hist.record(micros) {
            // auto-resize is on, so this is unreachable in practice.
            tracing::debug!(%err, micros, "latency sample rejected by the histogram");
        }
    }

    pub fn summary(&self) -> LatencySummary {
        LatencySummary {
            count: self.hist.len(),
            p50_us: self.hist.value_at_quantile(0.50),
            p99_us: self.hist.value_at_quantile(0.99),
            max_us: self.hist.max(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_clock(now: u64) -> Clock {
        // A `fn` pointer cannot capture, so the "now" is baked per test via a
        // small set of concrete functions.
        let _ = now;
        Clock {
            now: || 1_000_000,
            ticks_per_sec: 1_000_000,
        }
    }

    #[test]
    fn ticks_convert_to_microseconds_without_overflow() {
        let clock = Clock {
            now: || 0,
            ticks_per_sec: 10_000_000, // 100 ns ticks, a common QPC rate
        };
        assert_eq!(clock.ticks_to_micros(0), 0);
        assert_eq!(clock.ticks_to_micros(10), 1);
        assert_eq!(clock.ticks_to_micros(10_000_000), 1_000_000);
        // A full u64 of ticks must not overflow the intermediate multiply.
        assert_eq!(clock.ticks_to_micros(u64::MAX), u64::MAX / 10);
    }

    #[test]
    fn zero_frequency_degrades_to_zero_not_divide_by_zero() {
        let clock = Clock {
            now: || 0,
            ticks_per_sec: 0,
        };
        assert_eq!(clock.ticks_to_micros(12_345), 0);
    }

    #[test]
    fn recorder_measures_microseconds_from_the_capture_stamp() {
        // now = 1_000_000 ticks at 1 MHz; a stamp 500 ticks ago is 500 us.
        let mut rec = LatencyRecorder::new(fixed_clock(1_000_000));
        rec.record_since(1_000_000 - 500);
        rec.record_since(1_000_000 - 500);
        rec.record_since(1_000_000 - 2_000);
        let s = rec.summary();
        assert_eq!(s.count, 3);
        assert_eq!(s.p50_us, 500);
        assert_eq!(s.max_us, 2_000);
    }

    #[test]
    fn a_stamp_from_the_future_saturates_instead_of_panicking() {
        // The mock capture backend stamps `t` with the script index, not a
        // clock — the pipeline must survive it.
        let mut rec = LatencyRecorder::new(fixed_clock(0));
        rec.record_since(u64::MAX);
        assert_eq!(rec.summary().count, 1);
        assert_eq!(rec.summary().max_us, 0);
    }

    #[test]
    fn empty_summary_renders_and_is_not_in_budget() {
        let s = LatencySummary::default();
        assert!(!s.within_budget());
        assert!(s.render().contains("nothing to measure"), "{}", s.render());
    }

    #[test]
    fn budget_verdict_flips_at_the_p99_threshold() {
        let under = LatencySummary {
            count: 10,
            p50_us: 100,
            p99_us: BUDGET_P99_US - 1,
            max_us: 900,
        };
        let over = LatencySummary {
            p99_us: BUDGET_P99_US,
            ..under
        };
        assert!(under.within_budget());
        assert!(under.render().contains("[OK]"), "{}", under.render());
        assert!(!over.within_budget());
        assert!(over.render().contains("[WARN]"), "{}", over.render());
        assert_eq!(
            over.to_json().pointer("/within_budget"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn platform_clock_moves_forward() {
        let clock = Clock::platform();
        assert!(clock.ticks_per_sec > 0);
        let a = (clock.now)();
        let b = (clock.now)();
        assert!(b >= a, "monotonic clock went backwards: {a} then {b}");
    }
}
