//! `CompositeBackend` — several capture backends, one session.
//!
//! M6 makes backend choice **per device**: `[[device]] backend = "winusb"` for
//! each rebound I-PAC, `"interception"` for anything still on the keyboard
//! stack. A run can therefore need two claimed WinUSB interfaces *and* an
//! Interception context at the same time, while the supervisor above must keep
//! seeing exactly one [`CaptureBackend`]: one event stream, one control channel,
//! one health snapshot, one escape latch.
//!
//! # What "one session" has to mean
//!
//! - **One event stream.** Every child gets a clone of the same `Sender`. Order
//!   between devices is not defined and never was (two boards are genuinely
//!   concurrent); order *within* a device is preserved because one thread owns
//!   it.
//! - **One control channel.** `SetCaptured` carries the whole captured set, so
//!   it is broadcast verbatim and each child picks out its own devices. A
//!   `Shutdown` reaches every child.
//! - **One escape latch.** `LeftCtrl x5` on any board frees them all. This is
//!   not merged here — it falls out of handing every child the same
//!   [`EscapeHandle`] (see [`crate::escape::EscapeWatch`]), which is why
//!   [`Handles`] exists.
//! - **One health state.** Same trick: one [`HealthHandle`], flags OR, counters
//!   sum.
//! - **Presence merges, it does not share.** Two children publishing device
//!   lists into one handle would each clobber the other's, so presence is the
//!   one thing this module merges explicitly ([`PresenceHandle::merged`]).
//!
//! # Failure policy: any child dying ends the session
//!
//! A dead child is a keyboard nobody is pumping. On the WinUSB side that board
//! goes silent (its keys reach neither Windows nor the pads); on the
//! Interception side a dead thread with an armed filter is the classic lockout.
//! Either way the honest move is to tear the whole session down, in the
//! direction that frees keyboards: broadcast `Shutdown`, join everyone, and
//! report the *worst* reason seen. `ksx run`'s supervisor already treats a
//! capture exit as fatal and releases blocking on the way out.

use crossbeam_channel::{Receiver, Select, Sender};
use ksx_core::KeyEvent;

use crate::backend::{CaptureBackend, CaptureCtl, DeviceInfo, ExitReason, Handles};
use crate::escape::EscapeHandle;
use crate::health::HealthHandle;
use crate::presence::PresenceHandle;

/// Several [`CaptureBackend`]s driven as one.
pub struct CompositeBackend {
    children: Vec<Box<dyn CaptureBackend>>,
    handles: Handles,
}

impl CompositeBackend {
    /// Compose backends that were built with the **same** [`Handles`].
    ///
    /// Passing children built with different handles is a programming error the
    /// constructor cannot detect (the handles are opaque), so the intended
    /// construction is: make one `Handles`, hand a clone to every
    /// `*_with(handles)` constructor, then pass the same one here. `ksx-app`'s
    /// wiring does exactly that.
    pub fn new(children: Vec<Box<dyn CaptureBackend>>, handles: Handles) -> Self {
        Self { children, handles }
    }

    pub fn len(&self) -> usize {
        self.children.len()
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

/// Which exit reason wins when children disagree. Higher = louder.
fn severity(reason: ExitReason) -> u8 {
    match reason {
        ExitReason::Panicked => 4,
        ExitReason::DeviceLost => 3,
        ExitReason::ChannelClosed => 2,
        ExitReason::ScriptExhausted => 1,
        ExitReason::Shutdown => 0,
    }
}

impl CaptureBackend for CompositeBackend {
    fn devices(&mut self) -> Vec<DeviceInfo> {
        self.children.iter_mut().flat_map(|c| c.devices()).collect()
    }

    fn health(&self) -> HealthHandle {
        self.handles.health.clone()
    }

    fn escapes(&self) -> EscapeHandle {
        self.handles.escapes.clone()
    }

    fn presence(&self) -> PresenceHandle {
        PresenceHandle::merged(self.children.iter().map(|c| c.presence()).collect())
    }

    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        let CompositeBackend { children, .. } = *self;

        // Start every child first, so a spawn failure is reported before the
        // coordinator exists and nothing is left half-running.
        let mut ctl_senders = Vec::with_capacity(children.len());
        let mut handles = Vec::with_capacity(children.len());
        for child in children {
            // Unbounded: this carries at most a few messages a session, and a
            // bounded send from the coordinator could block while a child is
            // mid-teardown — which would delay the Shutdown broadcast that
            // frees the other keyboards.
            let (child_tx, child_rx) = crossbeam_channel::unbounded();
            match child.run(tx.clone(), child_rx) {
                Ok(handle) => {
                    ctl_senders.push(child_tx);
                    handles.push(handle);
                }
                Err(err) => {
                    // Undo: tell the ones already running to stop, and wait for
                    // them, so the caller's error means "nothing is running".
                    for sender in &ctl_senders {
                        let _ = sender.send(CaptureCtl::Shutdown);
                    }
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(err);
                }
            }
        }
        // The coordinator holds no Sender of its own.
        drop(tx);

        std::thread::Builder::new()
            .name("ksx-capture-composite".into())
            .spawn(move || coordinate(ctl, ctl_senders, handles))
    }
}

/// Fan control out, watch for any child finishing, then shut everything down.
fn coordinate(
    ctl: Receiver<CaptureCtl>,
    ctl_senders: Vec<Sender<CaptureCtl>>,
    handles: Vec<std::thread::JoinHandle<ExitReason>>,
) -> ExitReason {
    let broadcast = |message: &CaptureCtl| {
        for sender in &ctl_senders {
            // A child that has already exited is not an error here: the join
            // below is what decides the session's fate.
            let _ = sender.send(message.clone());
        }
    };

    let mut shutting_down = false;
    loop {
        // Waiting on the supervisor's ctl channel *and* on "did a child die"
        // at once. `is_finished` is the cheap way to ask the second question,
        // so the select only needs a timeout to come back and re-check.
        let mut select = Select::new();
        select.recv(&ctl);
        let ready = select.ready_timeout(std::time::Duration::from_millis(50));

        if ready.is_ok() {
            match ctl.try_recv() {
                Ok(CaptureCtl::Shutdown) => {
                    broadcast(&CaptureCtl::Shutdown);
                    shutting_down = true;
                }
                Ok(message) => broadcast(&message),
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    // Nobody can release these devices again.
                    broadcast(&CaptureCtl::Shutdown);
                    shutting_down = true;
                }
            }
        }

        if shutting_down || handles.iter().any(|h| h.is_finished()) {
            break;
        }
    }

    // One child down means the session is over: free the rest.
    broadcast(&CaptureCtl::Shutdown);
    drop(ctl_senders);

    let mut worst = ExitReason::Shutdown;
    for handle in handles {
        let reason = handle.join().unwrap_or(ExitReason::Panicked);
        if severity(reason) > severity(worst) {
            worst = reason;
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use ksx_core::{DeviceId, Key};

    use super::*;
    use crate::backend::{DeviceKind, Handles};
    use crate::keymap::{KEY_DOWN, KEY_UP};
    use crate::mock::{MockCaptureBackend, MockStroke};

    const IPAC_A: &str = "USB\\VID_D209&PID_0430&MI_00\\7&1A2B3C4D&0&0000";
    const IPAC_B: &str = "USB\\VID_D209&PID_0430&MI_00\\7&5E6F7A8B&0&0000";
    const DESK: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";

    fn device(id: &str) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::from(id),
            interception_slot: None,
            friendly: None,
            kind: DeviceKind::Keyboard,
        }
    }

    /// A mock built against shared handles, the way the real wiring does it.
    fn child(handles: &Handles, id: &str, script: Vec<MockStroke>) -> Box<dyn CaptureBackend> {
        let mock = MockCaptureBackend::new(vec![device(id)], script)
            .with_handles(handles.clone())
            .with_pace(Duration::from_millis(5));
        Box::new(mock)
    }

    fn press(code: u16) -> Vec<MockStroke> {
        vec![
            MockStroke {
                device: 0,
                code,
                state: KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code,
                state: KEY_UP,
            },
        ]
    }

    #[test]
    fn devices_and_presence_come_from_every_child() {
        let handles = Handles::new();
        let mut composite = CompositeBackend::new(
            vec![
                child(&handles, IPAC_A, vec![]),
                child(&handles, IPAC_B, vec![]),
                child(&handles, DESK, vec![]),
            ],
            handles,
        );
        assert_eq!(composite.len(), 3);
        let ids: Vec<String> = composite
            .devices()
            .iter()
            .map(|d| d.id.as_str().to_owned())
            .collect();
        assert_eq!(ids, vec![IPAC_A, IPAC_B, DESK]);

        let presence = composite.presence();
        let snap = presence.snapshot().expect("all children have published");
        assert_eq!(snap.len(), 3, "the merged view is the union: {snap:?}");
        assert!(snap.contains(&DeviceId::from(IPAC_B)));
    }

    #[test]
    fn events_from_every_child_arrive_on_one_channel() {
        let handles = Handles::new();
        let composite = CompositeBackend::new(
            vec![
                child(&handles, IPAC_A, press(30)), // 'a'
                child(&handles, IPAC_B, press(31)), // 's'
            ],
            handles,
        );
        let (tx, rx) = crossbeam_channel::bounded(32);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();

        let events: Vec<KeyEvent> = rx.iter().take(4).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        let a: Vec<&KeyEvent> = events
            .iter()
            .filter(|e| e.device == DeviceId::from(IPAC_A))
            .collect();
        let b: Vec<&KeyEvent> = events
            .iter()
            .filter(|e| e.device == DeviceId::from(IPAC_B))
            .collect();
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 2);
        assert_eq!(a[0].key, Key::A);
        assert!(a[0].down && !a[1].down, "per-device order is preserved");
        assert_eq!(b[0].key, Key::S);
    }

    #[test]
    fn set_captured_is_broadcast_and_each_child_takes_its_own() {
        let handles = Handles::new();
        let a = MockCaptureBackend::new(vec![device(IPAC_A)], press(30))
            .with_handles(handles.clone())
            .with_pace(Duration::from_millis(20));
        let b = MockCaptureBackend::new(vec![device(IPAC_B)], press(31))
            .with_handles(handles.clone())
            .with_pace(Duration::from_millis(20));
        let resent_a = a.resent_log();
        let resent_b = b.resent_log();

        let composite = CompositeBackend::new(vec![Box::new(a), Box::new(b)], handles);
        let (tx, rx) = crossbeam_channel::bounded(32);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        // Capture only board A.
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC_A)]))
            .unwrap();
        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();

        let _events: Vec<KeyEvent> = rx.iter().take(4).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        assert!(
            resent_a.lock().unwrap().is_empty(),
            "the captured board must not reach the OS"
        );
        assert_eq!(
            resent_b.lock().unwrap().len(),
            2,
            "the unassigned board keeps typing"
        );
    }

    /// The property the whole `Handles` design exists for: an escape gesture on
    /// one board frees every board in the session.
    #[test]
    fn an_escape_on_one_child_frees_them_all() {
        let handles = Handles::new();
        // Board A performs LeftCtrl x5 while both boards are captured.
        let gesture: Vec<MockStroke> = (0..5)
            .flat_map(|_| press(29)) // LeftControl
            .collect();
        let a = MockCaptureBackend::new(vec![device(IPAC_A)], gesture)
            .with_handles(handles.clone())
            .with_pace(Duration::from_millis(2));
        // Board B types afterwards; it must reach the OS.
        let b = MockCaptureBackend::new(vec![device(IPAC_B)], press(31))
            .with_handles(handles.clone())
            .with_pace(Duration::from_millis(60));
        let resent_b = b.resent_log();

        let escapes = handles.escapes.clone();
        let composite = CompositeBackend::new(vec![Box::new(a), Box::new(b)], handles);
        let (tx, rx) = crossbeam_channel::bounded(64);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![
                DeviceId::from(IPAC_A),
                DeviceId::from(IPAC_B),
            ]))
            .unwrap();
        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while resent_b.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
            let _ = rx.recv_timeout(Duration::from_millis(10));
        }
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);

        assert_eq!(escapes.snapshot().toggles, 1);
        assert!(escapes.snapshot().passthrough);
        assert_eq!(
            resent_b.lock().unwrap().len(),
            2,
            "board B was freed by board A's gesture, with no supervisor involved"
        );
    }

    #[test]
    fn health_counters_from_all_children_add_up() {
        let handles = Handles::new();
        let composite = CompositeBackend::new(
            vec![
                child(&handles, IPAC_A, press(30)),
                child(&handles, IPAC_B, press(31)),
            ],
            handles.clone(),
        );
        let health = composite.health();
        // Capacity 1, never drained: both children will fail to send.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while health.snapshot().dropped_events < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        handle.join().unwrap();
        assert!(
            health.snapshot().dropped_events >= 2,
            "both children report into one health handle"
        );
        drop(rx);
    }

    #[test]
    fn one_child_ending_tears_the_session_down() {
        let handles = Handles::new();
        // A child whose consumer is gone exits ChannelClosed immediately.
        let dying = child(&handles, IPAC_A, press(30));
        let other = child(&handles, IPAC_B, vec![]);
        let stops = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&stops);
        let watched = MockCaptureBackend::new(vec![device(DESK)], vec![])
            .with_handles(handles.clone())
            .with_ctl_observer(Box::new(move |message| {
                if matches!(message, CaptureCtl::Shutdown) {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }));

        let composite = CompositeBackend::new(vec![dying, other, Box::new(watched)], handles);
        let (tx, rx) = crossbeam_channel::bounded(4);
        let (_ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        drop(rx); // the consumer is gone before anything starts

        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();
        assert_eq!(
            handle.join().unwrap(),
            ExitReason::ChannelClosed,
            "the loudest child reason wins"
        );
        assert_eq!(
            stops.load(Ordering::SeqCst),
            1,
            "every other child was told to stop"
        );
    }

    #[test]
    fn severity_ranks_panic_above_everything() {
        assert!(severity(ExitReason::Panicked) > severity(ExitReason::DeviceLost));
        assert!(severity(ExitReason::DeviceLost) > severity(ExitReason::ChannelClosed));
        assert!(severity(ExitReason::ChannelClosed) > severity(ExitReason::ScriptExhausted));
        assert!(severity(ExitReason::ScriptExhausted) > severity(ExitReason::Shutdown));
    }

    #[test]
    fn a_single_child_behaves_exactly_like_that_child_alone() {
        // The degenerate case has to be free: wrapping one backend must not
        // change any observable behaviour, or `ksx run` would need two code
        // paths and only one of them would get tested.
        let handles = Handles::new();
        let composite = CompositeBackend::new(vec![child(&handles, IPAC_A, press(30))], handles);
        let (tx, rx) = crossbeam_channel::bounded(8);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();
        let events: Vec<KeyEvent> = rx.iter().take(2).collect();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].key, Key::A);
    }

    #[test]
    fn an_empty_composite_shuts_down_cleanly() {
        let handles = Handles::new();
        let composite = CompositeBackend::new(Vec::new(), handles);
        assert!(composite.is_empty());
        let (tx, _rx) = crossbeam_channel::bounded(1);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(composite).run(tx, ctl_rx).unwrap();
        ctl_tx.send(CaptureCtl::Shutdown).unwrap();
        assert_eq!(handle.join().unwrap(), ExitReason::Shutdown);
    }
}
