//! The `VirtualPadBackend` trait — design §3.2, the plan-B insurance policy.

use ksx_core::{PadState, Persona};

use crate::error::OutputError;

/// Opaque handle to a plugged virtual pad.
///
/// Handle ids are monotonically increasing and never reused within a backend's
/// lifetime, so a handle kept across an `unplug` can never alias a later pad —
/// stale handles always fail with [`OutputError::UnknownHandle`].
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct PadHandle(pub(crate) u32);

impl PadHandle {
    /// Raw id, for logging / `--json` output only.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Feedback pushed down by the XUSB stack: rumble motors + LED.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub struct Feedback {
    /// Low-frequency (left) motor, 0..=255.
    pub large_motor: u8,
    /// High-frequency (right) motor, 0..=255.
    pub small_motor: u8,
    /// Raw LED value as delivered by the bus. Interpretation (player index
    /// mapping) is verified on the cabinet by `tests/loopback.rs` before the
    /// engine relies on it; until then [`VirtualPadBackend::user_index`] is the
    /// slot-mapping source of truth.
    pub led_number: u8,
}

/// A backend that materializes virtual game pads.
///
/// Object-safe by design: the engine holds `Box<dyn VirtualPadBackend>` so a
/// plan-B backend (HIDMaestro, libvirtualhid) is a drop-in.
///
/// Contract:
/// - `plug` returns a handle to a ready-to-update pad (the ViGEm impl waits for
///   device readiness before returning).
/// - Dropping the backend must unplug every pad it still owns.
/// - `poll_feedback` never blocks; it drains one queued [`Feedback`] per call.
pub trait VirtualPadBackend: Send {
    /// Plugs a new virtual pad and waits until it accepts updates.
    ///
    /// Equivalent to [`plug_persona`](Self::plug_persona) with
    /// [`Persona::Xbox360`] — the persona every caller wanted before personas
    /// existed.
    fn plug(&mut self) -> Result<PadHandle, OutputError>;

    /// Plugs a new virtual pad presenting itself as `persona`.
    ///
    /// The default implementation supports **only** [`Persona::Xbox360`] and
    /// rejects everything else with [`OutputError::PersonaUnsupported`]. That is
    /// deliberate: a backend that silently downgraded an unimplemented persona
    /// to an Xbox pad would hand the user a working controller showing the wrong
    /// button prompts — the exact failure a persona exists to prevent, and one
    /// that looks like success. Backends override this to declare what they can
    /// really emulate.
    fn plug_persona(&mut self, persona: Persona) -> Result<PadHandle, OutputError> {
        match persona {
            Persona::Xbox360 => self.plug(),
            other => Err(OutputError::PersonaUnsupported(other)),
        }
    }

    /// The persona a plugged pad was created with, or `None` for a dead handle.
    fn persona(&self, handle: PadHandle) -> Option<Persona>;

    /// XInput `dwUserIndex` (0..=3) of the pad, if it got an XInput slot.
    ///
    /// `None` for pads beyond the 4 XInput slots (they still exist for
    /// DirectInput) or before the slot is known.
    fn user_index(&self, handle: PadHandle) -> Option<u8>;

    /// Submits a new pad state.
    ///
    /// [`PadState`] is in XInput wire shape, so an Xbox pad copies it and any
    /// other persona translates (see `ds4.rs`).
    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError>;

    /// Non-blocking: next queued rumble/LED feedback for this pad, if any.
    ///
    /// Always `None` for personas where [`Persona::has_feedback`] is false —
    /// ViGEmBus exposes no notification channel for those targets at all.
    fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback>;

    /// Unplugs the pad. The handle is dead afterwards.
    fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError>;
}

// Object-safety is contractual; break it and this fails to compile.
const _: Option<&'static dyn VirtualPadBackend> = None;
