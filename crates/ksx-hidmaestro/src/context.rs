//! Lifecycle — and the order is the whole content of this module.
//!
//! Every step below is ordered against a failure PadForge hit first
//! (`padforge-code-audit.md` §3.2). Doing them in a "sensible" order instead
//! fails in ways that look like unrelated bugs:
//!
//! 1. **Sweep before install.** `RemoveAllVirtualControllers()` first, to clear
//!    device nodes left by a crashed session. Skip it and `InstallDriver`'s
//!    `RemoveOldDriverPackages` fails with *"device using INF"* — an error that
//!    names the INF, not the stale pad, so it reads as a packaging problem.
//! 2. **Load profiles, then install the driver.** `InstallDriver` needs
//!    elevation; PadForge dodges this by always running elevated. ksx does not,
//!    so [`HmError::ElevationRequired`] is a first-class outcome.
//! 3. **PID pool published before enumeration.** If the descriptor carries a
//!    PID block, the FFB pool/state must exist *before* the device enumerates:
//!    DirectInput issues `GetFeature(PidPool)` during `CreateEffect`, so lazy
//!    init on the first output packet is already too late.
//! 4. **Teardown in reverse, with the feedback index parked first.** Park at -1
//!    before dispose so late async callbacks no-op, dispose dispatchers before
//!    the controller (final `OutputReceived` events fire *during* dispose), and
//!    sweep again on process exit as crash insurance.
//!
//! The driver surface is behind [`HmDriverApi`] so this ordering is testable
//! with no driver present — which, on a machine without HIDMaestro, is the only
//! way it can be tested at all.

use crate::error::HmError;
use crate::profile::HmProfile;
use crate::seqlock::{Latch, LatchStorage};

/// A live controller's identity within a context.
pub type SlotId = u32;

/// The feedback index parked before teardown so late callbacks no-op.
pub const PARKED_FEEDBACK_INDEX: i32 = -1;

/// The driver operations the lifecycle needs, in the vocabulary HIDMaestro's
/// SDK uses.
///
/// A trait rather than direct calls for one reason that matters more than
/// testing: it is the seam where "we have no driver" lives. The production
/// implementation ([`crate::driver::UnavailableDriver`] today) refuses at
/// [`HmDriverApi::install_driver`] with the probe evidence attached.
pub trait HmDriverApi {
    /// The latch storage this driver hands out. The *driver* owns the shared
    /// section, so the transport type belongs to the driver and not to the
    /// context: a real implementation yields `shm::MappedStorage`, a test
    /// double yields `HeapStorage`, and the seqlock discipline above them is
    /// the same code either way.
    type Storage: LatchStorage;

    /// Sweep stale device nodes. Returns how many were removed.
    fn remove_all_virtual_controllers(&mut self) -> Result<usize, HmError>;
    /// Load the embedded profile catalog. Returns the profile count.
    fn load_default_profiles(&mut self) -> Result<usize, HmError>;
    /// Install/refresh the UMDF driver package. Requires elevation.
    fn install_driver(&mut self) -> Result<(), HmError>;
    /// Publish the PID FFB pool for a slot. Must precede `create_controller`
    /// for any PID-carrying descriptor.
    fn publish_pid_pool(&mut self, slot: SlotId, profile: &HmProfile) -> Result<(), HmError>;
    /// Create the virtual device (this is where it enumerates) and return the
    /// latch its state is published into.
    fn create_controller(
        &mut self,
        slot: SlotId,
        profile: &HmProfile,
    ) -> Result<Latch<Self::Storage>, HmError>;
    /// Park the slot's feedback index so late async callbacks no-op.
    fn park_feedback_index(&mut self, slot: SlotId, index: i32) -> Result<(), HmError>;
    /// Dispose the slot's event dispatchers, before the controller itself.
    fn dispose_dispatchers(&mut self, slot: SlotId) -> Result<(), HmError>;
    /// Destroy the virtual device.
    fn destroy_controller(&mut self, slot: SlotId) -> Result<(), HmError>;
}

/// Lifecycle phase, so an out-of-order call is a named error rather than a
/// mysterious driver failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    New,
    Started,
    ShutDown,
}

/// A HIDMaestro session: sweep, install, then N controllers.
///
/// PadForge runs two contexts — a metadata-only one for the UI catalog and a
/// live one in the engine. ksx has no catalog browser yet, so there is one
/// context and it is the live one. If a Studio profile picker ever lands, it
/// gets its own context that never installs and never creates.
pub struct HmContext<D: HmDriverApi> {
    driver: D,
    phase: Phase,
    profile_count: usize,
    live: Vec<SlotId>,
    next_slot: SlotId,
}

impl<D: HmDriverApi> HmContext<D> {
    pub fn new(driver: D) -> Self {
        Self {
            driver,
            phase: Phase::New,
            profile_count: 0,
            live: Vec::new(),
            next_slot: 0,
        }
    }

    /// Steps 1–3: sweep, load catalog, install driver — in that order.
    ///
    /// The sweep runs **before** anything else, including before we know
    /// whether the driver is installed at all: a stale node from a crashed
    /// session is exactly the state where install is about to fail, so
    /// discovering "not installed" first and returning early would leave the
    /// machine in the broken state it was already in.
    pub fn start(&mut self) -> Result<(), HmError> {
        if self.phase != Phase::New {
            return Ok(());
        }
        let swept = self.driver.remove_all_virtual_controllers()?;
        if swept > 0 {
            tracing::warn!(swept, "swept stale HIDMaestro controllers from a prior run");
        }
        self.profile_count = self.driver.load_default_profiles()?;
        self.driver.install_driver()?;
        self.phase = Phase::Started;
        tracing::info!(profiles = self.profile_count, "HIDMaestro context started");
        Ok(())
    }

    pub fn profile_count(&self) -> usize {
        self.profile_count
    }

    pub fn live_slots(&self) -> &[SlotId] {
        &self.live
    }

    /// Step 3: create one controller, publishing its PID pool first when the
    /// descriptor carries a PID block. Returns the slot and its latch.
    pub fn create_controller(
        &mut self,
        profile: &HmProfile,
    ) -> Result<(SlotId, Latch<D::Storage>), HmError> {
        if self.phase != Phase::Started {
            return Err(HmError::DeadSlot(self.next_slot));
        }
        profile.validate()?;
        let slot = self.next_slot;
        if profile.has_pid_block() {
            // BEFORE create_controller. DirectInput's GetFeature(PidPool)
            // arrives during CreateEffect, which can happen the instant the
            // device enumerates.
            self.driver.publish_pid_pool(slot, profile)?;
        }
        let latch = self.driver.create_controller(slot, profile)?;
        self.next_slot += 1;
        self.live.push(slot);
        tracing::info!(slot, profile = %profile.slug, "HIDMaestro controller created");
        Ok((slot, latch))
    }

    /// Step 4, for one slot: park → dispose dispatchers → destroy.
    pub fn destroy_controller(&mut self, slot: SlotId) -> Result<(), HmError> {
        let Some(at) = self.live.iter().position(|s| *s == slot) else {
            return Err(HmError::DeadSlot(slot));
        };
        // Park first: from here on, a callback that was already in flight finds
        // an index of -1 and does nothing, instead of writing into a slot that
        // is being torn down.
        self.driver
            .park_feedback_index(slot, PARKED_FEEDBACK_INDEX)?;
        self.driver.dispose_dispatchers(slot)?;
        self.driver.destroy_controller(slot)?;
        self.live.remove(at);
        Ok(())
    }

    /// Full teardown: every controller, then a final sweep as crash insurance
    /// (PadForge hooks `ProcessExit` for the same reason — a killed process
    /// runs no destructors and leaves device nodes behind).
    pub fn shutdown(&mut self) -> Result<(), HmError> {
        if self.phase == Phase::ShutDown {
            return Ok(());
        }
        let mut first_err = None;
        for slot in std::mem::take(&mut self.live) {
            let step = self
                .driver
                .park_feedback_index(slot, PARKED_FEEDBACK_INDEX)
                .and_then(|()| self.driver.dispose_dispatchers(slot))
                .and_then(|()| self.driver.destroy_controller(slot));
            if let Err(err) = step {
                tracing::warn!(slot, %err, "HIDMaestro teardown step failed");
                first_err.get_or_insert(err);
            }
        }
        if let Err(err) = self.driver.remove_all_virtual_controllers() {
            first_err.get_or_insert(err);
        }
        self.phase = Phase::ShutDown;
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    pub fn driver_mut(&mut self) -> &mut D {
        &mut self.driver
    }
}

impl<D: HmDriverApi> Drop for HmContext<D> {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use ksx_core::Persona;

    use super::*;
    use crate::profile::{expected_profile, stub_descriptor};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        Sweep,
        LoadProfiles,
        Install,
        PublishPidPool(SlotId),
        Create(SlotId),
        Park(SlotId, i32),
        Dispose(SlotId),
        Destroy(SlotId),
    }

    #[derive(Default)]
    struct Recorder {
        calls: Vec<Call>,
        stale_nodes: usize,
        install_fails: bool,
    }

    fn heap_latch() -> Latch<crate::seqlock::HeapStorage> {
        Latch::new(
            crate::seqlock::HeapStorage::new(crate::state::FRAME_BYTES),
            crate::state::FRAME_BYTES,
        )
        .unwrap()
    }

    impl HmDriverApi for Recorder {
        type Storage = crate::seqlock::HeapStorage;

        fn remove_all_virtual_controllers(&mut self) -> Result<usize, HmError> {
            self.calls.push(Call::Sweep);
            Ok(std::mem::take(&mut self.stale_nodes))
        }
        fn load_default_profiles(&mut self) -> Result<usize, HmError> {
            self.calls.push(Call::LoadProfiles);
            Ok(228)
        }
        fn install_driver(&mut self) -> Result<(), HmError> {
            self.calls.push(Call::Install);
            if self.install_fails {
                return Err(HmError::ElevationRequired);
            }
            Ok(())
        }
        fn publish_pid_pool(&mut self, slot: SlotId, _p: &HmProfile) -> Result<(), HmError> {
            self.calls.push(Call::PublishPidPool(slot));
            Ok(())
        }
        fn create_controller(
            &mut self,
            slot: SlotId,
            _p: &HmProfile,
        ) -> Result<Latch<Self::Storage>, HmError> {
            self.calls.push(Call::Create(slot));
            Ok(heap_latch())
        }
        fn park_feedback_index(&mut self, slot: SlotId, index: i32) -> Result<(), HmError> {
            self.calls.push(Call::Park(slot, index));
            Ok(())
        }
        fn dispose_dispatchers(&mut self, slot: SlotId) -> Result<(), HmError> {
            self.calls.push(Call::Dispose(slot));
            Ok(())
        }
        fn destroy_controller(&mut self, slot: SlotId) -> Result<(), HmError> {
            self.calls.push(Call::Destroy(slot));
            Ok(())
        }
    }

    fn ds5() -> HmProfile {
        expected_profile(Persona::DualSense).unwrap()
    }

    /// The headline ordering test: sweep FIRST, then catalog, then install.
    #[test]
    fn the_stale_node_sweep_precedes_install_or_install_fails_with_device_using_inf() {
        let mut ctx = HmContext::new(Recorder {
            stale_nodes: 3,
            ..Recorder::default()
        });
        ctx.start().unwrap();
        assert_eq!(
            ctx.driver_mut().calls,
            vec![Call::Sweep, Call::LoadProfiles, Call::Install]
        );
        assert_eq!(ctx.profile_count(), 228);
    }

    #[test]
    fn the_pid_pool_is_published_before_the_device_enumerates() {
        let mut ctx = HmContext::new(Recorder::default());
        ctx.start().unwrap();
        let (slot, _latch) = ctx.create_controller(&ds5()).unwrap();
        let calls = &ctx.driver_mut().calls;
        let pool = calls
            .iter()
            .position(|c| *c == Call::PublishPidPool(slot))
            .expect("pool published");
        let create = calls
            .iter()
            .position(|c| *c == Call::Create(slot))
            .expect("controller created");
        assert!(
            pool < create,
            "GetFeature(PidPool) arrives during CreateEffect — lazy init is too late"
        );
    }

    #[test]
    fn a_profile_without_a_pid_block_skips_the_pool_step() {
        let mut profile = ds5();
        profile.descriptor = stub_descriptor(false);
        assert!(!profile.has_pid_block());
        let mut ctx = HmContext::new(Recorder::default());
        ctx.start().unwrap();
        let (slot, _latch) = ctx.create_controller(&profile).unwrap();
        assert!(!ctx.driver_mut().calls.contains(&Call::PublishPidPool(slot)));
    }

    #[test]
    fn teardown_parks_the_feedback_index_before_disposing_anything() {
        let mut ctx = HmContext::new(Recorder::default());
        ctx.start().unwrap();
        let (slot, _latch) = ctx.create_controller(&ds5()).unwrap();
        ctx.destroy_controller(slot).unwrap();
        let calls = ctx.driver_mut().calls.clone();
        let tail: Vec<_> = calls
            .iter()
            .skip_while(|c| !matches!(c, Call::Park(..)))
            .cloned()
            .collect();
        assert_eq!(
            tail,
            vec![
                Call::Park(slot, PARKED_FEEDBACK_INDEX),
                Call::Dispose(slot),
                Call::Destroy(slot),
            ],
            "park -> dispose dispatchers -> destroy; final OutputReceived events \
             fire DURING dispose"
        );
        assert!(ctx.live_slots().is_empty());
    }

    #[test]
    fn shutdown_tears_down_every_slot_and_sweeps_again_as_crash_insurance() {
        let mut ctx = HmContext::new(Recorder::default());
        ctx.start().unwrap();
        let (a, _la) = ctx.create_controller(&ds5()).unwrap();
        let (b, _lb) = ctx.create_controller(&ds5()).unwrap();
        ctx.shutdown().unwrap();
        let calls = ctx.driver_mut().calls.clone();
        for slot in [a, b] {
            assert!(calls.contains(&Call::Destroy(slot)), "slot {slot}");
        }
        assert_eq!(
            calls.last(),
            Some(&Call::Sweep),
            "a final sweep is the last thing that happens"
        );
        assert!(ctx.live_slots().is_empty());
        // Idempotent: a second shutdown (e.g. from Drop) adds nothing.
        let before = ctx.driver_mut().calls.len();
        ctx.shutdown().unwrap();
        assert_eq!(ctx.driver_mut().calls.len(), before);
    }

    #[test]
    fn dropping_the_context_sweeps_even_if_shutdown_was_never_called() {
        // Crash insurance in the ordinary path: a `?` early return anywhere
        // upstream must not leave device nodes behind.
        use std::sync::{Arc, Mutex};
        #[derive(Default)]
        struct Shared(Arc<Mutex<Vec<Call>>>);
        impl HmDriverApi for Shared {
            type Storage = crate::seqlock::HeapStorage;

            fn remove_all_virtual_controllers(&mut self) -> Result<usize, HmError> {
                self.0.lock().unwrap().push(Call::Sweep);
                Ok(0)
            }
            fn load_default_profiles(&mut self) -> Result<usize, HmError> {
                Ok(1)
            }
            fn install_driver(&mut self) -> Result<(), HmError> {
                Ok(())
            }
            fn publish_pid_pool(&mut self, _: SlotId, _: &HmProfile) -> Result<(), HmError> {
                Ok(())
            }
            fn create_controller(
                &mut self,
                s: SlotId,
                _: &HmProfile,
            ) -> Result<Latch<Self::Storage>, HmError> {
                self.0.lock().unwrap().push(Call::Create(s));
                Ok(heap_latch())
            }
            fn park_feedback_index(&mut self, _: SlotId, _: i32) -> Result<(), HmError> {
                Ok(())
            }
            fn dispose_dispatchers(&mut self, _: SlotId) -> Result<(), HmError> {
                Ok(())
            }
            fn destroy_controller(&mut self, s: SlotId) -> Result<(), HmError> {
                self.0.lock().unwrap().push(Call::Destroy(s));
                Ok(())
            }
        }
        let log = Arc::new(Mutex::new(Vec::new()));
        {
            let mut ctx = HmContext::new(Shared(log.clone()));
            ctx.start().unwrap();
            ctx.create_controller(&ds5()).unwrap();
        }
        let calls = log.lock().unwrap().clone();
        assert!(calls.contains(&Call::Destroy(0)));
        assert_eq!(calls.last(), Some(&Call::Sweep));
    }

    #[test]
    fn a_failed_install_leaves_the_context_unusable_rather_than_half_started() {
        let mut ctx = HmContext::new(Recorder {
            install_fails: true,
            ..Recorder::default()
        });
        let err = ctx.start().unwrap_err();
        assert!(matches!(err, HmError::ElevationRequired));
        // ...but the sweep still happened, so the machine is no dirtier.
        assert!(ctx.driver_mut().calls.contains(&Call::Sweep));
        // And no controller can be created against a context that never started.
        assert!(ctx.create_controller(&ds5()).is_err());
    }

    #[test]
    fn destroying_an_unknown_slot_is_a_named_error() {
        let mut ctx = HmContext::new(Recorder::default());
        ctx.start().unwrap();
        assert!(matches!(
            ctx.destroy_controller(7).unwrap_err(),
            HmError::DeadSlot(7)
        ));
    }

    #[test]
    fn slot_ids_are_never_reused_within_a_context() {
        let mut ctx = HmContext::new(Recorder::default());
        ctx.start().unwrap();
        let (a, _la) = ctx.create_controller(&ds5()).unwrap();
        ctx.destroy_controller(a).unwrap();
        let (b, _lb) = ctx.create_controller(&ds5()).unwrap();
        assert_ne!(a, b, "a recycled id could alias a stale feedback callback");
    }
}
