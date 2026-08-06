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

use ksx_core::{safe_axis, PadState, Persona};
use rusty_xinput::XInputHandle;
use vigem_client::{Client, DualShock4Wired, TargetId, XGamepad, Xbox360Wired};

use crate::backend::{Feedback, PadHandle, VirtualPadBackend};
use crate::ds4::to_ds4report;
use crate::error::OutputError;

/// Ample for plug+wait_ready; PNP device arrival is normally < 1 s.
const DEFAULT_PLUG_TIMEOUT: Duration = Duration::from_secs(5);
/// Below XINPUT_GAMEPAD_TRIGGER_THRESHOLD (30) — invisible to games, exact in
/// the raw XInputGetState read-back.
const CORRELATE_PULSE: u8 = 1;
const CORRELATE_TIMEOUT: Duration = Duration::from_millis(600);
const CORRELATE_POLL: Duration = Duration::from_millis(15);
/// Feedback is low-rate (rumble deltas + LED changes); overflow drops newest.
const FEEDBACK_QUEUE_CAP: usize = 64;

/// `PadState` (ksx-core) → `XGamepad` (vigem wire struct).
///
/// A plain field copy — `PadState` is defined in XInput wire shape and the
/// bit-level equivalence is locked down by the tests below — with exactly one
/// adjustment: the axes go through [`safe_axis`], which folds `i16::MIN` to
/// -32767.
///
/// That is not a rounding: it is the one value in the range with no positive
/// twin, so a game normalizing by 32767 reads -1.00003 and a game taking
/// `abs()` on an `i16` reads a negative magnitude. `AXIS_MIN` already is -32767,
/// so a preset saying `ly.min` never gets here — this covers what authoring
/// cannot, a hand-written `ly.-32768` or a legacy import, and makes "ksx never
/// puts i16::MIN on the wire" a property of the boundary rather than of every
/// caller upstream of it. See `ksx_core::AXIS_MIN` for the citation.
fn to_xgamepad(state: &PadState) -> XGamepad {
    XGamepad {
        buttons: vigem_client::XButtons {
            raw: state.buttons.bits(),
        },
        left_trigger: state.lt,
        right_trigger: state.rt,
        thumb_lx: safe_axis(state.lx),
        thumb_ly: safe_axis(state.ly),
        thumb_rx: safe_axis(state.rx),
        thumb_ry: safe_axis(state.ry),
    }
}

fn map_target_err(err: vigem_client::Error) -> OutputError {
    OutputError::TargetError(err)
}

/// `ERROR_INVALID_PARAMETER`. Only used for the unreachable persona arm in
/// [`VigemBackend::plug_with_timeout`], which the guard above it makes dead.
fn windows_error_invalid_parameter() -> u32 {
    87
}

/// The plugged target, one variant per [`Persona`] ViGEmBus can emulate.
///
/// These are separate types in the vendored client with no shared trait (their
/// IOCTLs, report structs, and capabilities genuinely differ), so the enum is
/// where the difference is absorbed.
enum Target {
    X360(Xbox360Wired<Arc<Client>>),
    /// No notification IOCTL exists for a DS4 target, so this variant never has
    /// a feedback thread — [`Persona::has_feedback`] is the contract.
    Ds4(DualShock4Wired<Arc<Client>>),
}

impl Target {
    fn persona(&self) -> Persona {
        match self {
            Target::X360(_) => Persona::Xbox360,
            Target::Ds4(_) => Persona::PlayStation,
        }
    }

    fn update(&mut self, state: &PadState) -> Result<(), vigem_client::Error> {
        match self {
            Target::X360(t) => t.update(&to_xgamepad(state)),
            Target::Ds4(t) => t.update(&to_ds4report(state)),
        }
    }

    fn unplug(&mut self) -> Result<(), vigem_client::Error> {
        match self {
            Target::X360(t) => t.unplug(),
            Target::Ds4(t) => t.unplug(),
        }
    }
}

struct PadEntry {
    target: Target,
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
    /// `None` when the XInput DLL failed to load — pads still work, but slot
    /// correlation (and therefore `user_index`) is unavailable.
    xinput: Option<XInputHandle>,
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
        let xinput = match XInputHandle::load_default() {
            Ok(xi) => Some(xi),
            Err(err) => {
                tracing::warn!(?err, "XInput unavailable — slot correlation disabled");
                None
            }
        };
        Ok(Self {
            client: Arc::new(client),
            pads: BTreeMap::new(),
            next_id: 0,
            plug_timeout: DEFAULT_PLUG_TIMEOUT,
            xinput,
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
    fn plug_with_timeout(&self, persona: Persona) -> Result<Target, OutputError> {
        // Refused here as well as in `plug_persona`, because this function is
        // where a target type would have to be *chosen*: there is no ViGEmBus
        // target for a DualSense, Switch Pro or Xbox Series pad, and inventing
        // a "closest" one is precisely the silent downgrade the persona system
        // exists to prevent (see `VirtualPadBackend::plug_persona`).
        if persona.backend() != ksx_core::PadBackend::Vigem {
            return Err(OutputError::PersonaUnsupported(persona));
        }
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Both arms are the same three calls; they differ only in type, and
            // the vendored client gives them no common trait to unify over.
            let result = match persona {
                Persona::Xbox360 => {
                    let mut target = Xbox360Wired::new(client, TargetId::XBOX360_WIRED);
                    target
                        .plugin()
                        .and_then(|()| target.wait_ready())
                        .map(|()| Target::X360(target))
                }
                Persona::PlayStation => {
                    let mut target = DualShock4Wired::new(client, TargetId::DUALSHOCK4_WIRED);
                    target
                        .plugin()
                        .and_then(|()| target.wait_ready())
                        .map(|()| Target::Ds4(target))
                }
                // Unreachable: the guard above returns before the thread is
                // spawned. Stated as an explicit arm rather than a wildcard so
                // that adding a persona to ksx-core fails to compile here until
                // someone decides, in writing, which stack it belongs on.
                Persona::DualSense | Persona::SwitchPro | Persona::XboxSeries => Err(
                    vigem_client::Error::WinError(windows_error_invalid_parameter()),
                ),
            };
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

    /// Resolves which XInput slot this pad occupies by pulsing LT and watching
    /// for the echo in `XInputGetState`. Measured on ViGEmBus 1.21.442.0:
    /// `get_user_index()` returns wrong/duplicate indices and LED notifications
    /// are mostly absent (docs/research/m2-xinput-findings.md) — the read-back
    /// echo is the only slot source that matches physical reality. `None` means
    /// no echo appeared (typically: all 4 slots already occupied, or pad 5+).
    fn correlate_user_index(
        xi: &XInputHandle,
        target: &mut Xbox360Wired<Arc<Client>>,
    ) -> Option<u8> {
        let baseline: Vec<Option<u8>> = (0..4u32)
            .map(|s| xi.get_state(s).ok().map(|st| st.raw.Gamepad.bLeftTrigger))
            .collect();
        let pulse = XGamepad {
            left_trigger: CORRELATE_PULSE,
            ..XGamepad::default()
        };
        if let Err(err) = target.update(&pulse) {
            tracing::warn!(%err, "correlation pulse failed");
            return None;
        }
        let deadline = Instant::now() + CORRELATE_TIMEOUT;
        let mut found = None;
        'scan: while Instant::now() < deadline {
            for slot in 0..4u32 {
                let Ok(st) = xi.get_state(slot) else { continue };
                if st.raw.Gamepad.bLeftTrigger == CORRELATE_PULSE
                    && baseline[slot as usize] != Some(CORRELATE_PULSE)
                {
                    found = Some(slot as u8);
                    break 'scan;
                }
            }
            std::thread::sleep(CORRELATE_POLL);
        }
        if let Err(err) = target.update(&XGamepad::default()) {
            tracing::warn!(%err, "correlation clear failed");
        }
        if found.is_none() {
            tracing::warn!("no XInput echo for pad — all 4 slots occupied, or pad 5+");
        }
        found
    }

    /// Plugs one pad and registers it. Shared by every persona; the
    /// persona-specific parts are the target itself, whether XInput correlation
    /// is worth running, and whether a feedback channel exists at all.
    fn plug_inner(&mut self, persona: Persona) -> Result<PadHandle, OutputError> {
        let mut target = self.plug_with_timeout(persona)?;

        // Correlation costs up to CORRELATE_TIMEOUT and can only ever fail for a
        // pad that is not an XInput device — skipping it for HID personas saves
        // 600 ms per pad and, more importantly, suppresses a "no XInput echo"
        // warning that would be describing correct behavior.
        let user_index = match (persona.is_xinput(), &mut target) {
            (true, Target::X360(t)) => self
                .xinput
                .as_ref()
                .and_then(|xi| Self::correlate_user_index(xi, t)),
            _ => None,
        };

        let (tx, feedback_rx) = mpsc::sync_channel::<Feedback>(FEEDBACK_QUEUE_CAP);
        let dropped_feedback = Arc::new(AtomicUsize::new(0));
        let notif_thread = match &mut target {
            Target::X360(t) => {
                let notification = t.request_notification().map_err(map_target_err)?;
                let dropped = dropped_feedback.clone();
                Some(notification.spawn_thread(move |_reqn, n| {
                    let feedback = Feedback {
                        large_motor: n.large_motor,
                        small_motor: n.small_motor,
                        led_number: n.led_number,
                    };
                    if tx.try_send(feedback).is_err() {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }))
            }
            // Nothing to subscribe to: ViGEmBus has no DS4 notification IOCTL.
            // `tx` drops here, so `feedback_rx` is a permanently empty channel
            // and `poll_feedback` returns None without a special case.
            Target::Ds4(_) => None,
        };

        let id = self.next_id;
        self.next_id += 1;
        self.pads.insert(
            id,
            PadEntry {
                target,
                user_index,
                feedback_rx,
                notif_thread,
                dropped_feedback,
            },
        );
        tracing::info!(handle = id, %persona, ?user_index, "virtual pad plugged");
        Ok(PadHandle(id))
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
        self.plug_inner(Persona::Xbox360)
    }

    fn plug_persona(&mut self, persona: Persona) -> Result<PadHandle, OutputError> {
        // ViGEmBus emulates X360 and DS4 natively and nothing else — the
        // project is frozen, so it never will (docs/ENHANCEMENTS.md E1). The
        // M8 personas are refused by name here; `RoutedBackend` is what sends
        // them to HIDMaestro instead, so this is the honest floor rather than a
        // dead end.
        if persona.backend() != ksx_core::PadBackend::Vigem {
            return Err(OutputError::PersonaUnsupported(persona));
        }
        self.plug_inner(persona)
    }

    fn persona(&self, handle: PadHandle) -> Option<Persona> {
        self.pads.get(&handle.0).map(|p| p.target.persona())
    }

    fn user_index(&self, handle: PadHandle) -> Option<u8> {
        self.pads.get(&handle.0).and_then(|p| p.user_index)
    }

    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
        self.entry_mut(handle)?
            .target
            .update(state)
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
            // Verbatim EXCEPT i16::MIN, which is folded to -32767 on the way
            // out (see `to_xgamepad`): the one value a consumer cannot negate,
            // abs, or normalize by 32767 without producing nonsense.
            assert_eq!(g.thumb_lx, safe_axis(lx));
            assert_eq!(g.thumb_ly, safe_axis(ly));
            assert_eq!(g.thumb_rx, safe_axis(rx));
            assert_eq!(g.thumb_ry, safe_axis(ry));
        }
    }

    /// The wire promise, stated as a test rather than as a comment: no axis ksx
    /// submits to ViGEm is ever `i16::MIN`, whatever the preset said.
    #[test]
    fn no_axis_reaches_the_wire_as_i16_min() {
        let state = PadState {
            lx: i16::MIN,
            ly: i16::MIN,
            rx: i16::MIN,
            ry: i16::MIN,
            ..PadState::default()
        };
        let g = to_xgamepad(&state);
        for axis in [g.thumb_lx, g.thumb_ly, g.thumb_rx, g.thumb_ry] {
            assert_ne!(axis, i16::MIN);
            assert_eq!(axis, ksx_core::AXIS_MIN);
            // Full deflection is still full deflection, and now symmetric.
            assert_eq!(axis.checked_neg(), Some(i16::MAX));
        }
    }

    #[test]
    fn neutral_state_is_neutral_gamepad() {
        assert_eq!(to_xgamepad(&PadState::default()), XGamepad::default());
    }
}
