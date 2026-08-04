//! Slot configuration and the invalidation taxonomy.

use crate::device::DeviceId;

/// Maximum slots (legacy `EmulationManager` cap; also the XInput ceiling).
pub const MAX_SLOTS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("slot number must be 1..=4, got {0}")]
pub struct InvalidSlotNumber(pub u8);

/// Desired configuration of one slot — pure data. The runtime slot (pad
/// handle, XInput user index, live invalidation) is orchestrated in ksx-app.
///
/// Slot number ≠ XInput user index: the user index is discovered from ViGEm's
/// notification callback after plug-in, never derived from this number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSpec {
    /// 1..=4 (enforced by [`SlotSpec::new`]).
    pub number: u8,
    pub keyboard: Option<DeviceId>,
    pub mouse: Option<DeviceId>,
    /// Preset referenced by name; resolution happens in the config layer.
    pub preset: String,
}

impl SlotSpec {
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
        })
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
        assert_eq!(SlotSpec::new(5, None, None, "p"), Err(InvalidSlotNumber(5)));
        for n in 1..=MAX_SLOTS {
            let spec = SlotSpec::new(n, Some(DeviceId::new("dev")), None, "p").unwrap();
            assert_eq!(spec.number, n);
            assert_eq!(spec.preset, "p");
        }
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
