//! Emergency escapes, evaluated **on the capture thread** — the only place they
//! cannot be starved.
//!
//! # Why this lives here and not in the app layer
//!
//! M4 originally ran [`ksx_core::EscapeDetector`] on the engine thread, one
//! bounded channel downstream of capture and one *blocking* send upstream of
//! output. That is a lockout: if the output thread wedges (a ViGEm IOCTL that
//! never returns), the engine blocks on its send, stops draining key events, and
//! the escape gesture is never evaluated — with every cabinet keyboard captured
//! and no second keyboard to rescue from. Legacy did it right:
//! `InputManager.CheckForEmergencyHit` ran inside the interception callback,
//! before the blocking decision, and that is what this module restores.
//!
//! # The contract
//!
//! - Every stroke is fed to [`EscapeWatch::observe`] **before** the pass/suppress
//!   decision, so the stroke that completes a gesture is already un-suppressed.
//! - On `LeftCtrl ×5` the capture thread flips its **own** passthrough latch —
//!   no channel, no supervisor round trip, no possible starvation. The gesture
//!   works even when the event channel is full and every consumer is wedged.
//! - `Ctrl+Alt+Del` forces the latch on (never off) and raises a stop request.
//! - The supervisor learns about all of it by polling [`EscapeHandle`] — the
//!   same shape as [`crate::HealthHandle`]: atomics, lock-free, cloned before
//!   `run` consumes the backend, valid for the life of the process.
//!
//! The latch is one-way per gesture and is **or**-ed into the ctl-driven
//! passthrough flag, so a supervisor that keeps sending `SetCaptured` (hotplug,
//! a stale mirror of its own state) can never re-arm blocking behind an escape's
//! back. Re-enabling capture takes a second `LeftCtrl ×5` — releasing keyboards
//! is instant, re-capturing them is deliberate.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use ksx_core::{DeviceId, Escape, EscapeDetector, KeyEvent};

#[derive(Debug, Default)]
struct Inner {
    toggles: AtomicU64,
    mouse_escapes: AtomicU64,
    stops: AtomicU64,
    passthrough: AtomicBool,
}

/// Cloneable, lock-free view of a capture thread's escape state.
#[derive(Clone, Debug, Default)]
pub struct EscapeHandle(Arc<Inner>);

/// Point-in-time snapshot of [`EscapeHandle`].
///
/// The counters are monotonic: a poller remembers the last value it saw and
/// treats any increase as "that many gestures happened", so a slow poll can
/// merge but never lose one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EscapeStatus {
    /// `LeftCtrl ×5` gestures seen since the thread started.
    pub toggles: u64,
    /// `RightCtrl ×5` gestures seen (mouse capture — never acted on in M4).
    pub mouse_escapes: u64,
    /// `Ctrl+Alt+Del` gestures seen: stop emulation.
    pub stops: u64,
    /// The capture thread's own passthrough latch, as it stands right now.
    /// `true` means the thread has already stopped suppressing everything.
    pub passthrough: bool,
}

impl EscapeHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> EscapeStatus {
        EscapeStatus {
            toggles: self.0.toggles.load(Ordering::Relaxed),
            mouse_escapes: self.0.mouse_escapes.load(Ordering::Relaxed),
            stops: self.0.stops.load(Ordering::Relaxed),
            passthrough: self.0.passthrough.load(Ordering::Relaxed),
        }
    }
}

/// What the capture thread must do about a stroke it just observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeAction {
    /// Nothing happened.
    None,
    /// `LeftCtrl ×5`: the latch flipped to `passthrough`. The caller must
    /// republish its decision snapshot before deciding this stroke.
    ToggledCapture { passthrough: bool },
    /// `RightCtrl ×5`: reported only (M4 never sets the mouse class filter).
    Mouse,
    /// `Ctrl+Alt+Del`: latch forced on AND a stop requested.
    Stop,
}

impl EscapeAction {
    /// Did this action change the passthrough latch? (The caller republishes.)
    pub fn changed_passthrough(&self) -> bool {
        matches!(
            self,
            EscapeAction::ToggledCapture { .. } | EscapeAction::Stop
        )
    }
}

/// The capture thread's escape state machine: [`EscapeDetector`] plus the
/// self-passthrough latch. Owned by one thread; never shared.
#[derive(Debug)]
pub struct EscapeWatch {
    detector: EscapeDetector,
    handle: EscapeHandle,
    passthrough: bool,
}

impl EscapeWatch {
    pub fn new(handle: EscapeHandle) -> Self {
        Self {
            detector: EscapeDetector::default(),
            handle,
            passthrough: false,
        }
    }

    /// The self-passthrough latch. `true` ⇒ suppress nothing, whatever the
    /// supervisor last asked for.
    pub fn passthrough(&self) -> bool {
        self.passthrough
    }

    /// Feed one stroke. Hot path: a few counters plus (only the first time a
    /// device is seen) one small push. No locks, no channels, no allocation in
    /// steady state.
    pub fn observe(&mut self, event: &KeyEvent) -> EscapeAction {
        match self.detector.observe(event) {
            None => EscapeAction::None,
            Some(Escape::LeftSequence) => {
                self.passthrough = !self.passthrough;
                self.handle
                    .0
                    .passthrough
                    .store(self.passthrough, Ordering::Relaxed);
                self.handle.0.toggles.fetch_add(1, Ordering::Relaxed);
                EscapeAction::ToggledCapture {
                    passthrough: self.passthrough,
                }
            }
            Some(Escape::RightSequence) => {
                self.handle.0.mouse_escapes.fetch_add(1, Ordering::Relaxed);
                EscapeAction::Mouse
            }
            Some(Escape::CtrlAltDel) => {
                // Never a toggle: the emergency stop only ever frees keyboards.
                self.passthrough = true;
                self.handle.0.passthrough.store(true, Ordering::Relaxed);
                self.handle.0.stops.fetch_add(1, Ordering::Relaxed);
                EscapeAction::Stop
            }
        }
    }

    /// A device vanished: drop its half-held modifier state so a removed board
    /// cannot leave `Ctrl+Alt+Del` armed.
    pub fn forget_device(&mut self, device: &DeviceId) {
        self.detector.forget_device(device);
    }
}

#[cfg(test)]
mod tests {
    use ksx_core::Key;

    use super::*;

    fn ev(key: Key, down: bool) -> KeyEvent {
        KeyEvent {
            device: DeviceId::from("A"),
            key,
            down,
            t: 0,
        }
    }

    fn cycles(w: &mut EscapeWatch, key: Key, n: u32) -> EscapeAction {
        let mut last = EscapeAction::None;
        for _ in 0..n {
            last = w.observe(&ev(key, true));
            let up = w.observe(&ev(key, false));
            if up != EscapeAction::None {
                last = up;
            }
        }
        last
    }

    #[test]
    fn left_ctrl_x5_flips_the_latch_without_any_channel() {
        let handle = EscapeHandle::new();
        let mut w = EscapeWatch::new(handle.clone());
        assert!(!w.passthrough());
        assert_eq!(handle.snapshot(), EscapeStatus::default());

        let action = cycles(&mut w, Key::LeftControl, 5);
        assert_eq!(action, EscapeAction::ToggledCapture { passthrough: true });
        assert!(action.changed_passthrough());
        assert!(
            w.passthrough(),
            "the capture thread frees itself, instantly"
        );
        let snap = handle.snapshot();
        assert_eq!(snap.toggles, 1);
        assert!(snap.passthrough);

        // A second gesture re-arms capture.
        let action = cycles(&mut w, Key::LeftControl, 5);
        assert_eq!(action, EscapeAction::ToggledCapture { passthrough: false });
        assert!(!w.passthrough());
        assert_eq!(handle.snapshot().toggles, 2);
        assert!(!handle.snapshot().passthrough);
    }

    #[test]
    fn ctrl_alt_del_forces_passthrough_on_and_requests_a_stop() {
        let handle = EscapeHandle::new();
        let mut w = EscapeWatch::new(handle.clone());
        w.observe(&ev(Key::LeftControl, true));
        w.observe(&ev(Key::LeftAlt, true));
        assert_eq!(w.observe(&ev(Key::Delete, true)), EscapeAction::Stop);
        assert!(w.passthrough());
        let snap = handle.snapshot();
        assert_eq!(snap.stops, 1);
        assert!(snap.passthrough);
    }

    #[test]
    fn a_stop_never_toggles_passthrough_back_off() {
        let handle = EscapeHandle::new();
        let mut w = EscapeWatch::new(handle.clone());
        w.observe(&ev(Key::LeftControl, true));
        w.observe(&ev(Key::LeftAlt, true));
        // Legacy fires the combo on every subsequent event while it stays held;
        // repeated firing must never re-capture the keyboards.
        for _ in 0..5 {
            assert_eq!(w.observe(&ev(Key::Delete, true)), EscapeAction::Stop);
            assert!(w.passthrough());
        }
        assert_eq!(handle.snapshot().stops, 5);
    }

    #[test]
    fn right_ctrl_x5_is_counted_but_changes_nothing() {
        let handle = EscapeHandle::new();
        let mut w = EscapeWatch::new(handle.clone());
        let action = cycles(&mut w, Key::RightControl, 5);
        assert_eq!(action, EscapeAction::Mouse);
        assert!(!action.changed_passthrough());
        assert!(!w.passthrough(), "the mouse escape never frees keyboards");
        assert_eq!(handle.snapshot().mouse_escapes, 1);
    }

    #[test]
    fn forget_device_disarms_a_half_held_combo() {
        let mut w = EscapeWatch::new(EscapeHandle::new());
        w.observe(&ev(Key::LeftControl, true));
        w.observe(&ev(Key::LeftAlt, true));
        w.forget_device(&DeviceId::from("A"));
        assert_eq!(w.observe(&ev(Key::Delete, true)), EscapeAction::None);
        assert!(!w.passthrough());
    }
}
