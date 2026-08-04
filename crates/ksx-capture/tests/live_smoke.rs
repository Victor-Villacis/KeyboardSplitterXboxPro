//! MANUAL-RUN-ONLY live observe smoke test for the Interception backend.
//!
//! Run explicitly with:
//! ```text
//! cargo test -p ksx-capture --test live_smoke -- --ignored --nocapture
//! ```
//!
//! Safety envelope (do not weaken — this machine's keyboards are production):
//! - **Passthrough only.** `SetCaptured` is never sent; every stroke is
//!   re-sent to the OS. Blocking is exercised later, supervised, with the
//!   user at the keyboard.
//! - **Hard bounded**: the observation window is 5 seconds (< the 10 s cap),
//!   after which Shutdown is sent; the loop's 50 ms wait timeout bounds exit
//!   latency.
//! - **Crash-safe**: the context's Drop guard (filter NONE before destroy) is
//!   active for the whole run; process death needs no cleanup at all.

#![cfg(windows)]

use std::time::{Duration, Instant};

use ksx_capture::{CaptureBackend, CaptureCtl, ExitReason, InterceptionBackend};

#[test]
#[ignore = "live Interception driver test — bounded 5 s, passthrough-only; run manually with --ignored"]
fn observe_smoke_bounded_passthrough() {
    let mut backend = match InterceptionBackend::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Interception unavailable ({e}); nothing to smoke-test");
            return;
        }
    };

    let devices = backend.devices();
    eprintln!("enumerated {} device(s):", devices.len());
    for d in &devices {
        eprintln!(
            "  slot {:?} {:?} {} ({})",
            d.interception_slot,
            d.kind,
            d.id,
            d.friendly.as_deref().unwrap_or("n/a")
        );
    }
    assert!(
        devices
            .iter()
            .any(|d| matches!(d.kind, ksx_capture::DeviceKind::Keyboard)),
        "a machine with a live Interception stack must show at least one keyboard"
    );

    let health = backend.health();
    let (tx, rx) = crossbeam_channel::bounded(1024);
    let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();

    let handle = Box::new(backend)
        .run(tx, ctl_rx)
        .expect("spawn capture thread");

    // Observe for 5 seconds. NO SetCaptured — passthrough start mode means
    // every keystroke typed during this window reaches the OS normally.
    eprintln!("observing for 5 s — type on any keyboard to see events…");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = 0u32;
    while Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            events += 1;
            eprintln!("  {} {:?} down={} t={}", ev.device, ev.key, ev.down, ev.t);
        }
    }

    ctl_tx.send(CaptureCtl::Shutdown).expect("send shutdown");
    let reason = handle.join().expect("capture thread must not panic");
    assert_eq!(reason, ExitReason::Shutdown);

    let snap = health.snapshot();
    eprintln!("events={events} health={snap:?}");
    assert!(
        !snap.watchdog_tripped,
        "consumer was live; no trip expected"
    );
    assert!(!snap.panicked);
}
