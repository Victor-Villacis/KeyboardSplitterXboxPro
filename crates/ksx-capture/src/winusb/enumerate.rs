//! **Read-only** USB enumeration: which interfaces could be claimed, and what
//! each one is bound to right now.
//!
//! # The safety property of this module
//!
//! Nothing here opens, claims, configures or resets a device. `nusb`'s
//! `list_devices()` reads descriptors the hub driver has already cached, and
//! the driver-binding lookup is a `KEY_READ` registry walk. Running
//! `ksx devices` on the cabinet mid-session cannot disturb a single keystroke,
//! which is the whole reason it is a separate module from
//! [`super::WinUsbBackend`] — the type that *does* claim is not reachable from
//! any enumeration path.
//!
//! # How an interface's identity is built
//!
//! `nusb` reports USB *devices*; Windows binds drivers to USB *interfaces*
//! (`&MI_xx` child devnodes of the composite parent). The two are joined
//! through `ParentIdPrefix`, the value `usbccgp` writes on the parent and
//! prefixes onto every child's instance id:
//!
//! ```text
//! USB\VID_D209&PID_0430\4                     ← nusb device, ParentIdPrefix "7&1a2b3c4d&0"
//!  └ USB\VID_D209&PID_0430&MI_00\7&1A2B3C4D&0&0000   ← the interface, Service = HidUsb
//! ```
//!
//! That last string is the [`DeviceId`] the WinUSB backend uses, and it is what
//! makes T4 (two identical encoders, `docs/USE-CASES.md`) structurally
//! solvable: the boards share `VID_D209&PID_0430&MI_00` but their
//! `ParentIdPrefix` values differ, because they are derived from where each
//! board is physically plugged in. Two identical boards, two different ids,
//! stable as long as neither moves ports.

use ksx_core::DeviceId;
use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

use crate::hid::INTERFACE_CLASS_HID;
use crate::regkey::RegKey;

/// Ultimarc's USB vendor id — every I-PAC family board enumerates with it.
pub const ULTIMARC_VID: u16 = 0xD209;

/// The service name Microsoft's in-box WinUSB driver registers under.
pub const WINUSB_SERVICE: &str = "winusb";

/// The service that owns a HID interface on the normal stack.
pub const HIDUSB_SERVICE: &str = "hidusb";

/// The USB composite parent driver.
const USBCCGP_SERVICE: &str = "usbccgp";

const ENUM_ROOT: &str = "SYSTEM\\CurrentControlSet\\Enum";

/// What a USB interface is currently bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// `winusb.sys` — claimable by ksx, and invisible to Windows as a keyboard.
    /// This is the state the M6 rebind produces.
    WinUsb,
    /// `hidusb.sys` — the normal HID stack. Windows sees the keyboard; ksx
    /// cannot claim the interface (WinUSB is not there to claim through).
    HidUsb,
    /// Some other service owns it.
    Other(String),
    /// No `Service` value: the devnode exists but nothing is driving it
    /// (the state right after a `pnputil /remove-device` and before a rescan).
    None,
}

impl Binding {
    fn from_service(service: Option<String>) -> Self {
        match service {
            None => Binding::None,
            Some(s) if s.eq_ignore_ascii_case(WINUSB_SERVICE) => Binding::WinUsb,
            Some(s) if s.eq_ignore_ascii_case(HIDUSB_SERVICE) => Binding::HidUsb,
            Some(s) => Binding::Other(s),
        }
    }

    /// Is the WinUSB rebind in place — i.e. can this interface be claimed?
    pub fn is_winusb(&self) -> bool {
        matches!(self, Binding::WinUsb)
    }

    /// Short label for `ksx devices`.
    pub fn label(&self) -> &str {
        match self {
            Binding::WinUsb => "winusb.sys",
            Binding::HidUsb => "hidusb.sys (keyboard stack)",
            Binding::Other(s) => s,
            Binding::None => "none",
        }
    }
}

/// One claimable USB interface, as seen without opening anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbCandidate {
    /// The interface's device instance path, uppercased — the [`DeviceId`] the
    /// WinUSB backend reports and `config.toml` stores.
    pub id: DeviceId,
    /// The composite parent's instance path (what `nusb` calls the device).
    pub parent_id: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// `bInterfaceNumber` — `MI_00` on the I-PAC's keyboard.
    pub interface_number: u8,
    pub interface_class: u8,
    pub interface_subclass: u8,
    pub interface_protocol: u8,
    /// `iInterface` string, when the device supplies one.
    pub interface_string: Option<String>,
    /// The device's product string (`iProduct`).
    pub product: Option<String>,
    /// The device's `iSerialNumber` string, read from the descriptor the hub
    /// driver already cached — **not** parsed out of the instance path.
    ///
    /// The distinction matters. Windows only puts a serial in the instance id
    /// while that serial is unique among connected devices of the same
    /// VID/PID; the moment two boards claim one serial it stops trusting it and
    /// falls back to generated port-derived ids for both. Reading the
    /// descriptor gives the same answer either way, so
    /// [`ksx_core::DeviceSelector`] can *notice* the collision instead of being
    /// silently downgraded by it.
    ///
    /// `None` when `iSerialNumber` is 0, which is most cheap HID boards.
    pub serial: Option<String>,
    /// `DeviceDesc` from the Enum tree, minus the INF prefix.
    pub device_desc: Option<String>,
    /// Physical port path (bus id + hub port chain) — why the id is stable.
    pub port_chain: Vec<u8>,
    pub bus_id: String,
    /// What owns the interface right now.
    pub binding: Binding,
}

impl UsbCandidate {
    /// A HID-class interface: the only kind that can carry keyboard reports.
    pub fn is_hid(&self) -> bool {
        self.interface_class == INTERFACE_CLASS_HID
    }

    /// Declares itself a boot keyboard (`subclass 1, protocol 1`).
    pub fn is_boot_keyboard(&self) -> bool {
        self.interface_subclass == crate::hid::INTERFACE_SUBCLASS_BOOT
            && self.interface_protocol == crate::hid::INTERFACE_PROTOCOL_KEYBOARD
    }

    /// An Ultimarc board.
    ///
    /// **Cosmetic only** — this decorates a row with a familiar word and
    /// nothing else. No capture path, claim path or refusal may branch on it:
    /// ksx works with any HID keyboard interface, and a program that treated
    /// one vendor id as special would be exactly the hardcoding
    /// `docs/DEVICE-IDENTITY.md` §6 forbids. See [`crate::vendors`].
    pub fn is_ultimarc(&self) -> bool {
        self.vendor_id == ULTIMARC_VID
    }

    /// The vendor-independent facts a [`ksx_core::DeviceSelector`] matches on.
    ///
    /// This is the seam between "what Windows says is plugged in" and "which
    /// board the config means". Everything on the selector side is pure and
    /// runs in CI; everything on this side is a read-only registry/descriptor
    /// walk. Neither knows about the other's problems.
    pub fn facts(&self) -> ksx_core::DeviceFacts {
        ksx_core::DeviceFacts {
            id: self.id.clone(),
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            interface_number: self.interface_number,
            serial: self.serial.clone(),
            instance: ksx_core::DeviceFacts::instance_of(self.id.as_str()).to_owned(),
        }
    }

    /// Best available human label.
    ///
    /// Product string first, on purpose. `DeviceDesc` on a USB interface node
    /// is almost always the generic `"USB Input Device"` written by
    /// `input.inf` — technically a description, useless for telling two boards
    /// apart on a screen full of them. `iProduct` is what the device calls
    /// itself. `DeviceDesc` stays as the last resort because some devices
    /// supply no strings at all.
    pub fn friendly(&self) -> Option<&str> {
        self.product
            .as_deref()
            .or(self.interface_string.as_deref())
            .or(self.device_desc.as_deref())
    }

    /// Would this interface plausibly deliver keyboard reports?
    ///
    /// Deliberately generous: once an interface is rebound to `winusb.sys` its
    /// `DeviceDesc` stops saying "Keyboard", and `bInterfaceProtocol` is 0 on
    /// plenty of NKRO firmware (the boot protocol is optional). So the answer
    /// is "it is HID", and the actual proof is the report descriptor the
    /// backend pulls at claim time. Guessing harder here would only produce
    /// confident wrong answers.
    pub fn is_keyboard_candidate(&self) -> bool {
        self.is_hid()
    }
}

/// Every USB interface on the machine, with its current driver binding.
///
/// Read-only. Errors only if `nusb` cannot enumerate at all; a device whose
/// registry entries cannot be read is reported with [`Binding::None`] rather
/// than dropped, because "I could not tell" must never look like "not there".
pub fn candidates() -> std::io::Result<Vec<UsbCandidate>> {
    use nusb::MaybeFuture as _;

    let devices = nusb::list_devices().wait()?;
    let mut out = Vec::new();
    for device in devices {
        let parent_id = device.instance_id().to_string_lossy().to_uppercase();
        let composite = device
            .driver()
            .is_some_and(|d| d.eq_ignore_ascii_case(USBCCGP_SERVICE));
        let prefix = composite.then(|| parent_id_prefix(&parent_id)).flatten();
        let shared_node = device.interfaces().count() > 1;

        for interface in device.interfaces() {
            let number = interface.interface_number();
            let (id, node) = match &prefix {
                // Composite: the interface is its own devnode under
                // `USB\VID_xxxx&PID_yyyy&MI_zz`.
                Some(prefix) => {
                    match interface_node(device.vendor_id(), device.product_id(), number, prefix) {
                        Some(found) => found,
                        // The child devnode is not present (an interface the
                        // composite driver has not enumerated). Report it with
                        // a synthetic id and no binding rather than hide it.
                        None => (
                            synthetic_id(device.vendor_id(), device.product_id(), number),
                            NodeInfo::default(),
                        ),
                    }
                }
                // Not composite: one devnode owns the whole device. If it owns
                // exactly one interface, that devnode *is* the interface and
                // its path is the identity. If it owns several (an Intel
                // Bluetooth radio, say: one driver, several interfaces), the
                // path alone names two different claimable things, so it is
                // qualified — see `qualified_id`.
                None => (
                    qualified_id(&parent_id, number, shared_node),
                    read_node(&parent_id),
                ),
            };

            out.push(UsbCandidate {
                id: DeviceId::new(id),
                parent_id: parent_id.clone(),
                vendor_id: device.vendor_id(),
                product_id: device.product_id(),
                interface_number: number,
                interface_class: interface.class(),
                interface_subclass: interface.subclass(),
                interface_protocol: interface.protocol(),
                interface_string: interface.interface_string().map(str::to_owned),
                product: device.product_string().map(str::to_owned),
                serial: device.serial_number().map(str::to_owned),
                device_desc: node.device_desc,
                port_chain: device.port_chain().to_vec(),
                bus_id: device.bus_id().to_owned(),
                binding: Binding::from_service(node.service),
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Registry facts about one devnode.
#[derive(Debug, Default)]
struct NodeInfo {
    service: Option<String>,
    device_desc: Option<String>,
}

fn read_node(instance_id: &str) -> NodeInfo {
    let path = format!("{ENUM_ROOT}\\{instance_id}");
    let Some(key) = RegKey::open(HKEY_LOCAL_MACHINE, &path) else {
        return NodeInfo::default();
    };
    NodeInfo {
        service: key.string_value("Service"),
        device_desc: key.string_value("DeviceDesc").map(|d| strip_inf_prefix(&d)),
    }
}

/// `usbccgp`'s `ParentIdPrefix` for a composite device, e.g. `7&1a2b3c4d&0`.
fn parent_id_prefix(instance_id: &str) -> Option<String> {
    let path = format!("{ENUM_ROOT}\\{instance_id}");
    RegKey::open(HKEY_LOCAL_MACHINE, &path)?.string_value("ParentIdPrefix")
}

/// Locate the `&MI_nn` child devnode belonging to *this* composite parent.
///
/// The `ParentIdPrefix` match is what disambiguates two identical boards: both
/// have a `USB\VID_D209&PID_0430&MI_00` key, and each contributes one instance
/// subkey under it.
fn interface_node(vid: u16, pid: u16, interface: u8, prefix: &str) -> Option<(String, NodeInfo)> {
    let hwid = interface_hwid(vid, pid, interface);
    let path = format!("{ENUM_ROOT}\\{hwid}");
    let key = RegKey::open(HKEY_LOCAL_MACHINE, &path)?;
    let wanted = format!("{}&", prefix.to_uppercase());
    let instance = key
        .subkey_names()
        .into_iter()
        .find(|name| name.to_uppercase().starts_with(&wanted))?;
    let id = format!("{hwid}\\{}", instance.to_uppercase());
    let info = read_node(&format!("{hwid}\\{instance}"));
    Some((id, info))
}

/// `USB\VID_xxxx&PID_yyyy&MI_zz`, uppercase — the interface's hardware id.
fn interface_hwid(vid: u16, pid: u16, interface: u8) -> String {
    format!("USB\\VID_{vid:04X}&PID_{pid:04X}&MI_{interface:02X}")
}

/// Identity for an interface whose devnode we could not find. Marked so it can
/// never be mistaken for a bindable id.
fn synthetic_id(vid: u16, pid: u16, interface: u8) -> String {
    format!("{}\\<NOT-ENUMERATED>", interface_hwid(vid, pid, interface))
}

/// Identity for an interface on a devnode that is not split per interface.
///
/// A real device instance path has exactly three `\`-separated parts
/// (`enumerator\hardware-id\instance`). Appending a fourth `MI_nn` part is
/// therefore unambiguous, reads the same way as the composite spelling, and —
/// the point — keeps [`UsbCandidate::id`] a *unique name for one claimable
/// interface*. Two candidates sharing an id would put us straight back into the
/// T4 ambiguity this backend exists to remove, so uniqueness is enforced here
/// rather than hoped for.
///
/// A devnode owning a single interface keeps its path verbatim: that is the
/// common case, it is what `pnputil` prints, and it is what a user pastes into
/// `config.toml`.
fn qualified_id(instance_path: &str, interface: u8, shared_node: bool) -> String {
    if shared_node {
        format!("{instance_path}\\MI_{interface:02X}")
    } else {
        instance_path.to_owned()
    }
}

/// `@input.inf,%hid_device%;HID Keyboard Device` → `HID Keyboard Device`.
fn strip_inf_prefix(desc: &str) -> String {
    match desc.rfind(';') {
        Some(i) => desc[i + 1..].to_owned(),
        None => desc.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_hwid_matches_the_documented_ipac_enumeration() {
        // docs/research/keyboard-capture-2026.md §5, measured on this machine.
        assert_eq!(
            interface_hwid(0xD209, 0x0430, 0),
            "USB\\VID_D209&PID_0430&MI_00"
        );
        assert_eq!(
            interface_hwid(0xD209, 0x0430, 2),
            "USB\\VID_D209&PID_0430&MI_02"
        );
        // The Ultimarc trackball, for contrast.
        assert_eq!(
            interface_hwid(0xD209, 0x15A2, 0),
            "USB\\VID_D209&PID_15A2&MI_00"
        );
    }

    #[test]
    fn binding_classifies_the_states_that_matter() {
        assert_eq!(
            Binding::from_service(Some("WinUSB".into())),
            Binding::WinUsb
        );
        assert_eq!(
            Binding::from_service(Some("winusb".into())),
            Binding::WinUsb
        );
        assert!(Binding::from_service(Some("winusb".into())).is_winusb());
        assert_eq!(
            Binding::from_service(Some("HidUsb".into())),
            Binding::HidUsb
        );
        assert!(!Binding::from_service(Some("HidUsb".into())).is_winusb());
        assert_eq!(
            Binding::from_service(Some("libusbK".into())),
            Binding::Other("libusbK".into())
        );
        assert_eq!(Binding::from_service(None), Binding::None);
        assert_eq!(Binding::None.label(), "none");
    }

    #[test]
    fn device_desc_loses_its_inf_prefix() {
        assert_eq!(
            strip_inf_prefix("@input.inf,%hid_device_system_keyboard%;HID Keyboard Device"),
            "HID Keyboard Device"
        );
        assert_eq!(strip_inf_prefix("WinUsb Device"), "WinUsb Device");
        assert_eq!(strip_inf_prefix(""), "");
    }

    fn candidate(id: &str, class: u8, subclass: u8, protocol: u8, vid: u16) -> UsbCandidate {
        UsbCandidate {
            id: DeviceId::from(id),
            parent_id: "USB\\VID_D209&PID_0430\\4".into(),
            vendor_id: vid,
            product_id: 0x0430,
            interface_number: 0,
            interface_class: class,
            interface_subclass: subclass,
            interface_protocol: protocol,
            interface_string: None,
            product: Some("I-PAC Ultimate I/O".into()),
            serial: Some("4".into()),
            device_desc: Some("HID Keyboard Device".into()),
            port_chain: vec![1, 4],
            bus_id: "1".into(),
            binding: Binding::HidUsb,
        }
    }

    #[test]
    fn keyboard_candidacy_is_hid_class_not_a_guess_about_protocol() {
        let boot = candidate(
            "USB\\VID_D209&PID_0430&MI_00\\7&1&0&0000",
            0x03,
            1,
            1,
            0xD209,
        );
        assert!(boot.is_hid());
        assert!(boot.is_boot_keyboard());
        assert!(boot.is_keyboard_candidate());
        assert!(boot.is_ultimarc());

        // NKRO firmware commonly reports subclass 0 / protocol 0 — still a
        // candidate, because the report descriptor is the real evidence.
        let nkro = candidate(
            "USB\\VID_D209&PID_0430&MI_02\\7&1&0&0002",
            0x03,
            0,
            0,
            0xD209,
        );
        assert!(!nkro.is_boot_keyboard());
        assert!(nkro.is_keyboard_candidate());

        // A vendor-class interface is not.
        let vendor = candidate("USB\\VID_1234&PID_5678&MI_00\\x", 0xFF, 0, 0, 0x1234);
        assert!(!vendor.is_hid());
        assert!(!vendor.is_keyboard_candidate());
        assert!(!vendor.is_ultimarc());
    }

    #[test]
    fn friendly_prefers_what_the_device_calls_itself() {
        // Measured on this machine: every I-PAC interface node carries
        // DeviceDesc = "USB Input Device". A screen listing four of those is
        // not a list anyone can pick a board out of.
        let mut c = candidate("USB\\X\\1", 0x03, 1, 1, 0xD209);
        c.device_desc = Some("USB Input Device".into());
        assert_eq!(c.friendly(), Some("I-PAC Ultimate I/O"));
        c.product = None;
        assert_eq!(c.friendly(), Some("USB Input Device"));
        c.device_desc = None;
        assert_eq!(c.friendly(), None);
    }

    #[test]
    fn a_devnode_owning_several_interfaces_still_yields_distinct_ids() {
        // An Intel Bluetooth radio (VID_8087&PID_0033) is not composite: one
        // devnode, several interfaces. Without qualification both interfaces
        // would answer to one id and "capture this device" would be ambiguous
        // again — the exact defect M6 removes.
        let path = "USB\\VID_8087&PID_0033\\5&CC3F949&0&14";
        let a = qualified_id(path, 0, true);
        let b = qualified_id(path, 1, true);
        assert_ne!(a, b);
        assert_eq!(a, "USB\\VID_8087&PID_0033\\5&CC3F949&0&14\\MI_00");
        assert_eq!(b, "USB\\VID_8087&PID_0033\\5&CC3F949&0&14\\MI_01");
        // The single-interface case keeps the real, pasteable instance path.
        assert_eq!(qualified_id(path, 0, false), path);
    }

    #[test]
    fn synthetic_ids_are_visibly_unusable() {
        let id = synthetic_id(0xD209, 0x0430, 1);
        assert!(id.contains("<NOT-ENUMERATED>"));
        assert!(!id.ends_with("MI_01"), "must not look like a real instance");
    }

    /// Live, read-only: enumeration must not panic on this machine's real USB
    /// tree, and every candidate must carry a usable identity. Nothing is
    /// opened or claimed — see the module docs.
    #[test]
    fn enumeration_is_safe_and_well_formed_on_real_hardware() {
        let found = candidates().expect("nusb enumeration");
        for c in &found {
            assert!(!c.id.as_str().is_empty());
            assert!(
                c.id.as_str().starts_with("USB\\"),
                "ids are USB instance paths: {}",
                c.id
            );
            assert_eq!(
                c.id.as_str(),
                c.id.as_str().to_uppercase(),
                "ids are canonicalized uppercase"
            );
        }
        // Ids must be unique — that is the entire T4 claim. A collision would
        // mean "capture this device" is ambiguous again, which is the defect
        // M6 exists to remove.
        let mut ids: Vec<&DeviceId> = found.iter().map(|c| &c.id).collect();
        ids.sort_unstable();
        let dupes: Vec<&&DeviceId> = ids
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| &w[0])
            .collect();
        assert!(
            dupes.is_empty(),
            "two interfaces reported one id: {dupes:?}"
        );
    }

    /// A candidate's facts must describe the candidate — in particular the
    /// instance tail has to be the *port-derived* part of the id, because that
    /// is the only thing the port rung may pin.
    #[test]
    fn facts_carry_the_port_tail_and_nothing_else_from_the_path() {
        let c = candidate(
            r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000",
            0x03,
            1,
            1,
            0xD209,
        );
        let facts = c.facts();
        assert_eq!(facts.id, c.id);
        assert_eq!(facts.vendor_id, 0xD209);
        assert_eq!(facts.interface_number, 0);
        assert_eq!(facts.serial.as_deref(), Some("4"));
        assert_eq!(facts.instance, "7&25EEA38C&0&0000");
    }

    /// Live, read-only. Every candidate must produce facts a selector can use,
    /// and the selector ksx would WRITE for it must find it again — on this
    /// machine's actual USB tree, not a fixture.
    ///
    /// This is the test that would have caught the shipped defect: the id in
    /// the config was a port-specific path, and nothing checked that the
    /// identity ksx persists is one it can re-resolve.
    #[test]
    fn every_real_candidate_round_trips_through_the_selector_ksx_would_write() {
        use ksx_core::DeviceSelector;

        let found = candidates().expect("nusb enumeration");
        let facts: Vec<ksx_core::DeviceFacts> = found.iter().map(UsbCandidate::facts).collect();
        for one in &facts {
            let selector = DeviceSelector::strongest_for(one, &facts);
            let text = selector.to_string();
            let reparsed = DeviceSelector::parse(&text).expect(&text);
            assert_eq!(reparsed, selector, "selector text must round-trip: {text}");
            assert_eq!(
                reparsed.match_against(&facts).id(),
                Some(&one.id),
                "the selector ksx would write for {} does not find it again ({text})",
                one.id
            );
        }
    }

    /// The measured fact this whole design is calibrated against, recorded so a
    /// future reader does not have to re-derive it: the Ultimarc boards on this
    /// machine DO report a serial, and it is a one-character constant.
    ///
    /// Skips when no Ultimarc board is connected — this is a note about
    /// hardware, not a requirement on it.
    #[test]
    fn ultimarc_serials_are_low_entropy_and_that_is_why_the_port_rung_exists() {
        let found = candidates().expect("nusb enumeration");
        for c in found.iter().filter(|c| c.is_ultimarc()) {
            if let Some(serial) = &c.serial {
                assert!(
                    serial.len() <= 4,
                    "if an Ultimarc board ever ships a REAL per-unit serial ({serial}), \
                     docs/DEVICE-IDENTITY.md §3 should be revisited — the serial rung \
                     would then be trustworthy for twins"
                );
            }
        }
    }
}
