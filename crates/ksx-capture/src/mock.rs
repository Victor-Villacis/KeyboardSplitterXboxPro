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
use crate::decision::{key_event, should_resend, CaptureSet};
use crate::escape::{EscapeHandle, EscapeWatch};
use crate::health::HealthHandle;
use crate::presence::PresenceHandle;
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

/// Observer invoked when the mock *applies* a control message — the hook the
/// supervisor's teardown-ordering test needs to interleave "keyboards were
/// released" with the pad log in one timeline.
pub type CtlObserver = Box<dyn Fn(&CaptureCtl) + Send>;

/// Hook run **synchronously inside [`CaptureBackend::run`]**, before the capture
/// thread is spawned and therefore before the supervisor's first loop
/// iteration.
///
/// It exists for one thing a test otherwise cannot express: a backend whose
/// health went bad *during this session* rather than before it. Simply setting
/// a flag on the handle before calling `supervise()` no longer says that — a
/// session baselines its health at start, so a pre-set flag is somebody else's
/// history by definition ([`crate::HealthView`]). Setting it from the spawned
/// thread instead would be a race with the very first poll.
pub type StartHook = Box<dyn FnOnce(&HealthHandle) + Send>;

/// Scripted [`CaptureBackend`].
pub struct MockCaptureBackend {
    devices: Vec<DeviceInfo>,
    script: Vec<MockStroke>,
    pace: Option<Duration>,
    health: HealthHandle,
    presence: PresenceHandle,
    escapes: EscapeHandle,
    resent: Arc<Mutex<Vec<ResentStroke>>>,
    ctl_observer: Option<CtlObserver>,
    start_hook: Option<StartHook>,
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
        let presence = PresenceHandle::new();
        presence.publish(devices.iter().map(|d| d.id.clone()).collect());
        Self {
            devices,
            script,
            pace: None,
            health: HealthHandle::new(),
            presence,
            escapes: EscapeHandle::new(),
            resent: Arc::new(Mutex::new(Vec::new())),
            ctl_observer: None,
            start_hook: None,
        }
    }

    /// Sleep this long before each scripted stroke (lets tests interleave ctl
    /// messages deterministically enough).
    pub fn with_pace(mut self, pace: Duration) -> Self {
        self.pace = Some(pace);
        self
    }

    /// Call `observer` whenever the capture thread applies a [`CaptureCtl`].
    pub fn with_ctl_observer(mut self, observer: CtlObserver) -> Self {
        self.ctl_observer = Some(observer);
        self
    }

    /// Publish into pre-existing handles instead of fresh ones — how several
    /// mocks become one session under [`crate::CompositeBackend`] (shared
    /// health counters, one escape latch). Presence stays this mock's own, by
    /// the merge-not-share rule in [`crate::presence`].
    pub fn with_handles(mut self, handles: crate::backend::Handles) -> Self {
        self.health = handles.health;
        self.escapes = handles.escapes;
        self
    }

    /// Publish health from inside `run`, before the capture thread exists —
    /// the only way to script "this session's backend went bad" rather than
    /// "the previous one did". See [`StartHook`].
    pub fn with_start_hook(mut self, hook: StartHook) -> Self {
        self.start_hook = Some(hook);
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

    fn escapes(&self) -> EscapeHandle {
        self.escapes.clone()
    }

    /// Seeded with the configured device list; tests script hotplug by
    /// `publish`ing a different list on a clone of this handle.
    fn presence(&self) -> PresenceHandle {
        self.presence.clone()
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
            presence: _,
            escapes,
            resent,
            ctl_observer,
            start_hook,
        } = *self;

        // Synchronously, on the caller's thread: whatever this publishes is
        // unambiguously inside the session that is starting.
        if let Some(hook) = start_hook {
            hook(&health);
        }

        std::thread::Builder::new()
            .name("ksx-capture-mock".into())
            .spawn(move || {
                let start = std::time::Instant::now();
                let mut set = CaptureSet::passthrough();
                let mut wd = Watchdog::default();
                let mut watchdog_passthrough = false;
                let mut escapes = EscapeWatch::new(escapes);
                let observe_ctl = |message: &CaptureCtl| {
                    if let Some(observer) = &ctl_observer {
                        observer(message);
                    }
                };

                for (t, stroke) in script.iter().enumerate() {
                    if let Some(p) = pace {
                        std::thread::sleep(p);
                    }

                    // Apply pending control right before each stroke (after the
                    // pacing sleep, so a paced test can interleave ctl messages
                    // deterministically).
                    while let Ok(message) = ctl.try_recv() {
                        observe_ctl(&message);
                        match message {
                            CaptureCtl::SetCaptured(ids) => {
                                set = CaptureSet::capturing(ids);
                                watchdog_passthrough = false;
                                // Mirror the real backend: re-enabling capture
                                // re-arms a tripped watchdog so the stall
                                // protection can fire again (health flag stays
                                // latched).
                                if wd.tripped() {
                                    wd = Watchdog::default();
                                }
                            }
                            CaptureCtl::SetPassthrough => set.passthrough = true,
                            CaptureCtl::Shutdown => return ExitReason::Shutdown,
                        }
                    }

                    let device = &devices[stroke.device].id;
                    let event = key_event(device, stroke.code, stroke.state, t as u64);

                    // Escapes FIRST, exactly like the real backend: the gesture
                    // is evaluated before the pass/suppress decision and acts on
                    // this thread's own latch — no channel is involved, so a
                    // stalled consumer cannot starve it.
                    escapes.observe(&event);

                    let passthrough =
                        set.passthrough || watchdog_passthrough || escapes.passthrough();
                    if should_resend(passthrough, set.contains(device)) {
                        resent
                            .lock()
                            .expect("resent log poisoned")
                            .push(ResentStroke {
                                device: device.clone(),
                                code: stroke.code,
                                state: stroke.state,
                            });
                    }

                    match tx.try_send(event) {
                        Ok(()) => wd.on_send_ok(),
                        Err(TrySendError::Full(_)) => {
                            health.add_dropped(1);
                            let now_ms = start.elapsed().as_millis() as u64;
                            if wd.on_send_failed(now_ms) {
                                tracing::error!(
                                    "mock capture: consumer stalled — forcing passthrough"
                                );
                                health.set_watchdog_tripped();
                                watchdog_passthrough = true;
                            }
                        }
                        Err(TrySendError::Disconnected(_)) => return ExitReason::ChannelClosed,
                    }
                }

                // Script done: stay controllable until told to stop (mirrors a
                // real backend that idles when no keys are pressed).
                loop {
                    match ctl.recv() {
                        Ok(message) => {
                            observe_ctl(&message);
                            if matches!(message, CaptureCtl::Shutdown) {
                                return ExitReason::Shutdown;
                            }
                        }
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

    /// Five LeftCtrl press+release cycles on `device`.
    fn left_ctrl_x5(device: usize) -> Vec<MockStroke> {
        (0..5)
            .flat_map(|_| {
                [
                    MockStroke {
                        device,
                        code: 29,
                        state: KEY_DOWN,
                    },
                    MockStroke {
                        device,
                        code: 29,
                        state: KEY_UP,
                    },
                ]
            })
            .collect()
    }

    #[test]
    fn escape_fires_while_the_device_is_being_suppressed() {
        // The gesture's own strokes never reach Windows — that is the point of
        // the escape hatch — and it still frees the keyboard.
        let backend = MockCaptureBackend::new(two_keyboards(), left_ctrl_x5(0));
        let escapes = backend.escapes();
        let resent = backend.resent_log();
        let (tx, rx) = crossbeam_channel::bounded(64);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .unwrap();

        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        let _events: Vec<KeyEvent> = rx.iter().take(10).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        let snap = escapes.snapshot();
        assert_eq!(snap.toggles, 1, "LeftCtrl x5 must fire while suppressed");
        assert!(
            snap.passthrough,
            "the capture thread must free itself immediately, without the supervisor"
        );
        // Nine of the ten strokes were swallowed; the tenth is the one that
        // completed the gesture, and escapes are evaluated BEFORE the
        // pass/suppress decision, so it flows to Windows like everything after.
        let log = resent.lock().unwrap();
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!(log[0].state, KEY_UP);
    }

    /// The regression test for the whole M4 escape finding: with the event
    /// channel FULL and nobody draining it, every downstream consumer is
    /// effectively wedged. Escape detection lives on this thread, upstream of
    /// the channel, so the gesture still works.
    #[test]
    fn escape_fires_when_the_event_channel_is_full() {
        let backend = MockCaptureBackend::new(two_keyboards(), left_ctrl_x5(0));
        let escapes = backend.escapes();
        let health = backend.health();
        let resent = backend.resent_log();
        // Capacity 1, never drained: the first stroke fits, every later
        // try_send fails.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .unwrap();

        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        // Nothing drains `rx`, so the only observable progress is the escape
        // handle itself — which is exactly the claim under test.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while escapes.snapshot().toggles == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        assert!(
            health.snapshot().dropped_events > 0,
            "the stall must be real: events were dropped"
        );
        let snap = escapes.snapshot();
        assert_eq!(
            snap.toggles, 1,
            "a full event channel must not be able to starve the escape hatch"
        );
        assert!(snap.passthrough, "keyboards must be free again");
        assert_eq!(
            resent.lock().unwrap().len(),
            1,
            "the gesture-completing stroke passes through once the latch flips"
        );
        drop(rx);
    }

    /// The whole lockout scenario in one test, and the only question a person
    /// standing at a wedged cabinet actually asks: *does my keyboard work
    /// again?*
    ///
    /// The device is captured, the event channel is full and nobody is draining
    /// it (every downstream consumer — engine, output thread, a ViGEm call —
    /// is effectively dead), and **no `CaptureCtl` is ever sent**: the
    /// supervisor might as well not exist. The user mashes LeftCtrl ×5, and
    /// from then on ordinary typing from that same still-bound device must
    /// reach Windows. `escape_fires_when_the_event_channel_is_full` proves the
    /// gesture is *registered*; this proves it is *effective*.
    #[test]
    fn typing_reaches_windows_again_after_an_escape_with_everything_wedged() {
        const AFTER: usize = 6; // press+release pairs typed after the gesture
        let mut script = left_ctrl_x5(0);
        script.extend((0..AFTER).flat_map(|_| {
            [
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
            ]
        }));
        let expected = AFTER * 2 + 1; // + the gesture-completing stroke

        let backend = MockCaptureBackend::new(two_keyboards(), script);
        let escapes = backend.escapes();
        let health = backend.health();
        let resent = backend.resent_log();
        // Capacity 1, never drained: the pipeline is wedged for good.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .unwrap();

        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while resent.lock().expect("resent log").len() < expected
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Only now — after the keyboard had to be working again on its own.
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        assert!(
            health.snapshot().dropped_events > 0,
            "the wedge must be real: events were dropped"
        );
        assert_eq!(escapes.snapshot().toggles, 1);
        let log = resent.lock().expect("resent log");
        assert_eq!(
            log.len(),
            expected,
            "everything typed after the escape must reach Windows, with no \
             SetPassthrough from anyone: {log:?}"
        );
        assert!(
            log[1..].iter().all(|r| r.code == 30),
            "the post-escape strokes are the typing, not the gesture: {log:?}"
        );
        drop(rx);
    }

    #[test]
    fn ctrl_alt_del_forces_passthrough_and_reports_a_stop() {
        let script = vec![
            MockStroke {
                device: 0,
                code: 29, // LeftControl
                state: KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code: 56, // LeftAlt
                state: KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code: 83, // E0 Delete
                state: KEY_E0 | KEY_DOWN,
            },
        ];
        let backend = MockCaptureBackend::new(two_keyboards(), script);
        let escapes = backend.escapes();
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .unwrap();

        let handle = Box::new(backend).run(tx, ctl_rx).unwrap();
        let events: Vec<KeyEvent> = rx.iter().take(3).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        assert_eq!(events[2].key, Key::Delete);
        let snap = escapes.snapshot();
        assert_eq!(snap.stops, 1);
        assert!(snap.passthrough, "the emergency stop frees keyboards first");
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
