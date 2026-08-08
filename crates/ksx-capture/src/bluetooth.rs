//! **Read-only** Bluetooth enumeration: the `BTHENUM\` half of "what input
//! devices are attached", beside [`crate::winusb::enumerate`]'s USB half.
//!
//! # Why a second pass exists at all
//!
//! `nusb` enumerates the USB tree, and a Bluetooth device is not on it. Until
//! this module existed `ksx device scan` walked USB and therefore could not see
//! a Bluetooth keyboard AT ALL, while `ksx devices` walked the Interception
//! keyboard stack and saw it with no grouping and no word about which backends
//! could capture it. Two incomplete lists, and a user with a Bluetooth keyboard
//! reading the more detailed of the two was told, in effect, that their
//! keyboard did not exist.
//!
//! # The safety property, identical to the USB pass
//!
//! Nothing here opens, claims, configures or resets a device. It is one
//! `CM_Get_Device_ID_ListW(FILTER_PRESENT)` walk plus `KEY_READ` registry
//! reads, performed by [`ksx_platform::winusb::present_nodes`] — the *same*
//! walk `ksx winusb status` does. One tree read, two consumers, so a device
//! list can never disagree with the refusal that guards a claim.
//!
//! # What a Bluetooth row can and cannot do (`ksx_core::transport`)
//!
//! A Bluetooth keyboard **can** be captured by Interception: it is a
//! keyboard-class devnode on the Windows input stack exactly like a USB one.
//! It can **never** be WinUSB-claimed: a claim is an INF binding a USB
//! interface by hardware id, and there is no USB interface here to bind. This
//! module does not decide that — it reports the transport, and
//! [`ksx_core::Reach::eligibility`] decides. See that module for why the
//! distinction is worded as the transport rather than as "unsupported".
//!
//! # The trap this pass must not fall into
//!
//! **A paired-but-disconnected Bluetooth keyboard reads PRESENT all day.**
//! Pairing is what puts the node in the tree; the batteries have nothing to do
//! with it. So every candidate carries [`BtCandidate::can_type`], sourced from
//! `CM_Get_DevNode_Status` (`CM_PROB_DEVICE_NOT_CONNECTED`), and the row stays
//! listed — hiding it would be its own lie — under a verdict that says it
//! cannot deliver a keystroke. `ksx_platform::winusb::Survey::keyboard_count`
//! already excludes it from the last-keyboard arithmetic, which is the refusal
//! standing between a user and a panel they cannot type on.

use ksx_core::{DeviceId, Reach, Transport};
use ksx_platform::winusb::DeviceNode;

/// The address parser and the enumerator name live beside the device tree in
/// `ksx-platform`, not here.
///
/// Two consumers need the same answer, and a second copy would be a second
/// answer: `ksx_platform::winusb::Survey` groups a Bluetooth keyboard's service
/// nodes by address to decide what a claim would cost, and this module groups
/// the device list by the same address. If those two ever disagreed, the list
/// and the refusal would be describing different devices — the failure mode
/// this crate's `regkey`/`vendors` consolidations already paid for twice.
pub use ksx_platform::winusb::{bd_addr, BTHENUM};

/// One Bluetooth device node, as seen without opening anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtCandidate {
    /// The device instance path, uppercased — canonicalized exactly like
    /// [`crate::UsbCandidate::id`], because both end up in the same list and a
    /// user copies either.
    pub id: DeviceId,
    /// The grouping key for "one physical device": `BTHENUM\<BD_ADDR>` when the
    /// address could be read, else this node's own id.
    ///
    /// The Bluetooth analogue of a USB composite parent. One headset or
    /// keyboard wears several service nodes (HID, audio sink, AVRCP); they are
    /// one device to a human, and grouping is what stops a picker printing four
    /// cryptic rows for one keyboard.
    pub device: String,
    /// The 12 hex digits of the Bluetooth device address, when the instance
    /// path carries one. `None` is not a failure — it means this node was not
    /// spelled in a shape ksx can read an address out of, and it is then
    /// grouped alone rather than merged with a guess.
    pub address: Option<String>,
    /// `FriendlyName` if the bus wrote one, else the `DeviceDesc` tail.
    pub name: String,
    /// The function driver's service name, when it has one.
    pub service: Option<String>,
    /// Is this device a keyboard as far as Windows is concerned — i.e. is there
    /// a keyboard-class devnode for it?
    ///
    /// The only positive signal available without opening anything, and the one
    /// that decides whether Interception could capture it.
    pub is_keyboard: bool,
    /// **Can it deliver a keystroke right now?** See the module docs: PRESENT
    /// and TYPING are different questions for a paired device.
    pub can_type: bool,
    /// Why it cannot, when it cannot — the phrase `CM_Get_DevNode_Status`
    /// justifies, never a guess.
    pub trouble: Option<&'static str>,
}

impl BtCandidate {
    /// Always [`Transport::Bluetooth`]: this pass enumerates one transport, and
    /// the method exists so a caller building a unified list never has to write
    /// the word itself.
    pub fn transport(&self) -> Transport {
        Transport::Bluetooth
    }

    /// The facts [`ksx_core::Reach::eligibility`] needs. `claimed` is
    /// unconditionally `false` and that is the point: nothing off the USB
    /// transport can be bound to `winusb.sys`.
    pub fn reach(&self) -> Reach {
        Reach {
            transport: Transport::Bluetooth,
            keyboard: self.is_keyboard,
            claimed: false,
            can_type: self.can_type,
        }
    }
}

/// Every present Bluetooth device node, from an already-collected tree.
///
/// Pure and cross-platform on purpose — the same property
/// `ksx_platform::winusb::Survey::from_nodes` has, and for the same reason: the
/// whole shape, including the paired-but-disconnected case, is exercised in CI
/// against a synthetic tree with no radio anywhere near it.
pub fn from_nodes(nodes: &[DeviceNode]) -> Vec<BtCandidate> {
    // Every keyboard-class node on the machine, with the Bluetooth address it
    // belongs to when it carries one. A Bluetooth HID keyboard's keyboard-class
    // devnode is sometimes the `BTHENUM\` node itself and sometimes a `HID\`
    // child of it, depending on the stack — both spellings put the same
    // BD_ADDR in the instance path, so the address is what joins them rather
    // than a parent walk that only works for one of the two shapes.
    let keyboard_addresses: Vec<String> = nodes
        .iter()
        .filter(|n| n.is_keyboard_class())
        .filter_map(bd_addr)
        .collect();

    let mut out: Vec<BtCandidate> = Vec::new();
    for node in nodes.iter().filter(|n| is_bluetooth(n)) {
        let address = bd_addr(node);
        let is_keyboard = node.is_keyboard_class()
            || address
                .as_deref()
                .is_some_and(|addr| keyboard_addresses.iter().any(|k| k == addr));

        // The status of the node itself, and of the keyboard node that belongs
        // to the same address when they are different devnodes: a live
        // `BTHENUM` service node with a dead HID child still cannot type.
        let mut trouble = node.trouble();
        if trouble.is_none() {
            if let Some(addr) = address.as_deref() {
                trouble = nodes
                    .iter()
                    .filter(|n| n.is_keyboard_class() && bd_addr(n).as_deref() == Some(addr))
                    .find_map(DeviceNode::trouble);
            }
        }

        out.push(BtCandidate {
            id: DeviceId::new(node.instance_id.to_uppercase()),
            device: match &address {
                Some(addr) => format!("{BTHENUM}\\{addr}"),
                None => node.instance_id.to_uppercase(),
            },
            address,
            name: node.display_name(),
            service: node.service.clone(),
            is_keyboard,
            can_type: trouble.is_none(),
            trouble,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Survey the live machine's Bluetooth devices. Read-only.
///
/// `Err` when the PnP device-id list came back EMPTY, which on a running
/// Windows machine is a failed read and not a machine with no devices. That
/// distinction is the whole reason this returns a `Result` at all: a surface
/// must be able to say "I could not read this" rather than "there is nothing
/// here" (`ksx_api::DevicesView::bluetooth_available`).
pub fn candidates() -> std::io::Result<Vec<BtCandidate>> {
    let nodes = ksx_platform::winusb::present_nodes();
    if nodes.is_empty() {
        return Err(std::io::Error::other(
            "the PnP device-id list came back empty — on a running Windows machine that is a \
             failed read, not a machine with no devices",
        ));
    }
    Ok(from_nodes(&nodes))
}

/// Is this node a Bluetooth device's service node?
fn is_bluetooth(node: &DeviceNode) -> bool {
    node.enumerator.eq_ignore_ascii_case(BTHENUM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_platform::winusb::{NodeStatus, CM_PROB_DEVICE_NOT_CONNECTED, KEYBOARD_CLASS_GUID};

    /// The measured shape on this machine (`crates/ksx-app/src/winusb.rs`'s
    /// `status_does_not_count_a_keyboard_that_cannot_type` uses the same path).
    const BT_KEYBOARD: &str = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&2A0B8CBA&0&001BDC0F1FE7_C00000000";
    const BT_AUDIO: &str = r"BTHENUM\Dev_B84DEEAA1122\7&2A0B8CBA&0&BluetoothDevice_B84DEEAA1122";
    const USB_PANEL: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";

    // ── Verbatim from this machine, 2026-08-08 (`Get-PnpDevice -PresentOnly`).
    // Fixtures invented by the author are how an enumerator ends up handling
    // three shapes and meeting a fourth.
    const REAL_XBOX_HID: &str = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&02E0\7&3562C725&0&EC8350B88A10_C00000000";
    const REAL_XBOX_DEV: &str =
        r"BTHENUM\DEV_EC8350B88A10\7&3562C725&0&BLUETOOTHDEVICE_EC8350B88A10";
    const REAL_XBOX_ALT: &str = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&02E0\2&21119A68&0&F2424BE71501";
    const REAL_TOWER: &str = r"BTHENUM\DEV_24C4069CF1F4\7&3562C725&0&BLUETOOTHDEVICE_24C4069CF1F4";
    /// The LOCAL radio's own pseudo-devices. Three unrelated nodes, one
    /// all-zero "address".
    const REAL_LOCAL_PERIPHERAL: &str = r"BTHENUM\{E836F634-9100-4CA8-95EE-F7B6E9115185}_LOCALMFG&0000\7&3562C725&0&000000000000_00000008";
    const REAL_LOCAL_VIRTUAL_HID: &str = r"BTHENUM\{4B6770D0-6471-43EA-B5A9-B39268F43B96}_LOCALMFG&0000\7&3562C725&0&000000000000_00000000";

    fn node(id: &str, class: Option<&str>, desc: &str) -> DeviceNode {
        DeviceNode::new(
            id,
            class.map(str::to_owned),
            Some("BthHFEnum".to_owned()),
            Some(format!("@bth.inf,%token%;{desc}")),
            None,
        )
    }

    fn keyboard_node(id: &str, desc: &str) -> DeviceNode {
        node(id, Some(KEYBOARD_CLASS_GUID), desc)
    }

    fn disconnected(node: DeviceNode) -> DeviceNode {
        node.with_status(NodeStatus {
            started: false,
            problem: CM_PROB_DEVICE_NOT_CONNECTED,
        })
    }

    fn live(node: DeviceNode) -> DeviceNode {
        node.with_status(NodeStatus {
            started: true,
            problem: 0,
        })
    }

    /// Both measured spellings of a Bluetooth address, and the refusal to guess
    /// when neither is there.
    ///
    /// Breaks against: any string surgery that takes "the last segment" —
    /// the `_C00000000` service suffix rides on the same segment as the
    /// address, and two devices on one radio share the `7&2A0B8CBA&0` stem, so
    /// a sloppy split merges unrelated devices into one row.
    #[test]
    fn a_bluetooth_address_is_read_from_either_spelling_and_never_guessed() {
        assert_eq!(
            bd_addr(&node(BT_KEYBOARD, None, "x")).as_deref(),
            Some("001BDC0F1FE7")
        );
        assert_eq!(
            bd_addr(&node(BT_AUDIO, None, "x")).as_deref(),
            Some("B84DEEAA1122")
        );
        // Lowercase in the registry, canonical out.
        assert_eq!(
            bd_addr(&node(&BT_KEYBOARD.to_lowercase(), None, "x")).as_deref(),
            Some("001BDC0F1FE7")
        );
        // Nothing twelve-hex-digits long: no address, and therefore no merge.
        assert_eq!(
            bd_addr(&node(r"BTHENUM\Dev_SHORT\7&1&0&2", None, "x")),
            None
        );
        assert_eq!(bd_addr(&node(USB_PANEL, None, "x")), None);
    }

    /// The three instance shapes actually on this machine, verbatim, and the
    /// two that name the SAME controller through different service nodes.
    ///
    /// Breaks against a parser written from one example: the `DEV_` form, the
    /// `_C00000000`-suffixed service form and the bare `2&…&0&<addr>` form are
    /// three different spellings of one fact.
    #[test]
    fn the_real_instance_shapes_on_this_machine_all_parse() {
        assert_eq!(
            bd_addr(&node(REAL_XBOX_HID, None, "x")).as_deref(),
            Some("EC8350B88A10")
        );
        assert_eq!(
            bd_addr(&node(REAL_XBOX_DEV, None, "x")).as_deref(),
            Some("EC8350B88A10"),
            "the controller's DEV_ node and its HID service node are one device"
        );
        assert_eq!(
            bd_addr(&node(REAL_XBOX_ALT, None, "x")).as_deref(),
            Some("F2424BE71501")
        );
        assert_eq!(
            bd_addr(&node(REAL_TOWER, None, "x")).as_deref(),
            Some("24C4069CF1F4")
        );
    }

    /// **The all-zero address is not a device.** Measured: the local radio's
    /// own service nodes all spell `…&0&000000000000_0000000n`.
    ///
    /// Breaks against `len() == 12 && all hex`, which is the obvious predicate
    /// — it files `Bluetooth Peripheral Device` and `Virtual Bluetooth HID
    /// Device` under one row named after whichever enumerated first, which is
    /// the SpinTrak-labelled-as-an-I-PAC failure in a different costume.
    #[test]
    fn the_local_radios_zero_address_never_merges_two_pseudo_devices() {
        assert_eq!(bd_addr(&node(REAL_LOCAL_PERIPHERAL, None, "x")), None);
        let found = from_nodes(&[
            live(node(
                REAL_LOCAL_PERIPHERAL,
                None,
                "Bluetooth Peripheral Device",
            )),
            live(node(
                REAL_LOCAL_VIRTUAL_HID,
                None,
                "Virtual Bluetooth HID Device",
            )),
        ]);
        assert_eq!(found.len(), 2);
        assert_ne!(
            found[0].device, found[1].device,
            "two unrelated local pseudo-devices, two rows: {found:?}"
        );
    }

    /// **The list this task exists to build.** A Bluetooth keyboard appears,
    /// says it is a keyboard, and reports Interception-eligible /
    /// never-WinUSB.
    ///
    /// Breaks against the shipped enumerator: `ksx_capture::usb_candidates`
    /// walks `nusb::list_devices()`, a Bluetooth device is not on the USB tree,
    /// and so `ksx device scan` produced NO row for this device at all.
    #[test]
    fn a_bluetooth_keyboard_is_enumerated_and_is_interception_only() {
        let found = from_nodes(&[
            live(keyboard_node(BT_KEYBOARD, "Bluetooth Keyboard")),
            live(node(USB_PANEL, None, "USB Input Device")),
        ]);
        assert_eq!(found.len(), 1, "only the BTHENUM node is ours: {found:?}");
        let kb = &found[0];
        assert!(kb.is_keyboard);
        assert!(kb.can_type);
        assert_eq!(kb.transport(), Transport::Bluetooth);
        assert_eq!(kb.device, r"BTHENUM\001BDC0F1FE7");

        let reach = kb.reach().eligibility();
        assert!(reach.interception, "it is a keyboard on the Windows stack");
        assert!(!reach.winusb);
        assert!(reach.winusb_impossible_by_transport);
    }

    /// A keyboard whose keyboard-class devnode is a `HID\` child rather than
    /// the `BTHENUM\` node itself — the other stack spelling. The address is
    /// what joins them.
    ///
    /// Breaks against a parent-walk join (`is_parent_of`), which needs a
    /// `ParentIdPrefix` the Bluetooth service node does not always write.
    #[test]
    fn a_keyboard_class_child_makes_its_bluetooth_node_a_keyboard() {
        let child = r"HID\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\9&1B2C&0&001BDC0F1FE7_C00000000";
        let found = from_nodes(&[
            live(node(BT_KEYBOARD, None, "Bluetooth HID Device")),
            live(keyboard_node(child, "HID Keyboard Device")),
        ]);
        assert_eq!(found.len(), 1);
        assert!(
            found[0].is_keyboard,
            "the keyboard-class node shares the BD_ADDR: {found:?}"
        );
    }

    /// **The trap, at the enumerator.** A paired-but-switched-off keyboard is
    /// PRESENT — pairing is what puts it in the tree — so it must be listed,
    /// and it must say it cannot type.
    ///
    /// Breaks against a pass that reports presence only: the row would look
    /// identical to a working keyboard, which is how someone reads "2
    /// keyboards", claims their panel, and is locked out by a keyboard in a
    /// drawer with dead batteries.
    #[test]
    fn a_paired_but_disconnected_keyboard_is_listed_and_marked_cannot_type() {
        let found = from_nodes(&[disconnected(keyboard_node(
            BT_KEYBOARD,
            "Bluetooth Keyboard",
        ))]);
        assert_eq!(found.len(), 1, "listed — hiding it would be its own lie");
        let kb = &found[0];
        assert!(kb.is_keyboard);
        assert!(!kb.can_type);
        assert_eq!(kb.trouble, Some("not connected (paired but absent?)"));
        // Still Interception-eligible: it works the moment it connects, and
        // saying otherwise would be a different wrong answer.
        assert!(kb.reach().eligibility().interception);
    }

    /// The other half of the same trap: the `BTHENUM` service node is live and
    /// its keyboard child is not. Nothing types either way.
    #[test]
    fn a_dead_keyboard_child_makes_its_live_bluetooth_node_unable_to_type() {
        let child = r"HID\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\9&1B2C&0&001BDC0F1FE7_C00000000";
        let found = from_nodes(&[
            live(node(BT_KEYBOARD, None, "Bluetooth HID Device")),
            disconnected(keyboard_node(child, "HID Keyboard Device")),
        ]);
        assert_eq!(found.len(), 1);
        assert!(!found[0].can_type, "{:?}", found[0]);
        assert!(found[0].trouble.is_some());
    }

    /// A Bluetooth device that is not a keyboard is still listed — "ksx cannot
    /// see my device" is a real support question — and gets no backend.
    #[test]
    fn a_bluetooth_device_that_is_not_a_keyboard_is_listed_with_no_backend() {
        let found = from_nodes(&[live(node(BT_AUDIO, None, "ULT TOWER 9"))]);
        assert_eq!(found.len(), 1);
        assert!(!found[0].is_keyboard);
        let reach = found[0].reach().eligibility();
        assert!(!reach.interception && !reach.winusb);
    }

    /// One physical device wearing several service nodes groups under one key —
    /// the Bluetooth analogue of a USB composite parent, and the thing that
    /// stops a picker printing three cryptic rows for one headset.
    #[test]
    fn several_service_nodes_of_one_device_share_a_grouping_key() {
        let a = r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_LOCALMFG&0000\7&2A0B8CBA&0&B84DEEAA1122_C00000000";
        let b = r"BTHENUM\{0000110E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0000\7&2A0B8CBA&0&B84DEEAA1122_C00000001";
        let found = from_nodes(&[
            live(node(a, None, "ULT TOWER 9")),
            live(node(b, None, "ULT TOWER 9")),
        ]);
        assert_eq!(found.len(), 2, "two devnodes");
        assert_eq!(found[0].device, found[1].device, "one device");
        assert_eq!(found[0].device, r"BTHENUM\B84DEEAA1122");
    }

    /// A node with no readable address is grouped ALONE. Merging it with
    /// anything would be inventing a link, and two unrelated devices sharing a
    /// row is the exact ambiguity the USB side's parent join exists to avoid.
    #[test]
    fn a_node_with_no_readable_address_is_never_merged_with_another() {
        let odd = r"BTHENUM\ODDBALL\1";
        let found = from_nodes(&[
            live(node(odd, None, "Something")),
            live(node(BT_AUDIO, None, "ULT TOWER 9")),
        ]);
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].device, found[1].device);
        assert!(found.iter().any(|c| c.device == odd.to_uppercase()));
    }

    /// The name a user reads is the one the DEVICE chose. A list of four
    /// `Bluetooth HID Device` rows is not a list anyone can pick from.
    #[test]
    fn the_friendly_name_wins_over_the_class_infs_generic_description() {
        let named = live(node(BT_AUDIO, None, "Bluetooth HID Device"))
            .with_friendly_name(Some("ULT TOWER 9".to_owned()));
        assert_eq!(from_nodes(&[named])[0].name, "ULT TOWER 9");
        // ...and the INF description is the fallback, not the other way round.
        let unnamed = live(node(BT_AUDIO, None, "Bluetooth HID Device"));
        assert_eq!(from_nodes(&[unnamed])[0].name, "Bluetooth HID Device");
    }

    /// Ids are canonicalized uppercase, exactly like the USB pass — both halves
    /// end up in one list and a user copies either.
    #[test]
    fn ids_are_canonicalized_like_the_usb_half() {
        let found = from_nodes(&[live(node(&BT_KEYBOARD.to_lowercase(), None, "x"))]);
        assert_eq!(found[0].id.as_str(), BT_KEYBOARD.to_uppercase());
    }

    /// Live, read-only: the real machine's tree must not panic this pass, and
    /// an empty tree must be reported as a FAILED READ rather than as a machine
    /// with no devices.
    #[test]
    #[cfg(windows)]
    fn enumeration_is_safe_and_well_formed_on_real_hardware() {
        let found = candidates().expect("the PnP tree is readable on a running machine");
        for c in &found {
            assert!(c.id.as_str().starts_with("BTHENUM\\"), "{}", c.id);
            assert_eq!(c.id.as_str(), c.id.as_str().to_uppercase());
            assert_eq!(c.can_type, c.trouble.is_none());
        }
    }
}
