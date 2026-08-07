//! HIDMaestro implementation of [`VirtualPadBackend`] (M8).
//!
//! Thin by design: the protocol lives in `ksx-hidmaestro` (seqlock transport,
//! lifecycle order, axis routing, keepalive, feedback decode) and this file is
//! only the adapter onto ksx's backend trait. Same split as ViGEm — protocol in
//! `vigem-client`, adapter in `vigem.rs`.
//!
//! # What this backend will and will not emulate
//!
//! Only the three personas ViGEmBus cannot express. **Xbox 360 and PlayStation
//! are refused here even though HIDMaestro could technically produce them**:
//! its XInput is a synthesis layer (the GIP companion mapping a HID device onto
//! an XInput slot) and that layer is the direct cause of its WGI double-input
//! bug. Slots 1–4 stay on the genuine `xusb22.sys` article — see
//! [`ksx_core::Persona::backend`], which is where the rule is stated once and
//! enforced by [`crate::router`].
//!
//! # Nothing here can plug a pad yet, on any machine
//!
//! Everything below is the adapter, and the adapter is finished: persona
//! refusal, axis routing, submit cadence, feedback preservation, teardown
//! order. What is missing is one level down — ksx has no code that maps
//! HIDMaestro's shared section, so the only [`HmDriverApi`] that ships
//! ([`UnavailableDriver`]) cannot create a controller.
//!
//! [`HidMaestroBackend::connect`] therefore fails **everywhere**, including on
//! a machine with HIDMaestro installed, and it fails as "not implemented in
//! this build" rather than "not installed". The distinction is the difference
//! between a true statement and an errand: nothing a user installs makes the
//! DualSense, Switch Pro or Xbox Series persona plug today. Users are kept away
//! from this path entirely by [`ksx_core::Persona::can_plug`], enforced in
//! [`crate::RoutedBackend`] before any backend is built.
//!
//! (`ksx doctor` still reports whether HIDMaestro is present, because it is
//! worth knowing. It gates nothing.)

use std::collections::BTreeMap;
use std::time::Instant;

use ksx_core::{PadState, Persona};
use ksx_hidmaestro::context::HmDriverApi;
use ksx_hidmaestro::{
    profile, Cadence, HmContext, HmError, HmGamepadState, Publish, UnavailableDriver, FRAME_BYTES,
};

use crate::backend::{Feedback, PadHandle, VirtualPadBackend};
use crate::error::OutputError;

/// Which personas this backend accepts. Derived from the single rule in
/// `ksx-core`, never restated: a persona belongs here iff its backend is
/// HIDMaestro.
fn accepts(persona: Persona) -> bool {
    persona.backend() == ksx_core::PadBackend::HidMaestro
}

struct Pad {
    persona: Persona,
    slot: ksx_hidmaestro::SlotId,
    profile: profile::HmProfile,
    cadence: Cadence<FRAME_BYTES>,
    frame: HmGamepadState,
    scratch: [u8; FRAME_BYTES],
    /// Last decoded motor state. Kept here because the Sony gate's failure mode
    /// is "preserve what was running", which needs somewhere to preserve it.
    last_feedback: Feedback,
    pending_feedback: Option<Feedback>,
}

/// HIDMaestro-backed virtual pads.
///
/// Generic over the driver so the adapter's behavior — persona refusal, submit
/// cadence, teardown — is testable with no driver installed, which on this
/// machine is the only way it is testable at all.
pub struct HidMaestroBackend<D: HmDriverApi = UnavailableDriver> {
    ctx: HmContext<D>,
    latches: BTreeMap<u32, ksx_hidmaestro::Latch<D::Storage>>,
    pads: BTreeMap<u32, Pad>,
    next_id: u32,
}

impl HidMaestroBackend<UnavailableDriver> {
    /// Starts a HIDMaestro session on this machine.
    ///
    /// Always fails today, with [`ksx_hidmaestro::HmError::NotImplemented`]
    /// wrapped in [`OutputError::HidMaestro`] — see the module docs for why
    /// that is not [`OutputError::HidMaestroMissing`].
    pub fn connect() -> Result<Self, OutputError> {
        Self::with_driver(UnavailableDriver::new())
    }
}

impl<D: HmDriverApi> HidMaestroBackend<D> {
    /// Runs the documented lifecycle (sweep → catalog → install) against any
    /// driver implementation.
    pub fn with_driver(driver: D) -> Result<Self, OutputError> {
        let mut ctx = HmContext::new(driver);
        ctx.start().map_err(map_hm_err)?;
        tracing::info!(profiles = ctx.profile_count(), "HIDMaestro context started");
        Ok(Self {
            ctx,
            latches: BTreeMap::new(),
            pads: BTreeMap::new(),
            next_id: 0,
        })
    }

    fn pad_mut(&mut self, handle: PadHandle) -> Result<&mut Pad, OutputError> {
        self.pads
            .get_mut(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))
    }

    /// Feeds a decoded output packet in, so `poll_feedback` can hand it out.
    ///
    /// Separate from the decode table itself (which is pure, in
    /// `ksx-hidmaestro::feedback`): this is only the *preserve* half — a gate
    /// failure keeps the previous motors instead of zeroing them.
    pub fn ingest_feedback(&mut self, handle: PadHandle, decoded: ksx_hidmaestro::Decoded) {
        let Some(pad) = self.pads.get_mut(&handle.0) else {
            return;
        };
        let next = match decoded {
            ksx_hidmaestro::Decoded::Motors(m) => {
                let (large, small) = m.as_u8();
                Feedback {
                    large_motor: large,
                    small_motor: small,
                    led_number: pad.last_feedback.led_number,
                }
            }
            // "Ignore", not "stop": a lightbar-only report must not stutter a
            // running rumble.
            ksx_hidmaestro::Decoded::Preserve => pad.last_feedback,
            ksx_hidmaestro::Decoded::Ignored => return,
        };
        pad.last_feedback = next;
        pad.pending_feedback = Some(next);
    }
}

fn map_hm_err(err: HmError) -> OutputError {
    match err {
        HmError::NotInstalled { probe } => OutputError::HidMaestroMissing {
            probe: probe.to_string(),
        },
        other => OutputError::HidMaestro(other),
    }
}

impl<D: HmDriverApi> VirtualPadBackend for HidMaestroBackend<D>
where
    D: Send,
    D::Storage: Send,
{
    fn plug(&mut self) -> Result<PadHandle, OutputError> {
        // The trait's `plug` means Xbox 360, which is exactly what this backend
        // must never produce.
        Err(OutputError::PersonaUnsupported(Persona::Xbox360))
    }

    fn plug_persona(&mut self, persona: Persona) -> Result<PadHandle, OutputError> {
        if !accepts(persona) {
            return Err(OutputError::PersonaUnsupported(persona));
        }
        let profile =
            profile::expected_profile(persona).ok_or(OutputError::PersonaUnsupported(persona))?;
        let (slot, latch) = self.ctx.create_controller(&profile).map_err(map_hm_err)?;

        let id = self.next_id;
        self.next_id += 1;
        self.latches.insert(id, latch);
        self.pads.insert(
            id,
            Pad {
                persona,
                slot,
                profile,
                cadence: Cadence::new(),
                frame: HmGamepadState::new(),
                scratch: [0; FRAME_BYTES],
                last_feedback: Feedback::default(),
                pending_feedback: None,
            },
        );
        tracing::info!(handle = id, %persona, slot, "HIDMaestro pad plugged");
        Ok(PadHandle(id))
    }

    fn persona(&self, handle: PadHandle) -> Option<Persona> {
        self.pads.get(&handle.0).map(|p| p.persona)
    }

    fn user_index(&self, _handle: PadHandle) -> Option<u8> {
        // Not resolved here. The Xbox Series persona *does* land on an XInput
        // slot through the GIP companion, but ksx's slot identity comes from
        // active LT-pulse correlation (docs/research/m2-xinput-findings.md) and
        // that has never been run against a synthesized slot. Returning `None`
        // says "unknown", which is true; returning a guess would be worse than
        // useless, because callers print it as fact.
        None
    }

    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
        let now = Instant::now();
        let pad = self.pad_mut(handle)?;
        // Route by name through the profile's map — never positionally.
        ksx_hidmaestro::route(&pad.profile.axis_map, state, &mut pad.frame);
        pad.frame.encode(&mut pad.scratch);
        let decision = pad.cadence.take(&pad.scratch, now);
        if decision == Publish::Skip {
            return Ok(());
        }
        let frame = pad.scratch;
        let latch = self
            .latches
            .get(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))?;
        latch.publish(&frame);
        Ok(())
    }

    fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
        self.pads.get_mut(&handle.0)?.pending_feedback.take()
    }

    fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
        let pad = self
            .pads
            .remove(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))?;
        self.latches.remove(&handle.0);
        self.ctx.destroy_controller(pad.slot).map_err(map_hm_err)
    }
}

impl<D: HmDriverApi> Drop for HidMaestroBackend<D> {
    fn drop(&mut self) {
        // `HmContext`'s own Drop sweeps, but doing it here too keeps the
        // "dropping the backend unplugs every pad" contract explicit and
        // ordered: latches (and the views behind them) go before the devices
        // they point into.
        self.latches.clear();
        if let Err(err) = self.ctx.shutdown() {
            tracing::warn!(%err, "HIDMaestro shutdown during backend drop failed");
        }
    }
}

/// Keeps the storage bound honest: a latch handed across the output thread
/// boundary has to be `Send`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ksx_hidmaestro::HeapStorage>();
};

#[cfg(test)]
mod tests {
    use ksx_core::pad::XButtons;
    use ksx_hidmaestro::seqlock::{HeapStorage, Latch};
    use ksx_hidmaestro::{HmProfile, SlotId};

    use super::*;

    /// A driver double that succeeds. It emulates nothing — it hands out heap
    /// latches — so it can only ever test *this adapter's* behavior, never the
    /// driver's. That distinction is the point: with no HIDMaestro installed,
    /// everything below is adapter logic, and nothing here should be read as
    /// evidence that a real pad works.
    #[derive(Default)]
    struct FakeDriver {
        created: Vec<SlotId>,
        destroyed: Vec<SlotId>,
    }

    impl HmDriverApi for FakeDriver {
        type Storage = HeapStorage;
        fn remove_all_virtual_controllers(&mut self) -> Result<usize, HmError> {
            Ok(0)
        }
        fn load_default_profiles(&mut self) -> Result<usize, HmError> {
            Ok(228)
        }
        fn install_driver(&mut self) -> Result<(), HmError> {
            Ok(())
        }
        fn publish_pid_pool(&mut self, _: SlotId, _: &HmProfile) -> Result<(), HmError> {
            Ok(())
        }
        fn create_controller(
            &mut self,
            slot: SlotId,
            _: &HmProfile,
        ) -> Result<Latch<Self::Storage>, HmError> {
            self.created.push(slot);
            Latch::new(HeapStorage::new(FRAME_BYTES), FRAME_BYTES)
        }
        fn park_feedback_index(&mut self, _: SlotId, _: i32) -> Result<(), HmError> {
            Ok(())
        }
        fn dispose_dispatchers(&mut self, _: SlotId) -> Result<(), HmError> {
            Ok(())
        }
        fn destroy_controller(&mut self, slot: SlotId) -> Result<(), HmError> {
            self.destroyed.push(slot);
            Ok(())
        }
    }

    fn backend() -> HidMaestroBackend<FakeDriver> {
        HidMaestroBackend::with_driver(FakeDriver::default()).unwrap()
    }

    #[test]
    fn it_accepts_exactly_the_personas_vigem_cannot_do() {
        let mut b = backend();
        for persona in [Persona::DualSense, Persona::SwitchPro, Persona::XboxSeries] {
            let h = b
                .plug_persona(persona)
                .unwrap_or_else(|e| panic!("{persona}: {e}"));
            assert_eq!(b.persona(h), Some(persona));
        }
    }

    /// The migration rule, enforced at the backend boundary: even with a
    /// working driver, this backend refuses the two personas that belong on
    /// ViGEmBus. A silent acceptance here is how slots 1–4 would drift onto a
    /// synthesis layer.
    #[test]
    fn it_refuses_xbox360_and_playstation_even_when_the_driver_works() {
        let mut b = backend();
        for persona in [Persona::Xbox360, Persona::PlayStation] {
            let err = b.plug_persona(persona).unwrap_err();
            assert!(
                matches!(err, OutputError::PersonaUnsupported(p) if p == persona),
                "{persona}: {err}"
            );
        }
        // ...including through the personaless `plug`, which means Xbox 360.
        assert!(matches!(
            b.plug().unwrap_err(),
            OutputError::PersonaUnsupported(Persona::Xbox360)
        ));
    }

    #[test]
    fn update_publishes_through_the_cadence_not_on_every_call() {
        let mut b = backend();
        let h = b.plug_persona(Persona::DualSense).unwrap();
        let neutral = PadState::default();
        b.update(h, &neutral).unwrap();
        let after_first = b.latches[&h.0].seq();
        assert_eq!(after_first, 2);
        // Immediately repeating the identical frame is elided.
        b.update(h, &neutral).unwrap();
        b.update(h, &neutral).unwrap();
        assert_eq!(
            b.latches[&h.0].seq(),
            after_first,
            "idle frames are deduped"
        );
        // A change goes out at once.
        b.update(
            h,
            &PadState {
                buttons: XButtons::A,
                ..PadState::default()
            },
        )
        .unwrap();
        assert_eq!(b.latches[&h.0].seq(), after_first + 2);
    }

    #[test]
    fn a_preserve_verdict_keeps_the_previous_motors_instead_of_zeroing() {
        use ksx_hidmaestro::{Decoded, Motors};
        let mut b = backend();
        let h = b.plug_persona(Persona::DualSense).unwrap();
        b.ingest_feedback(
            h,
            Decoded::Motors(Motors {
                large: 0xAA00,
                small: 0xBB00,
                ..Motors::default()
            }),
        );
        let first = b.poll_feedback(h).unwrap();
        assert_eq!((first.large_motor, first.small_motor), (0xAA, 0xBB));
        // A lightbar-only report arrives: same motors, not silence.
        b.ingest_feedback(h, Decoded::Preserve);
        let second = b.poll_feedback(h).unwrap();
        assert_eq!(second, first);
        // Nothing queued means nothing handed out.
        assert_eq!(b.poll_feedback(h), None);
        // A packet that is not feedback at all queues nothing.
        b.ingest_feedback(h, Decoded::Ignored);
        assert_eq!(b.poll_feedback(h), None);
    }

    #[test]
    fn unplug_kills_the_handle_and_destroys_the_controller() {
        let mut b = backend();
        let h = b.plug_persona(Persona::SwitchPro).unwrap();
        b.unplug(h).unwrap();
        assert!(matches!(
            b.update(h, &PadState::default()).unwrap_err(),
            OutputError::UnknownHandle(_)
        ));
        assert!(matches!(
            b.unplug(h).unwrap_err(),
            OutputError::UnknownHandle(_)
        ));
        assert_eq!(b.ctx.driver_mut().destroyed, vec![0]);
    }

    #[test]
    fn handles_are_never_reused() {
        let mut b = backend();
        let a = b.plug_persona(Persona::DualSense).unwrap();
        b.unplug(a).unwrap();
        let c = b.plug_persona(Persona::DualSense).unwrap();
        assert_ne!(a.raw(), c.raw());
    }

    #[test]
    fn user_index_is_unknown_rather_than_guessed() {
        let mut b = backend();
        let h = b.plug_persona(Persona::XboxSeries).unwrap();
        // XboxSeries IS an XInput persona, but we have never correlated a
        // synthesized slot, so the honest answer is None.
        assert!(Persona::XboxSeries.is_xinput());
        assert_eq!(b.user_index(h), None);
    }

    /// `connect()` must fail on **every** machine, and blame the right thing.
    ///
    /// It used to be written as "unless someone installs HIDMaestro, in which
    /// case this test has nothing to say" — and that escape hatch is exactly
    /// the hole: with the driver installed the old code still refused, with the
    /// still-wrong reason, and no test noticed.
    #[test]
    fn connect_refuses_on_every_machine_and_blames_the_build_not_the_driver() {
        let err = HidMaestroBackend::connect()
            .err()
            .expect("no build of ksx can start a HIDMaestro session");
        assert!(err.is_not_implemented(), "{err}");
        assert!(
            !err.is_hidmaestro_missing(),
            "{err} would send the user to install a driver that cannot help"
        );
        let msg = err.to_string();
        assert!(msg.contains("not implemented in this build"), "{msg}");
        assert!(msg.contains("on any machine, installed or not"), "{msg}");
    }
}
