//! Driverless [`VirtualPadBackend`] for engine-to-output integration tests.
//!
//! Always compiled (not `cfg(test)`) so `ksx-app` tests and `ksx pads --mock`
//! style dry-runs can use it without the ViGEmBus driver.

use std::collections::{BTreeMap, VecDeque};

use ksx_core::{PadState, Persona};

use crate::backend::{Feedback, PadHandle, VirtualPadBackend};
use crate::error::OutputError;

/// XInput exposes at most 4 user indices.
const XINPUT_SLOTS: u8 = 4;

#[derive(Debug, Default)]
struct MockPad {
    persona: Persona,
    user_index: Option<u8>,
    feedback: VecDeque<Feedback>,
    last_state: Option<PadState>,
}

/// In-memory backend: records every `update`, serves scripted feedback, and
/// mimics XInput's lowest-free-slot assignment (indices 0..=3, `None` beyond).
///
/// Personas are modeled the way the real driver behaves, because callers reason
/// about slot exhaustion against this mock: only XInput personas take a user
/// index, so a PlayStation pad never consumes one of the four
/// (`docs/research/m6.5-ds4-findings.md`). It emulates every persona — a mock
/// that refused one could not test the paths that use it.
#[derive(Debug, Default)]
pub struct MockBackend {
    next_id: u32,
    pads: BTreeMap<u32, MockPad>,
    updates: Vec<(PadHandle, PadState)>,
    bus_missing: bool,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend whose `plug` always fails with [`OutputError::BusNotFound`] —
    /// for exercising the CLI/engine "driver not installed" path.
    pub fn with_bus_missing() -> Self {
        Self {
            bus_missing: true,
            ..Self::default()
        }
    }

    /// Scripts a feedback event to be returned by a later `poll_feedback`.
    ///
    /// # Panics
    /// Panics on an unknown handle — scripting a dead pad is a test bug.
    pub fn push_feedback(&mut self, handle: PadHandle, feedback: Feedback) {
        self.pads
            .get_mut(&handle.0)
            .expect("push_feedback: unknown handle")
            .feedback
            .push_back(feedback);
    }

    /// Every `update` call in order, across all pads.
    pub fn updates(&self) -> &[(PadHandle, PadState)] {
        &self.updates
    }

    /// Last state submitted for a pad, if any.
    pub fn last_state(&self, handle: PadHandle) -> Option<PadState> {
        self.pads.get(&handle.0).and_then(|p| p.last_state)
    }

    pub fn is_plugged(&self, handle: PadHandle) -> bool {
        self.pads.contains_key(&handle.0)
    }

    pub fn plugged_count(&self) -> usize {
        self.pads.len()
    }

    fn lowest_free_user_index(&self) -> Option<u8> {
        (0..XINPUT_SLOTS).find(|i| !self.pads.values().any(|p| p.user_index == Some(*i)))
    }
}

impl VirtualPadBackend for MockBackend {
    fn plug(&mut self) -> Result<PadHandle, OutputError> {
        self.plug_persona(Persona::default())
    }

    fn plug_persona(&mut self, persona: Persona) -> Result<PadHandle, OutputError> {
        if self.bus_missing {
            return Err(OutputError::BusNotFound);
        }
        let user_index = persona
            .is_xinput()
            .then(|| self.lowest_free_user_index())
            .flatten();
        let id = self.next_id;
        self.next_id += 1;
        self.pads.insert(
            id,
            MockPad {
                persona,
                user_index,
                ..MockPad::default()
            },
        );
        Ok(PadHandle(id))
    }

    fn persona(&self, handle: PadHandle) -> Option<Persona> {
        self.pads.get(&handle.0).map(|p| p.persona)
    }

    fn user_index(&self, handle: PadHandle) -> Option<u8> {
        self.pads.get(&handle.0).and_then(|p| p.user_index)
    }

    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
        let pad = self
            .pads
            .get_mut(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))?;
        pad.last_state = Some(*state);
        self.updates.push((handle, *state));
        Ok(())
    }

    fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
        self.pads.get_mut(&handle.0)?.feedback.pop_front()
    }

    fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
        self.pads
            .remove(&handle.0)
            .map(|_| ())
            .ok_or(OutputError::UnknownHandle(handle))
    }
}

#[cfg(test)]
mod tests {
    use ksx_core::pad::XButtons;

    use super::*;

    /// Exercised through `dyn` on purpose: proves the trait is object-safe and
    /// that the engine can hold `Box<dyn VirtualPadBackend>`.
    fn boxed() -> Box<dyn VirtualPadBackend> {
        Box::new(MockBackend::new())
    }

    #[test]
    fn four_pads_get_distinct_indices_fifth_gets_none() {
        let mut b = boxed();
        let handles: Vec<PadHandle> = (0..5).map(|_| b.plug().unwrap()).collect();
        let indices: Vec<Option<u8>> = handles.iter().map(|&h| b.user_index(h)).collect();
        assert_eq!(
            indices,
            vec![Some(0), Some(1), Some(2), Some(3), None],
            "XInput caps at 4 slots"
        );
    }

    #[test]
    fn unplug_frees_the_slot_for_the_next_plug() {
        let mut b = MockBackend::new();
        let h0 = b.plug().unwrap();
        let h1 = b.plug().unwrap();
        b.unplug(h0).unwrap();
        let h2 = b.plug().unwrap();
        assert_eq!(b.user_index(h2), Some(0), "lowest free index is reused");
        assert_eq!(b.user_index(h1), Some(1));
        // But the *handle* is never reused: stale handles stay dead.
        assert_ne!(h2, h0);
        assert!(matches!(
            b.update(h0, &PadState::default()),
            Err(OutputError::UnknownHandle(h)) if h == h0
        ));
    }

    #[test]
    fn updates_are_recorded_in_order() {
        let mut b = MockBackend::new();
        let h = b.plug().unwrap();
        let s1 = PadState {
            buttons: XButtons::A,
            ..PadState::default()
        };
        let s2 = PadState {
            buttons: XButtons::A | XButtons::B,
            lt: 255,
            ..PadState::default()
        };
        b.update(h, &s1).unwrap();
        b.update(h, &s2).unwrap();
        assert_eq!(b.updates(), &[(h, s1), (h, s2)]);
        assert_eq!(b.last_state(h), Some(s2));
    }

    #[test]
    fn scripted_feedback_is_fifo_and_non_blocking() {
        let mut b = MockBackend::new();
        let h = b.plug().unwrap();
        assert_eq!(b.poll_feedback(h), None, "empty queue never blocks");
        let f1 = Feedback {
            large_motor: 200,
            small_motor: 10,
            led_number: 6,
        };
        let f2 = Feedback {
            large_motor: 0,
            small_motor: 0,
            led_number: 6,
        };
        b.push_feedback(h, f1);
        b.push_feedback(h, f2);
        assert_eq!(b.poll_feedback(h), Some(f1));
        assert_eq!(b.poll_feedback(h), Some(f2));
        assert_eq!(b.poll_feedback(h), None);
    }

    #[test]
    fn unknown_handles_error_everywhere() {
        let mut b = MockBackend::new();
        let ghost = PadHandle(999);
        assert_eq!(b.user_index(ghost), None);
        assert_eq!(b.poll_feedback(ghost), None);
        assert!(matches!(
            b.update(ghost, &PadState::default()),
            Err(OutputError::UnknownHandle(_))
        ));
        assert!(matches!(
            b.unplug(ghost),
            Err(OutputError::UnknownHandle(_))
        ));
    }

    #[test]
    fn bus_missing_mock_reports_bus_not_found() {
        let mut b = MockBackend::with_bus_missing();
        let err = b.plug().unwrap_err();
        assert!(err.is_bus_missing());
    }

    #[test]
    fn double_unplug_fails_cleanly() {
        let mut b = MockBackend::new();
        let h = b.plug().unwrap();
        b.unplug(h).unwrap();
        assert!(matches!(b.unplug(h), Err(OutputError::UnknownHandle(_))));
        assert_eq!(b.plugged_count(), 0);
    }
}
