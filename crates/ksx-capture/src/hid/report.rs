//! Raw HID input report → pressed-usage set → key transitions.
//!
//! This is the hot path's only real work, and it is deliberately pure: bytes in,
//! a 256-bit set out, a diff against the previous set, and a callback per
//! transition. No allocation, no locks, no clock, no OS. Everything below is
//! exercised from [`super::fixtures`], never from a live board.
//!
//! # The rollover rule (the one that matters)
//!
//! A boot-protocol keyboard that cannot report which keys are down fills every
//! array slot with `ErrorRollOver` (HID 1.11 §10 — the "phantom state"). The
//! naive reading — "no usages present, so release everything" — is the worst
//! possible answer on a captured arcade panel: hold three buttons on an
//! N-key-limited encoder and every held input snaps off for one frame, mid
//! fireball motion. So a rollover report is treated as **no information**: the
//! previous state is kept, no events are emitted, and a counter is bumped so
//! `ksx monitor` can show that the board is saturating. Releases arrive
//! honestly on the next report that is not a rollover.
//!
//! Bitmap (NKRO) reports cannot roll over by construction — every usage has its
//! own bit — so the rule only ever applies to array fields.

use super::descriptor::{Field, KeyboardFormat, ReportFormat};
use super::usage::is_error_usage;

/// Bitset over the 256 usages of HID page 0x07. Four words, `Copy`, no heap:
/// the capture loop keeps two of these (previous + scratch) on its stack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageSet {
    words: [u64; 4],
}

impl UsageSet {
    pub const fn new() -> Self {
        Self { words: [0; 4] }
    }

    #[inline]
    pub const fn insert(&mut self, usage: u8) {
        self.words[(usage >> 6) as usize] |= 1u64 << (usage & 0x3F);
    }

    #[inline]
    pub const fn contains(&self, usage: u8) -> bool {
        self.words[(usage >> 6) as usize] & (1u64 << (usage & 0x3F)) != 0
    }

    #[inline]
    pub const fn clear(&mut self) {
        self.words = [0; 4];
    }

    pub const fn is_empty(&self) -> bool {
        self.words[0] == 0 && self.words[1] == 0 && self.words[2] == 0 && self.words[3] == 0
    }

    pub fn len(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Every usage in the set, ascending.
    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=u8::MAX).filter(|&u| self.contains(u))
    }

    /// Call `on_change(usage, down)` for every usage that differs between
    /// `self` (previous) and `next`, ascending.
    ///
    /// Hot path: four XORs and one `trailing_zeros` per changed key. A chord of
    /// four buttons costs four callbacks and nothing else.
    #[inline]
    pub fn diff(&self, next: &UsageSet, mut on_change: impl FnMut(u8, bool)) {
        for (i, (&old, &new)) in self.words.iter().zip(&next.words).enumerate() {
            let mut changed = old ^ new;
            while changed != 0 {
                let bit = changed.trailing_zeros();
                changed &= changed - 1;
                let usage = (i as u32 * 64 + bit) as u8;
                on_change(usage, new & (1u64 << bit) != 0);
            }
        }
    }
}

/// What one report turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    /// Usable key state (possibly empty, i.e. "all keys up").
    Keys,
    /// A rollover/phantom report: the device is saturated and cannot say which
    /// keys are down. The previous state is authoritative; emit nothing.
    Rollover,
    /// The report did not match any keyboard format this interface declares
    /// (wrong report ID, or a short/truncated transfer). Ignored.
    Foreign,
}

/// Decodes reports for one claimed interface. Owned by the capture thread.
#[derive(Clone, Debug)]
pub struct ReportReader {
    format: KeyboardFormat,
    /// Usages held as of the last usable report.
    state: UsageSet,
    /// Rollover reports seen (diagnostics only; `ksx monitor` renders it).
    rollovers: u64,
    /// Reports that matched no declared format.
    foreign: u64,
}

impl ReportReader {
    pub fn new(format: KeyboardFormat) -> Self {
        Self {
            format,
            state: UsageSet::new(),
            rollovers: 0,
            foreign: 0,
        }
    }

    pub fn format(&self) -> &KeyboardFormat {
        &self.format
    }

    /// Usages currently held.
    pub fn state(&self) -> &UsageSet {
        &self.state
    }

    pub fn rollovers(&self) -> u64 {
        self.rollovers
    }

    pub fn foreign_reports(&self) -> u64 {
        self.foreign
    }

    /// Feed one raw report; `on_change(usage, down)` fires per transition.
    ///
    /// Allocation-free and panic-free for any input, including a zero-length or
    /// oversized buffer — the bytes come straight off a USB endpoint, so
    /// "hardware-supplied" is the threat model (`docs/PLAYBOOK.md` names this
    /// the M6 fuzz target).
    pub fn feed(&mut self, report: &[u8], on_change: impl FnMut(u8, bool)) -> ReportKind {
        let Some((format, payload)) = self.format.match_report(report) else {
            self.foreign += 1;
            return ReportKind::Foreign;
        };

        let mut next = UsageSet::new();
        if decode(format, payload, &mut next).is_rollover() {
            self.rollovers += 1;
            return ReportKind::Rollover;
        }

        self.state.diff(&next, on_change);
        self.state = next;
        ReportKind::Keys
    }

    /// Force every held usage to release, reporting each one.
    ///
    /// The device is gone (unplug) or the session is ending: whatever we hold
    /// must be let go, or a modifier stays down forever in the engine's view of
    /// the pad.
    pub fn release_all(&mut self, on_change: impl FnMut(u8, bool)) {
        let empty = UsageSet::new();
        self.state.diff(&empty, on_change);
        self.state = empty;
    }
}

/// Internal decode verdict.
enum Decoded {
    Keys,
    Rollover,
}

impl Decoded {
    fn is_rollover(&self) -> bool {
        matches!(self, Decoded::Rollover)
    }
}

fn decode(format: &ReportFormat, payload: &[u8], out: &mut UsageSet) -> Decoded {
    for field in &format.fields {
        match *field {
            Field::Bitmap {
                bit_offset,
                bits,
                usage_min,
            } => {
                for i in 0..bits {
                    if !bit_at(payload, bit_offset + i) {
                        continue;
                    }
                    // Usages beyond 0xFF cannot exist on this page; a bitmap
                    // declaring more than 256 bits is the device's problem, and
                    // wrapping into low usages would be a phantom keypress.
                    let Some(usage) = usage_min.checked_add(clip(i)) else {
                        continue;
                    };
                    if usage != 0 && !is_error_usage(usage) {
                        out.insert(usage);
                    }
                }
            }
            Field::Array {
                bit_offset,
                bits,
                count,
            } => {
                for slot in 0..count {
                    let value = bits_at(payload, bit_offset + slot * bits, bits);
                    let usage = value as u8;
                    if usage == 0 {
                        continue; // empty slot
                    }
                    if is_error_usage(usage) {
                        // One ErrorRollOver anywhere invalidates the whole
                        // report: the device is telling us it does not know.
                        return Decoded::Rollover;
                    }
                    out.insert(usage);
                }
            }
        }
    }
    Decoded::Keys
}

/// Bitmap index → `u8` offset, saturating rather than wrapping.
fn clip(i: usize) -> u8 {
    u8::try_from(i).unwrap_or(u8::MAX)
}

/// One bit out of the payload, LSB-first within each byte (HID's bit order).
/// Out-of-range reads are `false` — a short transfer means "no data", never a
/// panic and never a phantom press.
#[inline]
fn bit_at(payload: &[u8], bit: usize) -> bool {
    match payload.get(bit / 8) {
        Some(byte) => byte & (1 << (bit % 8)) != 0,
        None => false,
    }
}

/// Up to 32 bits out of the payload, LSB-first. Used for array slots, which are
/// byte-aligned and 8 bits wide on every real keyboard; the general form costs
/// nothing and removes an assumption.
#[inline]
fn bits_at(payload: &[u8], bit: usize, len: usize) -> u32 {
    if len == 8 && bit % 8 == 0 {
        return payload.get(bit / 8).copied().unwrap_or(0).into();
    }
    let mut value = 0u32;
    for i in 0..len.min(32) {
        if bit_at(payload, bit + i) {
            value |= 1 << i;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::{descriptor, fixtures};

    /// Collect transitions as `(usage, down)` in emission order.
    fn feed(reader: &mut ReportReader, report: &[u8]) -> (ReportKind, Vec<(u8, bool)>) {
        let mut events = Vec::new();
        let kind = reader.feed(report, |usage, down| events.push((usage, down)));
        (kind, events)
    }

    fn boot_reader() -> ReportReader {
        ReportReader::new(descriptor::parse(fixtures::BOOT_KEYBOARD_DESCRIPTOR))
    }

    fn nkro_reader() -> ReportReader {
        ReportReader::new(descriptor::parse(fixtures::NKRO_DESCRIPTOR))
    }

    // -- boot protocol ---------------------------------------------------

    #[test]
    fn single_key_press_then_release() {
        let mut r = boot_reader();
        let (kind, events) = feed(&mut r, &fixtures::boot_report(0, &[0x04])); // 'a'
        assert_eq!(kind, ReportKind::Keys);
        assert_eq!(events, vec![(0x04, true)]);
        assert!(r.state().contains(0x04));

        // The same report again is not a repeat: HID reports are state, not
        // edges. An auto-repeat would double-fire every arcade button.
        let (kind, events) = feed(&mut r, &fixtures::boot_report(0, &[0x04]));
        assert_eq!(kind, ReportKind::Keys);
        assert!(events.is_empty(), "an unchanged report emits nothing");

        let (kind, events) = feed(&mut r, &fixtures::boot_report(0, &[]));
        assert_eq!(kind, ReportKind::Keys);
        assert_eq!(events, vec![(0x04, false)]);
        assert!(r.state().is_empty());
    }

    #[test]
    fn empty_report_releases_everything_held() {
        let mut r = boot_reader();
        feed(
            &mut r,
            &fixtures::boot_report(0b0000_0011, &[0x04, 0x05, 0x06]),
        );
        assert_eq!(r.state().len(), 5); // 2 modifiers + 3 keys

        let (kind, mut events) = feed(&mut r, &fixtures::boot_report(0, &[]));
        assert_eq!(kind, ReportKind::Keys);
        events.sort_unstable();
        assert_eq!(
            events,
            vec![
                (0x04, false),
                (0x05, false),
                (0x06, false),
                (0xE0, false),
                (0xE1, false)
            ]
        );
        assert!(r.state().is_empty());
    }

    #[test]
    fn chord_adds_and_removes_one_key_at_a_time() {
        let mut r = boot_reader();
        // Arcade reality: joystick left + two buttons, then release the middle.
        let (_, events) = feed(&mut r, &fixtures::boot_report(0, &[0x50]));
        assert_eq!(events, vec![(0x50, true)]);
        let (_, events) = feed(&mut r, &fixtures::boot_report(0, &[0x50, 0x1D]));
        assert_eq!(events, vec![(0x1D, true)], "only the new key transitions");
        let (_, events) = feed(&mut r, &fixtures::boot_report(0, &[0x50, 0x1D, 0x1B]));
        assert_eq!(events, vec![(0x1B, true)]);
        // Slot order changes when the middle key is released — the parser must
        // be set-based, not slot-based, or this reads as three transitions.
        let (_, events) = feed(&mut r, &fixtures::boot_report(0, &[0x50, 0x1B]));
        assert_eq!(events, vec![(0x1D, false)]);
        assert_eq!(r.state().len(), 2);
    }

    #[test]
    fn slot_reordering_alone_is_not_a_transition() {
        let mut r = boot_reader();
        feed(&mut r, &fixtures::boot_report(0, &[0x04, 0x05, 0x06]));
        let (kind, events) = feed(&mut r, &fixtures::boot_report(0, &[0x06, 0x04, 0x05]));
        assert_eq!(kind, ReportKind::Keys);
        assert!(
            events.is_empty(),
            "keyboards reshuffle slots freely; that is not input"
        );
    }

    #[test]
    fn modifiers_come_from_the_bitmap_not_the_slots() {
        let mut r = boot_reader();
        // LeftControl (bit 0) + LeftAlt (bit 2) + RightGUI (bit 7).
        let (_, mut events) = feed(&mut r, &fixtures::boot_report(0b1000_0101, &[]));
        events.sort_unstable();
        assert_eq!(events, vec![(0xE0, true), (0xE2, true), (0xE7, true)]);

        // Drop LeftAlt only.
        let (_, events) = feed(&mut r, &fixtures::boot_report(0b1000_0001, &[]));
        assert_eq!(events, vec![(0xE2, false)]);
    }

    #[test]
    fn rollover_holds_state_and_emits_nothing() {
        let mut r = boot_reader();
        feed(&mut r, &fixtures::boot_report(0b0000_0001, &[0x04, 0x05]));
        let held = *r.state();

        let (kind, events) = feed(&mut r, &fixtures::boot_rollover(0b0000_0001));
        assert_eq!(kind, ReportKind::Rollover);
        assert!(
            events.is_empty(),
            "a saturated board must not release the player's held buttons"
        );
        assert_eq!(*r.state(), held, "previous state stays authoritative");
        assert_eq!(r.rollovers(), 1);

        // Recovery: the next honest report resolves the difference normally.
        let (kind, events) = feed(&mut r, &fixtures::boot_report(0, &[0x04]));
        assert_eq!(kind, ReportKind::Keys);
        let mut sorted = events;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![(0x05, false), (0xE0, false)]);
    }

    #[test]
    fn a_single_error_slot_invalidates_the_whole_report() {
        let mut r = boot_reader();
        feed(&mut r, &fixtures::boot_report(0, &[0x04]));
        // Some firmware puts ErrorRollOver in the first slot only.
        let mut report = fixtures::boot_report(0, &[0x00, 0x07, 0x08]);
        report[2] = crate::hid::usage::USAGE_ERROR_ROLLOVER;
        let (kind, events) = feed(&mut r, &report);
        assert_eq!(kind, ReportKind::Rollover);
        assert!(events.is_empty());
        assert!(r.state().contains(0x04), "still holding what we held");
    }

    #[test]
    fn post_fail_and_error_undefined_are_rollovers_too() {
        for code in [
            crate::hid::usage::USAGE_POST_FAIL,
            crate::hid::usage::USAGE_ERROR_UNDEFINED,
        ] {
            let mut r = boot_reader();
            let mut report = fixtures::boot_report(0, &[]);
            report[2] = code;
            assert_eq!(feed(&mut r, &report).0, ReportKind::Rollover);
        }
    }

    // -- NKRO ------------------------------------------------------------

    #[test]
    fn full_nkro_bitmap_reports_every_key_at_once() {
        let mut r = nkro_reader();
        // Far more than six simultaneous keys: the whole reason NKRO exists,
        // and the case a 4-player panel hits constantly.
        let keys: Vec<u8> = (0x04..=0x1D).collect(); // all 26 letters
        let (kind, events) = feed(&mut r, &fixtures::nkro_report(0b1111_1111, &keys));
        assert_eq!(kind, ReportKind::Keys);
        assert_eq!(events.len(), keys.len() + 8, "26 letters + 8 modifiers");
        assert!(events.iter().all(|&(_, down)| down));
        assert_eq!(r.state().len(), 34);
        for &usage in &keys {
            assert!(r.state().contains(usage), "usage {usage:#04x} missing");
        }
        for m in 0xE0..=0xE7u8 {
            assert!(r.state().contains(m));
        }

        // Release everything.
        let (_, events) = feed(&mut r, &fixtures::nkro_report(0, &[]));
        assert_eq!(events.len(), 34);
        assert!(events.iter().all(|&(_, down)| !down));
    }

    #[test]
    fn nkro_modifiers_are_not_double_counted_with_the_bitmap() {
        // The bitmap covers usages 0x00..=0xEF, so the 0xE0..=0xE7 modifiers
        // appear ONLY in the modifier byte here. Setting the modifier byte must
        // produce exactly 8 events, not 16.
        let mut r = nkro_reader();
        let (_, events) = feed(&mut r, &fixtures::nkro_report(0xFF, &[]));
        assert_eq!(events.len(), 8);
    }

    #[test]
    fn nkro_ignores_usage_zero() {
        let mut r = nkro_reader();
        let mut report = fixtures::nkro_report(0, &[]);
        report[2] |= 1; // bitmap bit 0 == usage 0x00 ("no event")
        let (kind, events) = feed(&mut r, &report);
        assert_eq!(kind, ReportKind::Keys);
        assert!(events.is_empty());
    }

    #[test]
    fn nkro_has_no_rollover_state() {
        // Bit 1 of the bitmap is usage 0x01 (ErrorRollOver). In a bitmap that
        // is a device bug, not a phantom state — the report is still usable and
        // the bit is dropped rather than blinding us to 200 real keys.
        let mut r = nkro_reader();
        let mut report = fixtures::nkro_report(0, &[0x04]);
        report[2] |= 1 << 1;
        let (kind, events) = feed(&mut r, &report);
        assert_eq!(kind, ReportKind::Keys);
        assert_eq!(events, vec![(0x04, true)]);
    }

    #[test]
    fn a_foreign_report_id_is_ignored_not_decoded() {
        let mut r = nkro_reader();
        feed(&mut r, &fixtures::nkro_report(0, &[0x04]));
        let mut other = fixtures::nkro_report(0, &[]);
        other[0] = 0x09; // some other collection on the same interface
        let (kind, events) = feed(&mut r, &other);
        assert_eq!(kind, ReportKind::Foreign);
        assert!(events.is_empty());
        assert!(r.state().contains(0x04), "state untouched");
        assert_eq!(r.foreign_reports(), 1);
    }

    // -- robustness ------------------------------------------------------

    #[test]
    fn short_and_empty_transfers_never_panic() {
        let mut r = boot_reader();
        feed(&mut r, &fixtures::boot_report(0xFF, &[0x04, 0x05]));
        // A 2-byte transfer: modifiers survive, the key slots read as absent,
        // so the keys release. That is the safe direction (a key that is not
        // reported is not held).
        let (kind, mut events) = feed(&mut r, &[0xFF, 0x00]);
        assert_eq!(kind, ReportKind::Keys);
        events.sort_unstable();
        assert_eq!(events, vec![(0x04, false), (0x05, false)]);
        // Zero-length: no report ID to match, but the ID-less format still
        // applies and everything releases.
        let (kind, _) = feed(&mut r, &[]);
        assert_eq!(kind, ReportKind::Keys);
        assert!(r.state().is_empty());
    }

    #[test]
    fn zero_length_report_with_report_ids_is_foreign_not_a_panic() {
        let mut r = nkro_reader();
        assert_eq!(feed(&mut r, &[]).0, ReportKind::Foreign);
    }

    #[test]
    fn oversized_transfers_are_truncated_by_the_format_not_by_luck() {
        let mut r = boot_reader();
        let mut report = vec![0u8; 64];
        report[0] = 0b0000_0001;
        report[2] = 0x04;
        report[40] = 0x05; // outside the declared 8 bytes
        let (_, mut events) = feed(&mut r, &report);
        events.sort_unstable();
        assert_eq!(events, vec![(0x04, true), (0xE0, true)]);
    }

    #[test]
    fn release_all_lets_go_of_everything() {
        let mut r = nkro_reader();
        feed(&mut r, &fixtures::nkro_report(0x03, &[0x04, 0x50]));
        let mut events = Vec::new();
        r.release_all(|usage, down| events.push((usage, down)));
        events.sort_unstable();
        assert_eq!(
            events,
            vec![(0x04, false), (0x50, false), (0xE0, false), (0xE1, false)]
        );
        assert!(r.state().is_empty());
        // Idempotent: a second release has nothing to say.
        let mut again = Vec::new();
        r.release_all(|u, d| again.push((u, d)));
        assert!(again.is_empty());
    }

    #[test]
    fn every_byte_pattern_is_survivable() {
        // Poor man's fuzz, deterministic and in-CI: a rolling pattern over the
        // full length range, asserting only that nothing panics and the state
        // never contains usage 0 or an error code.
        let mut boot = boot_reader();
        let mut nkro = nkro_reader();
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        for len in 0..80usize {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed >> 24) as u8
                })
                .collect();
            boot.feed(&bytes, |_, _| {});
            nkro.feed(&bytes, |_, _| {});
            for reader in [&boot, &nkro] {
                assert!(!reader.state().contains(0));
                for code in 1..=3u8 {
                    assert!(!reader.state().contains(code));
                }
            }
        }
    }

    // -- UsageSet --------------------------------------------------------

    #[test]
    fn usage_set_covers_the_whole_byte_range() {
        let mut set = UsageSet::new();
        for usage in [0u8, 1, 63, 64, 127, 128, 191, 192, 255] {
            set.insert(usage);
        }
        assert_eq!(set.len(), 9);
        assert!(set.contains(255));
        assert!(!set.contains(254));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![0, 1, 63, 64, 127, 128, 191, 192, 255]
        );
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn diff_reports_both_directions_in_usage_order() {
        let mut a = UsageSet::new();
        a.insert(0x04);
        a.insert(0xE0);
        let mut b = UsageSet::new();
        b.insert(0xE0);
        b.insert(0x50);
        let mut events = Vec::new();
        a.diff(&b, |u, d| events.push((u, d)));
        assert_eq!(events, vec![(0x04, false), (0x50, true)]);
    }
}
