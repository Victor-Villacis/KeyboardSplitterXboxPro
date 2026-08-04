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
use crate::pad::{Axis, PadState, Trigger, AXIS_CENTER};
use crate::preset::{Binding, Preset};
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

struct SlotRuntime {
    number: u8,
    /// Index into `Engine::devices`.
    keyboard: Option<u8>,
    mouse: Option<u8>,
    /// Endpoint -> dense ids of every key driving it (all-keys-up rule).
    endpoint_keys: HashMap<Binding, SmallVec<[u32; 4]>>,
    /// Flat axis entries for the opposite-axis scan on release.
    axis_entries: Vec<(Axis, i16, u32)>,
    current: PadState,
    last_emitted: PadState,
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
        }
    }

    /// `down` is the event device's key bitset, already updated for the
    /// triggering release.
    fn release(&mut self, binding: Binding, down: &[u64]) {
        // All-keys-up rule: the endpoint stays active while ANY key mapped to
        // it (on this device) is still held.
        if let Some(keys) = self.endpoint_keys.get(&binding) {
            if keys.iter().any(|&k| bit(down, k)) {
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
            if a != axis || !opposite || !bit(down, k) {
                continue;
            }
            best = Some(match best {
                Some(b) if b.unsigned_abs() >= v.unsigned_abs() => b,
                _ => v,
            });
        }
        best
    }
}

fn bit(words: &[u64], k: u32) -> bool {
    words[(k / 64) as usize] & (1 << (k % 64)) != 0
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
}

impl Engine {
    /// Build the engine over resolved slots.
    ///
    /// Preconditions (validated upstream by `SlotSpec`/config): slot numbers
    /// are unique and in 1..=4. All lookups are precompiled here so
    /// [`Engine::handle`] performs no per-event preset iteration and no heap
    /// allocation.
    pub fn new(slots: Vec<ResolvedSlot>) -> Self {
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
                if key == Key::None {
                    continue; // inert placeholder rows ("function present, unbound")
                }
                let dense = *index.entry(key).or_insert_with(|| {
                    targets.push(SmallVec::new());
                    (targets.len() - 1) as u32
                });
                targets[dense as usize].push(Target {
                    slot: si as u8,
                    binding,
                });
                endpoint_keys.entry(binding).or_default().push(dense);
                if let Binding::Axis { axis, value } = binding {
                    axis_entries.push((axis, value, dense));
                }
            }

            runtimes.push(SlotRuntime {
                number: rs.spec.number,
                keyboard,
                mouse,
                endpoint_keys,
                axis_entries,
                current: PadState::default(),
                last_emitted: PadState::default(),
            });
        }

        let words = targets.len().div_ceil(64).max(1);
        let down = vec![0u64; words * devices.len()];

        Self {
            slots: runtimes,
            devices,
            index,
            targets,
            down,
            words,
        }
    }

    /// Translate one key event into pad-state deltas.
    ///
    /// Applies the full contract above: fan-out to all matching slots,
    /// per-device key-state tracking, all-keys-up release, opposite-axis snap,
    /// then state diffing. Events from devices assigned to no slot, and
    /// entries keyed `Key::None`, produce no deltas. Repeated key-down of an
    /// already-down key must not produce a delta (diff idempotence).
    pub fn handle(&mut self, ev: &KeyEvent) -> Deltas {
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
            if ev.down {
                slot.press(t.binding);
            } else {
                slot.release(t.binding, down);
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
                slot.release(t.binding, down);
            }
        }

        self.collect_deltas(&mut deltas);
        deltas
    }

    /// Clear all per-device key state and pad states back to neutral.
    ///
    /// Emits nothing: after a reset the caller is expected to submit
    /// `PadState::default()` to each pad itself (emulation stop path).
    pub fn reset(&mut self) {
        self.down.iter_mut().for_each(|w| *w = 0);
        for slot in &mut self.slots {
            slot.current = PadState::default();
            slot.last_emitted = PadState::default();
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
