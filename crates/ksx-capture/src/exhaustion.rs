//! Interception 10-keyboard-slot exhaustion detector (risk review R2).
//!
//! Every unplug/replug or hibernate/resume cycle *increments* a device's
//! Interception slot number; past 10 the keyboard silently ceases to function
//! until reboot ("nothing I can do" — evilC). The legacy app went silently
//! deaf; we detect the climb and surface **reboot required** loudly.
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
    /// A hardware id previously seen at a lower slot reappeared at a higher one:
    /// the id is climbing toward the ceiling (each climb burns a slot forever
    /// this boot). Reboot required to reclaim the budget.
    IdClimb { hwid: String, from: i32, to: i32 },
}

/// Tracks hwid → last-seen slot and flags exhaustion conditions.
///
/// Note: two identical boards share a hwid (R2); a climb report for a shared
/// hwid means *some* board of that model climbed, which is exactly the level of
/// certainty Interception can offer.
#[derive(Debug, Default)]
pub struct ExhaustionDetector {
    seen: HashMap<String, i32>,
    reboot_required: bool,
}

impl ExhaustionDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one observation of a keyboard at `slot` with hardware id `hwid`
    /// (from initial rescan or from a stroke arriving on a new slot). Returns
    /// an event the first time an exhaustion condition is detected; later
    /// confirmations return `None` (the flag is already up — don't spam).
    pub fn observe_keyboard(&mut self, slot: i32, hwid: &str) -> Option<Exhaustion> {
        let already = self.reboot_required;

        if !(1..=MAX_KEYBOARD_SLOT).contains(&slot) {
            self.reboot_required = true;
            return (!already).then_some(Exhaustion::SlotOutOfRange { slot });
        }

        match self.seen.get(hwid).copied() {
            Some(prev) if slot > prev => {
                self.seen.insert(hwid.to_owned(), slot);
                self.reboot_required = true;
                (!already).then_some(Exhaustion::IdClimb {
                    hwid: hwid.to_owned(),
                    from: prev,
                    to: slot,
                })
            }
            _ => {
                self.seen.insert(hwid.to_owned(), slot);
                None
            }
        }
    }

    /// Has any exhaustion condition been detected this session?
    pub fn reboot_required(&self) -> bool {
        self.reboot_required
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const OTHER: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";

    #[test]
    fn stable_slots_are_healthy() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(d.observe_keyboard(1, IPAC), None);
        assert_eq!(d.observe_keyboard(2, OTHER), None);
        assert_eq!(d.observe_keyboard(1, IPAC), None); // repeat observation
        assert!(!d.reboot_required());
    }

    #[test]
    fn id_climb_is_reported_once() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(d.observe_keyboard(3, IPAC), None);
        // Replug: same hwid shows up one slot higher.
        assert_eq!(
            d.observe_keyboard(4, IPAC),
            Some(Exhaustion::IdClimb {
                hwid: IPAC.to_owned(),
                from: 3,
                to: 4,
            })
        );
        assert!(d.reboot_required());
        // Further climbs update state but don't re-emit.
        assert_eq!(d.observe_keyboard(5, IPAC), None);
        assert!(d.reboot_required());
    }

    #[test]
    fn slot_out_of_range_is_reported() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(
            d.observe_keyboard(11, IPAC),
            Some(Exhaustion::SlotOutOfRange { slot: 11 })
        );
        assert!(d.reboot_required());
        assert_eq!(d.observe_keyboard(12, OTHER), None); // already flagged
    }

    #[test]
    fn lower_slot_after_reboot_style_reset_is_not_a_climb() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(d.observe_keyboard(5, IPAC), None);
        // Seeing the same hwid at a LOWER slot (e.g. another identical board)
        // is not a climb; remember the lower slot so a genuine climb from it
        // still registers.
        assert_eq!(d.observe_keyboard(2, IPAC), None);
        assert!(!d.reboot_required());
        assert!(matches!(
            d.observe_keyboard(3, IPAC),
            Some(Exhaustion::IdClimb { from: 2, to: 3, .. })
        ));
    }

    #[test]
    fn distinct_hwids_do_not_interfere() {
        let mut d = ExhaustionDetector::new();
        assert_eq!(d.observe_keyboard(1, IPAC), None);
        assert_eq!(d.observe_keyboard(9, OTHER), None);
        assert!(!d.reboot_required());
    }
}
