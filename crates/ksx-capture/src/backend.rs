//! The `CaptureBackend` trait and its shared vocabulary types.
//!
//! Adapted from `docs/research/design-architecture.md` §3.2 to what M4 actually
//! consumes: `run` owns its capture thread and returns the `JoinHandle`, control
//! flows in through a channel (no `&mut self` after start), and health is read
//! through a cloneable atomic handle that stays valid after `run` consumed the
//! backend.

use crossbeam_channel::{Receiver, Sender};
use ksx_core::{DeviceId, KeyEvent};

use crate::escape::EscapeHandle;
use crate::health::HealthHandle;
use crate::presence::PresenceHandle;

/// What class of input device a capture slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
}

/// One enumerated input device as a capture backend sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Backend-scoped identity. For [`crate::interception::InterceptionBackend`]
    /// this is the Interception *hardware id* string (e.g.
    /// `HID\VID_D209&PID_0430&REV_0056&MI_00`) — the exact value legacy
    /// KeyboardSplitter persisted in `splitter_games.xml`. NOTE: two identical
    /// boards share this id (risk review R2); the RawInput instance-path
    /// correlation that disambiguates them is M4's job.
    pub id: DeviceId,
    /// The Interception device slot (1..=10 keyboards, 11..=20 mice) this device
    /// currently occupies. `None` for backends without slot semantics. Positional
    /// and unstable across replug — never persist it (legacy's `Keyboard_01`
    /// strong names were exactly this mistake).
    pub interception_slot: Option<u8>,
    /// Human-readable name, best effort from the registry Enum tree. `None` when
    /// the lookup fails (legacy showed "n/a").
    pub friendly: Option<String>,
    pub kind: DeviceKind,
}

/// Control messages consumed by a running capture thread.
///
/// Backends start in **passthrough** (observe-only) mode: every stroke is both
/// reported and re-sent to the OS. Nothing is ever suppressed until the first
/// `SetCaptured` arrives — the safe default for a machine whose keyboards are
/// production hardware.
#[derive(Clone, Debug)]
pub enum CaptureCtl {
    /// Enter capturing mode: strokes from these devices are suppressed from the
    /// OS (swallowed, still reported); strokes from every other device are
    /// re-sent verbatim. Captured == blocked, per the legacy blocking-scope rule
    /// (only assigned devices are ever blocked, risk review §3 item 3).
    SetCaptured(Vec<DeviceId>),
    /// Enter observe-only mode: report AND re-send everything.
    SetPassthrough,
    /// Leave the loop; the drop guard resets the driver filter on the way out.
    Shutdown,
}

/// Why a capture thread returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// `CaptureCtl::Shutdown` was received.
    Shutdown,
    /// The `KeyEvent` receiver was dropped — the consumer is gone, so keeping
    /// the filter open would black-hole keyboards. The guard has reset it.
    ChannelClosed,
    /// Mock only: the scripted stroke source ran dry and the control channel
    /// disconnected.
    ScriptExhausted,
    /// The loop body panicked. The panic was caught, health flagged, and the
    /// drop guard has already reset the driver filter — keyboards keep working.
    Panicked,
    /// The captured device physically went away (M6 WinUSB: the endpoint
    /// reported a disconnect). Distinct from `Shutdown` because nobody asked
    /// for it and the session cannot continue on that board — and distinct
    /// from `Panicked` because nothing is wrong with ksx. The backend has
    /// already released every key it was holding.
    DeviceLost,
}

/// The observation handles a backend publishes into.
///
/// Normally a backend makes its own (`Handles::new()`), and nothing else needs
/// this type. It exists so several backends can be handed the **same** handles
/// and add up to one session: since M6 a run can be driving a WinUSB thread per
/// claimed board alongside an Interception thread, and the supervisor must see
/// one health state and — critically — one escape latch, not three
/// (`crate::composite`, `crate::escape::EscapeWatch`).
///
/// Presence is deliberately *not* here: health and escapes merge by OR-ing and
/// summing, but two backends publishing device lists into one slot would
/// clobber each other. [`crate::CompositeBackend`] merges those separately.
#[derive(Clone, Debug, Default)]
pub struct Handles {
    pub health: HealthHandle,
    pub escapes: EscapeHandle,
}

impl Handles {
    pub fn new() -> Self {
        Self::default()
    }
}

/// A source of per-device key events that can also suppress them from the OS.
///
/// Object-safe and `Send`: `ksx-app` holds a `Box<dyn CaptureBackend>` chosen at
/// startup (interception / mock / winusb in M6).
pub trait CaptureBackend: Send {
    /// Enumerate devices this backend can currently see. Cold path; may allocate
    /// and touch the registry.
    fn devices(&mut self) -> Vec<DeviceInfo>;

    /// Cloneable handle to this backend's health flags (slot exhaustion,
    /// watchdog trips, drop counters). Grab it before `run` — it stays valid
    /// and live afterwards.
    fn health(&self) -> HealthHandle;

    /// Cloneable handle to the emergency escapes this backend detects. Grab it
    /// before `run`, like [`Self::health`].
    ///
    /// Deliberately **not** defaulted: escape detection is the lockout escape
    /// hatch, so a new backend has to state what it does about it rather than
    /// silently inheriting "never fires". A backend that genuinely cannot see
    /// strokes may return a fresh [`EscapeHandle`] — but it must not set any
    /// class filter either.
    fn escapes(&self) -> EscapeHandle;

    /// Cloneable handle to the devices this backend can currently see, kept
    /// live by the running capture thread (cold path — republished only while
    /// the driver is idle). Grab it before `run`, like [`Self::health`].
    ///
    /// The default is [`PresenceHandle::unsupported`]: a backend with no
    /// hotplug visibility reports `None` forever and supervisors degrade to
    /// never invalidating a slot, which is strictly safer than guessing.
    fn presence(&self) -> PresenceHandle {
        PresenceHandle::unsupported()
    }

    /// Consume the backend and start its capture thread. Events flow out `tx`
    /// (bounded; the thread never blocks on it — overflow is counted, never
    /// waited on), control flows in `ctl`.
    ///
    /// The returned handle joins to an [`ExitReason`]. Implementations guarantee
    /// crash safety: panic or normal exit both reset any OS-level filter before
    /// the thread dies (Drop-based guard), and process death needs no cleanup at
    /// all — the driver releases filters when the context handle closes.
    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>>;
}

/// Errors constructing or driving a backend.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Interception driver unavailable (interception_create_context returned null) — driver not installed or not loaded")]
    DriverUnavailable,
    #[error("{context} failed (win32 error {code})")]
    Os { context: &'static str, code: u32 },
    /// The WinUSB rebind has not been performed for this interface, so there
    /// is no `winusb.sys` to claim through. Never auto-fixed: rebinding is a
    /// supervised manual step (`docs/RECOVERY.md` §2).
    #[error(
        "{id} is bound to {bound}, not winusb.sys — the WinUSB rebind has not been performed          for this interface (see docs/WINUSB.md; ksx never rebinds a device itself)"
    )]
    NotRebound { id: String, bound: String },
    /// Enumeration found nothing matching the configured device id.
    #[error("no USB interface with instance path {id} is connected")]
    NoSuchDevice { id: String },
    /// The interface was claimed but does not describe a keyboard.
    #[error("{id} delivered a HID report descriptor with no keyboard fields — wrong interface?")]
    NotAKeyboard { id: String },
    /// No interrupt-IN endpoint: nothing to read reports from.
    #[error("{id} has no interrupt IN endpoint")]
    NoInterruptEndpoint { id: String },
    /// `nusb` failed to open/claim/transfer.
    #[error("{context}: {source}")]
    Usb {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
}

// Compile-time proof the trait stays object-safe (the whole point of it).
const _: () = {
    #[allow(dead_code)]
    fn assert_object_safe(_: &mut dyn CaptureBackend) {}
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}
    #[allow(dead_code)]
    fn check() {
        assert_send::<Box<dyn CaptureBackend>>();
    }
};
