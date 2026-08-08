//! ksx-capture — per-device keyboard capture + OS-input blocking.
//!
//! Three backends behind one trait (see `docs/research/keyboard-capture-2026.md`):
//! - `interception` (M3, day-1 default): via `kanata-interception`'s raw FFI
//!   layer (the safe wrapper mis-defines E1 and allocates on the hot path —
//!   see `src/interception.rs`); ships with a 10-slot ID-drift exhaustion
//!   detector surfacing "reboot required" loudly.
//! - `winusb` (M6, strategic primary): claims the I-PAC's `MI_00` interface via
//!   in-box winusb.sys + `nusb` — blocking and identity become structural.
//! - `rawinput` (M3): identify-only device picker; NEVER a blocking backend
//!   (the RawInput+LLHOOK correlation hack is rejected by design).
//!
//! …and one that reads no hardware at all: [`replay`] plays a recorded session
//! (`ksx monitor --record`) back through the same trait, so a file drives the
//! identical pipeline a person does.
//!
//! Hot-path rule: the capture thread only receives, evaluates emergency escapes
//! ([`escape`]), decides pass/suppress from an arc-swap snapshot, re-sends what
//! must pass (byte-for-byte), and pushes into a bounded channel with `try_send`
//! (drops counted, never blocks). Zero locks; zero allocation after startup
//! except the per-event `DeviceId` clone the ksx-core `KeyEvent` contract
//! requires and cold-path hotplug bookkeeping.
//!
//! Escapes are evaluated **here**, before the pass/suppress decision, and act on
//! this thread's own passthrough latch — never through a channel. That is the
//! one property that makes them unstarvable: a wedged consumer downstream
//! (engine, output, driver IOCTL) cannot stop `LeftCtrl ×5` from freeing the
//! keyboards.
//!
//! Safety posture (this machine's keyboards are production hardware):
//! - Backends start in **passthrough** (observe-only); nothing is suppressed
//!   until `CaptureCtl::SetCaptured` arrives.
//! - Crash-only: process death needs no cleanup (driver releases filters with
//!   the context handle); a panicking thread in a living process is covered by
//!   a Drop guard that sets filters to NONE before destroying the context.
//! - A watchdog force-flips to passthrough if the event consumer stalls
//!   > 500 ms, so keystrokes reach the OS instead of a black hole.
//! - The mouse filter is never set in M3 — mouse.sys is never touched.

pub mod backend;
/// The Bluetooth half of "what input devices are attached", beside
/// [`winusb::enumerate`]'s USB half. Not `cfg(windows)`: the decision surface
/// is a pure function of an already-collected device tree, so every wording —
/// including the paired-but-disconnected case — runs in CI on any platform.
pub mod bluetooth;
pub mod composite;
pub mod decision;
pub mod escape;
pub mod exhaustion;
pub mod guard;
pub mod health;
pub mod hid;
pub mod keymap;
pub mod mock;
pub mod presence;
pub mod replay;
pub mod watchdog;

#[cfg(windows)]
mod friendly;
#[cfg(windows)]
pub mod interception;
/// Runtime `interception.dll` loading — what keeps `ksx.exe` free of a static
/// import on a DLL a migrated cabinet no longer has.
#[cfg(windows)]
pub mod interception_dll;
#[cfg(windows)]
pub mod rawinput;
#[cfg(windows)]
mod regkey;
#[cfg(windows)]
pub mod winusb;

pub use backend::{
    CaptureBackend, CaptureCtl, CaptureError, DeviceInfo, DeviceKind, ExitReason, Handles,
};
/// Named to read beside `usb_candidates`: two enumeration passes, one list.
pub use bluetooth::{candidates as bt_candidates, BtCandidate};
pub use composite::CompositeBackend;
pub use decision::{
    key_event, process_keyboard_stroke, should_resend, CaptureSet, KeySet, StrokeOutcome, Take,
};
pub use escape::{EscapeAction, EscapeHandle, EscapeStatus, EscapeWatch};
pub use exhaustion::{Exhaustion, ExhaustionDetector, MAX_KEYBOARD_SLOT};
pub use health::{CaptureHealth, HealthHandle, HealthView};
pub use mock::{MockCaptureBackend, MockStroke, ResentStroke};
pub use presence::PresenceHandle;
pub use replay::{
    RealClock, RecordedEvent, Recording, RecordingError, ReplayBackend, ReplayClock,
    ReplayProgress, Silenced, Speed, SpeedError, VirtualClock,
};
pub use watchdog::Watchdog;

#[cfg(windows)]
pub use interception::InterceptionBackend;
#[cfg(windows)]
pub use rawinput::{observe_next_key, wait_for_keypress, IdentifiedPress, ObservedKey};
#[cfg(windows)]
pub use winusb::{enumerate::candidates as usb_candidates, Binding, UsbCandidate, WinUsbBackend};
