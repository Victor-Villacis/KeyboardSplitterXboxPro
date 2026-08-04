//! Emergency-escape *detection* — a pure state machine the app layer feeds
//! with every captured event, before blocking decisions. Detection only:
//! policy lives upstream (legacy wiring was LeftControl×5 → toggle keyboard
//! blocking, RightControl×5 → toggle mouse blocking, Ctrl+Alt+Del → emulation
//! stop; `InputManager.CheckForEmergencyHit`).

use crate::device::{DeviceId, KeyEvent};
use crate::key::Key;

/// What was detected — named after the gesture, not the policy.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Escape {
    /// Ctrl+Alt+Del held together **on one device** (any Ctrl/Alt side,
    /// `Delete` or `NumpadDelete`). Legacy fires this on every subsequent
    /// event while the combo stays held; ported as-is — upstream policy is
    /// expected to be idempotent.
    CtrlAltDel,
    /// `threshold` consecutive press+release cycles of the left escape key.
    LeftSequence,
    /// `threshold` consecutive press+release cycles of the right escape key.
    RightSequence,
}

const LCTRL: u8 = 1 << 0;
const RCTRL: u8 = 1 << 1;
const LALT: u8 = 1 << 2;
const RALT: u8 = 1 << 3;
const DEL: u8 = 1 << 4;
const NUMDEL: u8 = 1 << 5;

const fn mod_bit(key: Key) -> u8 {
    match key {
        Key::LeftControl => LCTRL,
        Key::RightControl => RCTRL,
        Key::LeftAlt => LALT,
        Key::RightAlt => RALT,
        Key::Delete => DEL,
        Key::NumpadDelete => NUMDEL,
        _ => 0,
    }
}

/// Ported from legacy `InputManager` (fields `emergencyLeft/RightDown/UpCount`,
/// `CheckForEmergencyHit`), with the same quirks:
///
/// - downs and ups are counted separately; a sequence fires only once BOTH
///   counters reach the threshold (auto-repeat inflates downs, never ups);
/// - any event of any other key resets all four counters — but the left and
///   right sequences do NOT reset each other;
/// - counters are global across devices, while the Ctrl+Alt+Del combo is
///   evaluated per device (all three parts must be down on the same device).
#[derive(Clone, Debug)]
pub struct EscapeDetector {
    left: Key,
    right: Key,
    threshold: u32,
    left_down: u32,
    left_up: u32,
    right_down: u32,
    right_up: u32,
    /// Per-device Ctrl/Alt/Del bits (`mod_bit` masks). Grows once per device,
    /// then steady-state observation is allocation-free.
    mods: Vec<(DeviceId, u8)>,
}

impl Default for EscapeDetector {
    /// Legacy defaults: LeftControl ×5 / RightControl ×5.
    fn default() -> Self {
        Self::new(Key::LeftControl, Key::RightControl, 5)
    }
}

impl EscapeDetector {
    /// `threshold` is clamped to at least 1 (0 would fire on every event).
    pub fn new(left: Key, right: Key, threshold: u32) -> Self {
        Self {
            left,
            right,
            threshold: threshold.max(1),
            left_down: 0,
            left_up: 0,
            right_down: 0,
            right_up: 0,
            mods: Vec::new(),
        }
    }

    /// Feed one captured event; returns a detection, if any. Combo detection
    /// is checked first (legacy order), and any firing resets the counters.
    pub fn observe(&mut self, ev: &KeyEvent) -> Option<Escape> {
        let b = mod_bit(ev.key);
        let mask = if b != 0 {
            let entry = match self.mods.iter_mut().position(|(d, _)| d == &ev.device) {
                Some(i) => &mut self.mods[i],
                None => {
                    self.mods.push((ev.device.clone(), 0));
                    self.mods.last_mut().expect("just pushed")
                }
            };
            if ev.down {
                entry.1 |= b;
            } else {
                entry.1 &= !b;
            }
            entry.1
        } else {
            self.mods
                .iter()
                .find(|(d, _)| d == &ev.device)
                .map(|(_, m)| *m)
                .unwrap_or(0)
        };

        if mask & (LCTRL | RCTRL) != 0 && mask & (LALT | RALT) != 0 && mask & (DEL | NUMDEL) != 0 {
            self.reset_counters();
            return Some(Escape::CtrlAltDel);
        }

        if ev.key == self.left {
            if ev.down {
                self.left_down += 1;
            } else {
                self.left_up += 1;
            }
        } else if ev.key == self.right {
            if ev.down {
                self.right_down += 1;
            } else {
                self.right_up += 1;
            }
        } else {
            self.reset_counters();
            return None;
        }

        if self.left_down >= self.threshold && self.left_up >= self.threshold {
            self.reset_counters();
            return Some(Escape::LeftSequence);
        }
        if self.right_down >= self.threshold && self.right_up >= self.threshold {
            self.reset_counters();
            return Some(Escape::RightSequence);
        }
        None
    }

    /// Drop tracked modifier state for an unplugged device (a removed device
    /// must not keep a combo half-armed).
    pub fn forget_device(&mut self, dev: &DeviceId) {
        self.mods.retain(|(d, _)| d != dev);
    }

    /// Clear all counters and per-device modifier state.
    pub fn reset(&mut self) {
        self.reset_counters();
        self.mods.clear();
    }

    fn reset_counters(&mut self) {
        self.left_down = 0;
        self.left_up = 0;
        self.right_down = 0;
        self.right_up = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(dev: &str, key: Key, down: bool) -> KeyEvent {
        KeyEvent {
            device: DeviceId::new(dev),
            key,
            down,
            t: 0,
        }
    }

    fn cycles(d: &mut EscapeDetector, dev: &str, key: Key, n: u32) -> Vec<Option<Escape>> {
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(d.observe(&ev(dev, key, true)));
            out.push(d.observe(&ev(dev, key, false)));
        }
        out
    }

    #[test]
    fn left_sequence_fires_on_fifth_release() {
        let mut d = EscapeDetector::default();
        let results = cycles(&mut d, "A", Key::LeftControl, 5);
        assert!(results[..9].iter().all(|r| r.is_none()));
        assert_eq!(results[9], Some(Escape::LeftSequence));
        // Counters reset after firing: the next 5 cycles fire again.
        let results = cycles(&mut d, "A", Key::LeftControl, 5);
        assert_eq!(results[9], Some(Escape::LeftSequence));
    }

    #[test]
    fn right_sequence_fires() {
        let mut d = EscapeDetector::default();
        let results = cycles(&mut d, "A", Key::RightControl, 5);
        assert_eq!(results[9], Some(Escape::RightSequence));
    }

    #[test]
    fn any_other_key_resets_counters() {
        let mut d = EscapeDetector::default();
        cycles(&mut d, "A", Key::LeftControl, 4);
        assert_eq!(d.observe(&ev("A", Key::G, true)), None);
        // 4 earlier cycles no longer count: one more cycle is not enough...
        let results = cycles(&mut d, "A", Key::LeftControl, 1);
        assert!(results.iter().all(|r| r.is_none()));
        // ...a full fresh 5 is.
        cycles(&mut d, "A", Key::LeftControl, 3);
        let results = cycles(&mut d, "A", Key::LeftControl, 1);
        assert_eq!(results[1], Some(Escape::LeftSequence));
    }

    #[test]
    fn left_and_right_do_not_reset_each_other() {
        // Legacy quirk ported as-is: only keys other than BOTH escape keys
        // reset the counters.
        let mut d = EscapeDetector::default();
        cycles(&mut d, "A", Key::LeftControl, 4);
        cycles(&mut d, "A", Key::RightControl, 4);
        let results = cycles(&mut d, "A", Key::LeftControl, 1);
        assert_eq!(results[1], Some(Escape::LeftSequence));
    }

    #[test]
    fn auto_repeat_downs_do_not_fire_without_releases() {
        let mut d = EscapeDetector::default();
        for _ in 0..10 {
            assert_eq!(d.observe(&ev("A", Key::LeftControl, true)), None);
        }
        assert_eq!(d.observe(&ev("A", Key::LeftControl, false)), None);
    }

    #[test]
    fn ctrl_alt_del_fires_on_completing_key() {
        let mut d = EscapeDetector::default();
        assert_eq!(d.observe(&ev("A", Key::LeftControl, true)), None);
        assert_eq!(d.observe(&ev("A", Key::LeftAlt, true)), None);
        assert_eq!(
            d.observe(&ev("A", Key::Delete, true)),
            Some(Escape::CtrlAltDel)
        );
        // Legacy fires again on every event while the combo stays held.
        assert_eq!(d.observe(&ev("A", Key::G, true)), Some(Escape::CtrlAltDel));
        // Releasing one part stops it.
        assert_eq!(d.observe(&ev("A", Key::Delete, false)), None);
    }

    #[test]
    fn right_side_and_numpad_delete_count_as_combo() {
        let mut d = EscapeDetector::default();
        d.observe(&ev("A", Key::RightControl, true));
        d.observe(&ev("A", Key::RightAlt, true));
        assert_eq!(
            d.observe(&ev("A", Key::NumpadDelete, true)),
            Some(Escape::CtrlAltDel)
        );
    }

    #[test]
    fn combo_split_across_devices_does_not_fire() {
        let mut d = EscapeDetector::default();
        assert_eq!(d.observe(&ev("A", Key::LeftControl, true)), None);
        assert_eq!(d.observe(&ev("B", Key::LeftAlt, true)), None);
        assert_eq!(d.observe(&ev("B", Key::Delete, true)), None);
    }

    #[test]
    fn combo_resets_sequence_counters() {
        let mut d = EscapeDetector::default();
        cycles(&mut d, "A", Key::LeftControl, 4);
        d.observe(&ev("A", Key::LeftControl, true));
        d.observe(&ev("A", Key::LeftAlt, true));
        assert_eq!(
            d.observe(&ev("A", Key::Delete, true)),
            Some(Escape::CtrlAltDel)
        );
        d.observe(&ev("A", Key::Delete, false));
        d.observe(&ev("A", Key::LeftAlt, false));
        d.observe(&ev("A", Key::LeftControl, false));
        // Neutral key clears the stray LeftControl-up above, then the
        // pre-combo 4 cycles must not carry over: a full fresh 5 is required.
        d.observe(&ev("A", Key::G, true));
        d.observe(&ev("A", Key::G, false));
        let results = cycles(&mut d, "A", Key::LeftControl, 5);
        assert!(results[..9].iter().all(|r| r.is_none()));
        assert_eq!(results[9], Some(Escape::LeftSequence));
    }

    #[test]
    fn forget_device_disarms_half_held_combo() {
        let mut d = EscapeDetector::default();
        d.observe(&ev("A", Key::LeftControl, true));
        d.observe(&ev("A", Key::LeftAlt, true));
        d.forget_device(&DeviceId::new("A"));
        assert_eq!(d.observe(&ev("A", Key::Delete, true)), None);
    }

    #[test]
    fn custom_keys_and_threshold() {
        let mut d = EscapeDetector::new(Key::F1, Key::F2, 2);
        let results = cycles(&mut d, "A", Key::F1, 2);
        assert_eq!(results[3], Some(Escape::LeftSequence));
        let results = cycles(&mut d, "A", Key::F2, 2);
        assert_eq!(results[3], Some(Escape::RightSequence));
    }

    #[test]
    fn threshold_zero_is_clamped_to_one() {
        let mut d = EscapeDetector::new(Key::F1, Key::F2, 0);
        assert_eq!(d.observe(&ev("A", Key::F1, true)), None);
        assert_eq!(
            d.observe(&ev("A", Key::F1, false)),
            Some(Escape::LeftSequence)
        );
    }
}
