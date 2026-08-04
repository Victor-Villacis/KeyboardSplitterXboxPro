//! Drop-based cleanup guard for the capture thread.
//!
//! The crash-safety contract (risk review §3 item 1): process **death** needs no
//! cleanup — the driver releases filters when the context handle closes. The
//! dangerous case is a *panicking thread in a living process*: the context stays
//! open with the filter set and nobody pumping `receive`, which deadens every
//! keyboard system-wide until reboot. A `Drop`-based guard runs during unwind,
//! so the filter is reset and the context destroyed no matter how the loop
//! exits. (If unwinding itself aborts the process, we're back in the safe
//! process-death case.)

/// Runs a closure exactly once on drop — including panic unwind.
pub struct ResetGuard<F: FnOnce()> {
    f: Option<F>,
}

impl<F: FnOnce()> ResetGuard<F> {
    pub fn new(f: F) -> Self {
        Self { f: Some(f) }
    }
}

impl<F: FnOnce()> Drop for ResetGuard<F> {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn runs_on_normal_exit() {
        let hits = AtomicU32::new(0);
        {
            let _g = ResetGuard::new(|| {
                hits.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runs_exactly_once_on_panic_unwind() {
        // Models the capture thread: guard armed, loop body panics, filter must
        // still be reset.
        let hits = AtomicU32::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _g = ResetGuard::new(|| {
                hits.fetch_add(1, Ordering::SeqCst);
            });
            panic!("simulated capture-loop panic");
        }));
        assert!(result.is_err());
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "guard must fire during unwind"
        );
    }
}
