//! ksx-hidmaestro — a Rust client for HIDMaestro's protocol.
//!
//! HIDMaestro (<https://github.com/hifihedgehog/HIDMaestro>, **MIT**) is a
//! user-mode (UMDF2) HID descriptor emulator. It is the only route to the
//! DualSense / Switch Pro / Xbox Series personas: ViGEmBus emulates X360 and
//! DS4 and nothing else, and the project is frozen
//! (`docs/ENHANCEMENTS.md` E1).
//!
//! # Status: written to spec, NOT verified against a live driver
//!
//! **HIDMaestro is not installed on this machine** (probed 2026-08-06 — see
//! [`driver`] for exactly what was checked and found). Everything in this crate
//! is implemented against the protocol map in
//! `docs/research/padforge-code-audit.md` §3, which was extracted from
//! PadForge's working C# client. That map is a good spec — it records the
//! *bugs* as well as the shapes, which is the expensive half — but a spec is not
//! a measurement.
//!
//! What that means concretely:
//!
//! | Layer | Verified | How |
//! |---|---|---|
//! | Seqlock discipline ([`seqlock`]) | **Yes** — real concurrency test | writer thread vs reader loop over shared atomics |
//! | Keepalive cadence ([`keepalive`]) | **Yes** — pure logic, derived constants | arithmetic against the three watchdogs |
//! | Axis routing by name ([`axis`], [`state`]) | **Yes** — pure logic | including a counterfactual that reproduces the phantom-trigger bug |
//! | Lifecycle order ([`context`]) | **Yes** — recorded call order | test double behind [`context::HmDriverApi`] |
//! | Feedback decode table ([`feedback`]) | **Table pinned, bytes unverified** | fixtures from the audit, incl. the BT length trap |
//! | Shared-section layout ([`shm`], [`state::HmGamepadState::encode`]) | **No** | the audit documents the field set and the discipline, not the byte layout |
//! | Anything touching a real device | **No** | there is no device to touch |
//!
//! Nothing here fakes a controller. On a machine without HIDMaestro,
//! [`driver::UnavailableDriver`] refuses with the probe evidence attached, and
//! `ksx doctor` reports the absence.
//!
//! # Shape
//!
//! - [`seqlock`] — the shared-memory latch and its read/write discipline. The
//!   consumer drives the cadence; **this crate spawns no threads**.
//! - [`keepalive`] — 16 ms idle-dedup, derived from three driver watchdogs.
//! - [`axis`] / [`state`] — HID-usage axes, routed **by role through the
//!   profile's map**, never positionally.
//! - [`profile`] — descriptor + axis map, PID-block detection.
//! - [`context`] — lifecycle, in the one order that works.
//! - [`feedback`] — the decode table, including the Bluetooth length trap.
//! - [`driver`] — availability probe and the honest not-installed driver.
//! - [`shm`] (Windows) — file-mapping storage for the latch.

pub mod axis;
pub mod context;
pub mod driver;
pub mod error;
pub mod feedback;
pub mod keepalive;
pub mod profile;
pub mod seqlock;
#[cfg(windows)]
pub mod shm;
pub mod state;

pub use axis::{AxisMap, AxisRole, HmAxis};
pub use context::{HmContext, HmDriverApi, SlotId};
pub use driver::{Availability, UnavailableDriver};
pub use error::{HmError, ProbeSummary};
pub use feedback::{decode_sony, decode_xbox_hid, decode_xinput, Decoded, Motors, OutputSource};
pub use keepalive::{Cadence, Publish, KEEPALIVE};
pub use profile::{slug_for, HmProfile, Transport};
pub use seqlock::{HeapStorage, Latch, LatchStorage};
pub use state::{route, HmGamepadState, HmHat, FRAME_BYTES};

/// One virtual pad: a profile, a latch to publish into, and its cadence.
///
/// This is the whole submit path, and it is deliberately small — everything
/// interesting is in the modules above.
pub struct HmController<S: LatchStorage> {
    slot: SlotId,
    profile: HmProfile,
    latch: Latch<S>,
    cadence: Cadence<FRAME_BYTES>,
    frame: HmGamepadState,
    /// Scratch encode buffer. Owned so the submit path allocates nothing.
    scratch: [u8; FRAME_BYTES],
}

impl<S: LatchStorage> HmController<S> {
    pub fn new(slot: SlotId, profile: HmProfile, latch: Latch<S>) -> Result<Self, HmError> {
        profile.validate()?;
        Ok(Self {
            slot,
            profile,
            latch,
            cadence: Cadence::new(),
            frame: HmGamepadState::new(),
            scratch: [0; FRAME_BYTES],
        })
    }

    pub fn slot(&self) -> SlotId {
        self.slot
    }

    pub fn profile(&self) -> &HmProfile {
        &self.profile
    }

    /// Routes a ksx pad state onto the wire and publishes it if the cadence says
    /// to.
    ///
    /// Allocation-free and lock-free: routing writes into an inline frame, the
    /// encode goes into an owned scratch buffer, and the publish is three
    /// atomic stores plus a byte copy. Returns what the cadence decided, so a
    /// caller can count elided submits without instrumenting the latch.
    pub fn submit(&mut self, state: &ksx_core::PadState, now: std::time::Instant) -> Publish {
        route(&self.profile.axis_map, state, &mut self.frame);
        self.frame.encode(&mut self.scratch);
        let decision = self.cadence.take(&self.scratch, now);
        if decision.should_publish() {
            self.latch.publish(&self.scratch);
        }
        decision
    }

    /// Current `SeqNo` — the number every driver watchdog is really counting.
    pub fn seq(&self) -> u32 {
        self.latch.seq()
    }
}

#[cfg(test)]
mod tests {
    use ksx_core::pad::XButtons;
    use ksx_core::{PadState, Persona};

    use super::*;
    use std::time::{Duration, Instant};

    fn controller() -> HmController<HeapStorage> {
        let profile = profile::expected_profile(Persona::DualSense).unwrap();
        let latch = Latch::new(HeapStorage::new(FRAME_BYTES), FRAME_BYTES).unwrap();
        HmController::new(0, profile, latch).unwrap()
    }

    /// End to end, with no driver: state in, deduped publications out, SeqNo
    /// advancing only when something was actually written.
    #[test]
    fn the_submit_path_dedups_idle_frames_and_advances_seqno_only_on_publish() {
        let mut c = controller();
        let t0 = Instant::now();
        let neutral = PadState::default();

        assert_eq!(c.submit(&neutral, t0), Publish::Changed);
        let after_first = c.seq();
        assert_eq!(after_first, 2);

        // 15 idle ticks: nothing is written, SeqNo does not move.
        for ms in 1..16u64 {
            assert_eq!(
                c.submit(&neutral, t0 + Duration::from_millis(ms)),
                Publish::Skip
            );
        }
        assert_eq!(c.seq(), after_first, "an elided submit writes nothing");

        // The keepalive tick republishes the identical frame, which is the only
        // reason SeqNo moves here — and the only reason the GIP companion does
        // not zero XInput state.
        assert_eq!(c.submit(&neutral, t0 + KEEPALIVE), Publish::Keepalive);
        assert_eq!(c.seq(), after_first + 2);

        // A change goes out immediately, mid-window.
        let pressed = PadState {
            buttons: XButtons::A,
            ..PadState::default()
        };
        assert_eq!(
            c.submit(&pressed, t0 + KEEPALIVE + Duration::from_micros(1)),
            Publish::Changed
        );
        assert_eq!(c.seq(), after_first + 4);
    }

    #[test]
    fn what_is_published_is_what_a_reader_reads_back() {
        let mut c = controller();
        let state = PadState {
            buttons: XButtons::B | XButtons::DPAD_LEFT,
            lt: 255,
            lx: i16::MAX,
            ..PadState::default()
        };
        c.submit(&state, Instant::now());

        let mut out = [0u8; FRAME_BYTES];
        c.latch.read_into(&mut out).unwrap();
        let mut expected = HmGamepadState::new();
        route(&c.profile.axis_map, &state, &mut expected);
        let mut expected_bytes = [0u8; FRAME_BYTES];
        expected.encode(&mut expected_bytes);
        assert_eq!(out, expected_bytes);
    }

    #[test]
    fn a_controller_cannot_be_built_from_an_undeployable_profile() {
        let mut profile = profile::expected_profile(Persona::SwitchPro).unwrap();
        profile.descriptor.clear();
        let latch = Latch::new(HeapStorage::new(FRAME_BYTES), FRAME_BYTES).unwrap();
        assert!(matches!(
            HmController::new(0, profile, latch),
            Err(HmError::ProfileNotDeployable(_))
        ));
    }
}
