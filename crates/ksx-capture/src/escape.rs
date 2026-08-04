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

    /// The passthrough latch itself, lock-free.
    ///
    /// Public because the latch is *shared state between capture threads*: with
    /// the M6 backend split (one WinUSB thread per claimed board, plus an
    /// Interception thread for everything else), `LeftCtrl x5` on any one board
    /// has to free them **all**. Handing every [`EscapeWatch`] the same
    /// [`EscapeHandle`] is what makes that true, and this is where they meet.
    pub fn passthrough(&self) -> bool {
        self.0.passthrough.load(Ordering::Relaxed)
    }

    /// Clear the passthrough latch — **session start only**.
    ///
    /// The latch is one-way *within* a session on purpose: nothing a supervisor
    /// does may re-capture keyboards behind an escape's back. Between sessions
    /// it is a different question, and M6 is what raised it. The handle now
    /// lives as long as the daemon's claim, so a `LeftCtrl ×5` used to escape a
    /// wedged game would still be latched when the next game starts — and
    /// "freed" means the panel types onto the desktop while the pads are being
    /// driven, i.e. double input on every press, with no gesture to explain it.
    ///
    /// Starting a session is the one moment where re-arming is the user's own
    /// instruction: they asked for emulation, now, after whatever they did
    /// before. So the *latch* is reset here and the *counters* deliberately are
    /// not — they stay monotonic, which is what lets a poller take a baseline
    /// and treat only its own increments as its own gestures
    /// ([`EscapeStatus`]).
    ///
    /// Idempotent, and safe to call while capture threads are running: they
    /// read the latch per stroke and compute the next toggle from it, so an
    /// armed latch simply means the next gesture frees rather than re-captures.
    pub fn arm(&self) {
        self.0.passthrough.store(false, Ordering::Relaxed);
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
/// self-passthrough latch.
///
/// The *detector* is owned by one thread and never shared — gesture progress is
/// per-thread state. The *latch* deliberately is not: it lives in the shared
/// [`EscapeHandle`], so several capture threads handed the same handle behave
/// as one escape hatch. With a single backend that is indistinguishable from a
/// local `bool`; with M6's split (a WinUSB thread per claimed board alongside
/// the Interception thread) it is the difference between `LeftCtrl x5` freeing
/// every keyboard and freeing only the one you happened to press it on.
#[derive(Debug)]
pub struct EscapeWatch {
    detector: EscapeDetector,
    handle: EscapeHandle,
}

impl EscapeWatch {
    pub fn new(handle: EscapeHandle) -> Self {
        Self {
            detector: EscapeDetector::default(),
            handle,
        }
    }

    /// The self-passthrough latch. `true` ⇒ suppress nothing, whatever the
    /// supervisor last asked for.
    pub fn passthrough(&self) -> bool {
        self.handle.passthrough()
    }

    /// Feed one stroke. Hot path: a few counters plus (only the first time a
    /// device is seen) one small push. No locks, no channels, no allocation in
    /// steady state.
    pub fn observe(&mut self, event: &KeyEvent) -> EscapeAction {
        match self.detector.observe(event) {
            None => EscapeAction::None,
            Some(Escape::LeftSequence) => {
                let next = !self.handle.passthrough();
                self.handle.0.passthrough.store(next, Ordering::Relaxed);
                self.handle.0.toggles.fetch_add(1, Ordering::Relaxed);
                EscapeAction::ToggledCapture { passthrough: next }
            }
            Some(Escape::RightSequence) => {
                self.handle.0.mouse_escapes.fetch_add(1, Ordering::Relaxed);
                EscapeAction::Mouse
            }
            Some(Escape::CtrlAltDel) => {
                // Never a toggle: the emergency stop only ever frees keyboards.
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

    /// M6: several capture threads, one escape hatch.
    ///
    /// With the WinUSB backend a session can be running three capture threads
    /// at once (two claimed I-PACs and an Interception thread for the desk
    /// keyboard). Freeing "the keyboards" must mean all of them — a player who
    /// mashes LeftCtrl x5 on player 1's panel and gets only player 1's board
    /// back is still locked out.
    #[test]
    fn one_handle_makes_several_watches_a_single_escape_hatch() {
        let handle = EscapeHandle::new();
        let mut winusb_p1 = EscapeWatch::new(handle.clone());
        let winusb_p2 = EscapeWatch::new(handle.clone());
        let mut interception = EscapeWatch::new(handle.clone());

        assert!(!winusb_p2.passthrough());
        let action = cycles(&mut winusb_p1, Key::LeftControl, 5);
        assert_eq!(action, EscapeAction::ToggledCapture { passthrough: true });

        assert!(winusb_p1.passthrough());
        assert!(winusb_p2.passthrough(), "the other board is freed too");
        assert!(interception.passthrough(), "and so is every other keyboard");

        // Re-arming is equally global, and the toggle is computed from the
        // shared latch — not from a stale per-thread bool.
        let action = cycles(&mut interception, Key::LeftControl, 5);
        assert_eq!(action, EscapeAction::ToggledCapture { passthrough: false });
        assert!(!winusb_p1.passthrough());
        assert!(!winusb_p2.passthrough());
        assert_eq!(handle.snapshot().toggles, 2);
    }

    /// A stop on any thread frees every thread and can never be toggled back.
    #[test]
    fn a_stop_on_one_thread_frees_them_all() {
        let handle = EscapeHandle::new();
        let mut a = EscapeWatch::new(handle.clone());
        let b = EscapeWatch::new(handle.clone());
        a.observe(&ev(Key::LeftControl, true));
        a.observe(&ev(Key::LeftAlt, true));
        assert_eq!(a.observe(&ev(Key::Delete, true)), EscapeAction::Stop);
        assert!(b.passthrough());
    }

    /// R2, at the level it is caused. A gesture that freed the panel in one
    /// session must not still be freeing it in the next one — but the counters
    /// it moved must survive, because that is what a baseline reader needs.
    #[test]
    fn arming_clears_the_latch_without_rewriting_the_counters() {
        let handle = EscapeHandle::new();
        let mut w = EscapeWatch::new(handle.clone());
        cycles(&mut w, Key::LeftControl, 5);
        assert!(handle.passthrough());

        handle.arm();
        assert!(!handle.passthrough(), "a new session starts armed");
        let snap = handle.snapshot();
        assert_eq!(snap.toggles, 1, "history is not rewritten, only re-armed");
        assert!(!snap.passthrough);

        // The next gesture frees, rather than re-capturing something that was
        // never captured: the watch computes from the shared latch, not from a
        // stale local bool.
        let action = cycles(&mut w, Key::LeftControl, 5);
        assert_eq!(action, EscapeAction::ToggledCapture { passthrough: true });
        assert!(handle.passthrough());
        assert_eq!(handle.snapshot().toggles, 2);

        // Idempotent, and it also undoes an emergency stop's forced latch —
        // the stop ended that session, this is the next one.
        handle.arm();
        handle.arm();
        assert!(!handle.passthrough());
    }

    /// The baseline-delta pattern a session poller uses, spelled out: what it
    /// must NOT see is every gesture that ever happened.
    #[test]
    fn a_baseline_reader_sees_only_the_gestures_after_it() {
        let handle = EscapeHandle::new();
        let mut w = EscapeWatch::new(handle.clone());
        cycles(&mut w, Key::LeftControl, 5);
        cycles(&mut w, Key::RightControl, 5);

        let baseline = handle.snapshot();
        assert_eq!(baseline.toggles, 1);
        assert_eq!(handle.snapshot().toggles - baseline.toggles, 0);

        cycles(&mut w, Key::LeftControl, 5);
        assert_eq!(
            handle.snapshot().toggles - baseline.toggles,
            1,
            "one gesture since the baseline, not two"
        );
        assert_eq!(handle.snapshot().mouse_escapes - baseline.mouse_escapes, 0);
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
