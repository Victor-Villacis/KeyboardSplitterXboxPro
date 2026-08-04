//! Main config file schema: `%APPDATA%\ksx\config.toml`
//! (`docs/research/design-architecture.md` §4.1).

use ksx_core::{DeviceId, Persona, SlotSpec};
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFile {
    pub schema_version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "device", skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<DeviceEntry>,
    #[serde(default, rename = "slot", skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<SlotEntry>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            settings: Settings::default(),
            devices: Vec::new(),
            slots: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Legacy `ShouldBlockKeyboards` (default true).
    pub block_keyboards: bool,
    /// Legacy `ShouldBlockMice` (default false).
    pub block_mice: bool,
    /// Legacy `MouseMoveDeadZone`, 0..=12.
    pub mouse_move_deadzone: u8,
    /// Legacy `StartingVirtualControllerUserIndex`, 1..=4.
    pub starting_user_index: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            block_keyboards: true,
            block_mice: false,
            mouse_move_deadzone: 5,
            starting_user_index: 1,
        }
    }
}

/// A known physical device: stable identity learned via the picker wizard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Device instance path (see [`ksx_core::DeviceId`] semantics).
    pub id: String,
    pub alias: String,
    #[serde(default)]
    pub backend: Backend,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Interception,
    Winusb,
}

/// One `[[slot]]` entry. `keyboard`/`mouse` refer to a `[[device]]` alias, or
/// hold a literal instance path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotEntry {
    pub number: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<String>,
    pub preset: String,
    /// Which controller this slot presents itself as. Absent means
    /// [`Persona::Xbox360`], which is why every pre-persona config still loads
    /// and still round-trips byte-identically.
    #[serde(
        default,
        with = "crate::persona_serde",
        skip_serializing_if = "crate::persona_serde::is_default"
    )]
    pub persona: Persona,
}

impl ConfigFile {
    /// Alias → instance path. A literal instance path (contains `\`) passes
    /// through unchanged; anything else must match a `[[device]]` alias.
    pub fn resolve_device(&self, alias_or_id: &str) -> Result<DeviceId, ConfigError> {
        if let Some(device) = self.devices.iter().find(|d| d.alias == alias_or_id) {
            return Ok(DeviceId::new(device.id.clone()));
        }
        if alias_or_id.contains('\\') {
            return Ok(DeviceId::new(alias_or_id));
        }
        Err(ConfigError::UnknownDeviceAlias(alias_or_id.to_owned()))
    }

    /// Resolve one slot entry into a core [`SlotSpec`].
    pub fn slot_spec(&self, slot: &SlotEntry) -> Result<SlotSpec, ConfigError> {
        let keyboard = slot
            .keyboard
            .as_deref()
            .map(|k| self.resolve_device(k))
            .transpose()?;
        let mouse = slot
            .mouse
            .as_deref()
            .map(|m| self.resolve_device(m))
            .transpose()?;
        SlotSpec::new(slot.number, keyboard, mouse, slot.preset.clone())
            .map(|spec| spec.with_persona(slot.persona))
            .map_err(Into::into)
    }

    /// Reverse of [`ConfigFile::slot_spec`]: an instance path folds back to
    /// its alias when a `[[device]]` entry matches.
    pub fn slot_entry(&self, spec: &SlotSpec) -> SlotEntry {
        let display = |id: &DeviceId| {
            self.devices
                .iter()
                .find(|d| d.id == id.as_str())
                .map(|d| d.alias.clone())
                .unwrap_or_else(|| id.as_str().to_owned())
        };
        SlotEntry {
            number: spec.number,
            keyboard: spec.keyboard.as_ref().map(display),
            mouse: spec.mouse.as_ref().map(display),
            preset: spec.preset.clone(),
            persona: spec.persona,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/research/design-architecture.md §4.1, verbatim.
    const DOC_EXAMPLE: &str = r#"
schema_version = 1

[settings]
block_keyboards = true
block_mice = false
mouse_move_deadzone = 5          # 0..12
starting_user_index = 1          # 1..4

[[device]]
id = "HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"
alias = "P1 I-PAC"
backend = "interception"

[[slot]]
number = 1
keyboard = "P1 I-PAC"
preset = "street-fighter-p1"
"#;

    #[test]
    fn doc_example_parses() {
        let cfg: ConfigFile = toml::from_str(DOC_EXAMPLE).unwrap();
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        assert_eq!(cfg.settings, Settings::default());
        assert_eq!(cfg.devices.len(), 1);
        assert_eq!(
            cfg.devices[0].id,
            r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000"
        );
        assert_eq!(cfg.devices[0].alias, "P1 I-PAC");
        assert_eq!(cfg.devices[0].backend, Backend::Interception);
        assert_eq!(cfg.slots.len(), 1);
        assert_eq!(cfg.slots[0].keyboard.as_deref(), Some("P1 I-PAC"));
        assert_eq!(cfg.slots[0].mouse, None);
        assert_eq!(cfg.slots[0].preset, "street-fighter-p1");
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg: ConfigFile = toml::from_str(DOC_EXAMPLE).unwrap();
        let serialized = toml::to_string(&cfg).unwrap();
        let reparsed: ConfigFile = toml::from_str(&serialized).unwrap();
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn missing_sections_use_defaults_and_unknown_keys_are_lenient() {
        let cfg: ConfigFile =
            toml::from_str("schema_version = 1\nfuture_option = \"ignored\"\n").unwrap();
        assert_eq!(cfg.settings, Settings::default());
        assert!(cfg.settings.block_keyboards);
        assert!(!cfg.settings.block_mice);
        assert_eq!(cfg.settings.mouse_move_deadzone, 5);
        assert_eq!(cfg.settings.starting_user_index, 1);
        assert!(cfg.devices.is_empty());
        assert!(cfg.slots.is_empty());
    }

    #[test]
    fn backend_winusb_parses() {
        let entry: DeviceEntry =
            toml::from_str("id = \"USB\\\\X\"\nalias = \"p\"\nbackend = \"winusb\"\n").unwrap();
        assert_eq!(entry.backend, Backend::Winusb);
    }

    #[test]
    fn slot_spec_resolves_alias_to_instance_path() {
        let cfg: ConfigFile = toml::from_str(DOC_EXAMPLE).unwrap();
        let spec = cfg.slot_spec(&cfg.slots[0]).unwrap();
        assert_eq!(spec.number, 1);
        assert_eq!(
            spec.keyboard,
            Some(ksx_core::DeviceId::new(
                r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000"
            ))
        );
        assert_eq!(spec.mouse, None);
        assert_eq!(spec.preset, "street-fighter-p1");

        // And back: the id folds to its alias.
        let entry = cfg.slot_entry(&spec);
        assert_eq!(entry, cfg.slots[0]);
    }

    #[test]
    fn literal_instance_path_passes_through() {
        let cfg = ConfigFile::default();
        let id = cfg.resolve_device(r"HID\VID_AAAA&PID_BBBB\1&2&3").unwrap();
        assert_eq!(id.as_str(), r"HID\VID_AAAA&PID_BBBB\1&2&3");
    }

    #[test]
    fn unknown_alias_is_an_error() {
        let cfg = ConfigFile::default();
        assert!(matches!(
            cfg.resolve_device("P9 Ghost"),
            Err(ConfigError::UnknownDeviceAlias(_))
        ));
    }

    #[test]
    fn bad_slot_number_is_an_error() {
        let cfg = ConfigFile::default();
        let slot = SlotEntry {
            number: ksx_core::MAX_SLOTS + 1,
            keyboard: None,
            mouse: None,
            preset: "p".into(),
            persona: Persona::default(),
        };
        assert!(matches!(
            cfg.slot_spec(&slot),
            Err(ConfigError::InvalidSlotNumber(_))
        ));
    }
}
