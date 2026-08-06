//! The translation engine: `KeyEvent`s in, `PadDelta`s out.
//!
//! Ported from legacy `KeyboardSplitter/Models/Splitter.cs`
//! (`TranslateInput`/`GetMappings`/`AreAllKeysUp`/`HasOppositeAxisKeysDown`/
//! `SetButton`/`SetTrigger`/`SetAxis`/`SetDpad`) and
//! `KeyboardSplitter/Presets/Preset.cs` (`FilterByKey`/`GetKeys`). The legacy
//! cross-category custom-function aggregation collapses here into `Binding`
//! equality, because the importer folds `XboxCustomFunction` into `Binding`.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::device::{DeviceId, KeyEvent};
use crate::key::Key;
use crate::macros::{Interrupt, OnRelease, Retrigger};
use crate::pad::{Axis, PadState, Trigger, AXIS_CENTER};
use crate::preset::{Binding, Chord, Preset};
use crate::slot::SlotSpec;

/// A slot with its preset already resolved by the config layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSlot {
    pub spec: SlotSpec,
    pub preset: Preset,
}

/// A genuine pad-state transition for one slot. `slot` is the slot *number*
/// (1..=4), never an XInput user index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadDelta {
    pub slot: u8,
    pub state: PadState,
}

/// Delta batch: at most one entry per slot, so 4 inline slots never allocate.
pub type Deltas = SmallVec<[PadDelta; 4]>;

/// One precompiled dispatch target: an entry of `slots[slot]`'s preset.
#[derive(Clone, Copy)]
struct Target {
    slot: u8,
    binding: Binding,
}

/// One precompiled chord in a slot (docs/INPUT-TRANSFORMS.md §1b).
///
/// Keys are dense ids, so evaluating the guard is a handful of bit tests
/// against the device's key bitset — no key lookup, no allocation, no scan of
/// the preset.
struct ChordRt {
    binding: Binding,
    /// Dense id of the trigger key.
    trigger: u32,
    /// Dense ids that must ALL be down.
    when: SmallVec<[u32; 3]>,
    /// Dense ids that must NONE be down.
    unless: SmallVec<[u32; 3]>,
    /// `when.len() + unless.len()` — larger wins (see [`Chord::specificity`]).
    specificity: u16,
    /// Recomputed on every relevant event; also mirrored into `held` so the
    /// all-keys-up rule can treat a chord as just another holder.
    active: bool,
}

/// One precompiled macro in a slot (docs/INPUT-TRANSFORMS.md §1c).
///
/// A macro STEP is an ordinary holder: `holder_bindings[first_holder + i]` is
/// step `i`'s `hold` set, and the step is "held" exactly while the macro is on
/// it. That is the whole integration — the all-keys-up rule, the opposite-axis
/// snap, the releases-before-presses order and the one-batch discipline are the
/// chord machinery, unchanged, so a macro can never strand a button that a
/// chord could not.
struct MacroRt {
    /// **Absolute** end-of-step offsets from the macro's start, in ms, with the
    /// sampling floor already applied ([`crate::Macro::deadlines`]).
    ///
    /// Absolute, not per-step, is the anti-drift decision: step `i` ends at
    /// `start + ends[i]`, so wake jitter is corrected at every step instead of
    /// accumulating across the sequence. The engine never re-decides either
    /// number.
    ends: SmallVec<[u32; 8]>,
    /// The shortest window each step may ever be given, however late the
    /// scheduler runs ([`crate::MacroStep::min_visible_ms`]). This is what
    /// turns "the timeline already passed" into "publish it anyway, briefly"
    /// rather than into a skipped — invisible — input.
    min_visible: SmallVec<[u32; 8]>,
    /// Holder id of step 0; step `i` is `first_holder + i`.
    first_holder: u32,
    on_release: OnRelease,
    retrigger: Retrigger,
    interrupt: Interrupt,
    /// Dense ids of the keys that start this macro (multi-bind: several keys
    /// may, and one key may start several macros).
    triggers: SmallVec<[u32; 2]>,
    /// Which step is live. `None` ⇒ not running, and every one of this macro's
    /// holders is down.
    step: Option<u16>,
    /// When the current run began, on the caller's clock. The origin every
    /// deadline in `ends` is measured from.
    start: u64,
}

/// One armed macro deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timer {
    /// Absolute milliseconds on the caller's clock.
    deadline: u64,
    slot: u8,
    mac: u16,
}

/// **The** timer structure: one ordered list for every macro in every slot.
///
/// Not a thread per macro and not a per-step allocation: entries are `Copy`,
/// the backing `Vec` is sized at [`EngineTables::build`] time to the total
/// macro count, and arming is an insertion into an already-sorted list. With
/// at most a handful of macros per cabinet the linear insert beats a heap on
/// both cache behavior and code you have to trust.
///
/// Ties are FIFO (`partition_point` on `<=`), so two macros armed for the same
/// millisecond always fire in the order they were armed — the determinism the
/// replay corpus needs.
#[derive(Debug, Default)]
struct Timers {
    /// Ascending by `deadline`.
    armed: Vec<Timer>,
}

impl Timers {
    fn with_capacity(macros: usize) -> Self {
        Self {
            armed: Vec::with_capacity(macros),
        }
    }

    fn arm(&mut self, slot: u8, mac: u16, deadline: u64) {
        self.cancel(slot, mac);
        let at = self.armed.partition_point(|t| t.deadline <= deadline);
        self.armed.insert(
            at,
            Timer {
                deadline,
                slot,
                mac,
            },
        );
    }

    fn cancel(&mut self, slot: u8, mac: u16) {
        if let Some(i) = self
            .armed
            .iter()
            .position(|t| t.slot == slot && t.mac == mac)
        {
            self.armed.remove(i);
        }
    }

    fn clear(&mut self) {
        self.armed.clear();
    }

    /// When the engine next needs to be ticked, if ever.
    fn next(&self) -> Option<u64> {
        self.armed.first().map(|t| t.deadline)
    }

    /// Take the earliest timer that is due at `now`.
    fn pop_due(&mut self, now: u64) -> Option<(u8, u16)> {
        match self.armed.first() {
            Some(t) if t.deadline <= now => {
                let t = self.armed.remove(0);
                Some((t.slot, t.mac))
            }
            _ => None,
        }
    }
}

struct SlotRuntime {
    number: u8,
    /// Index into `Engine::devices`.
    keyboard: Option<u8>,
    mouse: Option<u8>,
    /// The device whose key state chord guards are evaluated against
    /// (`keyboard`, else `mouse`). Events from the slot's *other* device only
    /// update that key's own heldness — a chord never spans two devices.
    chord_device: Option<u8>,
    /// Endpoint -> ids of every HOLDER driving it (all-keys-up rule). Ids
    /// below `chord_base` are dense keys; ids at or above it are chords.
    endpoint_keys: HashMap<Binding, SmallVec<[u32; 4]>>,
    /// Flat axis entries for the opposite-axis scan on release (same id space).
    axis_entries: Vec<(Axis, i16, u32)>,
    current: PadState,
    last_emitted: PadState,

    // ---- chord runtime -----------------------------------------------------
    // Every field below is EMPTY for a chord-free slot, and `chords.is_empty()`
    // is the branch that keeps such a slot on exactly the pre-chord code path:
    // no extra memory, no extra iteration, no behavioural difference.
    /// Most-specific-first (stable within a specificity level).
    chords: Vec<ChordRt>,
    /// First chord holder id; equals the total number of dense keys.
    chord_base: u32,
    /// Holder id -> the bindings it drives in THIS slot, in preset order.
    holder_bindings: Vec<SmallVec<[Binding; 2]>>,
    /// Every holder that drives something — the full-resync scan list.
    all_holders: Vec<u32>,
    /// Dense ids consumed by some chord (trigger + `when`), deduped.
    chord_keys: SmallVec<[u32; 8]>,
    /// Effective heldness per holder: a key that is down but CONSUMED by an
    /// active chord is not held, which is exactly what suppression means.
    held: Vec<u64>,
    /// `held` as of the previous event — the diff that produces press/release.
    prev_held: Vec<u64>,
    /// Keys consumed by the currently active chords.
    consumed: Vec<u64>,
    /// Keys consumed by strictly MORE specific chords — the specificity rule.
    blocked: Vec<u64>,
    /// Preallocated scan list for the current event (never reallocates).
    scan: Vec<u32>,

    // ---- macro runtime -----------------------------------------------------
    /// Precompiled macros; EMPTY for a slot with none.
    macros: Vec<MacroRt>,
    /// First macro-step holder id (`chord_base + chords.len()`). Holders at or
    /// above it are macro steps, below it chords, below that dense keys.
    macro_base: u32,
    /// Holder ids whose macro state moved and have not been applied yet.
    /// Drained into `scan`, so a trigger press and the step it starts land in
    /// ONE delta batch. Sized to the slot's total step count at build time.
    macro_dirty: Vec<u32>,
    /// `false` ⇒ neither chords nor macros: this slot takes the pre-chord code
    /// path end to end, exactly as it did before either feature existed.
    stateful: bool,
}

impl SlotRuntime {
    fn axis_field(&mut self, axis: Axis) -> &mut i16 {
        match axis {
            Axis::X => &mut self.current.lx,
            Axis::Y => &mut self.current.ly,
            Axis::Rx => &mut self.current.rx,
            Axis::Ry => &mut self.current.ry,
        }
    }

    fn press(&mut self, binding: Binding) {
        match binding {
            Binding::Button(b) => self.current.buttons |= b.flag(),
            Binding::Trigger(Trigger::Left) => self.current.lt = u8::MAX,
            Binding::Trigger(Trigger::Right) => self.current.rt = u8::MAX,
            Binding::Axis { axis, value } => *self.axis_field(axis) = value,
            Binding::Dpad(d) => self.current.buttons |= d.flag(),
            // A consume-only chord drives no endpoint — its entire effect is
            // the suppression of its constituents (docs/INPUT-TRANSFORMS.md
            // §2.6). Unreachable in practice: `build` never registers it as a
            // holder binding.
            Binding::Consume => {}
        }
    }

    /// Is this holder currently driving its bindings?
    ///
    /// Plain slots take the first line and are bit-for-bit the pre-chord check
    /// (`bit(down, k)`). With chords, a key holds only while it is down AND not
    /// consumed, and a chord holds while its guard is satisfied; with macros, a
    /// step holds while its macro is on it.
    fn holds(&self, id: u32, down: &[u64]) -> bool {
        if !self.stateful {
            return bit(down, id);
        }
        bit(&self.held, id)
    }

    /// `down` is the event device's key bitset, already updated for the
    /// triggering release.
    fn release(&mut self, binding: Binding, down: &[u64]) {
        // All-keys-up rule: the endpoint stays active while ANY holder mapped
        // to it (on this device) is still held.
        if let Some(keys) = self.endpoint_keys.get(&binding) {
            if keys.iter().any(|&k| self.holds(k, down)) {
                return;
            }
        }

        match binding {
            Binding::Button(b) => self.current.buttons &= !b.flag(),
            Binding::Trigger(Trigger::Left) => self.current.lt = 0,
            Binding::Trigger(Trigger::Right) => self.current.rt = 0,
            Binding::Axis { axis, value } => {
                let snap = self.opposite_snap(axis, value, down);
                *self.axis_field(axis) = snap.unwrap_or(AXIS_CENTER);
            }
            Binding::Dpad(d) => self.current.buttons &= !d.flag(),
            Binding::Consume => {}
        }
    }

    /// Opposite-axis snap: releasing an axis binding while an opposite-sign
    /// binding on the same axis is held snaps to the held binding's OWN bound
    /// value — deliberate fix of legacy `Splitter.cs:161`, which hardcoded
    /// Min/Max and missed custom-valued opposites entirely
    /// (docs/research/design-risk-review.md §3 item 6). Several held opposite
    /// bindings resolve deterministically to the largest deflection
    /// (max `|value|`; build order breaks exact ties).
    fn opposite_snap(&self, axis: Axis, released: i16, down: &[u64]) -> Option<i16> {
        let mut best: Option<i16> = None;
        for &(a, v, k) in &self.axis_entries {
            let opposite = (released < 0 && v > 0) || (released > 0 && v < 0);
            if a != axis || !opposite || !self.holds(k, down) {
                continue;
            }
            best = Some(match best {
                Some(b) if b.unsigned_abs() >= v.unsigned_abs() => b,
                _ => v,
            });
        }
        best
    }

    // ---- chords ------------------------------------------------------------
    //
    // The whole feature is three steps, run only for slots that HAVE chords:
    //
    //   1. recompute which chords are satisfied, most-specific-first, and
    //      which keys they consume;
    //   2. recompute effective heldness for every holder the event could have
    //      moved (chord constituents, the event key, and the chords);
    //   3. apply the transitions — RELEASES first, then presses.
    //
    // Step 3's order is the no-stranded-button rule: when a chord activates,
    // the outputs its consumed constituents were holding are released in the
    // same pass that presses the chord's own output, so both land in ONE
    // delta batch (`collect_deltas` runs once per event). When it deactivates,
    // the chord's output is released and any constituent still down resumes
    // its individual binding in that same batch — no flicker, no stuck bit.

    /// Full pass: recompute chords, then apply every holder that moved.
    ///
    /// `event_key` is the key whose event triggered this (its own heldness is
    /// always rechecked). `full` rescans every holder — used when a device is
    /// yanked and everything on it must be released at once.
    fn sync(&mut self, down: &[u64], event_key: Option<u32>, full: bool) {
        self.recompute_chords(down);

        self.scan.clear();
        if full {
            self.scan.extend_from_slice(&self.all_holders);
            // `all_holders` already covers every macro step.
            self.macro_dirty.clear();
        } else {
            for i in 0..self.chord_keys.len() {
                self.scan.push(self.chord_keys[i]);
            }
            if let Some(k) = event_key {
                if !self.chord_keys.contains(&k) {
                    self.scan.push(k);
                }
            }
            for c in 0..self.chords.len() {
                self.scan.push(self.chord_base + c as u32);
            }
            self.drain_macro_dirty();
        }
        self.apply_scan(down);
    }

    /// Apply only what a macro transition moved — the tick path, where no key
    /// changed and chord activation therefore cannot have.
    fn sync_macros(&mut self, down: &[u64]) {
        self.scan.clear();
        self.drain_macro_dirty();
        if self.scan.is_empty() {
            return;
        }
        self.apply_scan(down);
    }

    /// Move the pending macro-step holders into `scan`, so they are applied in
    /// the same pass (and therefore the same delta batch) as everything else.
    fn drain_macro_dirty(&mut self) {
        for i in 0..self.macro_dirty.len() {
            let h = self.macro_dirty[i];
            if !self.scan.contains(&h) {
                self.scan.push(h);
            }
        }
        self.macro_dirty.clear();
    }

    /// Cheap pass for an event on a device that is NOT this slot's chord
    /// device: update that one key's heldness and apply it. Chord guards are
    /// deliberately not re-evaluated against a foreign device's key bitset.
    fn sync_key(&mut self, down: &[u64], dense: u32) {
        self.scan.clear();
        self.scan.push(dense);
        self.apply_scan(down);
    }

    /// Step 1: which chords are satisfied, and what do they consume.
    ///
    /// Chords are stored most-specific-first, so one forward pass resolves
    /// specificity: a chord is BLOCKED when a strictly more specific active
    /// chord already consumed one of its constituents (A+B+C beats A+B).
    /// Chords of EQUAL specificity never block each other — that is what makes
    /// `A+B → RT` and `A+B → LB` a plain multi-bind rather than a race;
    /// genuinely ambiguous equal-specificity pairs are a config error caught
    /// by validation, never resolved here by build order.
    fn recompute_chords(&mut self, down: &[u64]) {
        if self.chords.is_empty() {
            return;
        }
        self.consumed.iter_mut().for_each(|w| *w = 0);
        let mut i = 0;
        while i < self.chords.len() {
            let level = self.chords[i].specificity;
            let mut j = i;
            while j < self.chords.len() && self.chords[j].specificity == level {
                j += 1;
            }
            // Everything consumed so far came from strictly more specific
            // chords, because the list is sorted by specificity.
            self.blocked.copy_from_slice(&self.consumed);
            for c in i..j {
                let satisfied = {
                    let ch = &self.chords[c];
                    bit(down, ch.trigger)
                        && ch.when.iter().all(|&k| bit(down, k))
                        && !ch.unless.iter().any(|&k| bit(down, k))
                };
                let blocked = {
                    let ch = &self.chords[c];
                    bit(&self.blocked, ch.trigger) || ch.when.iter().any(|&k| bit(&self.blocked, k))
                };
                self.chords[c].active = satisfied && !blocked;
            }
            for c in i..j {
                if !self.chords[c].active {
                    continue;
                }
                let trigger = self.chords[c].trigger;
                set_bit(&mut self.consumed, trigger, true);
                for w in 0..self.chords[c].when.len() {
                    let k = self.chords[c].when[w];
                    set_bit(&mut self.consumed, k, true);
                }
            }
            i = j;
        }
    }

    /// Steps 2 and 3: recompute heldness for `self.scan`, then apply it.
    fn apply_scan(&mut self, down: &[u64]) {
        self.prev_held.copy_from_slice(&self.held);
        for i in 0..self.scan.len() {
            let h = self.scan[i];
            let now = if h >= self.macro_base {
                let (m, step) = self.step_of(h);
                self.macros[m].step == Some(step)
            } else if h >= self.chord_base {
                self.chords[(h - self.chord_base) as usize].active
            } else {
                bit(down, h) && !bit(&self.consumed, h)
            };
            set_bit(&mut self.held, h, now);
        }
        // Releases before presses: an endpoint handed over from a consumed
        // constituent to the chord (or back) must never be left down by the
        // losing holder, and must end the batch pressed if anyone still holds
        // it (the all-keys-up check inside `release` sees the NEW world,
        // because `held` is already updated above).
        for press in [false, true] {
            for i in 0..self.scan.len() {
                let h = self.scan[i];
                let was = bit(&self.prev_held, h);
                let now = bit(&self.held, h);
                if was == now || now != press {
                    continue;
                }
                for b in 0..self.holder_bindings[h as usize].len() {
                    let binding = self.holder_bindings[h as usize][b];
                    if now {
                        self.press(binding);
                    } else {
                        self.release(binding, down);
                    }
                }
            }
        }
    }

    /// Back to "nothing held": chord and macro state included.
    fn clear_chord_state(&mut self) {
        for chord in &mut self.chords {
            chord.active = false;
        }
        for mac in &mut self.macros {
            mac.step = None;
        }
        self.macro_dirty.clear();
        self.held.iter_mut().for_each(|w| *w = 0);
        self.prev_held.iter_mut().for_each(|w| *w = 0);
        self.consumed.iter_mut().for_each(|w| *w = 0);
    }

    // ---- macros ------------------------------------------------------------
    //
    // Three transitions, and every one of them only ever moves `step` and marks
    // the holders that changed. Nothing here touches `current`: the pad state
    // follows from `apply_scan`, which is the same code the keys and chords go
    // through, so a macro cannot invent a release path of its own.

    /// Which `(macro, step)` a macro holder id names.
    fn step_of(&self, holder: u32) -> (usize, u16) {
        for (m, mac) in self.macros.iter().enumerate() {
            let len = mac.ends.len() as u32;
            if holder >= mac.first_holder && holder < mac.first_holder + len {
                return (m, (holder - mac.first_holder) as u16);
            }
        }
        debug_assert!(false, "holder {holder} is not a macro step");
        (0, 0)
    }

    /// A holder whose macro state moved since the last apply.
    fn mark(&mut self, holder: u32) {
        if !self.macro_dirty.contains(&holder) {
            self.macro_dirty.push(holder);
        }
    }

    /// When step `step` of macro `m` should end, given the wall clock is at
    /// `now`.
    ///
    /// Absolute first (`start + ends[step]`), which is what makes the schedule
    /// drift-free. The `max` is the late-wake rule: if that instant has already
    /// passed, the step is not skipped — it is published for its sampling
    /// minimum and the rest of the timeline slides, because an input the game
    /// never sampled is not an input at all (§0.2).
    fn macro_deadline(&self, m: usize, step: u16, now: u64) -> u64 {
        let mac = &self.macros[m];
        let i = usize::from(step);
        let scheduled = mac.start + u64::from(mac.ends[i]);
        scheduled.max(now + u64::from(mac.min_visible[i]))
    }

    /// The trigger key went down.
    fn macro_start(&mut self, m: usize, now: u64, si: u8, timers: &mut Timers) {
        let first = self.macros[m].first_holder;
        if self.macros[m].ends.is_empty() {
            return; // an empty macro is inert; validation names it
        }
        if let Some(step) = self.macros[m].step {
            match self.macros[m].retrigger {
                // Mashing must not stutter a sequence back to its first step.
                Retrigger::Ignore => return,
                // The old step's holder is released and the new one pressed in
                // the same batch, so a restart strands nothing.
                Retrigger::Restart => self.mark(first + u32::from(step)),
            }
        }
        // A run's whole timeline is fixed here, from THIS instant.
        self.macros[m].start = now;
        self.macros[m].step = Some(0);
        self.mark(first);
        let deadline = self.macro_deadline(m, 0, now);
        timers.arm(si, m as u16, deadline);
    }

    /// Stop now and release everything this macro held. The one path that
    /// [`OnRelease::Abort`], a device yank, a session stop and an escape
    /// gesture all share.
    fn macro_cancel(&mut self, m: usize, si: u8, timers: &mut Timers) {
        let first = self.macros[m].first_holder;
        if let Some(step) = self.macros[m].step.take() {
            self.mark(first + u32::from(step));
        }
        timers.cancel(si, m as u16);
    }

    /// A deadline was reached: move to the next step, or finish.
    fn macro_advance(&mut self, m: usize, now: u64, si: u8, timers: &mut Timers) {
        let first = self.macros[m].first_holder;
        let Some(step) = self.macros[m].step else {
            return;
        };
        self.mark(first + u32::from(step));
        let next = step + 1;
        if usize::from(next) >= self.macros[m].ends.len() {
            self.macros[m].step = None;
            timers.cancel(si, m as u16);
            return;
        }
        self.macros[m].step = Some(next);
        self.mark(first + u32::from(next));
        let deadline = self.macro_deadline(m, next, now);
        timers.arm(si, m as u16, deadline);
    }

    /// A key moved on this slot's chord device: start or abort whatever it
    /// triggers. Returns `true` if any macro state changed.
    fn macro_key(&mut self, dense: u32, down: bool, now: u64, si: u8, timers: &mut Timers) -> bool {
        let mut moved = false;
        for m in 0..self.macros.len() {
            if !self.macros[m].triggers.contains(&dense) {
                continue;
            }
            if down {
                self.macro_start(m, now, si, timers);
                moved = true;
            } else if self.macros[m].on_release == OnRelease::Abort {
                // `finish` (the default, and the fighting-game expectation)
                // deliberately does nothing here: letting go of the button
                // must not eat the second half of the quarter-circle.
                self.macro_cancel(m, si, timers);
                moved = true;
            }
        }
        moved
    }

    /// Other input arrived: abort whichever running macros said it should.
    ///
    /// Runs BEFORE [`SlotRuntime::macro_key`] on the same event, so one press
    /// can abort one macro and start another, and both land in the single delta
    /// batch that event produces. A macro is never interrupted by its own
    /// trigger — that is a retrigger, and [`Retrigger`] decides it.
    fn macro_interrupt(&mut self, dense: u32, si: u8, timers: &mut Timers) {
        for m in 0..self.macros.len() {
            let Some(step) = self.macros[m].step else {
                continue;
            };
            if !self.macros[m].interrupt.is_active() || self.macros[m].triggers.contains(&dense) {
                continue;
            }
            let abort = match self.macros[m].interrupt {
                Interrupt::None => false,
                // Any key this slot binds or triggers on. A key the slot does
                // not use at all never reaches here — `sync_slots` only lists
                // slots that care about it — so "any input" means any input
                // THIS player made.
                Interrupt::AnyInput => true,
                Interrupt::Opposing => {
                    let holding = self.macros[m].first_holder + u32::from(step);
                    // Rule 1: a direction against one this step is holding.
                    let contradicts = self.holder_bindings[dense as usize].iter().any(|&pressed| {
                        self.holder_bindings[holding as usize]
                            .iter()
                            .any(|&held| crate::socd::opposes(pressed, held))
                    });
                    // Rule 2: asking for a different sequence.
                    contradicts
                        || self
                            .macros
                            .iter()
                            .enumerate()
                            .any(|(other, mac)| other != m && mac.triggers.contains(&dense))
                }
            };
            if abort {
                self.macro_cancel(m, si, timers);
            }
        }
    }

    /// Cancel every macro of this slot — the "everything releases on the way
    /// out" primitive. The caller applies the resulting releases.
    fn cancel_all_macros(&mut self, si: u8, timers: &mut Timers) -> bool {
        let mut moved = false;
        for m in 0..self.macros.len() {
            if self.macros[m].step.is_some() {
                self.macro_cancel(m, si, timers);
                moved = true;
            }
        }
        moved
    }
}

fn bit(words: &[u64], k: u32) -> bool {
    words[(k / 64) as usize] & (1 << (k % 64)) != 0
}

fn set_bit(words: &mut [u64], k: u32, value: bool) {
    let word = (k / 64) as usize;
    let mask = 1u64 << (k % 64);
    if value {
        words[word] |= mask;
    } else {
        words[word] &= !mask;
    }
}

/// Everything [`Engine::handle`] dispatches through, precompiled.
///
/// Split out of the engine so it can be BUILT OFF THE HOT PATH and swapped in
/// later ([`Engine::swap_tables`]): a binding edit rebuilds this on the
/// supervisor's own thread and the engine thread only moves the pointers. The
/// engine keeps no other per-run allocation, so a swap allocates nothing at
/// all where it happens.
pub struct EngineTables {
    slots: Vec<SlotRuntime>,
    devices: Vec<DeviceId>,
    index: HashMap<Key, u32>,
    targets: Vec<SmallVec<[Target; 4]>>,
    down: Vec<u64>,
    words: usize,
    /// A permanently-empty key bitset, for a slot with no input device at all:
    /// a macro can still be cancelled there, and releasing needs *some* `down`.
    zeros: Vec<u64>,
    /// Dense key -> the stateful slots that must resync when it moves. Empty
    /// (and never consulted) when no preset in the build has a chord or macro.
    sync_slots: Vec<SmallVec<[u8; 2]>>,
    /// `false` ⇒ the engine takes the pre-chord path end to end.
    has_state: bool,
    /// `false` ⇒ the engine never looks at the clock or the timer list, and
    /// [`Engine::next_deadline`] is always `None`.
    has_macros: bool,
    /// Built here, off the hot path, so a hot swap moves it rather than
    /// allocating one on the engine thread.
    timers: Timers,
}

impl EngineTables {
    /// Precompile the dispatch tables for `slots`.
    ///
    /// Preconditions (validated upstream by `SlotSpec`/config): slot numbers
    /// are unique and in 1..=8.
    pub fn build(slots: Vec<ResolvedSlot>) -> Self {
        debug_assert!(
            {
                let mut numbers: Vec<u8> = slots.iter().map(|s| s.spec.number).collect();
                numbers.sort_unstable();
                numbers.windows(2).all(|w| w[0] != w[1])
            },
            "slot numbers must be unique"
        );

        fn intern(devices: &mut Vec<DeviceId>, dev: &DeviceId) -> u8 {
            match devices.iter().position(|d| d == dev) {
                Some(i) => i as u8,
                None => {
                    devices.push(dev.clone());
                    (devices.len() - 1) as u8
                }
            }
        }

        fn intern_key(
            index: &mut HashMap<Key, u32>,
            targets: &mut Vec<SmallVec<[Target; 4]>>,
            key: Key,
        ) -> u32 {
            *index.entry(key).or_insert_with(|| {
                targets.push(SmallVec::new());
                (targets.len() - 1) as u32
            })
        }

        let mut devices: Vec<DeviceId> = Vec::new();
        let mut index: HashMap<Key, u32> = HashMap::new();
        let mut targets: Vec<SmallVec<[Target; 4]>> = Vec::new();
        let mut runtimes = Vec::with_capacity(slots.len());

        for (si, rs) in slots.iter().enumerate() {
            let keyboard = rs.spec.keyboard.as_ref().map(|d| intern(&mut devices, d));
            let mouse = rs.spec.mouse.as_ref().map(|d| intern(&mut devices, d));
            let mut endpoint_keys: HashMap<Binding, SmallVec<[u32; 4]>> = HashMap::new();
            let mut axis_entries = Vec::new();

            for &(key, binding) in &rs.preset.entries {
                // Inert rows: placeholders ("function present, unbound"), and
                // `Consume` outside a guard, which consumes nothing by
                // definition (validation reports it).
                if key == Key::None || binding == Binding::Consume {
                    continue;
                }
                let dense = intern_key(&mut index, &mut targets, key);
                targets[dense as usize].push(Target {
                    slot: si as u8,
                    binding,
                });
                endpoint_keys.entry(binding).or_default().push(dense);
                if let Binding::Axis { axis, value } = binding {
                    axis_entries.push((axis, value, dense));
                }
            }

            // Chords, most-specific-first. Guard keys are interned even when
            // nothing else binds them, so an event on a dedicated chord key
            // still reaches the engine (`handle` early-returns on unknown
            // keys). A chord keyed `Key::None` is an inert placeholder, like
            // an unbound entry row.
            let mut chords: Vec<&Chord> = rs
                .preset
                .chords
                .iter()
                .filter(|c| c.key != Key::None)
                .collect();
            // Stable, so equal-specificity chords keep preset order — the
            // multi-bind case, where both fire and order is irrelevant.
            chords.sort_by_key(|c| std::cmp::Reverse(c.specificity()));
            let chord_rts: Vec<ChordRt> = chords
                .iter()
                .map(|c| ChordRt {
                    binding: c.binding,
                    trigger: intern_key(&mut index, &mut targets, c.key),
                    when: c
                        .when
                        .iter()
                        .filter(|k| **k != Key::None)
                        .map(|&k| intern_key(&mut index, &mut targets, k))
                        .collect(),
                    unless: c
                        .unless
                        .iter()
                        .filter(|k| **k != Key::None)
                        .map(|&k| intern_key(&mut index, &mut targets, k))
                        .collect(),
                    specificity: c.specificity().min(usize::from(u16::MAX)) as u16,
                    active: false,
                })
                .collect();

            // Macros. Trigger keys are interned like guard keys, so a dedicated
            // macro button with no other binding still reaches the engine. A
            // macro with no steps, and a trigger with a dangling index, are
            // both dropped here and reported by validation — the engine never
            // panics on a preset it was handed.
            let macro_rts: Vec<MacroRt> = rs
                .preset
                .macros
                .defs
                .iter()
                .enumerate()
                .map(|(m, def)| MacroRt {
                    ends: def.deadlines().collect(),
                    min_visible: def.steps.iter().map(|s| s.min_visible_ms()).collect(),
                    // Patched in the second pass, which knows the final dense
                    // key count (and therefore where holders start).
                    first_holder: 0,
                    on_release: def.on_release,
                    retrigger: def.retrigger,
                    interrupt: def.interrupt,
                    triggers: rs
                        .preset
                        .macros
                        .triggers
                        .iter()
                        .filter(|t| usize::from(t.index) == m && t.key != Key::None)
                        .map(|t| intern_key(&mut index, &mut targets, t.key))
                        .collect(),
                    step: None,
                    start: 0,
                })
                .collect();

            runtimes.push(SlotRuntime {
                number: rs.spec.number,
                keyboard,
                mouse,
                chord_device: keyboard.or(mouse),
                endpoint_keys,
                axis_entries,
                current: PadState::default(),
                last_emitted: PadState::default(),
                chords: chord_rts,
                chord_base: 0,
                holder_bindings: Vec::new(),
                all_holders: Vec::new(),
                chord_keys: SmallVec::new(),
                held: Vec::new(),
                prev_held: Vec::new(),
                consumed: Vec::new(),
                blocked: Vec::new(),
                scan: Vec::new(),
                macros: macro_rts,
                macro_base: 0,
                macro_dirty: Vec::new(),
                stateful: false,
            });
        }

        let words = targets.len().div_ceil(64).max(1);
        let down = vec![0u64; words * devices.len()];
        for slot in &mut runtimes {
            slot.stateful = !slot.chords.is_empty() || !slot.macros.is_empty();
        }
        let has_state = runtimes.iter().any(|s| s.stateful);
        let macro_count: usize = runtimes.iter().map(|s| s.macros.len()).sum();

        // Second pass: everything that needs the FINAL dense-key count. Only
        // chorded slots allocate any of it, so a chord-free build is byte-for
        // byte the pre-chord table set.
        let mut sync_slots: Vec<SmallVec<[u8; 2]>> = Vec::new();
        if has_state {
            let chord_base = targets.len() as u32;
            sync_slots = vec![SmallVec::new(); targets.len()];
            for (si, rs) in slots.iter().enumerate() {
                if !runtimes[si].stateful {
                    continue;
                }
                let macro_base = chord_base + runtimes[si].chords.len() as u32;
                // Macro steps are holders too, laid out after the chords: one
                // holder per step, so "this macro is on step i" is a bit.
                let mut steps = 0u32;
                for m in 0..runtimes[si].macros.len() {
                    runtimes[si].macros[m].first_holder = macro_base + steps;
                    steps += runtimes[si].macros[m].ends.len() as u32;
                }
                let holders = (macro_base + steps) as usize;
                let mut holder_bindings: Vec<SmallVec<[Binding; 2]>> =
                    vec![SmallVec::new(); holders];
                for &(key, binding) in &rs.preset.entries {
                    if key == Key::None || binding == Binding::Consume {
                        continue;
                    }
                    holder_bindings[index[&key] as usize].push(binding);
                }
                let mut chord_keys: SmallVec<[u32; 8]> = SmallVec::new();
                for c in 0..runtimes[si].chords.len() {
                    let id = chord_base + c as u32;
                    let chord = &runtimes[si].chords[c];
                    // A consume-only chord holds nothing: it never presses,
                    // never releases, and never joins an endpoint's holder
                    // list. It still consumes, which is the whole point.
                    if chord.binding != Binding::Consume {
                        holder_bindings[id as usize].push(chord.binding);
                    }
                    for k in std::iter::once(chord.trigger).chain(chord.when.iter().copied()) {
                        if !chord_keys.contains(&k) {
                            chord_keys.push(k);
                        }
                    }
                }
                // Chords join the all-keys-up and opposite-axis tables as
                // ordinary holders, so "released only when nothing drives it"
                // covers a chord exactly like a key.
                for c in 0..runtimes[si].chords.len() {
                    let id = chord_base + c as u32;
                    let binding = runtimes[si].chords[c].binding;
                    if binding == Binding::Consume {
                        continue;
                    }
                    runtimes[si]
                        .endpoint_keys
                        .entry(binding)
                        .or_default()
                        .push(id);
                    if let Binding::Axis { axis, value } = binding {
                        runtimes[si].axis_entries.push((axis, value, id));
                    }
                }
                // A macro step drives its `hold` set, and joins the all-keys-up
                // and opposite-axis tables like any other holder: an endpoint a
                // macro and a key both drive stays down while either holds it,
                // and a step handing an endpoint to the next step never emits
                // an intermediate release.
                for m in 0..runtimes[si].macros.len() {
                    let first = runtimes[si].macros[m].first_holder;
                    for (i, step) in rs.preset.macros.defs[m].steps.iter().enumerate() {
                        let id = first + i as u32;
                        for &binding in &step.hold {
                            if binding == Binding::Consume {
                                continue; // nothing to hold; validation says so
                            }
                            if !holder_bindings[id as usize].contains(&binding) {
                                holder_bindings[id as usize].push(binding);
                                runtimes[si]
                                    .endpoint_keys
                                    .entry(binding)
                                    .or_default()
                                    .push(id);
                                if let Binding::Axis { axis, value } = binding {
                                    runtimes[si].axis_entries.push((axis, value, id));
                                }
                            }
                        }
                    }
                }

                // Chords and macro steps are always holders even when they
                // drive nothing, so a full rescan (device yank) still clears
                // their `held` bit.
                let all_holders: Vec<u32> = (0..holders as u32)
                    .filter(|h| *h >= chord_base || !holder_bindings[*h as usize].is_empty())
                    .collect();

                // Which keys make this slot resync: anything it binds, every
                // key any of its guards mentions, and every macro trigger.
                let mut touches: Vec<u32> = all_holders
                    .iter()
                    .copied()
                    .filter(|h| *h < chord_base)
                    .collect();
                for chord in &runtimes[si].chords {
                    touches.push(chord.trigger);
                    touches.extend(chord.when.iter().copied());
                    touches.extend(chord.unless.iter().copied());
                }
                for mac in &runtimes[si].macros {
                    touches.extend(mac.triggers.iter().copied());
                }
                touches.sort_unstable();
                touches.dedup();
                for key in touches {
                    sync_slots[key as usize].push(si as u8);
                }

                let hwords = holders.div_ceil(64).max(1);
                let slot = &mut runtimes[si];
                slot.chord_base = chord_base;
                slot.macro_base = macro_base;
                // One entry per step is the worst case (`mark` dedupes), so the
                // hot path can never reallocate this either.
                slot.macro_dirty = Vec::with_capacity(steps as usize);
                // Big enough for BOTH scan shapes, so `scan.push` in the hot
                // path can never reallocate.
                let scan_cap = all_holders
                    .len()
                    .max(chord_keys.len() + 1 + slot.chords.len() + steps as usize)
                    .max(1);
                slot.scan = Vec::with_capacity(scan_cap);
                slot.holder_bindings = holder_bindings;
                slot.all_holders = all_holders;
                slot.chord_keys = chord_keys;
                slot.held = vec![0u64; hwords];
                slot.prev_held = vec![0u64; hwords];
                slot.consumed = vec![0u64; words];
                slot.blocked = vec![0u64; words];
            }
        }

        Self {
            slots: runtimes,
            devices,
            index,
            targets,
            zeros: vec![0u64; words],
            down,
            words,
            sync_slots,
            has_state,
            has_macros: macro_count > 0,
            timers: Timers::with_capacity(macro_count),
        }
    }

    /// Slot numbers in build order — the shape a hot swap must preserve.
    pub fn slot_numbers(&self) -> Vec<u8> {
        self.slots.iter().map(|s| s.number).collect()
    }
}

/// The pure translation engine: `KeyEvent`s in, `PadDelta`s out.
///
/// Contract (each clause maps to a legacy behavior in
/// `docs/research/design-risk-review.md` §3):
///
/// - **Fan-out**: an event is translated for *every* slot whose keyboard or
///   mouse matches the event's device — no early break. One physical keyboard
///   (an I-PAC4) legitimately drives up to 4 pads with disjoint presets.
/// - **All-keys-up**: a function releases only when *every* key mapped to it in
///   that slot's preset is up on the event's device. Aggregation is by
///   `Binding` equality, which reproduces the legacy cross-category
///   custom-function reverse lookup.
/// - **Opposite-axis snap**: releasing a key bound to axis value `v` while a
///   key bound to the same axis with an opposite-sign value is still held snaps
///   the axis to the *held binding's own value* — not hardcoded ±32767. This is
///   the deliberate fix of the legacy custom-axis inconsistency
///   (`Splitter.cs:161`); document any test that depends on it.
/// - **Chords with consumption** (docs/INPUT-TRANSFORMS.md §1b): a
///   [`Chord`] applies only while its guard holds, and while it applies its
///   constituents are SUPPRESSED — their unguarded entries stop driving
///   anything, so the game sees the chord's output instead of the parts.
///   Activation and the releases it forces land in the SAME delta batch (no
///   stranded buttons); on release, constituents still down resume their
///   individual bindings in that same batch. A larger guard beats a smaller
///   one where they overlap; equal specificity is a config error, not a race.
///   **There is no deferral**: a constituent that is also individually bound
///   flashes its own output between the first and the second keypress. Chord
///   keys with no individual binding are free and instant, and validation
///   warns when they are not.
/// - **State diffing**: a `PadDelta` is emitted only when a slot's `PadState`
///   genuinely changed versus the last emitted state (legacy early-returned on
///   unchanged state so only real transitions hit the driver).
/// - Entries keyed `Key::None` never match any event.
pub struct Engine {
    slots: Vec<SlotRuntime>,
    /// Distinct devices assigned to any slot; events from others are ignored.
    devices: Vec<DeviceId>,
    /// Key -> dense id. Built once; `handle` never scans presets.
    index: HashMap<Key, u32>,
    /// Dense id -> dispatch targets across all slots (fan-out preserved).
    targets: Vec<SmallVec<[Target; 4]>>,
    /// Per-device key bitsets, `words` u64s per device: a key held on device A
    /// is distinct from the same key held on device B.
    down: Vec<u64>,
    words: usize,
    /// Stand-in key bitset for a slot with no device (see [`EngineTables`]).
    zeros: Vec<u64>,
    /// Dense key -> stateful slots to resync (see [`EngineTables`]).
    sync_slots: Vec<SmallVec<[u8; 2]>>,
    /// The one branch chords and macros cost a configuration with neither.
    has_state: bool,
    has_macros: bool,
    /// Every armed macro deadline, in one ordered list.
    timers: Timers,
    /// The engine's notion of "now", in milliseconds, as last supplied by
    /// [`Engine::tick`] or [`Engine::handle_at`]. The engine never reads a
    /// clock itself — that is what makes macros a pure function of
    /// `(events, clock)` and therefore replayable.
    now: u64,
}

impl Engine {
    /// Build the engine over resolved slots.
    ///
    /// Preconditions (validated upstream by `SlotSpec`/config): slot numbers
    /// are unique and in 1..=8. All lookups are precompiled in
    /// [`EngineTables::build`] so [`Engine::handle`] performs no per-event
    /// preset iteration and no heap allocation.
    pub fn new(slots: Vec<ResolvedSlot>) -> Self {
        Self::from_tables(EngineTables::build(slots))
    }

    /// Build the engine over tables somebody else precompiled.
    pub fn from_tables(tables: EngineTables) -> Self {
        let EngineTables {
            slots,
            devices,
            index,
            targets,
            down,
            words,
            zeros,
            sync_slots,
            has_state,
            has_macros,
            timers,
        } = tables;
        Self {
            slots,
            devices,
            index,
            targets,
            down,
            words,
            zeros,
            sync_slots,
            has_state,
            has_macros,
            timers,
            now: 0,
        }
    }

    /// Replace the dispatch tables **in place** — the binding hot-swap.
    ///
    /// This is what makes "edit one binding" stop meaning "unplug four pads":
    /// the pads, their handles and the capture filters are untouched; only the
    /// key→function tables change. `tables` must have been built by
    /// [`EngineTables::build`] on another thread, so the swap itself moves
    /// pointers and allocates nothing.
    ///
    /// **Every control is released across the swap.** Dense key ids are an
    /// artifact of the old tables, so the per-device down bitset cannot carry
    /// over; keeping the old pad state instead would strand whatever was held
    /// at the moment of the edit (the one failure a mapper must never cause).
    /// The returned deltas are exactly the neutral states the caller has to
    /// push so no pad is left holding a button — slots that were already
    /// neutral produce nothing, which is the ordinary case (nobody is leaning
    /// on the panel while they retype a binding).
    ///
    /// Slot state is matched by slot NUMBER, not by position, so a swap that
    /// keeps the same slots in a different build order still reports honestly.
    pub fn swap_tables(&mut self, tables: EngineTables) -> Deltas {
        let previous: SmallVec<[(u8, PadState); 8]> = self
            .slots
            .iter()
            .map(|s| (s.number, s.last_emitted))
            .collect();
        let EngineTables {
            mut slots,
            devices,
            index,
            targets,
            down,
            words,
            zeros,
            sync_slots,
            has_state,
            has_macros,
            timers,
        } = tables;
        for slot in &mut slots {
            slot.current = PadState::default();
            slot.last_emitted = previous
                .iter()
                .find(|(number, _)| *number == slot.number)
                .map_or_else(PadState::default, |(_, state)| *state);
        }
        self.slots = slots;
        self.devices = devices;
        self.index = index;
        self.targets = targets;
        self.down = down;
        self.words = words;
        self.zeros = zeros;
        self.sync_slots = sync_slots;
        self.has_state = has_state;
        self.has_macros = has_macros;
        // Macros in flight are dropped with the old tables, and the neutral
        // deltas below release whatever they were holding. Carrying a run
        // across a rebind would mean stepping a sequence whose steps no longer
        // exist — the one failure a mapper must never cause.
        self.timers = timers;

        let mut deltas = Deltas::new();
        self.collect_deltas(&mut deltas);
        deltas
    }

    /// Translate one key event into pad-state deltas.
    ///
    /// Applies the full contract above: fan-out to all matching slots,
    /// per-device key-state tracking, all-keys-up release, opposite-axis snap,
    /// then state diffing. Events from devices assigned to no slot, and
    /// entries keyed `Key::None`, produce no deltas. Repeated key-down of an
    /// already-down key must not produce a delta (diff idempotence).
    pub fn handle(&mut self, ev: &KeyEvent) -> Deltas {
        self.handle_at(ev, self.now)
    }

    /// [`Engine::handle`], with the current time in milliseconds.
    ///
    /// `now` is only ever *used* by macros; a configuration with none behaves
    /// identically whatever is passed, which is what keeps the M3 replay corpus
    /// digest fixed. It is supplied rather than read so the whole feature is a
    /// pure function of `(events, clock)` and testable with a fake one.
    ///
    /// `KeyEvent::t` is deliberately NOT used for this: its unit is
    /// backend-defined (QPC ticks on Windows), and a step duration is
    /// milliseconds by definition of the sampling rule.
    pub fn handle_at(&mut self, ev: &KeyEvent, now: u64) -> Deltas {
        self.now = now;
        let mut deltas = Deltas::new();
        let Some(dev) = self.devices.iter().position(|d| d == &ev.device) else {
            return deltas;
        };
        let Some(&dense) = self.index.get(&ev.key) else {
            return deltas;
        };

        // Key state updates before translation: the all-keys-up check must see
        // this transition applied (legacy interceptor state worked the same).
        let word = dev * self.words + (dense / 64) as usize;
        let mask = 1u64 << (dense % 64);
        if ev.down {
            self.down[word] |= mask;
        } else {
            self.down[word] &= !mask;
        }

        let dev8 = dev as u8;
        // Fan-out: every matching slot is fed; legacy's dispatch loop has no
        // break (Splitter.cs:408-419) — the I-PAC4 case, contractual.
        for &t in &self.targets[dense as usize] {
            let down = &self.down[dev * self.words..(dev + 1) * self.words];
            let slot = &mut self.slots[t.slot as usize];
            if slot.keyboard != Some(dev8) && slot.mouse != Some(dev8) {
                continue;
            }
            // A stateful slot resolves its whole holder set below instead: the
            // same press/release, but after consumption has been applied and
            // whatever the macro scheduler moved.
            if slot.stateful {
                continue;
            }
            if ev.down {
                slot.press(t.binding);
            } else {
                slot.release(t.binding, down);
            }
        }
        if self.has_state {
            for i in 0..self.sync_slots[dense as usize].len() {
                let si = self.sync_slots[dense as usize][i] as usize;
                let down = &self.down[dev * self.words..(dev + 1) * self.words];
                let slot = &mut self.slots[si];
                if slot.keyboard != Some(dev8) && slot.mouse != Some(dev8) {
                    continue;
                }
                if slot.chord_device == Some(dev8) {
                    // Macro triggers are evaluated on the slot's chord device
                    // for the same reason guards are: a sequence belongs to one
                    // panel, and "one device decides" is already the rule.
                    // Interrupts first, so one press can stop one sequence and
                    // start another inside a single delta batch.
                    if ev.down {
                        slot.macro_interrupt(dense, si as u8, &mut self.timers);
                    }
                    slot.macro_key(dense, ev.down, now, si as u8, &mut self.timers);
                    slot.sync(down, Some(dense), false);
                } else {
                    slot.sync_key(down, dense);
                }
            }
        }

        self.collect_deltas(&mut deltas);
        deltas
    }

    /// Unplug-mid-press: treat every key currently down on `dev` as released
    /// at once and return the resulting deltas (stuck-key invariant — a
    /// removed device must leave no residual contribution on any pad).
    pub fn release_device(&mut self, dev: &DeviceId) -> Deltas {
        let mut deltas = Deltas::new();
        let Some(dev) = self.devices.iter().position(|d| d == dev) else {
            return deltas;
        };
        let base = dev * self.words;
        let dev8 = dev as u8;

        for dense in 0..self.targets.len() as u32 {
            let word = base + (dense / 64) as usize;
            let mask = 1u64 << (dense % 64);
            if self.down[word] & mask == 0 {
                continue;
            }
            self.down[word] &= !mask;
            for &t in &self.targets[dense as usize] {
                let down = &self.down[base..base + self.words];
                let slot = &mut self.slots[t.slot as usize];
                if slot.keyboard != Some(dev8) && slot.mouse != Some(dev8) {
                    continue;
                }
                if slot.stateful {
                    continue;
                }
                slot.release(t.binding, down);
            }
            // A stateful slot fed by ANOTHER of its devices: this key's own
            // heldness is all that can have moved.
            if self.has_state {
                for i in 0..self.sync_slots[dense as usize].len() {
                    let si = self.sync_slots[dense as usize][i] as usize;
                    let down = &self.down[base..base + self.words];
                    let slot = &mut self.slots[si];
                    if slot.keyboard != Some(dev8) && slot.mouse != Some(dev8) {
                        continue;
                    }
                    if slot.chord_device != Some(dev8) {
                        slot.sync_key(down, dense);
                    }
                }
            }
        }
        // Every stateful slot on this device now resolves from an empty key
        // state: chords fall inactive, consumption lifts, macros in flight are
        // cancelled, everything releases — in one delta batch, which is the
        // stuck-key invariant. A macro is the one holder a yank could not
        // clear on its own: nobody is going to release its "key".
        if self.has_state {
            for si in 0..self.slots.len() {
                let down = &self.down[base..base + self.words];
                let slot = &mut self.slots[si];
                if !slot.stateful || slot.chord_device != Some(dev8) {
                    continue;
                }
                slot.cancel_all_macros(si as u8, &mut self.timers);
                slot.sync(down, None, true);
            }
        }

        self.collect_deltas(&mut deltas);
        deltas
    }

    /// Advance every macro whose deadline has passed, and publish what moved.
    ///
    /// This is the whole scheduler surface: the engine thread calls it when it
    /// wakes, whether that was an input event, a poll, or [`Engine::next_deadline`]
    /// coming due. Late is fine — a step re-arms from `now`, so a delayed tick
    /// makes a macro run *long* rather than making a step invisible (§0.2).
    /// It never blocks, never allocates, and is a no-op when nothing is armed.
    pub fn tick(&mut self, now: u64) -> Deltas {
        let mut deltas = Deltas::new();
        self.now = now;
        if !self.has_macros || self.timers.next().is_none_or(|next| next > now) {
            return deltas;
        }
        while let Some((si, mac)) = self.timers.pop_due(now) {
            self.slots[si as usize].macro_advance(usize::from(mac), now, si, &mut self.timers);
        }
        self.apply_macro_moves(&mut deltas);
        deltas
    }

    /// When [`Engine::tick`] next has something to do, in the same milliseconds
    /// `now` is expressed in. `None` ⇒ nothing is armed and the caller may
    /// sleep on its input channel alone.
    pub fn next_deadline(&self) -> Option<u64> {
        self.timers.next()
    }

    /// Cancel every macro in flight and release everything they held.
    ///
    /// The explicit "on the way out" path: session stop and the emergency
    /// escape gesture both call it, because both mean *the player is no longer
    /// driving this pad* — and a quarter-circle finishing into a game the user
    /// just escaped from is exactly the stuck-input failure this project
    /// refuses to ship. Device yank and hot-swap get the same guarantee
    /// through [`Engine::release_device`] and [`Engine::swap_tables`].
    pub fn cancel_macros(&mut self) -> Deltas {
        let mut deltas = Deltas::new();
        if !self.has_macros {
            return deltas;
        }
        for si in 0..self.slots.len() {
            self.slots[si].cancel_all_macros(si as u8, &mut self.timers);
        }
        self.apply_macro_moves(&mut deltas);
        deltas
    }

    /// Apply whatever macro transitions marked, for every slot that has any.
    fn apply_macro_moves(&mut self, deltas: &mut Deltas) {
        for si in 0..self.slots.len() {
            if self.slots[si].macro_dirty.is_empty() {
                continue;
            }
            let down = match self.slots[si].chord_device {
                Some(dev) => {
                    let base = usize::from(dev) * self.words;
                    &self.down[base..base + self.words]
                }
                // No input device at all: nothing can be held by a key, so the
                // all-keys-up check reads an empty world. Still releases.
                None => &self.zeros[..],
            };
            self.slots[si].sync_macros(down);
        }
        self.collect_deltas(deltas);
    }

    /// Clear all per-device key state and pad states back to neutral.
    ///
    /// Emits nothing: after a reset the caller is expected to submit
    /// `PadState::default()` to each pad itself (emulation stop path).
    pub fn reset(&mut self) {
        self.down.iter_mut().for_each(|w| *w = 0);
        self.timers.clear();
        for slot in &mut self.slots {
            slot.current = PadState::default();
            slot.last_emitted = PadState::default();
            slot.clear_chord_state();
        }
    }

    /// Current pad state for slot `number` (equal to the last emitted state —
    /// the engine syncs them before returning from `handle`/`release_device`).
    pub fn pad_state(&self, number: u8) -> Option<PadState> {
        self.slots
            .iter()
            .find(|s| s.number == number)
            .map(|s| s.current)
    }

    fn collect_deltas(&mut self, out: &mut Deltas) {
        for slot in &mut self.slots {
            if slot.current != slot.last_emitted {
                slot.last_emitted = slot.current;
                out.push(PadDelta {
                    slot: slot.number,
                    state: slot.current,
                });
            }
        }
    }
}
