//! `MockCaptureBackend` — a scripted stroke source implementing
//! [`CaptureBackend`] for CLI dry-runs and M4 integration tests.
//!
//! It drives the exact same pure decision core
//! ([`crate::decision::process_keyboard_stroke`]) as the real Interception
//! backend; "re-sending to the OS" becomes appending to an inspectable log, and
//! "suppressing" is pure data. No OS interaction of any kind.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ksx_core::{DeviceId, KeyEvent};

use crate::backend::{CaptureBackend, CaptureCtl, DeviceInfo, ExitReason};
use crate::decision::{process_keyboard_stroke, CaptureSet};
use crate::health::HealthHandle;
use crate::watchdog::Watchdog;

/// One scripted keyboard stroke: raw (code, state) exactly as the driver would
/// deliver it, attributed to `devices[device]`.
#[derive(Clone, Copy, Debug)]
pub struct MockStroke {
    /// Index into the backend's device list.
    pub device: usize,
    /// Set-1 scancode.
    pub code: u16,
    /// Interception state word (see [`crate::keymap`] constants).
    pub state: u16,
}

/// A stroke the mock "re-sent to the OS", recorded verbatim for assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResentStroke {
    pub device: DeviceId,
    pub code: u16,
    pub state: u16,
}

/// Scripted [`CaptureBackend`].
pub struct MockCaptureBackend {
    devices: Vec<DeviceInfo>,
    script: Vec<MockStroke>,
    pace: Option<Duration>,
    health: HealthHandle,
    resent: Arc<Mutex<Vec<ResentStroke>>>,
}

impl MockCaptureBackend {
    /// Panics if any scripted stroke references a device index out of range —
    /// a broken script is a test bug, fail fast.
    pub fn new(devices: Vec<DeviceInfo>, script: Vec<MockStroke>) -> Self {
        for (i, s) in script.iter().enumerate() {
            assert!(
                s.device < devices.len(),
                "script stroke {i} references device {} but only {} devices exist",
                s.device,
                devices.len()
            );
        }
        Self {
            devices,
            script,
            pace: None,
            health: HealthHandle::new(),
            resent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Sleep this long before each scripted stroke (lets tests interleave ctl
    /// messages deterministically enough).
    pub fn with_pace(mut self, pace: Duration) -> Self {
        self.pace = Some(pace);
        self
    }

    /// Handle to the "re-sent to OS" log; clone it before `run`.
    pub fn resent_log(&self) -> Arc<Mutex<Vec<ResentStroke>>> {
        Arc::clone(&self.resent)
    }
}

impl CaptureBackend for MockCaptureBackend {
    fn devices(&mut self) -> Vec<DeviceInfo> {
        self.devices.clone()
    }

    fn health(&self) -> HealthHandle {
        self.health.clone()
    }

    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        let MockCaptureBackend {
            devices,
            script,
            pace,
            health,
            resent,
        } = *self;

        std::thread::Builder::new()
            .name("ksx-capture-mock".into())
            .spawn(move || {
                let start = std::time::Instant::now();
                let mut set = CaptureSet::passthrough();
                let mut wd = Watchdog::default();

                for (t, stroke) in script.iter().enumerate() {
                    if let Some(p) = pace {
                        std::thread::sleep(p);
                    }

                    // Apply pending control right before each stroke (after the
                    // pacing sleep, so a paced test can interleave ctl messages
                    // deterministically).
                    loop {
                        match ctl.try_recv() {
                            Ok(CaptureCtl::SetCaptured(ids)) => {
                                set = CaptureSet::capturing(ids);
                                // Mirror the real backend: re-enabling capture
                                // re-arms a tripped watchdog so the stall
                                // protection can fire again (health flag stays
                                // latched).
                                if wd.tripped() {
                                    wd = Watchdog::default();
                                }
                            }
                            Ok(CaptureCtl::SetPassthrough) => set.passthrough = true,
                            Ok(CaptureCtl::Shutdown) => return ExitReason::Shutdown,
                            Err(_) => break,
                        }
                    }

                    let device = &devices[stroke.device].id;
                    let out = process_keyboard_stroke(
                        set.passthrough,
                        set.is_captured(device),
                        device,
                        stroke.code,
                        stroke.state,
                        t as u64,
                    );

                    if out.resend {
                        resent
                            .lock()
                            .expect("resent log poisoned")
                            .push(ResentStroke {
                                device: device.clone(),
                                code: stroke.code,
                                state: stroke.state,
                            });
                    }

                    match tx.try_send(out.event) {
                        Ok(()) => wd.on_send_ok(),
                        Err(TrySendError::Full(_)) => {
                            health.add_dropped(1);
                            let now_ms = start.elapsed().as_millis() as u64;
                            if wd.on_send_failed(now_ms) {
                                tracing::error!(
                                    "mock capture: consumer stalled — forcing passthrough"
                                );
                                health.set_watchdog_tripped();
                                set.passthrough = true;
                            }
                        }
                        Err(TrySendError::Disconnected(_)) => return ExitReason::ChannelClosed,
                    }
                }

                // Script done: stay controllable until told to stop (mirrors a
                // real backend that idles when no keys are pressed).
                loop {
                    match ctl.recv() {
                        Ok(CaptureCtl::Shutdown) => return ExitReason::Shutdown,
                        Ok(_) => {}
                        Err(_) => return ExitReason::ScriptExhausted,
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DeviceKind;
    use crate::keymap::{KEY_DOWN, KEY_E0, KEY_UP};
    use ksx_core::Key;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const LOGI: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";

    fn two_keyboards() -> Vec<DeviceInfo> {
        vec![
            DeviceInfo {
                id: DeviceId::from(IPAC),
                interception_slot: Some(1),
                friendly: Some("I-PAC Arcade Control Interface".into()),
                kind: DeviceKind::Keyboard,
            },
            DeviceInfo {
                id: DeviceId::from(LOGI),
                interception_slot: Some(2),
                friendly: None,
                kind: DeviceKind::Keyboard,
            },
        ]
    }

    #[test]
    fn passthrough_default_reports_and_resends_everything() {
        let script = vec![
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_DOWN,
            },
            MockStroke {
                device: 1,
                code: 75,
                state: KEY_E0 | KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_UP,
            },
        ];
        let backend = MockCaptureBackend::new(two_keyboards(), script);
        let resent = backend.resent_log();
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();

        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        let events: Vec<KeyEvent> = rx.iter().take(3).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].key, Key::A);
        assert!(events[0].down);
        assert_eq!(events[1].key, Key::Left); // E0 correction applied
        assert_eq!(events[1].device, DeviceId::from(LOGI));
        assert!(!events[2].down);
        assert_eq!(resent.lock().unwrap().len(), 3, "everything re-sent");
    }

    #[test]
    fn captured_device_swallowed_others_pass() {
        let script = vec![
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_DOWN,
            },
            MockStroke {
                device: 1,
                code: 31,
                state: KEY_DOWN,
            },
        ];
        let backend = MockCaptureBackend::new(two_keyboards(), script);
        let resent = backend.resent_log();
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();

        // Capture the I-PAC before the script starts.
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .unwrap();
        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();

        let events: Vec<KeyEvent> = rx.iter().take(2).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        // Both reported (engine sees everything)...
        assert_eq!(events.len(), 2);
        // ...but only the non-captured device's stroke was re-sent.
        let log = resent.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].device, DeviceId::from(LOGI));
        assert_eq!((log[0].code, log[0].state), (31, KEY_DOWN));
    }

    #[test]
    fn set_passthrough_releases_a_captured_device() {
        let script = vec![
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_UP,
            },
        ];
        let backend = MockCaptureBackend::new(two_keyboards(), script);
        let resent = backend.resent_log();
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();

        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .unwrap();
        let backend = backend.with_pace(Duration::from_millis(100));
        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();

        // First stroke swallowed; then flip to passthrough mid-script.
        let first = rx.recv().unwrap();
        assert!(first.down);
        ctl_tx.send(CaptureCtl::SetPassthrough).unwrap();
        let second = rx.recv().unwrap();
        assert!(!second.down);
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        let log = resent.lock().unwrap();
        // The release (post-SetPassthrough) must have been re-sent. The press
        // may or may not have been, depending on ctl arrival timing vs pacing;
        // the release is the deterministic assertion.
        assert!(log
            .iter()
            .any(|r| r.state == KEY_UP && r.device == DeviceId::from(IPAC)));
    }

    #[test]
    fn full_channel_counts_drops_and_trips_watchdog() {
        // Channel of capacity 1 that nobody drains: first stroke fits, the rest
        // fail continuously; with pacing, >500 ms of failure trips the dog.
        let script: Vec<MockStroke> = (0..40)
            .map(|_| MockStroke {
                device: 0,
                code: 30,
                state: KEY_DOWN,
            })
            .collect();
        let backend =
            MockCaptureBackend::new(two_keyboards(), script).with_pace(Duration::from_millis(20));
        let health = backend.health();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();

        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        // Hold rx alive but never read: consumer stall. Wait for the trip
        // (script runs ~800 ms of continuous failure; threshold is 500 ms).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !health.snapshot().watchdog_tripped && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        let reason = handle.join().unwrap();
        assert_eq!(reason, ExitReason::Shutdown);

        let snap = health.snapshot();
        assert!(snap.dropped_events > 0, "drops must be counted");
        assert!(snap.watchdog_tripped, "stalled consumer must trip watchdog");
        drop(rx);
    }

    #[test]
    fn dropped_receiver_ends_the_thread() {
        let script = vec![MockStroke {
            device: 0,
            code: 30,
            state: KEY_DOWN,
        }];
        let backend = MockCaptureBackend::new(two_keyboards(), script);
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (_ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<CaptureCtl>();

        // Receiver is gone before the thread even starts: the first try_send
        // observes Disconnected deterministically.
        drop(rx);
        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::ChannelClosed);
    }

    #[test]
    fn script_exhaustion_after_ctl_disconnect() {
        let backend = MockCaptureBackend::new(two_keyboards(), vec![]);
        let (tx, _rx) = crossbeam_channel::bounded(4);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<CaptureCtl>();
        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        drop(ctl_tx);
        assert_eq!(handle.join().unwrap(), ExitReason::ScriptExhausted);
    }
}
