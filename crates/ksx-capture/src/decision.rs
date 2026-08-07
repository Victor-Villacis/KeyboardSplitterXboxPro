//! The pure pass/suppress decision core, shared by the real Interception
//! backend and the mock. This is the logic that gets unit-tested heavily; the
//! backends are thin shells that feed it strokes and obey its verdicts.

use ksx_core::{DeviceId, Key, KeyEvent};

use crate::keymap::{corrected_key, is_down};

/// How much of one device ksx takes.
///
/// # Why this is not a bool
///
/// A cabinet encoder is not a typing keyboard, so taking the WHOLE device is
/// right there: an I-PAC exists to drive pads and nothing else, and letting its
/// unmapped buttons reach Windows would spray junk into whatever has focus.
///
/// A desk keyboard is the opposite case. Someone splitting one keyboard into
/// two pads for a couch game still needs Enter, Escape and the letters to type
/// — and until now they lost the entire keyboard the moment a session started.
/// [`Take::BoundKeys`] is that case: suppress the keys a preset actually binds,
/// re-send everything else byte-for-byte, exactly as an uncaptured device
/// already is.
///
/// Both are legitimate and neither is a default the other can live with, which
/// is why the choice is data rather than a flag.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Take {
    /// Every stroke from this device is suppressed, mapped or not. The cabinet
    /// behaviour, and the default — changing what an existing config means
    /// would be a regression on the machines ksx already runs.
    #[default]
    Whole,
    /// Only the listed keys are suppressed; anything else types normally.
    ///
    /// The set is the UNION across every slot fed by this device. A key bound
    /// in *any* slot must be suppressed, or player 1's `W` would type into the
    /// chat window while it drives player 2's stick.
    BoundKeys(KeySet),
}

/// The keys one device suppresses, as a bitset over [`Key`]'s dense range.
///
/// A `Vec<Key>` would put a linear scan on the hot path, which runs per
/// keystroke on a thread whose latency budget is the product's headline number.
/// This is one shift and one mask.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeySet {
    words: Vec<u64>,
}

impl KeySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: Key) {
        let bit = key as usize;
        let word = bit / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1u64 << (bit % 64);
    }

    pub fn contains(&self, key: Key) -> bool {
        let bit = key as usize;
        self.words
            .get(bit / 64)
            .is_some_and(|w| w & (1u64 << (bit % 64)) != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| *w == 0)
    }

    /// How many keys are in the set. Not used on the hot path — it exists so
    /// `ksx run --dry-run` can say how many keys a device will stop typing
    /// before anyone starts a session and finds out by typing.
    pub fn len(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }
}

impl FromIterator<Key> for KeySet {
    fn from_iter<I: IntoIterator<Item = Key>>(keys: I) -> Self {
        let mut set = Self::new();
        for key in keys {
            set.insert(key);
        }
        set
    }
}

/// The mode a capture thread is in, swapped atomically via `arc-swap` in the
/// real backend (the hot loop reads a snapshot, the ctl handler publishes new
/// ones — no locks).
#[derive(Clone, Debug, Default)]
pub struct CaptureSet {
    /// Observe-only: report AND re-send everything, suppress nothing.
    pub passthrough: bool,
    /// Devices whose strokes are suppressed from the OS while capturing, and
    /// how much of each is taken.
    pub captured: Vec<(DeviceId, Take)>,
}

impl CaptureSet {
    /// The startup mode: passthrough, nothing captured. Safe by construction —
    /// a freshly started backend can never eat a keystroke.
    pub fn passthrough() -> Self {
        Self {
            passthrough: true,
            captured: Vec::new(),
        }
    }

    /// Capturing mode, taking each listed device whole.
    pub fn capturing(ids: Vec<DeviceId>) -> Self {
        Self {
            passthrough: false,
            captured: ids.into_iter().map(|id| (id, Take::Whole)).collect(),
        }
    }

    /// Capturing mode with an explicit [`Take`] per device.
    pub fn capturing_with(takes: Vec<(DeviceId, Take)>) -> Self {
        Self {
            passthrough: false,
            captured: takes,
        }
    }

    /// How much of this device is taken right now, if any.
    pub fn take_for(&self, id: &DeviceId) -> Option<&Take> {
        if self.passthrough {
            return None;
        }
        self.captured.iter().find(|(c, _)| c == id).map(|(_, t)| t)
    }

    /// Is this device's input suppressed from the OS right now?
    ///
    /// Device-level only, and therefore NOT the whole answer once
    /// [`Take::BoundKeys`] exists — a device can be captured while a given key
    /// still types. Kept because plenty of callers legitimately ask "is ksx
    /// holding this device at all"; the per-stroke question is
    /// [`Self::suppresses`].
    pub fn is_captured(&self, id: &DeviceId) -> bool {
        !self.passthrough && self.contains(id)
    }

    /// Does this exact stroke get swallowed?
    ///
    /// The per-stroke question, and the one the hot loop asks.
    pub fn suppresses(&self, id: &DeviceId, key: Key) -> bool {
        match self.take_for(id) {
            None => false,
            Some(Take::Whole) => true,
            Some(Take::BoundKeys(keys)) => keys.contains(key),
        }
    }

    /// Set membership only — "is this device bound to a slot", regardless of
    /// whether passthrough is currently overriding suppression. Backends keep
    /// membership and passthrough separate because passthrough has three
    /// independent sources (ctl, the stall watchdog, and an emergency escape).
    pub fn contains(&self, id: &DeviceId) -> bool {
        self.captured.iter().any(|(c, _)| c == id)
    }
}

/// Verdict for one keyboard stroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrokeOutcome {
    /// Re-send the stroke to the OS, byte-for-byte as received. `false` only
    /// when capturing and the source device is captured (swallowed).
    pub resend: bool,
    /// The event to report to the engine. Always produced for keyboard strokes:
    /// the engine sees *all* devices (emergency escapes fire on any keyboard,
    /// risk review §3 item 2; `ksx monitor` wants unassigned traffic too).
    pub event: KeyEvent,
}

/// Translate one raw stroke into the [`KeyEvent`] the rest of ksx speaks.
///
/// Split out from [`process_keyboard_stroke`] because escapes are evaluated
/// **before** the pass/suppress decision (`crate::escape`): a backend builds the
/// event, feeds it to its [`crate::EscapeWatch`], and only then asks
/// [`should_resend`] — so the very stroke that completes `LeftCtrl ×5` is
/// already flowing to Windows again.
pub fn key_event(device: &DeviceId, code: u16, state: u16, t: u64) -> KeyEvent {
    KeyEvent {
        device: device.clone(),
        key: corrected_key(code, state),
        down: is_down(state),
        t,
    }
}

/// The pass/suppress rule, in one place: suppress only a captured device, and
/// only while nothing has forced passthrough (ctl, watchdog, or escape).
///
/// Device-level. Callers that support [`Take::BoundKeys`] pass the already
/// per-key answer as `suppressed` (see [`CaptureSet::suppresses`]); callers
/// that only ever take a device whole pass `is_captured` and get the same
/// behaviour they always had.
pub const fn should_resend(passthrough: bool, suppressed: bool) -> bool {
    passthrough || !suppressed
}

/// Decide one keyboard stroke. Pure: no clock, no OS, no channel.
///
/// `is_captured` is precomputed by the caller (slot-table lookup in the real
/// backend, set lookup in the mock) so this function stays allocation-free
/// except for the `DeviceId` clone the owned `KeyEvent` contract requires.
pub fn process_keyboard_stroke(
    passthrough: bool,
    is_captured: bool,
    device: &DeviceId,
    code: u16,
    state: u16,
    t: u64,
) -> StrokeOutcome {
    StrokeOutcome {
        resend: should_resend(passthrough, is_captured),
        event: key_event(device, code, state, t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{KEY_DOWN, KEY_E0, KEY_UP};
    use ksx_core::Key;

    fn dev(s: &str) -> DeviceId {
        DeviceId::from(s)
    }

    #[test]
    fn passthrough_resends_and_reports_everything() {
        let set = CaptureSet::passthrough();
        let d = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let out =
            process_keyboard_stroke(set.passthrough, set.is_captured(&d), &d, 30, KEY_DOWN, 7);
        assert!(out.resend);
        assert_eq!(out.event.key, Key::A);
        assert!(out.event.down);
        assert_eq!(out.event.t, 7);
        assert_eq!(out.event.device, d);
    }

    #[test]
    fn captured_device_is_swallowed_but_still_reported() {
        let d = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let set = CaptureSet::capturing(vec![d.clone()]);
        let out = process_keyboard_stroke(set.passthrough, set.is_captured(&d), &d, 30, KEY_UP, 8);
        assert!(!out.resend, "captured strokes must NOT reach the OS");
        assert_eq!(out.event.key, Key::A);
        assert!(!out.event.down);
    }

    #[test]
    fn unknown_device_passes_through_while_capturing() {
        let captured = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let other = dev("HID\\VID_046D&PID_C31C&REV_6402&MI_00");
        let set = CaptureSet::capturing(vec![captured]);
        let out = process_keyboard_stroke(
            set.passthrough,
            set.is_captured(&other),
            &other,
            75,
            KEY_E0 | KEY_DOWN,
            9,
        );
        assert!(out.resend, "unassigned keyboards must keep typing");
        assert_eq!(out.event.key, Key::Left, "correction still applies");
        assert!(out.event.down);
    }

    #[test]
    fn passthrough_mode_overrides_captured_set() {
        // SetPassthrough (or a watchdog trip) must leak captured devices to the
        // OS rather than black-hole them.
        let d = dev("X");
        let mut set = CaptureSet::capturing(vec![d.clone()]);
        set.passthrough = true;
        assert!(!set.is_captured(&d));
        let out =
            process_keyboard_stroke(set.passthrough, set.is_captured(&d), &d, 30, KEY_DOWN, 0);
        assert!(out.resend);
    }

    #[test]
    fn capture_set_membership_is_exact() {
        let a = dev("A");
        let b = dev("a"); // case-sensitive by DeviceId contract
        let set = CaptureSet::capturing(vec![a.clone()]);
        assert!(set.is_captured(&a));
        assert!(!set.is_captured(&b));
    }

    #[test]
    fn contains_survives_passthrough_but_is_captured_does_not() {
        // Membership and suppression are different questions: an escape flips
        // passthrough on without forgetting which devices are bound, so a later
        // re-arm captures exactly the same set.
        let a = dev("A");
        let mut set = CaptureSet::capturing(vec![a.clone()]);
        set.passthrough = true;
        assert!(set.contains(&a));
        assert!(!set.is_captured(&a));
        assert!(should_resend(true, set.contains(&a)));
        set.passthrough = false;
        assert!(set.is_captured(&a));
        assert!(!should_resend(false, set.contains(&a)));
    }

    // -----------------------------------------------------------------
    // Take::BoundKeys — the desk-keyboard case
    // -----------------------------------------------------------------

    /// The whole point: a captured keyboard still types the keys nothing
    /// bound.
    ///
    /// Without this, splitting one keyboard into two pads costs the user their
    /// keyboard — no Enter, no Escape, no letters — for as long as the session
    /// runs. That is correct for a cabinet encoder and unacceptable on a desk.
    #[test]
    fn an_unbound_key_still_types_on_a_partially_taken_device() {
        let kb = dev("HID\\VID_046D&PID_C31C&REV_0110");
        let set = CaptureSet::capturing_with(vec![(
            kb.clone(),
            Take::BoundKeys([Key::W, Key::A, Key::S, Key::D].into_iter().collect()),
        )]);

        assert!(set.suppresses(&kb, Key::W), "W drives the stick");
        assert!(
            !set.suppresses(&kb, Key::Enter),
            "Enter is bound to nothing and must reach Windows"
        );
        assert!(!set.suppresses(&kb, Key::Escape));
        assert!(!set.suppresses(&kb, Key::Q));

        // And the verdict the hot loop actually acts on.
        assert!(!should_resend(false, set.suppresses(&kb, Key::W)));
        assert!(should_resend(false, set.suppresses(&kb, Key::Enter)));
    }

    /// The union rule, and why it is not optional.
    ///
    /// Two slots share one keyboard: WASD drives player 1, the arrows drive
    /// player 2. Both key sets have to be suppressed on that ONE device — a
    /// per-slot answer would let player 1's `W` type into whatever has focus
    /// while it was also moving player 2.
    #[test]
    fn the_suppressed_set_is_the_union_across_every_slot_on_that_device() {
        let kb = dev("HID\\VID_046D&PID_C31C&REV_0110");
        let both: KeySet = [Key::W, Key::A, Key::S, Key::D, Key::Up, Key::Down]
            .into_iter()
            .collect();
        let set = CaptureSet::capturing_with(vec![(kb.clone(), Take::BoundKeys(both))]);

        assert!(set.suppresses(&kb, Key::W), "player 1's key");
        assert!(set.suppresses(&kb, Key::Up), "player 2's key");
        assert!(!set.suppresses(&kb, Key::Enter));
    }

    /// The cabinet is unchanged, and that is the compatibility promise: an
    /// existing config means exactly what it meant before.
    #[test]
    fn taking_a_device_whole_is_the_default_and_swallows_everything() {
        let ipac = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        assert_eq!(Take::default(), Take::Whole);

        let set = CaptureSet::capturing(vec![ipac.clone()]);
        for key in [Key::W, Key::Enter, Key::Escape, Key::F12] {
            assert!(
                set.suppresses(&ipac, key),
                "a whole-device take suppresses {key:?}, bound or not"
            );
        }
    }

    /// Every passthrough source still wins. The escape gesture, the stall
    /// watchdog and an explicit ctl all set this flag, and each of them exists
    /// to hand the keyboard back RIGHT NOW — a per-key set must never be able
    /// to keep suppressing after one fires.
    #[test]
    fn passthrough_overrides_a_partial_take_completely() {
        let kb = dev("HID\\VID_046D&PID_C31C&REV_0110");
        let mut set = CaptureSet::capturing_with(vec![(
            kb.clone(),
            Take::BoundKeys([Key::W].into_iter().collect()),
        )]);
        assert!(set.suppresses(&kb, Key::W));

        set.passthrough = true;
        assert!(
            !set.suppresses(&kb, Key::W),
            "an escape must free even a bound key"
        );
        assert!(should_resend(set.passthrough, set.suppresses(&kb, Key::W)));
    }

    /// A device ksx was never told about is untouched in either mode — the
    /// property that makes "use your other keyboard to type" work today.
    #[test]
    fn an_unlisted_device_is_never_suppressed() {
        let mine = dev("HID\\VID_D209&PID_0430&REV_0056&MI_00");
        let theirs = dev("HID\\VID_1234&PID_5678&REV_0001");
        let set = CaptureSet::capturing(vec![mine]);
        assert!(!set.suppresses(&theirs, Key::A));
        assert!(set.take_for(&theirs).is_none());
    }

    #[test]
    fn the_key_bitset_holds_every_key_it_is_given() {
        let keys = [Key::A, Key::Z, Key::F12, Key::Up, Key::Numpad7];
        let set: KeySet = keys.into_iter().collect();
        for key in keys {
            assert!(set.contains(key), "{key:?}");
        }
        assert!(!set.contains(Key::B));
        assert!(!KeySet::new().contains(Key::A));
        assert!(KeySet::new().is_empty());
        assert!(!set.is_empty());
        assert_eq!(set.len(), keys.len(), "each key counted once");
        // Re-inserting is idempotent — the count is of KEYS, not of writes, and
        // the union that builds it hands the same key over once per slot.
        let mut twice = set.clone();
        twice.insert(Key::A);
        assert_eq!(twice.len(), keys.len());
        assert_eq!(KeySet::new().len(), 0);
    }
}
