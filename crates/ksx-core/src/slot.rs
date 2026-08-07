//! Slot configuration and the invalidation taxonomy.

use crate::device::DeviceId;
use crate::macros::MacroSwitch;
use crate::persona::Persona;
use crate::socd::Socd;

/// Maximum slots ksx will configure.
///
/// Was 4 — the legacy `EmulationManager` cap, which matched the XInput ceiling
/// exactly because Xbox 360 was the only persona. Since PlayStation personas are
/// plain HID and do not touch XInput's four slots
/// (`docs/research/m6.5-ds4-findings.md`), the two limits are now different
/// things: [`MAX_XINPUT_SLOTS`] is imposed by Windows, this one is ours.
///
/// Was then 8, on the guess that an 8-player panel is the largest anyone
/// builds. The guess was the only thing holding the number down — nothing in
/// the pipeline degrades past it. Every slot-keyed structure is a `Vec` or a
/// `SmallVec` that spills to the heap rather than truncating, and the one hard
/// limit is the `u8` a slot index is carried in and indexed by
/// (`engine.rs::handle_at`), which stops at 255. Meanwhile four I-PAC4 boards
/// on one cabinet is sixteen players, and the guess was refusing them.
///
/// 16 is therefore a headroom number, not a hardware one: high enough to stop
/// being the thing that says no, low enough that a per-slot array is still
/// nothing. Raise it again freely — but only with the derived limits in
/// mind, because they do not all track it automatically:
/// `ksx-app`'s clap ranges and `ksx-api`'s refusal wording read this constant
/// (both have tests that pin that), while the engine's `SmallVec` inline
/// capacities size themselves off it for speed only.
pub const MAX_SLOTS: u8 = 16;

/// How many slots may use an XInput persona at once — a Windows limit, not ours.
///
/// Windows exposes exactly four XInput slots and no virtual bus can create a
/// fifth (measured: `docs/research/m2-xinput-findings.md`). A fifth Xbox 360
/// target still plugs, but no game will ever see it, so configuration refuses it
/// with an actionable message instead of producing a silently dead pad.
///
/// **Physical pads count too.** This is the *configured* ceiling; a real
/// controller already connected takes a slot from the same four, which surfaces
/// at runtime as [`InvalidationReason::XinputBusFull`].
pub const MAX_XINPUT_SLOTS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("slot number must be 1..={MAX_SLOTS}, got {0}")]
pub struct InvalidSlotNumber(pub u8);

/// Desired configuration of one slot — pure data. The runtime slot (pad
/// handle, XInput user index, live invalidation) is orchestrated in ksx-app.
///
/// Slot number ≠ XInput user index: the user index is discovered from ViGEm's
/// notification callback after plug-in, never derived from this number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSpec {
    /// 1..=[`MAX_SLOTS`] (enforced by [`SlotSpec::new`]).
    pub number: u8,
    pub keyboard: Option<DeviceId>,
    pub mouse: Option<DeviceId>,
    /// Preset referenced by name; resolution happens in the config layer.
    pub preset: String,
    /// Which controller this slot presents itself as. Defaults to
    /// [`Persona::Xbox360`]; set with [`SlotSpec::with_persona`].
    pub persona: Persona,
    /// What this slot does with simultaneous opposing directions. Defaults to
    /// [`Socd::Off`] — no generated chords, no behavioral change at all — and
    /// is applied by generating chords onto the resolved preset
    /// ([`crate::socd`]), never by a special case in the engine.
    pub socd: Socd,
    /// Does this slot run macros at all? Defaults to [`MacroSwitch::On`] — the
    /// behavior of every configuration written before the switch existed — and
    /// [`MacroSwitch::Off`] silences every macro of this slot's preset whatever
    /// each one's own `enabled` says. The tournament switch; see
    /// [`MacroSwitch`].
    pub macros: MacroSwitch,
}

impl SlotSpec {
    /// A slot with the default [`Persona::Xbox360`].
    ///
    /// Kept non-breaking on purpose: 16-odd construction sites (mostly tests)
    /// want an Xbox pad and should not have to say so. Chain
    /// [`SlotSpec::with_persona`] for anything else.
    pub fn new(
        number: u8,
        keyboard: Option<DeviceId>,
        mouse: Option<DeviceId>,
        preset: impl Into<String>,
    ) -> Result<Self, InvalidSlotNumber> {
        if number == 0 || number > MAX_SLOTS {
            return Err(InvalidSlotNumber(number));
        }
        Ok(Self {
            number,
            keyboard,
            mouse,
            preset: preset.into(),
            persona: Persona::default(),
            socd: Socd::default(),
            macros: MacroSwitch::default(),
        })
    }

    /// Sets the persona this slot presents itself as.
    #[must_use]
    pub fn with_persona(mut self, persona: Persona) -> Self {
        self.persona = persona;
        self
    }

    /// Sets the SOCD cleaning policy for this slot.
    #[must_use]
    pub fn with_socd(mut self, socd: Socd) -> Self {
        self.socd = socd;
        self
    }

    /// Sets the slot-wide macro master switch ("tournament mode").
    #[must_use]
    pub fn with_macros(mut self, macros: MacroSwitch) -> Self {
        self.macros = macros;
        self
    }
}

/// Why a slot cannot emulate — ported variant-for-variant from legacy
/// `SplitterCore/Emulation/SlotInvalidationReason.cs`.
///
/// Legacy collapsed all of these into one "Slot is invalidated" message; each
/// variant here carries its own root-cause explanation so the CLI can say what
/// actually went wrong.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum InvalidationReason {
    /// Legacy `None`. Prefer `Option::<InvalidationReason>::None` in new code;
    /// this variant exists for 1:1 legacy fidelity.
    None,
    VirtualBusNotInstalled,
    AdditionalDriversNotInstalled,
    VirtualBusFull,
    ControllerAlreadyPluggedIn,
    ControllerInUse,
    KeyboardUnplugged,
    MouseUnplugged,
    PresetsParseFailed,
    ControllerPlugInFailed,
    XinputBusFull,
    ControllerUnplugged,
    NoInputDeviceSelected,
}

impl InvalidationReason {
    pub const ALL: &'static [InvalidationReason] = &[
        InvalidationReason::None,
        InvalidationReason::VirtualBusNotInstalled,
        InvalidationReason::AdditionalDriversNotInstalled,
        InvalidationReason::VirtualBusFull,
        InvalidationReason::ControllerAlreadyPluggedIn,
        InvalidationReason::ControllerInUse,
        InvalidationReason::KeyboardUnplugged,
        InvalidationReason::MouseUnplugged,
        InvalidationReason::PresetsParseFailed,
        InvalidationReason::ControllerPlugInFailed,
        InvalidationReason::XinputBusFull,
        InvalidationReason::ControllerUnplugged,
        InvalidationReason::NoInputDeviceSelected,
    ];

    /// Human explanation with root-cause context.
    pub const fn explanation(self) -> &'static str {
        match self {
            InvalidationReason::None => "Slot is not invalidated.",
            InvalidationReason::VirtualBusNotInstalled => {
                "The virtual gamepad bus driver (ViGEmBus) is not installed. \
                 Install it (see 'ksx install-drivers') and try again."
            }
            InvalidationReason::AdditionalDriversNotInstalled => {
                "Drivers required by the virtual controller are missing from this system."
            }
            InvalidationReason::VirtualBusFull => {
                "The virtual bus has no free slots left to plug in another virtual controller."
            }
            InvalidationReason::ControllerAlreadyPluggedIn => {
                "This slot's virtual controller is already plugged in, \
                 likely by another slot or a previous session."
            }
            InvalidationReason::ControllerInUse => {
                "This slot's virtual controller is in use, probably owned by another process."
            }
            InvalidationReason::KeyboardUnplugged => {
                "The keyboard assigned to this slot has been unplugged from the system."
            }
            InvalidationReason::MouseUnplugged => {
                "The mouse assigned to this slot has been unplugged from the system."
            }
            InvalidationReason::PresetsParseFailed => {
                "The preset library failed to parse; emulation is disabled until it is repaired."
            }
            InvalidationReason::ControllerPlugInFailed => {
                "Plugging this slot's virtual controller into the bus failed."
            }
            InvalidationReason::XinputBusFull => {
                "Windows allows at most 4 XInput controllers; the XInput bus is full \
                 (already-connected physical pads count toward the limit)."
            }
            InvalidationReason::ControllerUnplugged => {
                "This slot's virtual controller has been unplugged."
            }
            InvalidationReason::NoInputDeviceSelected => {
                "No keyboard or mouse is assigned to this slot."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn slot_numbers_validated() {
        assert_eq!(SlotSpec::new(0, None, None, "p"), Err(InvalidSlotNumber(0)));
        assert_eq!(
            SlotSpec::new(MAX_SLOTS + 1, None, None, "p"),
            Err(InvalidSlotNumber(MAX_SLOTS + 1))
        );
        for n in 1..=MAX_SLOTS {
            let spec = SlotSpec::new(n, Some(DeviceId::new("dev")), None, "p").unwrap();
            assert_eq!(spec.number, n);
            assert_eq!(spec.preset, "p");
        }
    }

    /// The raise past the old eight-player guess is real, and the ceiling
    /// still exists above it.
    ///
    /// Slot 9 is the whole point: it is the first number the 8-slot build
    /// refused, so this test fails against that build — and the `MAX_SLOTS + 1`
    /// half proves the raise did not turn the check off, which is the way a
    /// "just make the number bigger" change goes wrong.
    #[test]
    fn a_ninth_player_is_configurable_and_one_past_the_ceiling_still_is_not() {
        assert!(SlotSpec::new(9, None, None, "p").is_ok());
        assert!(SlotSpec::new(MAX_SLOTS, None, None, "p").is_ok());
        assert_eq!(
            SlotSpec::new(MAX_SLOTS + 1, None, None, "p"),
            Err(InvalidSlotNumber(MAX_SLOTS + 1))
        );
        // The refusal names the live bound, so a raise cannot leave the
        // sentence advertising a ceiling that moved.
        assert_eq!(
            InvalidSlotNumber(MAX_SLOTS + 1).to_string(),
            format!("slot number must be 1..={MAX_SLOTS}, got {}", MAX_SLOTS + 1)
        );
    }

    #[test]
    fn slots_default_to_xbox360_and_opt_into_others() {
        // The compatibility guarantee: a spec built the old way is an Xbox pad.
        let spec = SlotSpec::new(1, None, None, "p").unwrap();
        assert_eq!(spec.persona, Persona::Xbox360);
        // Same guarantee for SOCD: absent means "behave exactly as before".
        assert_eq!(spec.socd, Socd::Off);
        assert_eq!(spec.clone().with_socd(Socd::Neutral).socd, Socd::Neutral);
        // ...and for the macro master switch: absent means "macros run", which
        // is what every pre-switch config meant.
        assert_eq!(spec.macros, MacroSwitch::On);
        assert_eq!(
            spec.clone().with_macros(MacroSwitch::Off).macros,
            MacroSwitch::Off
        );
        let ps = spec.clone().with_persona(Persona::PlayStation);
        assert_eq!(ps.persona, Persona::PlayStation);
        // …and changes nothing else about the slot.
        assert_eq!((ps.number, ps.preset), (spec.number, spec.preset));
    }

    #[test]
    fn the_xinput_ceiling_is_below_the_slot_ceiling() {
        // If these ever converge again, the "slots 5+ must be HID" rule in
        // ksx-config becomes unreachable and should be deleted, not left lying.
        // (Read through black_box: both are constants, and a plain compare here
        // is a clippy error rather than the tripwire this test intends.)
        let (xinput, max) = std::hint::black_box((MAX_XINPUT_SLOTS, MAX_SLOTS));
        assert!(xinput < max);
        assert_eq!(xinput, 4, "Windows' XInput ceiling is fixed at 4");
    }

    #[test]
    fn all_thirteen_legacy_reasons_present() {
        assert_eq!(InvalidationReason::ALL.len(), 13);
        let explanations: HashSet<&str> = InvalidationReason::ALL
            .iter()
            .map(|r| r.explanation())
            .collect();
        assert_eq!(explanations.len(), 13);
        assert!(InvalidationReason::ALL
            .iter()
            .all(|r| !r.explanation().is_empty()));
    }
}
