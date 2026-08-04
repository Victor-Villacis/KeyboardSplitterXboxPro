//! ViGEmBus implementation of [`VirtualPadBackend`] over the vendored
//! `vigem-client` (raw IOCTLs against `\\.\ViGEmBus`, no C DLL).
//!
//! Thread/lifetime model (dictated by the vendored API — see
//! `crates/vigem-client/src/{x360,event,bus}.rs`):
//! - One `Client` connection, shared by all targets via `Arc`.
//! - `wait_ready`'s IOCTL blocks with no timeout knob, so `plug` runs
//!   `plugin() + wait_ready()` on a short-lived thread and applies the deadline
//!   at the channel. On timeout the thread still owns the target: whenever the
//!   IOCTL finally completes the target drops → auto-unplugs, so a timed-out
//!   plug can never leak a ghost pad.
//! - One `request_notification` per target (the vendored API loses events with
//!   multiple listeners). Its `spawn_thread` callback forwards into a bounded
//!   channel with `try_send`, so the notification thread never blocks on a slow
//!   consumer and `poll_feedback` never blocks on the driver.
//! - Unplugging a target aborts its pending notification IOCTL
//!   (`ERROR_OPERATION_ABORTED`), which is what terminates the notification
//!   thread; we join it *after* the unplug, and always before any subsequent
//!   plug. That ordering also defuses the serial-reuse race noted in
//!   `x360.rs::poll` (a stale listener grabbing a recycled serial number).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ksx_core::PadState;
use vigem_client::{Client, TargetId, XGamepad, Xbox360Wired};

use crate::backend::{Feedback, PadHandle, VirtualPadBackend};
use crate::error::OutputError;

/// Ample for plug+wait_ready; PNP device arrival is normally < 1 s.
const DEFAULT_PLUG_TIMEOUT: Duration = Duration::from_secs(5);
/// XInput slot binding can lag device readiness by a beat.
const USER_INDEX_WAIT: Duration = Duration::from_millis(1000);
const USER_INDEX_POLL: Duration = Duration::from_millis(20);
/// Feedback is low-rate (rumble deltas + LED changes); overflow drops newest.
const FEEDBACK_QUEUE_CAP: usize = 64;

/// `PadState` (ksx-core) → `XGamepad` (vigem wire struct).
///
/// Deliberately a plain field copy — `PadState` is defined in XInput wire shape
/// and the bit-level equivalence is locked down by the tests below.
fn to_xgamepad(state: &PadState) -> XGamepad {
    XGamepad {
        buttons: vigem_client::XButtons {
            raw: state.buttons.bits(),
        },
        left_trigger: state.lt,
        right_trigger: state.rt,
        thumb_lx: state.lx,
        thumb_ly: state.ly,
        thumb_rx: state.rx,
        thumb_ry: state.ry,
    }
}

fn map_target_err(err: vigem_client::Error) -> OutputError {
    OutputError::TargetError(err)
}

struct PadEntry {
    target: Xbox360Wired<Arc<Client>>,
    user_index: Option<u8>,
    feedback_rx: mpsc::Receiver<Feedback>,
    notif_thread: Option<JoinHandle<()>>,
    dropped_feedback: Arc<AtomicUsize>,
}

/// ViGEmBus-backed virtual Xbox 360 pads.
///
/// Dropping the backend unplugs every remaining pad (a clean process exit never
/// leaves ghost pads). A *killed* process runs no destructors: ViGEm pads then
/// linger until the driver notices the client handle is gone — the `cab-tests`
/// loopback documents the observed behavior.
pub struct VigemBackend {
    client: Arc<Client>,
    pads: BTreeMap<u32, PadEntry>,
    next_id: u32,
    plug_timeout: Duration,
}

impl VigemBackend {
    /// Connects to the ViGEmBus driver.
    ///
    /// Returns [`OutputError::BusNotFound`] (with install instructions) when the
    /// bus device interface does not exist — i.e. ViGEmBus is not installed.
    pub fn connect() -> Result<Self, OutputError> {
        let client = Client::connect().map_err(|e| match e {
            vigem_client::Error::BusNotFound => OutputError::BusNotFound,
            other => OutputError::TargetError(other),
        })?;
        tracing::info!("connected to ViGEmBus");
        Ok(Self {
            client: Arc::new(client),
            pads: BTreeMap::new(),
            next_id: 0,
            plug_timeout: DEFAULT_PLUG_TIMEOUT,
        })
    }

    /// Overrides the plug readiness deadline (default 5 s).
    pub fn with_plug_timeout(mut self, timeout: Duration) -> Self {
        self.plug_timeout = timeout;
        self
    }

    /// Notifications dropped because the feedback queue was full (consumer not
    /// polling). Diagnostic for `ksx doctor`.
    pub fn dropped_feedback(&self, handle: PadHandle) -> Option<usize> {
        self.pads
            .get(&handle.0)
            .map(|p| p.dropped_feedback.load(Ordering::Relaxed))
    }

    fn entry_mut(&mut self, handle: PadHandle) -> Result<&mut PadEntry, OutputError> {
        self.pads
            .get_mut(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))
    }

    /// `plugin() + wait_ready()` under a deadline (see module docs for why this
    /// needs a helper thread).
    fn plug_with_timeout(&self) -> Result<Xbox360Wired<Arc<Client>>, OutputError> {
        let mut target = Xbox360Wired::new(self.client.clone(), TargetId::XBOX360_WIRED);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = target
                .plugin()
                .and_then(|()| target.wait_ready())
                .map(|()| target);
            let _ = tx.send(result);
        });
        match rx.recv_timeout(self.plug_timeout) {
            Ok(Ok(target)) => Ok(target),
            Ok(Err(e)) => Err(map_target_err(e)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                tracing::warn!(timeout = ?self.plug_timeout, "plug timed out waiting for readiness");
                Err(OutputError::PlugTimeout(self.plug_timeout))
            }
            // Helper thread panicked: the IOCTL machinery is in an unknown state.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(map_target_err(vigem_client::Error::OperationAborted))
            }
        }
    }

    fn read_user_index(target: &mut Xbox360Wired<Arc<Client>>) -> Option<u8> {
        let deadline = Instant::now() + USER_INDEX_WAIT;
        loop {
            match target.get_user_index() {
                Ok(i) => return u8::try_from(i).ok(),
                // Not (yet) bound to an XInput slot: retry until the deadline —
                // pads 5+ legitimately never get one.
                Err(vigem_client::Error::UserIndexOutOfRange) => {}
                Err(err) => {
                    tracing::warn!(%err, "get_user_index failed");
                    return None;
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(USER_INDEX_POLL);
        }
    }

    fn teardown(mut entry: PadEntry) -> Result<(), OutputError> {
        let result = entry.target.unplug().map_err(map_target_err);
        // Drop before joining: if the explicit unplug failed, Drop retries it —
        // the notification thread only exits once its IOCTL aborts.
        drop(entry.target);
        if let Some(thread) = entry.notif_thread.take() {
            let _ = thread.join();
        }
        result
    }
}

impl VirtualPadBackend for VigemBackend {
    fn plug(&mut self) -> Result<PadHandle, OutputError> {
        let mut target = self.plug_with_timeout()?;
        let user_index = Self::read_user_index(&mut target);

        let notification = target.request_notification().map_err(map_target_err)?;
        let (tx, feedback_rx) = mpsc::sync_channel::<Feedback>(FEEDBACK_QUEUE_CAP);
        let dropped_feedback = Arc::new(AtomicUsize::new(0));
        let dropped = dropped_feedback.clone();
        let notif_thread = notification.spawn_thread(move |_reqn, n| {
            let feedback = Feedback {
                large_motor: n.large_motor,
                small_motor: n.small_motor,
                led_number: n.led_number,
            };
            if tx.try_send(feedback).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        });

        let id = self.next_id;
        self.next_id += 1;
        self.pads.insert(
            id,
            PadEntry {
                target,
                user_index,
                feedback_rx,
                notif_thread: Some(notif_thread),
                dropped_feedback,
            },
        );
        tracing::info!(handle = id, ?user_index, "virtual pad plugged");
        Ok(PadHandle(id))
    }

    fn user_index(&self, handle: PadHandle) -> Option<u8> {
        self.pads.get(&handle.0).and_then(|p| p.user_index)
    }

    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
        let gamepad = to_xgamepad(state);
        self.entry_mut(handle)?
            .target
            .update(&gamepad)
            .map_err(map_target_err)
    }

    fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
        self.pads.get_mut(&handle.0)?.feedback_rx.try_recv().ok()
    }

    fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
        let entry = self
            .pads
            .remove(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))?;
        let result = Self::teardown(entry);
        tracing::info!(
            handle = handle.0,
            ok = result.is_ok(),
            "virtual pad unplugged"
        );
        result
    }
}

impl Drop for VigemBackend {
    fn drop(&mut self) {
        while let Some((id, entry)) = self.pads.pop_first() {
            if let Err(err) = Self::teardown(entry) {
                tracing::warn!(handle = id, %err, "unplug during backend drop failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ksx_core::pad::XButtons;

    use super::*;

    /// (ksx-core flag, vigem constant) — every named XInput button bit.
    const BUTTON_BITS: [(XButtons, u16); 15] = [
        (XButtons::DPAD_UP, vigem_client::XButtons::UP),
        (XButtons::DPAD_DOWN, vigem_client::XButtons::DOWN),
        (XButtons::DPAD_LEFT, vigem_client::XButtons::LEFT),
        (XButtons::DPAD_RIGHT, vigem_client::XButtons::RIGHT),
        (XButtons::START, vigem_client::XButtons::START),
        (XButtons::BACK, vigem_client::XButtons::BACK),
        (XButtons::LEFT_THUMB, vigem_client::XButtons::LTHUMB),
        (XButtons::RIGHT_THUMB, vigem_client::XButtons::RTHUMB),
        (XButtons::LEFT_BUMPER, vigem_client::XButtons::LB),
        (XButtons::RIGHT_BUMPER, vigem_client::XButtons::RB),
        (XButtons::GUIDE, vigem_client::XButtons::GUIDE),
        (XButtons::A, vigem_client::XButtons::A),
        (XButtons::B, vigem_client::XButtons::B),
        (XButtons::X, vigem_client::XButtons::X),
        (XButtons::Y, vigem_client::XButtons::Y),
    ];

    #[test]
    fn every_button_bit_matches_the_vigem_constant() {
        for (ours, theirs) in BUTTON_BITS {
            assert_eq!(
                ours.bits(),
                theirs,
                "{ours:?} must be wire bit {theirs:#06x}"
            );
        }
        // The table is exhaustive on both sides: all named flags covered, and
        // the combined mask is identical.
        let ours_all: u16 = BUTTON_BITS
            .iter()
            .map(|(f, _)| f.bits())
            .fold(0, |a, b| a | b);
        let theirs_all: u16 = BUTTON_BITS.iter().fold(0, |a, (_, b)| a | b);
        assert_eq!(ours_all, XButtons::all().bits());
        assert_eq!(ours_all, theirs_all);
        // 15 distinct bits: only 0x0800 (reserved in XInput) is unnamed.
        assert_eq!(ours_all, !0x0800u16);
    }

    #[test]
    fn button_conversion_preserves_each_single_bit() {
        for (flag, wire) in BUTTON_BITS {
            let state = PadState {
                buttons: flag,
                ..PadState::default()
            };
            assert_eq!(to_xgamepad(&state).buttons.raw, wire);
        }
    }

    #[test]
    fn button_conversion_preserves_arbitrary_raw_masks() {
        // Including the reserved 0x0800 bit: conversion is a pass-through, not
        // a re-encoding — unknown bits must survive verbatim.
        for raw in [0u16, 0x0800, 0x5109, 0xFFFF, XButtons::all().bits()] {
            let state = PadState {
                buttons: XButtons::from_bits_retain(raw),
                ..PadState::default()
            };
            assert_eq!(to_xgamepad(&state).buttons.raw, raw);
        }
    }

    #[test]
    fn axes_and_triggers_copy_verbatim() {
        let cases = [
            (0u8, 0u8, 0i16, 0i16, 0i16, 0i16),
            (255, 128, i16::MIN, i16::MAX, -1, 1),
            (1, 254, 12345, -12345, i16::MAX, i16::MIN),
        ];
        for (lt, rt, lx, ly, rx, ry) in cases {
            let state = PadState {
                buttons: XButtons::empty(),
                lt,
                rt,
                lx,
                ly,
                rx,
                ry,
            };
            let g = to_xgamepad(&state);
            assert_eq!(g.left_trigger, lt);
            assert_eq!(g.right_trigger, rt);
            assert_eq!(g.thumb_lx, lx);
            assert_eq!(g.thumb_ly, ly);
            assert_eq!(g.thumb_rx, rx);
            assert_eq!(g.thumb_ry, ry);
        }
    }

    #[test]
    fn neutral_state_is_neutral_gamepad() {
        assert_eq!(to_xgamepad(&PadState::default()), XGamepad::default());
    }
}
