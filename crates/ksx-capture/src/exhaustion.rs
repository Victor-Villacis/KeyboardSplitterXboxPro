//! Interception 10-keyboard-slot exhaustion detector (risk review R2).
//!
//! Every unplug/replug or hibernate/resume cycle *increments* a device's
//! Interception slot number; past 10 the keyboard silently ceases to function
//! until reboot ("nothing I can do" — evilC). The legacy app went silently
//! deaf; we detect the climb and surface **reboot required** loudly.
//!
//! # Why the API is pass-oriented
//!
//! A climb is a statement about *time*: the same board occupies a higher slot
//! than it did before. Two identical boards occupying slots 1 and 2 is not a
//! climb, it is Tuesday — but a detector that compares observations pairwise
//! sees the ascending walk of one rescan (`1: X`, then `2: X`) as X climbing
//! from 1 to 2 and screams REBOOT REQUIRED at a perfectly healthy cabinet.
//!
//! So observations are grouped into **passes**: [`ExhaustionDetector::begin_pass`],
//! one [`ExhaustionDetector::observe_keyboard`] per slot, then
//! [`ExhaustionDetector::end_pass`], which is the only thing that can report a
//! climb — and only by comparing this pass's highest slot per hardware id
//! against the previous pass's. Within a pass, duplicates are just duplicates.
//! [`ExhaustionDetector::observe_hotplug`] is the single-observation form for
//! the hot path (a stroke arriving on a slot we had not seen).
//!
//! Pure (fed observed slot numbers + hardware ids) so the policy is
//! unit-testable with mock device numbers.

use std::collections::HashMap;

/// The keyboard slot budget: Interception device ids 1..=10 are keyboards.
pub const MAX_KEYBOARD_SLOT: i32 = 10;

/// What the detector concluded from an observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exhaustion {
    /// A keyboard was observed outside the 1..=10 budget — should be impossible
    /// through the driver API, so treat it as corruption: reboot required.
    SlotOutOfRange { slot: i32 },
    /// A hardware id occupied a higher slot in this pass than it did in the
    /// previous one: the id is climbing toward the ceiling (each climb burns a
    /// slot forever this boot). Reboot required to reclaim the budget.
    IdClimb { hwid: String, from: i32, to: i32 },
}

/// Tracks hwid → highest slot per pass and flags exhaustion conditions.
///
/// Note: two identical boards share a hwid (R2). Their *count* is invisible to
/// Interception, so a climb report for a shared hwid means "some board of that
/// model climbed", which is exactly the level of certainty the driver offers.
/// (`ksx run` refuses to start when a duplicated id is bound to a slot, so the
/// ambiguity never reaches the translation path.)
#[derive(Debug, Default)]
pub struct ExhaustionDetector {
    /// Highest slot each hwid occupied in the last completed pass. Entries are
    /// never removed: a board that disappears and comes back at a higher slot
    /// is precisely the climb we exist to catch.
    last: HashMap<String, i32>,
    /// Highest slot per hwid in the pass currently being built.
    current: HashMap<String, i32>,
    reboot_required: bool,
}

impl ExhaustionDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a fresh observation pass (one full slot-table rescan).
    pub fn begin_pass(&mut self) {
        self.current.clear();
    }

    /// Feed one keyboard observation into the current pass.
    ///
    /// Returns immediately only for the unambiguous failure — a slot outside the
    /// 1..=10 budget. Climbs are a cross-pass verdict and come from
    /// [`Self::end_pass`].
    pub fn observe_keyboard(&mut self, slot: i32, hwid: &str) -> Option<Exhaustion> {
        if !(1..=MAX_KEYBOARD_SLOT).contains(&slot) {
            return self.flag(Exhaustion::SlotOutOfRange { slot });
        }
        let highest = self.current.entry(hwid.to_owned()).or_insert(slot);
        *highest = (*highest).max(slot);
        None
    }

    /// Close the pass: compare it against the previous one and promote it.
    /// Reports the first climb found (ordered by hwid, so the report is stable).
    pub fn end_pass(&mut self) -> Option<Exhaustion> {
        let mut climb: Option<Exhaustion> = None;
        let mut entries: Vec<(&String, &i32)> = self.current.iter().collect();
        entries.sort_unstable();
        for (hwid, &slot) in entries {
            if let Some(&prev) = self.last.get(hwid) {
                if slot > prev && climb.is_none() {
                    climb = Some(Exhaustion::IdClimb {
                        hwid: hwid.clone(),
                        from: prev,
                        to: slot,
                    });
                }
            }
        }
        for (hwid, slot) in self.current.drain() {
            self.last.insert(hwid, slot);
        }
        climb.and_then(|event| self.flag(event))
    }

    /// One-shot observation outside a rescan (the hot path's unknown-slot
    /// case): a pass containing a single device.
    pub fn observe_hotplug(&mut self, slot: i32, hwid: &str) -> Option<Exhaustion> {
        self.begin_pass();
        if let Some(event) = self.observe_keyboard(slot, hwid) {
            self.current.clear();
            return Some(event);
        }
        self.end_pass()
    }

    /// Has any exhaustion condition been detected this session?
    pub fn reboot_required(&self) -> bool {
        self.reboot_required
    }

    /// Latch the flag; report the event only the first time (don't spam).
    fn flag(&mut self, event: Exhaustion) -> Option<Exhaustion> {
        let already = self.reboot_required;
        self.reboot_required = true;
        (!already).then_some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const OTHER: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";

    /// Run one full rescan pass.
    fn pass(d: &mut ExhaustionDetector, seen: &[(i32, &str)]) -> Option<Exhaustion> {
        d.begin_pass();
        let mut first = None;
        for &(slot, hwid) in seen {
            if let Some(event) = d.observe_keyboard(slot, hwid) {
                first = first.or(Some(event));
            }
        }
        d.end_pass().or(first)
    }

    #[test]
    fn stable_slots_are_healthy() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(pass(&mut d, &[(1, IPAC), (2, OTHER)]), None);
        assert_eq!(pass(&mut d, &[(1, IPAC), (2, OTHER)]), None);
        assert!(!d.reboot_required());
    }

    /// The M4 review's false positive: `rescan()` walks slots ascending, so two
    /// identical I-PACs at slots 1 and 2 looked like one board climbing 1→2 and
    /// produced a spurious "REBOOT REQUIRED" on a perfectly healthy machine.
    /// Within a single pass, an id appearing twice is two boards, never a climb.
    #[test]
    fn two_identical_boards_in_one_pass_are_not_a_climb() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(
            pass(&mut d, &[(1, IPAC), (2, IPAC)]),
            None,
            "two boards sharing a hardware id must not read as slot exhaustion"
        );
        assert!(!d.reboot_required());
        // ...and it stays quiet rescan after rescan, in either walk order.
        assert_eq!(pass(&mut d, &[(1, IPAC), (2, IPAC)]), None);
        assert_eq!(pass(&mut d, &[(2, IPAC), (1, IPAC)]), None);
        assert!(!d.reboot_required());
        // A genuine climb of the pair (one board replugged: 2 → 3) still fires.
        assert_eq!(
            pass(&mut d, &[(1, IPAC), (3, IPAC)]),
            Some(Exhaustion::IdClimb {
                hwid: IPAC.to_owned(),
                from: 2,
                to: 3,
            })
        );
        assert!(d.reboot_required());
    }

    #[test]
    fn id_climb_across_passes_is_reported_once() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(pass(&mut d, &[(3, IPAC)]), None);
        // Replug: same hwid shows up one slot higher, in a LATER pass.
        assert_eq!(
            pass(&mut d, &[(4, IPAC)]),
            Some(Exhaustion::IdClimb {
                hwid: IPAC.to_owned(),
                from: 3,
                to: 4,
            })
        );
        assert!(d.reboot_required());
        // Further climbs update state but don't re-emit.
        assert_eq!(pass(&mut d, &[(5, IPAC)]), None);
        assert!(d.reboot_required());
    }

    #[test]
    fn slot_out_of_range_is_reported() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(
            pass(&mut d, &[(11, IPAC)]),
            Some(Exhaustion::SlotOutOfRange { slot: 11 })
        );
        assert!(d.reboot_required());
        assert_eq!(pass(&mut d, &[(12, OTHER)]), None); // already flagged
    }

    #[test]
    fn lower_slot_after_reboot_style_reset_is_not_a_climb() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(pass(&mut d, &[(5, IPAC)]), None);
        // Seeing the same hwid at a LOWER slot is not a climb; remember the
        // lower slot so a genuine climb from it still registers.
        assert_eq!(pass(&mut d, &[(2, IPAC)]), None);
        assert!(!d.reboot_required());
        assert!(matches!(
            pass(&mut d, &[(3, IPAC)]),
            Some(Exhaustion::IdClimb { from: 2, to: 3, .. })
        ));
    }

    #[test]
    fn distinct_hwids_do_not_interfere() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(pass(&mut d, &[(1, IPAC), (9, OTHER)]), None);
        assert!(!d.reboot_required());
    }

    #[test]
    fn a_device_that_vanishes_and_returns_higher_still_climbs() {
        // The hot-path form: the board is gone for a pass, then a stroke shows
        // up on a slot we had not seen. Absence must not amnesty the climb.
        let mut d = ExhaustionDetector::new();
        assert_eq!(pass(&mut d, &[(1, IPAC)]), None);
        assert_eq!(pass(&mut d, &[]), None);
        assert_eq!(
            d.observe_hotplug(2, IPAC),
            Some(Exhaustion::IdClimb {
                hwid: IPAC.to_owned(),
                from: 1,
                to: 2,
            })
        );
    }

    #[test]
    fn hotplug_of_a_known_stable_device_is_silent() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(pass(&mut d, &[(1, IPAC), (2, OTHER)]), None);
        // Re-observing a slot we already know about proves nothing new, and a
        // one-device observation must never wipe the other slots' history.
        assert_eq!(d.observe_hotplug(1, IPAC), None);
        assert_eq!(pass(&mut d, &[(1, IPAC), (2, OTHER)]), None);
        assert!(!d.reboot_required());
    }
}
