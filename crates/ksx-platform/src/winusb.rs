//! The WinUSB rebind lifecycle: survey, claim, release.
//!
//! # What a "claim" is
//!
//! The I-PAC is a USB composite device. Its keyboard lives on interface `MI_00`,
//! which Windows binds to `HidUsb` → `hidclass` → `kbdhid` → `kbdclass`.
//! *Claiming* it means rebinding that one interface to Microsoft's in-box
//! `winusb.sys`, after which ksx reads the HID interrupt-IN endpoint directly
//! with `nusb`. Two properties fall out for free, and they are the entire reason
//! M6 exists (`docs/research/keyboard-capture-2026.md` §4):
//!
//! - **Blocking is structural.** The interface is not in the keyboard stack any
//!   more, so nothing else on the machine can see a keystroke from it. No filter
//!   driver, no hook, no race — and no cross-signed `keyboard.sys` waiting for
//!   the 2026 enforcement flip to stop loading.
//! - **Identity is structural.** One `nusb` device per board, keyed on the
//!   instance path. No 10-slot ceiling, no id drift on replug, and two identical
//!   I-PACs stop being indistinguishable (`docs/USE-CASES.md` T4).
//!
//! # What this module does and does not do
//!
//! It **plans**. [`survey`] reads the device tree (read-only — see
//! `win::devices`), [`plan_claim`] and [`plan_release`] turn a request into an
//! INF plus an ordered list of `pnputil` invocations, and [`apply`] runs one of
//! those lists. Every mutating verb in `ksx winusb` is dry-run by default and
//! needs an explicit `--yes`, so the normal outcome of asking for a claim is a
//! printed INF and a printed command line.
//!
//! Nothing here opens or claims a device. The read side is registry + one
//! `CM_Get_Device_ID_List` call; the write side is `pnputil.exe`, which means
//! the rebind is auditable, reproducible by hand, and reversible by the same
//! tool with no ksx involved.
//!
//! # The refusal that matters
//!
//! [`plan_claim`] counts *present* keyboard-class devices and refuses to claim
//! the last one ([`Refusal::LastKeyboard`], exit 2). Claiming the only keyboard
//! on a machine leaves the user with no way to type the release command, no way
//! through a UAC prompt, and no way onto the lock screen — `SendInput`
//! re-injection cannot reach the secure desktop (see [`crate::inject`]). That is
//! not a warning, it is a refusal.
//!
//! # Signing (the honest part)
//!
//! `winusb.sys` itself is in-box and WHQL-signed, so it is immune to the 2026
//! cross-signed cliff. The *INF that points at it* is a third-party INF, and
//! 64-bit Windows will not install one without a trusted catalog. There is no
//! way around that from inside ksx. [`ClaimPlan::signing_note`] prints the two
//! real options — a self-signed catalog in the machine's Trusted Root +
//! Trusted Publishers stores (what Zadig/libwdi automate), or attestation
//! signing through Partner Center — and `ksx winusb claim --yes` reports the
//! resulting `pnputil` failure verbatim rather than pretending it worked.

use std::path::{Path, PathBuf};

pub use ksx_core::Transport;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Setup class of a HID keyboard (`kbdclass`). Counting *present* devices in
/// this class is how the last-keyboard refusal is decided.
pub const KEYBOARD_CLASS_GUID: &str = "{4D36E96B-E325-11CE-BFC1-08002BE10318}";
/// Setup class of the generic `USBDevice` bucket a WinUSB-claimed interface
/// moves into.
pub const USB_DEVICE_CLASS_GUID: &str = "{88BAE032-5A81-49F0-BC3D-A4FF138216D6}";
/// The `winusb.sys` service name as it appears in `Enum\...\Service`.
pub const WINUSB_SERVICE: &str = "WinUSB";
/// The function driver a HID interface is normally bound to.
pub const HIDUSB_SERVICE: &str = "HidUsb";

/// Device interface class ksx publishes on every interface it claims, so `nusb`
/// can find them. One GUID for all of them — they are told apart by instance
/// path, which is the identity the whole config file is keyed on.
pub const KSX_DEVICE_INTERFACE_GUID: &str = "{B8B2D1F8-6E0E-4C7F-9E5A-3A9C1D6F2E10}";

/// Filename prefix for generated INFs. Also how [`store_drivers_matching`]
/// finds ksx's own entries in the driver store when releasing.
pub const INF_PREFIX: &str = "ksx-winusb-";

/// `DriverVer` date. Fixed, not "today": an INF whose bytes change per run is an
/// INF nobody can diff against what is installed. Rank against the in-box
/// `input.inf` is decided by match specificity (hardware id beats compatible id),
/// not by this date.
pub const DRIVER_VER: &str = "01/01/2026,1.0.0.0";

/// Ultimarc's USB vendor id — the I-PAC family and the trackball.
///
/// Re-exported from [`ksx_core::vendors`] rather than spelled again: three
/// crates each carried their own copy, and each grew its own predicate off it.
/// See `docs/DEVICE-IDENTITY.md` §6 for why a vendor id may pick a display name
/// and may not decide anything else.
pub use ksx_core::vendors::ULTIMARC_VID;

/// The vendor id inside a device instance path, if it carries one.
///
/// Used only to decide whether a board-specific *sentence* is worth adding to a
/// refusal. Nothing branches on the answer.
fn vendor_of(instance_id: &str) -> Option<u16> {
    let upper = instance_id.to_ascii_uppercase();
    let at = upper.find("VID_")? + 4;
    u16::from_str_radix(&upper[at..].chars().take(4).collect::<String>(), 16).ok()
}

// ---------------------------------------------------------------------------
// Device tree
// ---------------------------------------------------------------------------

/// `CM_PROB_DEVICE_NOT_CONNECTED` — what a *paired but absent* Bluetooth
/// keyboard reports. The node is present in the tree (that is what pairing
/// means) and cannot deliver a keystroke.
pub const CM_PROB_DEVICE_NOT_CONNECTED: u32 = 45;
/// `CM_PROB_DISABLED` — disabled in Device Manager or by policy.
pub const CM_PROB_DISABLED: u32 = 22;

/// What the PnP manager says about a node right now (`CM_Get_DevNode_Status`).
///
/// The registry alone cannot answer "could this keyboard type for me?": the
/// `Enum` tree is a graveyard, and even among *present* nodes a paired-but-
/// disconnected Bluetooth keyboard, a disabled device and a driverless one all
/// look exactly like a working keyboard. Since the last-keyboard refusal is the
/// thing standing between a user and a panel they cannot type on, it counts
/// this instead of counting rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeStatus {
    /// `DN_STARTED`: a function driver is loaded and running for this node.
    pub started: bool,
    /// `CM_PROB_*`, `0` when there is none.
    pub problem: u32,
}

impl NodeStatus {
    /// Is this node doing its job right now?
    pub fn is_live(self) -> bool {
        self.started && self.problem == 0
    }

    /// One short phrase for the status screen, or `None` when it is fine.
    pub fn trouble(self) -> Option<&'static str> {
        match (self.started, self.problem) {
            (_, CM_PROB_DEVICE_NOT_CONNECTED) => Some("not connected (paired but absent?)"),
            (_, CM_PROB_DISABLED) => Some("disabled"),
            (_, p) if p != 0 => Some("has a PnP problem"),
            (false, _) => Some("not started (no working driver)"),
            _ => None,
        }
    }
}

/// One node of the PnP device tree, as `HKLM\SYSTEM\CurrentControlSet\Enum`
/// describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceNode {
    /// `USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000`
    pub instance_id: String,
    /// `USB`, `HID`, `ACPI`, …
    pub enumerator: String,
    /// The middle segment: `VID_D209&PID_0430&MI_00`.
    pub device_key: String,
    /// The leaf segment: `7&25eea38c&0&0000`.
    pub instance: String,
    pub class_guid: Option<String>,
    /// The function driver's service name (`HidUsb`, `WinUSB`, `kbdhid`, …).
    pub service: Option<String>,
    /// Raw `DeviceDesc`, e.g. `@input.inf,%hid.devicedesc%;USB Input Device`.
    pub device_desc: Option<String>,
    /// `FriendlyName` — the name the DEVICE chose or the user gave it, when the
    /// bus writes one.
    ///
    /// Rare on USB and the norm on Bluetooth, which is why it exists: a paired
    /// device's `DeviceDesc` is a generic string from the class INF
    /// (`Bluetooth HID Device`), while its `FriendlyName` is what it is called
    /// on the phone it was paired with — `ULT TOWER 9`, measured on this
    /// machine. A list of four `Bluetooth HID Device` rows is not a list anyone
    /// can pick from. See [`Self::display_name`].
    pub friendly_name: Option<String>,
    /// Prefix Windows gives this node's children's instance ids. The only
    /// reliable registry-level link from a USB interface to its HID child.
    pub parent_id_prefix: Option<String>,
    /// What the PnP manager says about it, when anyone asked. `None` means
    /// nobody did — a node built from registry values alone, as every test
    /// fixture and every pre-`NodeStatus` caller does — and is read as "no
    /// reason to think it is broken".
    pub status: Option<NodeStatus>,
}

impl DeviceNode {
    pub fn new(
        instance_id: &str,
        class_guid: Option<String>,
        service: Option<String>,
        device_desc: Option<String>,
        parent_id_prefix: Option<String>,
    ) -> Self {
        let mut parts = instance_id.splitn(3, '\\');
        let enumerator = parts.next().unwrap_or_default().to_owned();
        let device_key = parts.next().unwrap_or_default().to_owned();
        let instance = parts.next().unwrap_or_default().to_owned();
        Self {
            instance_id: instance_id.to_owned(),
            enumerator,
            device_key,
            instance,
            class_guid,
            service,
            device_desc,
            friendly_name: None,
            parent_id_prefix,
            status: None,
        }
    }

    /// Attach what `CM_Get_DevNode_Status` said about this node.
    #[must_use]
    pub fn with_status(mut self, status: NodeStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Attach the bus's `FriendlyName` for this node.
    #[must_use]
    pub fn with_friendly_name(mut self, name: Option<String>) -> Self {
        self.friendly_name = name.filter(|n| !n.trim().is_empty());
        self
    }

    /// Is this node live? `true` when nobody asked the PnP manager (see
    /// [`Self::status`]) — an unasked question is not evidence of a fault.
    pub fn is_live(&self) -> bool {
        self.status.is_none_or(NodeStatus::is_live)
    }

    /// Why this node cannot be doing its job, if it cannot.
    pub fn trouble(&self) -> Option<&'static str> {
        self.status.and_then(NodeStatus::trouble)
    }

    /// The human-readable tail of `DeviceDesc` (`…;USB Input Device`).
    /// Un-indirected `@file.inf,%token%` descriptions are the norm in the
    /// registry; the tail after the last `;` is the resolved string Windows
    /// cached there.
    pub fn description(&self) -> String {
        match &self.device_desc {
            Some(desc) => desc.rsplit(';').next().unwrap_or(desc).trim().to_owned(),
            None => String::new(),
        }
    }

    /// The best name a human has for this node: what the device calls itself,
    /// else the class INF's description, else nothing.
    ///
    /// Same precedence rule as `ksx_capture::UsbCandidate::friendly` and for
    /// the same reason — a generic INF string is technically a description and
    /// useless on a screen full of them.
    pub fn display_name(&self) -> String {
        match self.friendly_name.as_deref() {
            Some(name) => name.trim().to_owned(),
            None => self.description(),
        }
    }

    /// The `USB\VID_xxxx&PID_xxxx&MI_xx` hardware id an INF must match. `None`
    /// for a node that is not a USB device.
    pub fn usb_hardware_id(&self) -> Option<String> {
        (self.enumerator.eq_ignore_ascii_case("USB"))
            .then(|| format!("USB\\{}", self.device_key.to_uppercase()))
    }

    pub fn vid_pid(&self) -> Option<(u16, u16)> {
        let vid = u16::try_from(hex_after(&self.device_key, "VID_")?).ok()?;
        let pid = u16::try_from(hex_after(&self.device_key, "PID_")?).ok()?;
        Some((vid, pid))
    }

    /// The `MI_xx` interface number, for a composite device's interface node.
    pub fn interface_number(&self) -> Option<u8> {
        hex_after(&self.device_key, "MI_").map(|n| n as u8)
    }

    pub fn is_class(&self, guid: &str) -> bool {
        self.class_guid
            .as_deref()
            .is_some_and(|g| g.eq_ignore_ascii_case(guid))
    }

    pub fn is_keyboard_class(&self) -> bool {
        self.is_class(KEYBOARD_CLASS_GUID)
    }

    pub fn service_is(&self, name: &str) -> bool {
        self.service
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(name))
    }

    /// Is `child` a child of this node? Windows stamps a node's children with
    /// its `ParentIdPrefix`, which is the only link visible from the registry
    /// alone (the device ids themselves do not nest).
    pub fn is_parent_of(&self, child: &DeviceNode) -> bool {
        let Some(prefix) = self.parent_id_prefix.as_deref() else {
            return false;
        };
        if prefix.is_empty() {
            return false;
        }
        // The USB interface `…&MI_00` and its HID child `…&MI_00` (or
        // `…&MI_01&Col03`) share the device-key stem; the child's *instance*
        // starts with the parent's ParentIdPrefix.
        child
            .instance
            .to_lowercase()
            .starts_with(&prefix.to_lowercase())
            && child
                .device_key
                .to_lowercase()
                .starts_with(&self.device_key.to_lowercase())
    }
}

/// The `BTHENUM` enumerator — a paired Bluetooth device's service nodes.
pub const BTHENUM: &str = "BTHENUM";

/// The Bluetooth device address inside an instance path, uppercased.
///
/// Lives beside the device tree rather than in `ksx-capture` because two
/// consumers need the same answer and a second copy would be a second answer:
/// [`Survey::from_nodes`] groups a Bluetooth keyboard's service nodes with it,
/// and `ksx_capture::bluetooth` groups the device list with it. Two spellings
/// appear on this machine and both are handled:
///
/// ```text
/// BTHENUM\{00001124-…}_VID&0002045E_PID&02E0\7&3562C725&0&EC8350B88A10_C00000000
///                                                         ^^^^^^^^^^^^
/// BTHENUM\DEV_EC8350B88A10\7&3562C725&0&BLUETOOTHDEVICE_EC8350B88A10
///             ^^^^^^^^^^^^
/// ```
///
/// `None` rather than a partial match when nothing in the path is twelve hex
/// digits: a wrong address merges two different devices into one row, which is
/// exactly the ambiguity the USB side's `ParentIdPrefix` join exists to avoid.
pub fn bd_addr(node: &DeviceNode) -> Option<String> {
    let key = node.device_key.to_uppercase();
    if let Some(rest) = key.strip_prefix("DEV_") {
        let addr: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
        if is_bd_addr(&addr) {
            return Some(addr);
        }
    }
    // Otherwise the address is the last `&`-separated segment of the instance,
    // up to the `_` that starts the per-service suffix.
    let instance = node.instance.to_uppercase();
    let tail = instance.rsplit('&').next()?;
    let candidate = tail.split('_').find(|part| is_bd_addr(part))?;
    Some(candidate.to_owned())
}

/// Twelve hex digits, and not the all-zero address.
///
/// The zero address is not a device. Measured on this machine: the LOCAL
/// radio's own service nodes — `Bluetooth Peripheral Device`, `Virtual
/// Bluetooth HID Device`, `Standard Serial over Bluetooth link (COM4)` — all
/// spell `…&0&000000000000_0000000n`. Accepting it would file three unrelated
/// pseudo-devices under one row named after whichever enumerated first.
fn is_bd_addr(text: &str) -> bool {
    text.len() == 12
        && text.chars().all(|c| c.is_ascii_hexdigit())
        && text.chars().any(|c| c != '0')
}

/// Parse the hex value after `marker` in a device key (`VID_D209` → `0xD209`).
/// Crate-visible: `virtual_pads` classifies bus children with the same parse.
pub(crate) fn hex_after(key: &str, marker: &str) -> Option<u32> {
    let upper = key.to_uppercase();
    let at = upper.find(marker)? + marker.len();
    let digits: String = upper[at..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    u32::from_str_radix(&digits, 16).ok()
}

// ---------------------------------------------------------------------------
// Survey
// ---------------------------------------------------------------------------

/// What ksx could do with one interface right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimState {
    /// Bound to `winusb.sys`. ksx can open it with `nusb`; Windows sees no
    /// keyboard.
    Claimed,
    /// Bound to the HID/keyboard stack, and it really is a keyboard. This is
    /// the rebind candidate.
    Claimable,
    /// A HID interface with no keyboard child — the I-PAC's `MI_01`/`MI_02`
    /// collections, the trackball. ksx will not claim these: it has no reason
    /// to, and claiming `MI_01` would kill the panel's trackball too.
    NotAKeyboard,
    /// Somebody else's function driver (`CyUsb`, a vendor stack). Out of scope.
    ForeignDriver,
    /// A keyboard on a transport a WinUSB claim can never bind — today, a
    /// Bluetooth one.
    ///
    /// Deliberately NOT `NotAKeyboard`: it *is* a keyboard, Interception can
    /// capture it, and splitting it into virtual pads works right now. What it
    /// has no answer to is a claim, because a claim is an INF binding a USB
    /// interface by hardware id and there is no USB interface here. That is
    /// permanent, which is why it is its own state rather than a variant of
    /// "not yet" (`ksx_core::transport`).
    InterceptionOnly,
}

impl ClaimState {
    pub fn code(self) -> &'static str {
        match self {
            ClaimState::Claimed => "claimed",
            ClaimState::Claimable => "claimable",
            ClaimState::NotAKeyboard => "not-a-keyboard",
            ClaimState::ForeignDriver => "foreign-driver",
            ClaimState::InterceptionOnly => "interception-only",
        }
    }
}

/// One USB interface ksx has an opinion about, with its keyboard child if it
/// still has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The `USB\…&MI_xx` node — the thing a rebind actually retargets.
    pub interface: DeviceNode,
    /// The `HID\…` keyboard node this interface currently produces, if any.
    /// `None` once claimed: WinUSB removes the HID stack entirely, which is
    /// precisely why the panel stops typing.
    pub keyboard: Option<DeviceNode>,
    pub state: ClaimState,
    /// The **physical board** this interface belongs to — see [`board_of`].
    /// Claiming an interface takes its whole board out of the keyboard count,
    /// so this is what the last-keyboard refusal subtracts.
    pub board: String,
    /// How this device is attached — and therefore which backends can ever
    /// reach it (`ksx_core::transport`).
    ///
    /// Carried rather than re-derived from [`Self::interface`]'s enumerator at
    /// each call site, because "which enumerator prefix means what" is a rule
    /// that would then exist in as many places as there are refusals.
    pub transport: Transport,
}

impl Candidate {
    /// The identity a ksx config file uses. Prefer the HID keyboard node's path
    /// while it exists — that is what `ksx devices` reports and what
    /// `[[slot]] device = …` is keyed on — and fall back to the USB interface
    /// once the claim has removed the HID node.
    pub fn ksx_device_id(&self) -> &str {
        match &self.keyboard {
            Some(kb) => &kb.instance_id,
            None => &self.interface.instance_id,
        }
    }

    pub fn is_ultimarc(&self) -> bool {
        self.interface
            .vid_pid()
            .is_some_and(|(vid, _)| vid == ULTIMARC_VID)
    }
}

/// One keyboard-class device, with the two things a *count* of keyboards has to
/// know and a bare node cannot tell you.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardNode {
    pub node: DeviceNode,
    /// The physical board it belongs to ([`board_of`]). Two HID collections of
    /// one I-PAC share this; two identical I-PACs on different ports do not.
    pub board: String,
    /// Why it cannot deliver a keystroke to Windows right now — `None` when it
    /// can, which is the only case the refusal counts.
    pub unusable: Option<&'static str>,
}

impl KeyboardNode {
    pub fn is_usable(&self) -> bool {
        self.unusable.is_none()
    }

    pub fn instance_id(&self) -> &str {
        &self.node.instance_id
    }
}

/// The machine's present device tree, reduced to what `ksx winusb` reasons about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Survey {
    pub candidates: Vec<Candidate>,
    /// Every *present* keyboard-class device, usable or not. The last-keyboard
    /// refusal counts the usable ones, by board — see [`Survey::keyboard_count`].
    pub keyboards: Vec<KeyboardNode>,
}

impl Survey {
    /// Build a survey from an already-collected set of present device nodes.
    ///
    /// Pure, so the whole claim/release decision surface — including the
    /// refusal that prevents bricking a panel — is exercised in CI against
    /// synthetic trees, on any platform, with no device anywhere near it.
    pub fn from_nodes(nodes: &[DeviceNode]) -> Self {
        let keyboards: Vec<KeyboardNode> = nodes
            .iter()
            .filter(|n| n.is_keyboard_class())
            .map(|n| KeyboardNode {
                node: n.clone(),
                board: board_of(n, nodes),
                unusable: why_unusable(n, nodes),
            })
            .collect();

        let mut candidates = Vec::new();
        for node in nodes
            .iter()
            .filter(|n| n.enumerator.eq_ignore_ascii_case("USB"))
        {
            // A composite device's *parent* (`USB\VID&PID\4`, service usbccgp)
            // is never a rebind target: claiming it would take every interface.
            if node.usb_hardware_id().is_none() || node.interface_number().is_none() {
                // Non-composite devices (a plain USB keyboard) have no MI_; they
                // are still valid targets, so only skip the obvious hub/composite
                // parents.
                if node.service_is("usbccgp") || node.service_is("usbhub") {
                    continue;
                }
            }
            let claimed = node.service_is(WINUSB_SERVICE);
            let hid = node.service_is(HIDUSB_SERVICE);
            if !claimed && !hid {
                // Vendor stacks (CyUsb on the I-PAC's firmware-upgrade device)
                // are reported only if something else asks for them.
                continue;
            }
            let keyboard = keyboards
                .iter()
                .find(|kb| node.is_parent_of(&kb.node))
                .map(|kb| kb.node.clone());
            let state = if claimed {
                ClaimState::Claimed
            } else if keyboard.is_some() {
                ClaimState::Claimable
            } else {
                ClaimState::NotAKeyboard
            };
            candidates.push(Candidate {
                board: board_of(node, nodes),
                interface: node.clone(),
                keyboard,
                state,
                transport: Transport::Usb,
            });
        }

        // Bluetooth keyboards, in the SAME list. They belong here for two
        // reasons and neither is cosmetic: `resolve` is what `ksx device pick`
        // calls, so a device missing from this list produces "no device matches
        // that" for a keyboard sitting right there — and `plan_claim` cannot
        // refuse with the transport reason a device it has never heard of.
        //
        // One candidate per physical device, not per service node: a paired
        // keyboard wears several `BTHENUM` nodes and they are one keyboard.
        let mut seen: Vec<String> = Vec::new();
        for node in nodes
            .iter()
            .filter(|n| n.enumerator.eq_ignore_ascii_case(BTHENUM))
        {
            let address = bd_addr(node);
            let board = match &address {
                Some(addr) => format!("{BTHENUM}\\{addr}").to_lowercase(),
                None => node.instance_id.to_lowercase(),
            };
            // Only devices with a keyboard behind them. The rest of a radio's
            // service nodes — audio sinks, serial ports, the local pseudo-
            // devices — are listed by the DEVICE LIST, which is a different
            // question from "what could ksx capture or claim".
            let keyboard = keyboards
                .iter()
                .find(|kb| {
                    kb.node.instance_id.eq_ignore_ascii_case(&node.instance_id)
                        || (address.is_some() && bd_addr(&kb.node) == address)
                })
                .map(|kb| kb.node.clone());
            if keyboard.is_none() || seen.contains(&board) {
                continue;
            }
            seen.push(board.clone());
            candidates.push(Candidate {
                board,
                interface: node.clone(),
                keyboard,
                // Permanent, and its own state: it IS a keyboard and
                // Interception can capture it today. See the variant's docs.
                state: ClaimState::InterceptionOnly,
                transport: Transport::Bluetooth,
            });
        }
        candidates.sort_by(|a, b| a.interface.instance_id.cmp(&b.interface.instance_id));
        Survey {
            candidates,
            keyboards,
        }
    }

    /// **Keyboards that could actually type for you right now**, counted by
    /// physical board.
    ///
    /// This is the number the last-keyboard refusal is decided on, and every
    /// part of it is there because the naive count — rows in the keyboard class
    /// — can be talked into claiming the only board that works:
    ///
    /// - **Usable only.** A keyboard bound to `winusb.sys` (by ksx or by
    ///   anything else), disabled, driverless, or present-but-not-connected —
    ///   a paired Bluetooth keyboard that is switched off is the everyday case —
    ///   cannot deliver a keystroke. Counting it says "you have a spare" about a
    ///   keyboard that will not type the release command.
    /// - **By board, not by node.** One I-PAC produces several HID nodes
    ///   (`MI_00`'s keyboard, `MI_01`'s collections), and Windows is perfectly
    ///   happy to call more than one of them keyboard-class. Counting nodes lets
    ///   a single board look like two keyboards and claim itself.
    ///
    /// Two identical I-PACs on different ports are still two boards: they have
    /// different parents in the tree, which is the same structural identity the
    /// WinUSB backend uses (`docs/USE-CASES.md` T4).
    pub fn keyboard_count(&self) -> usize {
        self.usable_boards().len()
    }

    /// Keyboards that can deliver a keystroke right now.
    pub fn usable_keyboards(&self) -> impl Iterator<Item = &KeyboardNode> {
        self.keyboards.iter().filter(|kb| kb.is_usable())
    }

    /// The distinct physical boards behind [`Self::usable_keyboards`].
    pub fn usable_boards(&self) -> std::collections::BTreeSet<&str> {
        self.usable_keyboards()
            .map(|kb| kb.board.as_str())
            .collect()
    }

    /// How many usable keyboard boards would be left if `board` stopped being a
    /// keyboard — the whole of the last-keyboard question.
    pub fn keyboards_without(&self, board: &str) -> usize {
        self.usable_boards()
            .iter()
            .filter(|other| **other != board)
            .count()
    }

    /// Find the candidate a user meant.
    ///
    /// Accepts a full instance path, or any case-insensitive substring of one
    /// that is unique (`MI_00`, `D209&PID_0430&MI_00`, the HID child's path).
    /// Ambiguity is a refusal, never a guess — picking one of two identical
    /// I-PACs on the user's behalf is exactly the failure T4 is about.
    pub fn resolve(&self, requested: &str) -> Result<&Candidate, Refusal> {
        let needle = requested.trim().to_lowercase();
        if needle.is_empty() {
            return Err(Refusal::UnknownDevice {
                requested: requested.to_owned(),
                known: self.candidate_ids(),
            });
        }
        let exact: Vec<&Candidate> = self
            .candidates
            .iter()
            .filter(|c| {
                c.interface.instance_id.to_lowercase() == needle
                    || c.keyboard
                        .as_ref()
                        .is_some_and(|k| k.instance_id.to_lowercase() == needle)
            })
            .collect();
        let matches = if exact.is_empty() {
            self.candidates
                .iter()
                .filter(|c| {
                    c.interface.instance_id.to_lowercase().contains(&needle)
                        || c.keyboard
                            .as_ref()
                            .is_some_and(|k| k.instance_id.to_lowercase().contains(&needle))
                })
                .collect()
        } else {
            exact
        };
        match matches.len() {
            0 => Err(Refusal::UnknownDevice {
                requested: requested.to_owned(),
                known: self.candidate_ids(),
            }),
            1 => Ok(matches[0]),
            _ => Err(Refusal::Ambiguous {
                requested: requested.to_owned(),
                matches: matches
                    .iter()
                    .map(|c| c.interface.instance_id.clone())
                    .collect(),
            }),
        }
    }

    fn candidate_ids(&self) -> Vec<String> {
        self.candidates
            .iter()
            .map(|c| c.interface.instance_id.clone())
            .collect()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            // Boards that can type right now, not rows in the keyboard class —
            // see `Survey::keyboard_count`.
            "keyboard_count": self.keyboard_count(),
            "keyboards": self.keyboards.iter().map(|k| serde_json::json!({
                "instance_id": k.node.instance_id,
                "description": k.node.description(),
                "service": k.node.service,
                "board": k.board,
                "usable": k.is_usable(),
                "unusable_because": k.unusable,
            })).collect::<Vec<_>>(),
            "candidates": self.candidates.iter().map(|c| serde_json::json!({
                "instance_id": c.interface.instance_id,
                "hardware_id": c.interface.usb_hardware_id(),
                "description": c.interface.description(),
                "driver": c.interface.service,
                "vid": c.interface.vid_pid().map(|(v, _)| format!("{v:04X}")),
                "pid": c.interface.vid_pid().map(|(_, p)| format!("{p:04X}")),
                "interface": c.interface.interface_number(),
                "state": c.state.code(),
                "board": c.board,
                "claimable": c.state == ClaimState::Claimable,
                "ksx_device_id": c.ksx_device_id(),
                "keyboard_instance_id": c.keyboard.as_ref().map(|k| k.instance_id.clone()),
                "vendor": c.interface.vid_pid().and_then(|(v, p)| ksx_core::vendors::name_for(v, p)),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Guard against a malformed tree turning the walk below into a spin. A USB
/// keyboard is three levels deep (composite → interface → HID child); nothing
/// ksx cares about is anywhere near eight.
const MAX_TREE_DEPTH: usize = 8;

/// The **physical board** a node belongs to: its topmost ancestor in the
/// present tree.
///
/// Windows stamps a node's children's instance ids with its `ParentIdPrefix`,
/// which is the only parent link visible from the registry alone
/// ([`DeviceNode::is_parent_of`]). Walking it up from a HID keyboard node lands
/// on the USB interface that produced it and then on the composite device
/// itself — so every collection and every interface of one I-PAC answers with
/// the same string, and two identical I-PACs on different ports answer with
/// different ones.
///
/// Deliberately *not* done by string surgery on the instance path: two devices
/// on one hub share the leading segments of their instance ids, and merging
/// them would under-count the machine's keyboards.
pub fn board_of(node: &DeviceNode, nodes: &[DeviceNode]) -> String {
    let mut current = node;
    for _ in 0..MAX_TREE_DEPTH {
        let parent = nodes.iter().find(|n| {
            !n.instance_id.eq_ignore_ascii_case(&current.instance_id) && n.is_parent_of(current)
        });
        match parent {
            Some(parent) => current = parent,
            None => break,
        }
    }
    // The walk can stop one level short when a composite parent is missing from
    // the tree, or carries no `ParentIdPrefix` to link by. An interface node's
    // instance is exactly its parent's prefix plus an index (`7&25eea38c&0` +
    // `&0000`), so dropping the index names the same board without inventing a
    // link. Applied ONLY to `MI_xx` interface nodes, where that shape is
    // guaranteed — on an ordinary device the trailing segment is a port number
    // and two different devices on one hub would collapse into one.
    if let (true, Some(_)) = (
        current.enumerator.eq_ignore_ascii_case("USB"),
        current.interface_number(),
    ) {
        if let Some((stem, _)) = current.instance.rsplit_once('&') {
            let device = match current.device_key.to_uppercase().find("&MI_") {
                Some(at) => &current.device_key[..at],
                None => current.device_key.as_str(),
            };
            return format!("{}\\{device}\\{stem}", current.enumerator).to_lowercase();
        }
    }
    current.instance_id.to_lowercase()
}

/// Why this keyboard-class node cannot deliver a keystroke to Windows right
/// now, or `None` if it can.
///
/// The `winusb.sys` checks look at the node *and its ancestors*: a claim binds
/// the USB interface, and until Windows re-enumerates, a stale HID child can
/// still be sitting in the tree looking like a working keyboard. It is not one —
/// nothing is reading the keyboard stack for it any more.
pub fn why_unusable(node: &DeviceNode, nodes: &[DeviceNode]) -> Option<&'static str> {
    if let Some(trouble) = node.trouble() {
        return Some(trouble);
    }
    let mut current = node;
    for _ in 0..MAX_TREE_DEPTH {
        if current.service_is(WINUSB_SERVICE) {
            return Some("claimed through winusb.sys — not in the keyboard stack");
        }
        if current.status.is_some_and(|s| !s.is_live()) {
            return Some("its parent device is not working");
        }
        match nodes.iter().find(|n| {
            !n.instance_id.eq_ignore_ascii_case(&current.instance_id) && n.is_parent_of(current)
        }) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    // A keyboard-class node with no function driver at all is a yellow bang in
    // Device Manager, not a keyboard.
    if node.service.as_deref().unwrap_or_default().is_empty() {
        return Some("no driver is bound to it");
    }
    None
}

/// Every device the PnP manager reports as **present**, with its `Enum`
/// properties and its live status. Read-only — see `win::devices`.
///
/// Public because the Bluetooth enumeration in `ksx-capture` reads the same
/// tree this survey does, and reading it twice through two different
/// mechanisms is how a device list ends up disagreeing with the refusal that
/// guards it. One `CM_Get_Device_ID_ListW` walk, two consumers.
///
/// Off Windows there is no device tree, so the answer is an empty tree rather
/// than a compile error — every caller already treats that conservatively.
#[cfg(windows)]
pub fn present_nodes() -> Vec<DeviceNode> {
    crate::win::devices::present_nodes()
}

#[cfg(not(windows))]
pub fn present_nodes() -> Vec<DeviceNode> {
    Vec::new()
}

/// Survey the live machine. Read-only.
#[cfg(windows)]
pub fn survey() -> Survey {
    Survey::from_nodes(&present_nodes())
}

/// Off Windows there is no device tree; every claim then refuses for want of a
/// device, which is the correct answer rather than a compile error.
#[cfg(not(windows))]
pub fn survey() -> Survey {
    Survey::default()
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Why ksx will not do what was asked. Every one of these is exit code 2.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("no device matches '{requested}'")]
    UnknownDevice {
        requested: String,
        known: Vec<String>,
    },
    #[error("'{requested}' matches {} devices", matches.len())]
    Ambiguous {
        requested: String,
        matches: Vec<String>,
    },
    #[error("{instance_id} is not a keyboard interface")]
    NotAKeyboard { instance_id: String },
    /// The device is a keyboard, and it is on a transport a claim can never
    /// bind.
    ///
    /// Separate from [`Self::NotAKeyboard`] because the two send a user to
    /// opposite places. "Not a keyboard" means *pick a different interface*.
    /// This means *the interface is right and the BACKEND is wrong* — use
    /// Interception, which captures this device today. Nothing about it changes
    /// with a reboot, a driver, or a future ksx release, so the advice must not
    /// read as "not yet".
    #[error("{instance_id} is a {transport} keyboard, and a WinUSB claim can never bind one")]
    TransportCannotClaim {
        instance_id: String,
        transport: Transport,
    },
    #[error("{instance_id} is already bound to winusb.sys")]
    AlreadyClaimed { instance_id: String },
    #[error("{instance_id} is not claimed by ksx (driver: {driver})")]
    NotClaimed { instance_id: String, driver: String },
    /// The bricking case.
    #[error(
        "{instance_id} is the ONLY keyboard on this machine — claiming it would leave you \
         with no way to type"
    )]
    LastKeyboard { instance_id: String },
    /// The twins case, and the reason it cannot simply be allowed.
    ///
    /// A generated INF binds by **hardware id**, and two boards of the same
    /// model share one (`USB\VID_D209&PID_0430&MI_00` carries no instance or
    /// port component). So `pnputil /install` would claim every matching
    /// interface, not the one that was asked for — and the last-keyboard
    /// refusal, which subtracts a single board, would happily approve it.
    ///
    /// Windows offers no INF-level escape: an instance id is not matchable in a
    /// models section. Binding one twin and not the other needs per-devnode
    /// installation (SetupAPI `SetupDiSetSelectedDriver` + `DiInstallDevice`),
    /// which ksx does not do yet. Until it does, this refuses.
    #[error(
        "{instance_id} shares the hardware id {hardware_id} with {} other connected \
         interface(s) — a claim would take all of them",
        siblings.len()
    )]
    SharedHardwareId {
        instance_id: String,
        hardware_id: String,
        /// The other instance ids the same INF would bind.
        siblings: Vec<String>,
    },
    #[error("this needs an administrator token; re-run from an elevated prompt")]
    NeedsElevation,
}

impl Refusal {
    /// Stable code for `--json` consumers and scripts.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::UnknownDevice { .. } => "unknown-device",
            Refusal::Ambiguous { .. } => "ambiguous-device",
            Refusal::NotAKeyboard { .. } => "not-a-keyboard",
            Refusal::TransportCannotClaim { .. } => "transport-cannot-claim",
            Refusal::AlreadyClaimed { .. } => "already-claimed",
            Refusal::NotClaimed { .. } => "not-claimed",
            Refusal::LastKeyboard { .. } => "last-keyboard",
            Refusal::SharedHardwareId { .. } => "shared-hardware-id",
            Refusal::NeedsElevation => "needs-elevation",
        }
    }

    /// What to do about it. A refusal with no way forward is just an error
    /// message.
    pub fn advice(&self) -> String {
        match self {
            Refusal::UnknownDevice { known, .. } => {
                if known.is_empty() {
                    "run `ksx winusb status` — no USB HID interfaces were found at all".to_owned()
                } else {
                    format!(
                        "run `ksx winusb status` and pass one of:\n  {}",
                        known.join("\n  ")
                    )
                }
            }
            Refusal::Ambiguous { matches, .. } => format!(
                "be more specific — these all matched:\n  {}",
                matches.join("\n  ")
            ),
            // Generic first, board-specific only when we actually recognise the
            // board. This used to tell every user about I-PAC interface numbers
            // regardless of what they had plugged in — advice that is wrong for
            // anyone who does not own the author's encoder, which is everyone
            // else (`docs/DEVICE-IDENTITY.md` §6).
            Refusal::NotAKeyboard { instance_id } => {
                let mut advice = "only the keyboard interface is claimable. A composite board's \
                                  other interfaces carry its mouse, system/consumer and vendor \
                                  collections; claiming one of those takes the device off the \
                                  input stack without giving ksx any keys to read. \
                                  `ksx winusb status` marks which interface is the keyboard."
                    .to_owned();
                if vendor_of(instance_id) == Some(ULTIMARC_VID) {
                    advice.push_str(
                        "\n\nOn this board (Ultimarc): the keyboard is MI_00. MI_01 carries the \
                         mouse/system/consumer collections and MI_02 the vendor ones — claiming \
                         either would break the trackball for nothing.",
                    );
                }
                advice
            }
            // Never "not supported yet". The instruction is to use the backend
            // that works, right now, on this exact device — because it does.
            Refusal::TransportCannotClaim { instance_id, .. } => format!(
                "{}\n\nThis keyboard is NOT out of reach: Interception captures it today — it is \
                 a keyboard on the Windows input stack like any other, so ksx can split it into \
                 virtual pads with no claim at all. Pick it and leave the backend alone:\n  ksx \
                 device pick {instance_id}\n\nThat writes backend = \"interception\", which is \
                 the only backend this device will ever have.",
                ksx_core::transport::WINUSB_NEEDS_A_USB_INTERFACE
            ),
            Refusal::AlreadyClaimed { .. } => {
                "nothing to do. `ksx winusb release <device>` puts it back on the keyboard \
                 driver."
                    .to_owned()
            }
            Refusal::NotClaimed { .. } => {
                "release only undoes a ksx WinUSB claim. This device is already on its normal \
                 driver."
                    .to_owned()
            }
            Refusal::LastKeyboard { .. } => {
                "plug in a second keyboard on a different USB port first, and keep it \
                 unassigned. A WinUSB-claimed interface is invisible to Windows: ksx can \
                 re-inject its keystrokes while it is running (see `ksx winusb status`), but \
                 SendInput cannot reach the lock screen, a UAC prompt or Ctrl+Alt+Del, and it \
                 cannot do anything at all if ksx is not running. docs/RECOVERY.md §2."
                    .to_owned()
            }
            Refusal::SharedHardwareId {
                hardware_id,
                siblings,
                ..
            } => format!(
                "two boards of the same model are indistinguishable to an INF: the driver \
                 matches on {hardware_id}, which these also carry, so installing it would \
                 claim every one of them:\n  {}\n\nUnplug the others and claim this board on \
                 its own, or keep them on the Interception backend. Telling identical boards \
                 apart during a claim needs per-device installation, which ksx does not do \
                 yet — see docs/DEVICE-IDENTITY.md §2.",
                siblings.join("\n  ")
            ),
            Refusal::NeedsElevation => {
                "pnputil changes driver bindings, which needs administrator. ksx never \
                 self-elevates: open an elevated PowerShell and re-run the same command."
                    .to_owned()
            }
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "refused": true,
            "code": self.code(),
            "message": self.to_string(),
            "advice": self.advice(),
        })
    }
}

// ---------------------------------------------------------------------------
// Planned commands
// ---------------------------------------------------------------------------

/// One `pnputil` invocation, with the reason it is in the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
    /// One line of "why", printed next to the command in `--dry-run`.
    pub why: &'static str,
}

impl PlannedCommand {
    pub fn pnputil(args: &[&str], why: &'static str) -> Self {
        Self {
            program: pnputil_path().display().to_string(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            why,
        }
    }

    /// Copy-pasteable, with the same quoting rules `ksx install-drivers` uses.
    pub fn command_line(&self) -> String {
        let mut argv = vec![self.program.clone()];
        argv.extend(self.args.iter().cloned());
        crate::installer::quote_argv(&argv)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "program": self.program,
            "args": self.args,
            "why": self.why,
            "command_line": self.command_line(),
        })
    }
}

/// Full path to `pnputil.exe`. Absolute and from `%SystemRoot%`, never bare —
/// a driver-binding tool resolved through `PATH` is a tool somebody else can
/// supply.
pub fn pnputil_path() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
    PathBuf::from(root).join("System32").join("pnputil.exe")
}

// ---------------------------------------------------------------------------
// INF generation
// ---------------------------------------------------------------------------

/// The INF filename for one hardware id: `ksx-winusb-vid_d209-pid_0430-mi_00.inf`.
pub fn inf_file_name(hardware_id: &str) -> String {
    let slug: String = hardware_id
        .trim_start_matches("USB\\")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse runs of '-' so `&` and `\` do not produce `--`.
    let mut out = String::with_capacity(slug.len());
    let mut last_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !last_dash {
                out.push(c);
            }
            last_dash = true;
        } else {
            out.push(c);
            last_dash = false;
        }
    }
    format!("{INF_PREFIX}{}.inf", out.trim_matches('-'))
}

/// Render the WinUSB device INF for one interface.
///
/// Shape follows Microsoft's "WinUSB device INF" template: `Include`/`Needs`
/// against the in-box `winusb.inf` rather than shipping a driver, so the only
/// binary involved is `%SystemRoot%\System32\drivers\winusb.sys` — WHQL-signed,
/// in-box, and unaffected by the cross-signed-trust removal.
///
/// Both `NTamd64` and `NTarm64` model sections are emitted: the cabinet is x64,
/// but an INF that silently does nothing on ARM64 is a worse failure than one
/// that never installed.
pub fn render_inf(hardware_id: &str, device_name: &str) -> String {
    let file = inf_file_name(hardware_id);
    let cat = file.replace(".inf", ".cat");
    format!(
        r#";
; {file}
;
; Binds ONE USB interface to Microsoft's in-box winusb.sys so ksx can read its
; HID interrupt-IN endpoint directly. Generated by `ksx winusb claim`.
;
; Consequence, stated plainly: this interface leaves the keyboard stack. Windows
; will not see a single keystroke from it again until the binding is removed
; (`ksx winusb release`, or pnputil by hand — see docs/RECOVERY.md section 2).
; That is the feature: blocking becomes structural, with no filter driver and no
; cross-signed certificate to expire.
;
; This INF must be signed with a trusted catalog before pnputil will install it.
; winusb.sys is signed; a third-party INF pointing at it is not. See
; docs/MIGRATION-WINUSB.md.
;
[Version]
Signature   = "$Windows NT$"
Class       = USBDevice
ClassGuid   = {USB_DEVICE_CLASS_GUID}
Provider    = %ProviderName%
CatalogFile = {cat}
DriverVer   = {DRIVER_VER}
PnpLockdown = 1

[Manufacturer]
%ManufacturerName% = ksxDevice,NTamd64,NTarm64

[ksxDevice.NTamd64]
%DeviceName% = USB_Install, {hardware_id}

[ksxDevice.NTarm64]
%DeviceName% = USB_Install, {hardware_id}

[USB_Install]
Include = winusb.inf
Needs   = WINUSB.NT

[USB_Install.Services]
Include = winusb.inf
Needs   = WINUSB.NT.Services

[USB_Install.HW]
AddReg = Dev_AddReg

[Dev_AddReg]
; ksx finds claimed interfaces through this device interface class.
HKR,,DeviceInterfaceGUIDs,0x10000,"{KSX_DEVICE_INTERFACE_GUID}"

[Strings]
ProviderName     = "ksx"
ManufacturerName = "ksx (WinUSB claim)"
DeviceName       = "{device_name}"
"#
    )
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

/// Everything `ksx winusb claim` would do, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimPlan {
    pub instance_id: String,
    pub hardware_id: String,
    pub device_name: String,
    /// The ksx device id that will stop being a keyboard.
    pub ksx_device_id: String,
    pub inf_path: PathBuf,
    pub inf_text: String,
    pub commands: Vec<PlannedCommand>,
    /// Present keyboard-class devices before and after. `after` is what makes
    /// the refusal decidable, and printing both makes the refusal explicable.
    pub keyboards_before: usize,
    pub keyboards_after: usize,
}

/// Plan a claim, or refuse.
///
/// `inf_dir` is where the generated INF will be written — nothing is written
/// here; the plan only says where.
pub fn plan_claim(survey: &Survey, requested: &str, inf_dir: &Path) -> Result<ClaimPlan, Refusal> {
    let candidate = survey.resolve(requested)?;
    match candidate.state {
        ClaimState::Claimed => {
            return Err(Refusal::AlreadyClaimed {
                instance_id: candidate.interface.instance_id.clone(),
            })
        }
        ClaimState::NotAKeyboard | ClaimState::ForeignDriver => {
            return Err(Refusal::NotAKeyboard {
                instance_id: candidate.interface.instance_id.clone(),
            })
        }
        // The interface is right and the BACKEND is wrong. Saying "not a
        // keyboard" here would send someone hunting for a different interface
        // of a device that has exactly one, and there is no interface on it a
        // claim could bind.
        ClaimState::InterceptionOnly => {
            return Err(Refusal::TransportCannotClaim {
                instance_id: candidate.interface.instance_id.clone(),
                transport: candidate.transport,
            })
        }
        ClaimState::Claimable => {}
    }

    let hardware_id =
        candidate
            .interface
            .usb_hardware_id()
            .ok_or_else(|| Refusal::NotAKeyboard {
                instance_id: candidate.interface.instance_id.clone(),
            })?;

    // An INF binds by hardware id, and a hardware id has no instance or port
    // component — so `USB\VID_D209&PID_0430&MI_00` names the keyboard interface
    // of EVERY I-PAC 4X plugged in, not the one that was asked for. Installing
    // it would claim all of them.
    //
    // This has to be checked before the last-keyboard arithmetic below, because
    // that arithmetic subtracts one board: with two identical boards and no
    // other keyboard it computes "one left" and approves a claim that takes
    // both. The refusal that exists to prevent a lockout would authorise one.
    let siblings: Vec<String> = survey
        .candidates
        .iter()
        .filter(|other| {
            other.interface.instance_id != candidate.interface.instance_id
                && other
                    .interface
                    .usb_hardware_id()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&hardware_id))
        })
        .map(|other| other.interface.instance_id.clone())
        .collect();
    if !siblings.is_empty() {
        return Err(Refusal::SharedHardwareId {
            instance_id: candidate.interface.instance_id.clone(),
            hardware_id,
            siblings,
        });
    }

    // What a claim actually costs you is the whole **board**, not the one node
    // hanging off this interface: the other interfaces and collections of the
    // same physical I-PAC are the same piece of plastic on the same cable, and
    // "you still have a keyboard" is not true of a board you just claimed.
    // Counting boards is also what stops one board being talked into looking
    // like two keyboards and claiming itself (see `Survey::keyboard_count`).
    let before = survey.keyboard_count();
    let after = survey.keyboards_without(&candidate.board);
    if after == 0 {
        return Err(Refusal::LastKeyboard {
            instance_id: candidate.interface.instance_id.clone(),
        });
    }

    let device_name = claim_device_name(candidate);
    let inf_text = render_inf(&hardware_id, &device_name);
    let inf_path = inf_dir.join(inf_file_name(&hardware_id));
    let inf_arg = inf_path.display().to_string();

    let commands = vec![
        PlannedCommand::pnputil(
            &["/add-driver", &inf_arg, "/install"],
            "add the generated INF to the driver store and bind it to the matching interface",
        ),
        PlannedCommand::pnputil(
            &["/scan-devices"],
            "re-enumerate, so the rebind takes effect without a replug",
        ),
    ];

    Ok(ClaimPlan {
        instance_id: candidate.interface.instance_id.clone(),
        hardware_id,
        device_name,
        ksx_device_id: candidate.ksx_device_id().to_owned(),
        inf_path,
        inf_text,
        commands,
        keyboards_before: before,
        keyboards_after: after,
    })
}

fn claim_device_name(candidate: &Candidate) -> String {
    let desc = candidate
        .keyboard
        .as_ref()
        .map(|k| k.description())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| candidate.interface.description());
    let iface = candidate
        .interface
        .interface_number()
        .map(|n| format!(" MI_{n:02}"))
        .unwrap_or_default();
    if desc.is_empty() {
        format!("ksx WinUSB claim{iface}")
    } else {
        format!("{desc} (ksx WinUSB claim{iface})")
    }
}

impl ClaimPlan {
    /// The signing prerequisite, spelled out. Printed by every dry run because
    /// it is the step that actually stops people.
    pub fn signing_note(&self) -> String {
        let file = self.inf_path.display();
        let cat = self.inf_path.with_extension("cat");
        format!(
            "SIGNING (required — pnputil will reject an unsigned third-party INF on x64):\n\
             \n\
             winusb.sys itself is in-box and WHQL-signed. The INF above is not, and 64-bit\n\
             Windows will not add an unsigned INF to the driver store. Two real options:\n\
             \n\
             (a) Self-signed catalog — what Zadig/libwdi do, and what a dedicated cabinet\n\
             wants. From an elevated Developer Command Prompt with the WDK on PATH:\n\
             \n\
             \x20   inf2cat /driver:\"{dir}\" /os:10_X64\n\
             \x20   makecert -r -pe -ss PrivateCertStore -n CN=ksx-cabinet ksx-cabinet.cer\n\
             \x20   signtool sign /fd sha256 /s PrivateCertStore /n ksx-cabinet \"{cat}\"\n\
             \x20   certutil -addstore -f Root ksx-cabinet.cer\n\
             \x20   certutil -addstore -f TrustedPublisher ksx-cabinet.cer\n\
             \n\
             (b) Attestation signing through Partner Center — free, but needs an EV code\n\
             signing certificate and an account. The right answer if ksx is ever\n\
             redistributed; overkill for one cabinet.\n\
             \n\
             What NOT to do: test-signing mode (`bcdedit /set testsigning on`). It disables\n\
             a Secure Boot guarantee machine-wide to install one INF, and the watermark is\n\
             the least of it.\n\
             \n\
             Or skip the INF entirely and use Zadig once, by hand, on {hwid}. ksx does not\n\
             care how the interface got bound to winusb.sys — only that it is.\n\
             \n\
             INF to sign: {file}",
            dir = self
                .inf_path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            cat = cat.display(),
            hwid = self.hardware_id,
        )
    }

    pub fn render_human(&self, dry_run: bool) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "CLAIM  {}\n  hardware id : {}\n  ksx device  : {}\n  after claim : Windows sees no \
             keyboard here; ksx reads the interrupt endpoint directly\n",
            self.instance_id, self.hardware_id, self.ksx_device_id
        ));
        out.push_str(&format!(
            "  keyboards   : {} present now -> {} after the claim\n",
            self.keyboards_before, self.keyboards_after
        ));
        out.push_str(&format!("\nINF ({}):\n\n", self.inf_path.display()));
        for line in self.inf_text.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("\nCOMMANDS (in this order):\n");
        for (i, cmd) in self.commands.iter().enumerate() {
            out.push_str(&format!(
                "  {}. {}\n     # {}\n",
                i + 1,
                cmd.command_line(),
                cmd.why
            ));
        }
        out.push('\n');
        out.push_str(&self.signing_note());
        out.push('\n');
        if dry_run {
            out.push_str(
                "\nDRY RUN — nothing was written and nothing was run. Re-run with --yes to \
                 apply.\nBefore you do: read docs/RECOVERY.md section 2 and have a second \
                 keyboard plugged in.\n",
            );
        }
        out
    }

    pub fn to_json(&self, dry_run: bool) -> serde_json::Value {
        serde_json::json!({
            "action": "claim",
            "dry_run": dry_run,
            "instance_id": self.instance_id,
            "hardware_id": self.hardware_id,
            "ksx_device_id": self.ksx_device_id,
            "device_name": self.device_name,
            "inf_path": self.inf_path.display().to_string(),
            "inf_text": self.inf_text,
            "commands": self.commands.iter().map(PlannedCommand::to_json).collect::<Vec<_>>(),
            "keyboards_before": self.keyboards_before,
            "keyboards_after": self.keyboards_after,
            "signing_required": true,
        })
    }
}

// ---------------------------------------------------------------------------
// Release
// ---------------------------------------------------------------------------

/// Everything `ksx winusb release` would do — the rollback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasePlan {
    pub instance_id: String,
    pub hardware_id: Option<String>,
    /// The generated INF's filename, so the driver-store entry can be found.
    pub inf_file: Option<String>,
    pub commands: Vec<PlannedCommand>,
}

/// Plan a release.
///
/// `force` releases a device that is *not* WinUSB-bound. That is the recovery
/// case: a half-finished claim, or a device tree ksx cannot read because the
/// registry is inconsistent. Without it, `release` refuses anything that is not
/// currently claimed, so a typo cannot bounce a working keyboard.
pub fn plan_release(survey: &Survey, requested: &str, force: bool) -> Result<ReleasePlan, Refusal> {
    let candidate = survey.resolve(requested)?;
    if candidate.state != ClaimState::Claimed && !force {
        return Err(Refusal::NotClaimed {
            instance_id: candidate.interface.instance_id.clone(),
            driver: candidate
                .interface
                .service
                .clone()
                .unwrap_or_else(|| "none".to_owned()),
        });
    }
    let hardware_id = candidate.interface.usb_hardware_id();
    let inf_file = hardware_id.as_deref().map(inf_file_name);
    Ok(ReleasePlan {
        commands: release_commands(&candidate.interface.instance_id, inf_file.as_deref()),
        instance_id: candidate.interface.instance_id.clone(),
        hardware_id,
        inf_file,
    })
}

/// The rollback sequence, also reachable without a survey — `docs/RECOVERY.md`
/// prints exactly this, and a user with a dead panel and a mouse needs it to
/// work when ksx does not.
pub fn release_commands(instance_id: &str, inf_file: Option<&str>) -> Vec<PlannedCommand> {
    let mut commands = vec![PlannedCommand::pnputil(
        &["/remove-device", instance_id],
        "remove the devnode so the binding is dropped",
    )];
    if inf_file.is_some() {
        commands.push(PlannedCommand::pnputil(
            &["/enum-drivers"],
            "find the oemNN.inf the generated INF was published as",
        ));
        commands.push(PlannedCommand::pnputil(
            &["/delete-driver", "<oemNN.inf>", "/uninstall", "/force"],
            "REQUIRED: the ksx INF matches on hardware id and outranks the in-box input.inf, \
             so a rescan would re-bind WinUSB while it is still in the driver store",
        ));
    }
    commands.push(PlannedCommand::pnputil(
        &["/scan-devices"],
        "re-enumerate; HidUsb/kbdhid bind again and the keyboard comes back",
    ));
    commands
}

impl ReleasePlan {
    pub fn render_human(&self, dry_run: bool) -> String {
        let mut out = format!(
            "RELEASE  {}\n  after release: the keyboard driver binds again and the device types \
             normally\n",
            self.instance_id
        );
        if let Some(inf) = &self.inf_file {
            out.push_str(&format!("  ksx INF      : {inf}\n"));
        }
        out.push_str("\nCOMMANDS (in this order):\n");
        for (i, cmd) in self.commands.iter().enumerate() {
            out.push_str(&format!(
                "  {}. {}\n     # {}\n",
                i + 1,
                cmd.command_line(),
                cmd.why
            ));
        }
        if self.inf_file.is_some() {
            out.push_str(
                "\nNOTE: the /delete-driver step removes ksx's OWN INF. If this interface was\n\
                 bound to winusb.sys by something else (Zadig, a vendor tool, another app's\n\
                 installer), ksx will find no matching driver-store entry and skip that step —\n\
                 and you must delete THAT INF yourself, or the rescan re-binds WinUSB. Find it\n\
                 with `pnputil /enum-drivers` and match on the hardware id.\n",
            );
        }
        out.push_str(
            "\nWith --yes, ksx resolves <oemNN.inf> itself from `pnputil /enum-drivers`.\n\
             If ksx will not run at all, run these by hand from an elevated prompt — or use\n\
             Device Manager: View > Devices by connection, find the interface, Uninstall\n\
             device (leave \"delete the driver software\" UNCHECKED), then Action > Scan for\n\
             hardware changes. docs/RECOVERY.md section 2 has the mouse-only walkthrough.\n",
        );
        if dry_run {
            out.push_str("\nDRY RUN — nothing was run. Re-run with --yes to apply.\n");
        }
        out
    }

    pub fn to_json(&self, dry_run: bool) -> serde_json::Value {
        serde_json::json!({
            "action": "release",
            "dry_run": dry_run,
            "instance_id": self.instance_id,
            "hardware_id": self.hardware_id,
            "inf_file": self.inf_file,
            "commands": self.commands.iter().map(PlannedCommand::to_json).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Driver store
// ---------------------------------------------------------------------------

/// One entry of `pnputil /enum-drivers`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreDriver {
    /// `oem42.inf` — the name `pnputil /delete-driver` takes.
    pub published_name: String,
    /// `ksx-winusb-vid-d209-pid-0430-mi-00.inf` — the name it was added under.
    pub original_name: String,
    pub provider: String,
}

/// Parse `pnputil /enum-drivers` output.
///
/// Label-agnostic on purpose: `pnputil`'s field labels are localised, so this
/// keys on the *shape* of the values — the published name is the `oemNN.inf`,
/// the original name is the other `.inf` — instead of on the English strings.
/// A machine in German must still be able to roll back.
pub fn parse_enum_drivers(text: &str) -> Vec<StoreDriver> {
    let mut out = Vec::new();
    let mut current = StoreDriver::default();
    let flush = |d: &mut StoreDriver, out: &mut Vec<StoreDriver>| {
        if !d.published_name.is_empty() {
            out.push(std::mem::take(d));
        } else {
            *d = StoreDriver::default();
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut current, &mut out);
            continue;
        }
        let Some((_label, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let lower = value.to_lowercase();
        if lower.ends_with(".inf") {
            if lower.starts_with("oem") {
                // A second oemNN.inf means a new block started without a blank
                // line between them.
                if !current.published_name.is_empty() {
                    flush(&mut current, &mut out);
                }
                current.published_name = value.to_owned();
            } else {
                current.original_name = value.to_owned();
            }
        } else if current.provider.is_empty() && !lower.ends_with(".cat") {
            // First non-filename field after the names is the provider on every
            // locale's layout; harmless if it picks up the class instead.
            current.provider = value.to_owned();
        }
    }
    flush(&mut current, &mut out);
    out
}

/// The driver-store entries ksx published for `inf_file`.
pub fn store_drivers_matching<'a>(
    drivers: &'a [StoreDriver],
    inf_file: &str,
) -> Vec<&'a StoreDriver> {
    drivers
        .iter()
        .filter(|d| d.original_name.eq_ignore_ascii_case(inf_file))
        .collect()
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("could not write the INF to {path}: {source}")]
    WriteInf {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not run {command}: {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{command} failed (exit {code})\n{output}")]
    Failed {
        command: String,
        code: i32,
        output: String,
    },
    /// Stopped a release BEFORE the rescan, because finishing it would have
    /// re-claimed the board.
    ///
    /// The release order is: remove the devnode, delete the ksx INF from the
    /// driver store, rescan. That middle step is not optional — the generated
    /// INF matches on hardware id and outranks the in-box `input.inf`, so a
    /// rescan performed while it is still in the store binds WinUSB straight
    /// back on. If ksx cannot confirm the INF is gone, rescanning would undo
    /// the release and report success for it.
    #[error(
        "released the devnode but could not confirm the ksx INF is out of the driver store \
         ({reason}) — stopping before the rescan, because a rescan now could re-claim the board"
    )]
    ReleaseUnconfirmed { reason: String, instance_id: String },
}

impl ApplyError {
    /// Recognise the failure everyone hits first, and say what it means.
    pub fn hint(&self) -> Option<&'static str> {
        let ApplyError::Failed { output, .. } = self else {
            return None;
        };
        let lower = output.to_lowercase();
        if lower.contains("signature") || lower.contains("0xe0000247") || lower.contains("signed") {
            return Some(
                "pnputil rejected the INF because it has no trusted catalog. This is the \
                 expected first failure — see the SIGNING section of `ksx winusb claim \
                 --dry-run`, or bind the interface once with Zadig instead.",
            );
        }
        if lower.contains("access") || lower.contains("denied") || lower.contains("0x5") {
            return Some("run from an elevated prompt: pnputil needs an administrator token.");
        }
        None
    }

    /// What the machine looks like right now, and how to finish by hand.
    ///
    /// A release that stops partway leaves a devnode removed and a board that
    /// is neither claimed nor a keyboard until something rescans — which is
    /// exactly the moment a user needs instructions rather than a stack trace.
    pub fn recovery(&self) -> Option<String> {
        match self {
            ApplyError::ReleaseUnconfirmed { instance_id, .. } => Some(format!(
                concat!(
                    "The devnode was removed, so the board is currently bound to nothing. ",
                    "To finish by hand from an elevated prompt:\n",
                    "\n  pnputil /enum-drivers    # find the ksx oemNN.inf",
                    "\n  pnputil /delete-driver oemNN.inf /uninstall /force",
                    "\n  pnputil /scan-devices    # the keyboard driver binds again\n",
                    "\nIf you rescan WITHOUT deleting the INF, {} goes straight back to ",
                    "WinUSB. docs/RECOVERY.md §2.",
                ),
                instance_id
            )),
            ApplyError::Failed { .. } => Some(
                "A release that failed partway may have removed the devnode already. \
                 `pnputil /scan-devices` from an elevated prompt re-enumerates; if the board \
                 comes back on winusb.sys rather than the keyboard driver, the ksx INF is \
                 still in the store — see docs/RECOVERY.md §2."
                    .to_owned(),
            ),
            _ => None,
        }
    }
}

/// Run one planned command, capturing its output.
///
/// `no_window`: `pnputil`'s output is captured, decoded and re-printed by ksx
/// (that is what [`ApplyError::hint`] reads), so a console window of its own
/// would show the user nothing they are not already being shown — and
/// `ksx winusb claim` is reachable from a daemon that has no console.
pub fn run_command(cmd: &PlannedCommand) -> Result<String, ApplyError> {
    let output =
        crate::process::no_window(std::process::Command::new(&cmd.program).args(&cmd.args))
            .output()
            .map_err(|source| ApplyError::Spawn {
                command: cmd.command_line(),
                source,
            })?;
    let mut text = crate::autostart::decode_console_output(&output.stdout);
    let err = crate::autostart::decode_console_output(&output.stderr);
    if !err.trim().is_empty() {
        text.push('\n');
        text.push_str(&err);
    }
    if output.status.success() {
        Ok(text)
    } else {
        Err(ApplyError::Failed {
            command: cmd.command_line(),
            code: output.status.code().unwrap_or(-1),
            output: text,
        })
    }
}

/// Write the INF, then run the claim commands in order. Stops at the first
/// failure — a half-applied claim is easier to reason about than one that
/// pushed on past an error.
///
/// The caller is responsible for having checked `--yes` and elevation. This
/// function is the only place in ksx that changes a driver binding, and it does
/// it by shelling out to `pnputil`, never through `SetupDi*`/`DiInstall*`: the
/// operation stays something a user can watch, repeat by hand, and undo with
/// the same tool.
pub fn apply_claim(plan: &ClaimPlan) -> Result<Vec<String>, ApplyError> {
    if let Some(dir) = plan.inf_path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| ApplyError::WriteInf {
            path: dir.display().to_string(),
            source,
        })?;
    }
    std::fs::write(&plan.inf_path, &plan.inf_text).map_err(|source| ApplyError::WriteInf {
        path: plan.inf_path.display().to_string(),
        source,
    })?;
    plan.commands.iter().map(run_command).collect()
}

/// What `/enum-drivers` was able to say about the ksx INF.
///
/// The distinction is the whole point. "Not in the store" and "I could not find
/// out" both used to arrive as `None`, and both then skipped the delete and
/// rescanned — but only one of them is safe to rescan after.
enum OemLookup {
    /// Published as this `oemNN.inf`; delete it.
    Published(String),
    /// `/enum-drivers` answered and the ksx INF is not there. Nothing to
    /// delete, and a rescan will bind the keyboard driver. This is the good
    /// case, including a second `release` of a board already released.
    NotInStore,
    /// `/enum-drivers` could not be run or could not be read. We do not know
    /// whether the INF is still in the store, so we must not rescan.
    Unknown(String),
}

/// Run the release commands, resolving `<oemNN.inf>` from the live driver store
/// on the way past.
///
/// # Why this refuses to finish rather than pushing on
///
/// The sequence is: remove the devnode, delete the ksx INF, rescan. The middle
/// step is marked REQUIRED in [`release_commands`] for a concrete reason — the
/// generated INF matches on hardware id and outranks the in-box `input.inf`, so
/// a rescan while it is still in the driver store binds WinUSB straight back
/// on.
///
/// This function used to swallow a failed `/enum-drivers` with `.ok()?`, treat
/// it as "no INF found", skip the delete with a log line, run the rescan
/// anyway, and return `Ok`. The rescan re-claimed the board; ksx reported the
/// release succeeded. The user is then told their keyboard is back while it is
/// still off the input stack — the single worst thing this module can say,
/// because the whole point of `release` is being able to trust it.
pub fn apply_release(plan: &ReleasePlan) -> Result<Vec<String>, ApplyError> {
    let mut log = Vec::new();
    let oem = match plan.inf_file.as_deref() {
        None => OemLookup::NotInStore,
        Some(inf) => match run_command(&PlannedCommand::pnputil(&["/enum-drivers"], "")) {
            Err(err) => OemLookup::Unknown(format!("pnputil /enum-drivers failed: {err}")),
            Ok(listing) => {
                let drivers = parse_enum_drivers(&listing);
                match store_drivers_matching(&drivers, inf).first() {
                    Some(found) => OemLookup::Published(found.published_name.clone()),
                    None if drivers.is_empty() => OemLookup::Unknown(
                        "pnputil /enum-drivers listed no drivers at all, which is not a state \
                         Windows reports for a working machine — the output could not be read"
                            .to_owned(),
                    ),
                    None => OemLookup::NotInStore,
                }
            }
        },
    };

    for cmd in &plan.commands {
        if cmd.args.first().map(String::as_str) == Some("/enum-drivers") {
            continue; // already done above
        }
        if cmd.args.iter().any(|a| a == "<oemNN.inf>") {
            match &oem {
                OemLookup::Published(oem) => {
                    let resolved = PlannedCommand::pnputil(
                        &["/delete-driver", oem, "/uninstall", "/force"],
                        "remove the ksx INF so a rescan cannot re-bind WinUSB",
                    );
                    log.push(run_command(&resolved)?);
                }
                OemLookup::NotInStore => log.push(
                    "nothing to delete: the ksx INF is not in the driver store, so a rescan \
                     binds the keyboard driver"
                        .to_owned(),
                ),
                // Stop here, with the devnode already removed. Continuing would
                // run the rescan, and a rescan is exactly what re-claims.
                OemLookup::Unknown(reason) => {
                    return Err(ApplyError::ReleaseUnconfirmed {
                        reason: reason.clone(),
                        instance_id: plan.instance_id.clone(),
                    })
                }
            }
            continue;
        }
        log.push(run_command(cmd)?);
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // A synthetic copy of THIS cabinet's device tree (verified read-only
    // against the live registry, 2026-08-04). Every refusal test runs
    // against it, so the tests describe the machine the milestone ships on.
    // -----------------------------------------------------------------

    fn node(
        id: &str,
        class: &str,
        service: &str,
        desc: &str,
        parent_prefix: Option<&str>,
    ) -> DeviceNode {
        DeviceNode::new(
            id,
            Some(class.to_owned()),
            (!service.is_empty()).then(|| service.to_owned()),
            Some(desc.to_owned()),
            parent_prefix.map(str::to_owned),
        )
    }

    const HID_CLASS: &str = "{745a17a0-74d3-11d0-b6fe-00a0c90f57da}";
    const MOUSE_CLASS: &str = "{4d36e96f-e325-11ce-bfc1-08002be10318}";
    const COMPOSITE_CLASS: &str = "{36fc9e60-c465-11cf-8056-444553540000}";

    /// The I-PAC4 plus the Ultimarc trackball plus one ordinary USB keyboard.
    fn cabinet_tree() -> Vec<DeviceNode> {
        vec![
            // The composite parent — never a rebind target. Its ParentIdPrefix
            // is what stamps the interface children below (`7&25eea38c&0` +
            // `&0000`/`&0001`/`&0002`), and it is what makes all three of them
            // one physical board (`board_of`).
            node(
                r"USB\VID_D209&PID_0430\4",
                COMPOSITE_CLASS,
                "usbccgp",
                "@usb.inf,%usb\\composite.devicedesc%;USB Composite Device",
                Some("7&25eea38c&0"),
            ),
            // MI_00 — the keyboard interface, and its HID keyboard child.
            node(
                r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000",
                HID_CLASS,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("8&2a0d0500&0"),
            ),
            node(
                r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000",
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
                None,
            ),
            // MI_01 — system/consumer/mouse. Claiming this would kill the
            // trackball for no gain.
            node(
                r"USB\VID_D209&PID_0430&MI_01\7&25eea38c&0&0001",
                HID_CLASS,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("8&124978bb&0"),
            ),
            node(
                r"HID\VID_D209&PID_0430&MI_01&Col03\8&124978bb&0&0002",
                MOUSE_CLASS,
                "mouhid",
                "@msmouse.inf,%hid.mousedevice%;HID-compliant mouse",
                None,
            ),
            // MI_02 — two vendor collections, no keyboard.
            node(
                r"USB\VID_D209&PID_0430&MI_02\7&25eea38c&0&0002",
                HID_CLASS,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("8&3620b67d&0"),
            ),
            // The Ultimarc trackball: a mouse, not a keyboard.
            node(
                r"USB\VID_D209&PID_15A2\6",
                HID_CLASS,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("7&14cbd05c&0"),
            ),
            node(
                r"HID\VID_D209&PID_15A2\7&14cbd05c&0&0000",
                MOUSE_CLASS,
                "mouhid",
                "@msmouse.inf,%hid.mousedevice%;HID-compliant mouse",
                None,
            ),
            // The firmware-upgrade device on a vendor stack — out of scope.
            node(
                r"USB\VID_D209&PID_0750\6&3faf36a&0&4",
                COMPOSITE_CLASS,
                "CyUsb",
                "@oem193.inf,%vid_d209&pid_0750.devicedesc%;U-HID Firmware upgrade",
                None,
            ),
            // A second, ordinary USB keyboard on another port — the lifeline.
            node(
                r"USB\VID_3434&PID_0630&MI_00\7&1a8050ae&0&0000",
                HID_CLASS,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("8&13507575&0"),
            ),
            node(
                r"HID\VID_3434&PID_0630&MI_00\8&13507575&0&0000",
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
                None,
            ),
        ]
    }

    fn cabinet() -> Survey {
        Survey::from_nodes(&cabinet_tree())
    }

    fn inf_dir() -> PathBuf {
        PathBuf::from(r"C:\ProgramData\ksx\winusb")
    }

    // -----------------------------------------------------------------
    // Parsing
    // -----------------------------------------------------------------

    #[test]
    fn instance_paths_split_into_enumerator_key_and_instance() {
        let n = &cabinet_tree()[1];
        assert_eq!(n.enumerator, "USB");
        assert_eq!(n.device_key, "VID_D209&PID_0430&MI_00");
        assert_eq!(n.instance, "7&25eea38c&0&0000");
        assert_eq!(n.vid_pid(), Some((0xD209, 0x0430)));
        assert_eq!(n.interface_number(), Some(0));
        assert_eq!(
            n.usb_hardware_id().as_deref(),
            Some(r"USB\VID_D209&PID_0430&MI_00")
        );
        assert_eq!(n.description(), "USB Input Device");
    }

    /// The registry gives no parent pointer; `ParentIdPrefix` is the link, and
    /// getting it wrong would attribute the wrong keyboard to an interface —
    /// which is how you claim MI_01 thinking it is MI_00.
    #[test]
    fn parent_id_prefix_links_an_interface_to_its_own_hid_child_only() {
        let tree = cabinet_tree();
        let mi00 = &tree[1];
        let kb = &tree[2];
        let mi01 = &tree[3];
        let mouse = &tree[4];
        assert!(mi00.is_parent_of(kb));
        assert!(!mi00.is_parent_of(mouse));
        assert!(mi01.is_parent_of(mouse));
        assert!(!mi01.is_parent_of(kb));
    }

    // -----------------------------------------------------------------
    // Survey
    // -----------------------------------------------------------------

    #[test]
    fn the_cabinet_survey_finds_exactly_one_claimable_interface_per_keyboard() {
        let survey = cabinet();
        assert_eq!(survey.keyboard_count(), 2, "I-PAC + the spare keyboard");
        let claimable: Vec<&str> = survey
            .candidates
            .iter()
            .filter(|c| c.state == ClaimState::Claimable)
            .map(|c| c.interface.instance_id.as_str())
            .collect();
        assert_eq!(
            claimable,
            vec![
                r"USB\VID_3434&PID_0630&MI_00\7&1a8050ae&0&0000",
                r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000",
            ]
        );
    }

    /// MI_01 and MI_02 must never be claimable: MI_01 carries the trackball's
    /// mouse collection and MI_02 the vendor ones, and neither produces keys.
    #[test]
    fn the_ipacs_non_keyboard_interfaces_are_not_claimable() {
        let survey = cabinet();
        for id in ["MI_01", "MI_02"] {
            let candidate = survey.resolve(id).expect(id);
            assert_eq!(candidate.state, ClaimState::NotAKeyboard, "{id}");
        }
        // ...and the trackball, which is a whole device rather than an interface.
        let ball = survey.resolve("PID_15A2").unwrap();
        assert_eq!(ball.state, ClaimState::NotAKeyboard);
    }

    #[test]
    fn the_composite_parent_and_vendor_stacks_are_not_candidates() {
        let survey = cabinet();
        assert!(survey
            .candidates
            .iter()
            .all(|c| !c.interface.service_is("usbccgp")));
        assert!(survey
            .candidates
            .iter()
            .all(|c| !c.interface.service_is("CyUsb")));
    }

    #[test]
    fn an_already_claimed_interface_reports_claimed_and_loses_its_keyboard_child() {
        let mut tree = cabinet_tree();
        tree[1].service = Some("WinUSB".into());
        tree.remove(2); // WinUSB removes the HID stack, so the keyboard node goes
        let survey = Survey::from_nodes(&tree);
        let c = survey.resolve("D209&PID_0430&MI_00").unwrap();
        assert_eq!(c.state, ClaimState::Claimed);
        assert!(c.keyboard.is_none());
        // Identity survives: the interface path is what ksx keys on now.
        assert_eq!(
            c.ksx_device_id(),
            r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"
        );
        assert_eq!(survey.keyboard_count(), 1, "only the spare is left");
    }

    #[test]
    fn resolution_accepts_a_substring_the_hid_path_or_the_full_path() {
        let survey = cabinet();
        let want = r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000";
        for needle in [
            want,
            r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000",
            "PID_0430&MI_00",
            "25eea38c&0&0000",
        ] {
            assert_eq!(
                survey.resolve(needle).unwrap().interface.instance_id,
                want,
                "{needle}"
            );
        }
    }

    /// Two identical I-PACs is the T4 case. Guessing which one the user meant
    /// is the one thing that must not happen.
    #[test]
    fn two_identical_boards_make_a_bare_vid_pid_ambiguous_not_a_guess() {
        let mut tree = cabinet_tree();
        tree.push(node(
            r"USB\VID_D209&PID_0430&MI_00\7&99999999&0&0000",
            HID_CLASS,
            "HidUsb",
            "@input.inf,%hid.devicedesc%;USB Input Device",
            Some("8&deadbeef&0"),
        ));
        tree.push(node(
            r"HID\VID_D209&PID_0430&MI_00\8&deadbeef&0&0000",
            KEYBOARD_CLASS_GUID,
            "kbdhid",
            "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
            None,
        ));
        let survey = Survey::from_nodes(&tree);
        let err = survey.resolve("PID_0430&MI_00").unwrap_err();
        assert_eq!(err.code(), "ambiguous-device");
        assert!(err.advice().contains("7&25eea38c"), "{}", err.advice());
        // The full instance path still resolves — that is the whole point of
        // keying identity on it.
        assert!(survey
            .resolve(r"USB\VID_D209&PID_0430&MI_00\7&99999999&0&0000")
            .is_ok());
    }

    #[test]
    fn an_unknown_device_lists_what_there_is() {
        let survey = cabinet();
        let err = survey.resolve("VID_DEAD").unwrap_err();
        assert_eq!(err.code(), "unknown-device");
        assert!(err.advice().contains("ksx winusb status"));
        assert!(err.advice().contains("PID_0430&MI_00"), "{}", err.advice());
    }

    // -----------------------------------------------------------------
    // The refusal that prevents a bricked panel
    // -----------------------------------------------------------------

    /// **The bricking case.** One keyboard on the machine, and it is the panel.
    /// Claim it and there is no way to type the release command, no way through
    /// a UAC prompt, and no way onto the lock screen.
    #[test]
    fn claiming_the_only_keyboard_is_refused() {
        // Strip the spare keyboard: the I-PAC is now the only one.
        let tree: Vec<DeviceNode> = cabinet_tree()
            .into_iter()
            .filter(|n| !n.device_key.contains("VID_3434"))
            .collect();
        let survey = Survey::from_nodes(&tree);
        assert_eq!(survey.keyboard_count(), 1);

        let err = plan_claim(&survey, "PID_0430&MI_00", &inf_dir()).unwrap_err();
        assert_eq!(err.code(), "last-keyboard");
        assert!(err.to_string().contains("ONLY keyboard"), "{err}");
        assert!(
            err.advice().contains("second keyboard"),
            "the refusal must say how to proceed: {}",
            err.advice()
        );
        assert!(
            err.advice().contains("lock screen") || err.advice().contains("UAC"),
            "the refusal must say why re-injection is not a substitute: {}",
            err.advice()
        );
    }

    /// A machine with no keyboards at all (already claimed, or a headless box)
    /// must refuse too — `after == 0` is the condition, not `before == 1`.
    #[test]
    fn claiming_when_no_keyboard_would_remain_is_refused_however_it_got_there() {
        let mut tree = cabinet_tree();
        // The spare is already WinUSB-claimed, so its keyboard node is gone.
        tree.retain(|n| n.instance_id != r"HID\VID_3434&PID_0630&MI_00\8&13507575&0&0000");
        for n in &mut tree {
            if n.instance_id == r"USB\VID_3434&PID_0630&MI_00\7&1a8050ae&0&0000" {
                n.service = Some("WinUSB".into());
            }
        }
        let survey = Survey::from_nodes(&tree);
        assert_eq!(
            plan_claim(&survey, "PID_0430&MI_00", &inf_dir())
                .unwrap_err()
                .code(),
            "last-keyboard"
        );
    }

    // -----------------------------------------------------------------
    // ...and the ways the count itself could be talked into lying (F2).
    //
    // Every test below describes a machine where the naive count — rows in
    // the keyboard class — says "you have a spare" about a keyboard that
    // cannot type the release command. The claim must be refused anyway.
    // -----------------------------------------------------------------

    fn with_status(mut node: DeviceNode, started: bool, problem: u32) -> DeviceNode {
        node.status = Some(NodeStatus { started, problem });
        node
    }

    fn live(node: DeviceNode) -> DeviceNode {
        with_status(node, true, 0)
    }

    /// The cabinet with every node's PnP status filled in as "working", which
    /// is what the live collector reports for a healthy tree.
    fn healthy_cabinet_tree() -> Vec<DeviceNode> {
        cabinet_tree().into_iter().map(live).collect()
    }

    /// A keyboard node hanging off `parent_prefix`, i.e. off some interface.
    fn keyboard_node(id: &str) -> DeviceNode {
        live(node(
            id,
            KEYBOARD_CLASS_GUID,
            "kbdhid",
            "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
            None,
        ))
    }

    /// **One board is one keyboard.** A composite board whose *second*
    /// interface also enumerates as keyboard-class (every gaming keyboard with
    /// a consumer-control collection, and an I-PAC with its vendor interfaces)
    /// must not read as two keyboards.
    ///
    /// Counting nodes, the machine below has two: claim the panel's `MI_00` and
    /// "one keyboard is left" — the panel's own vendor collection, which types
    /// nothing a user can log in with. That is the guard defeating itself.
    #[test]
    fn two_keyboard_collections_of_one_board_are_one_keyboard() {
        let mut tree: Vec<DeviceNode> = healthy_cabinet_tree()
            .into_iter()
            .filter(|n| !n.device_key.contains("VID_3434")) // no spare
            .collect();
        // A second keyboard-class node, on the same board, under MI_02.
        tree.push(keyboard_node(
            r"HID\VID_D209&PID_0430&MI_02&Col02\8&3620b67d&0&0001",
        ));
        let survey = Survey::from_nodes(&tree);

        assert_eq!(
            survey.keyboards.len(),
            2,
            "the machine really does have two keyboard-class nodes"
        );
        assert_eq!(
            survey.keyboard_count(),
            1,
            "...but one board, so one keyboard — this is the count the refusal uses"
        );
        let boards: Vec<&str> = survey.keyboards.iter().map(|k| k.board.as_str()).collect();
        assert_eq!(
            boards[0], boards[1],
            "both collections must resolve to the same physical board: {boards:?}"
        );

        let err = plan_claim(&survey, "PID_0430&MI_00", &inf_dir()).unwrap_err();
        assert_eq!(
            err.code(),
            "last-keyboard",
            "claiming a board must not be excused by another collection of that same board"
        );
    }

    /// Two identical I-PACs on different ports are still two keyboards. The
    /// dedupe above must group by *board*, not by model — collapsing these
    /// would refuse a claim the user is entitled to make (and is the same
    /// identity question T4 is about).
    #[test]
    fn two_boards_of_the_same_model_are_two_keyboards() {
        let mut tree = healthy_cabinet_tree();
        tree.push(live(node(
            r"USB\VID_D209&PID_0430\9",
            COMPOSITE_CLASS,
            "usbccgp",
            "@usb.inf,%usb\\composite.devicedesc%;USB Composite Device",
            Some("7&99999999&0"),
        )));
        tree.push(live(node(
            r"USB\VID_D209&PID_0430&MI_00\7&99999999&0&0000",
            HID_CLASS,
            "HidUsb",
            "@input.inf,%hid.devicedesc%;USB Input Device",
            Some("8&deadbeef&0"),
        )));
        tree.push(keyboard_node(
            r"HID\VID_D209&PID_0430&MI_00\8&deadbeef&0&0000",
        ));
        let survey = Survey::from_nodes(&tree);
        assert_eq!(
            survey.keyboard_count(),
            3,
            "two I-PACs and the spare are three keyboards, not one model"
        );
    }

    // ── Bluetooth: in the survey, and permanently unclaimable ─────────────

    /// A paired Bluetooth keyboard, in the shape this machine spells them.
    fn bluetooth_keyboard_tree() -> Vec<DeviceNode> {
        let mut tree = healthy_cabinet_tree();
        tree.push(live(node(
            r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&3562C725&0&001BDC0F1FE7_C00000000",
            KEYBOARD_CLASS_GUID,
            "kbdhid",
            "@keyboard.inf,%hid.keyboarddevice%;Bluetooth Keyboard",
            None,
        )));
        tree
    }

    /// **A Bluetooth keyboard has to BE in the survey.**
    ///
    /// `resolve` is what `ksx device pick` calls and what every refusal is
    /// worded against. A keyboard missing from this list produces "no device
    /// matches that" for a keyboard sitting on the desk — and `plan_claim`
    /// cannot refuse with the transport reason a device it has never heard of.
    ///
    /// FAILS against the shipped `Survey::from_nodes`, which iterated
    /// `enumerator == "USB"` and nothing else.
    #[test]
    fn a_bluetooth_keyboard_is_a_candidate_the_survey_can_resolve() {
        let survey = Survey::from_nodes(&bluetooth_keyboard_tree());
        let bt = survey
            .resolve("001BDC0F1FE7")
            .expect("the Bluetooth keyboard must be findable by name");
        assert_eq!(bt.transport, Transport::Bluetooth);
        assert_eq!(bt.state, ClaimState::InterceptionOnly);
        assert!(
            bt.keyboard.is_some(),
            "it IS a keyboard — that is why Interception can capture it"
        );
    }

    /// **The rule this list exists to teach, at the refusal.**
    ///
    /// A claim on a Bluetooth keyboard is refused for the TRANSPORT, and the
    /// refusal says so and points at the backend that works today. It must not
    /// read as "not a keyboard" (which sends someone hunting for a different
    /// interface of a device that has one) and it must not read as "not
    /// supported yet" (which invites waiting for a release that cannot come).
    ///
    /// FAILS against routing Bluetooth through `Refusal::NotAKeyboard`, which
    /// is the obvious first implementation and is wrong in both directions.
    #[test]
    fn claiming_a_bluetooth_keyboard_is_refused_for_the_transport_not_for_being_a_keyboard() {
        let survey = Survey::from_nodes(&bluetooth_keyboard_tree());
        let err = plan_claim(&survey, "001BDC0F1FE7", &inf_dir())
            .expect_err("no INF can bind a Bluetooth device");
        assert_eq!(err.code(), "transport-cannot-claim");
        assert_ne!(err.code(), "not-a-keyboard");

        let message = err.to_string();
        assert!(message.contains("Bluetooth"), "{message}");
        assert!(message.contains("never"), "{message}");

        let advice = err.advice();
        assert!(
            advice.contains("no USB interface to bind"),
            "the transport fact, not a vague refusal: {advice}"
        );
        assert!(
            advice.contains("Interception captures it today"),
            "and the backend that DOES work, right now: {advice}"
        );
        assert!(
            advice.contains("ksx device pick"),
            "with the command that uses it: {advice}"
        );
    }

    /// A Bluetooth keyboard counts as a keyboard for the last-keyboard
    /// refusal — it is a real spare, and refusing a legitimate claim because
    /// ksx would not count it is its own failure.
    #[test]
    fn a_bluetooth_keyboard_counts_as_a_spare_for_the_last_keyboard_refusal() {
        // The panel alone — the lifeline keyboard unplugged.
        let mut lonely = healthy_cabinet_tree();
        lonely.retain(|n| !n.instance_id.contains("VID_3434"));
        let boards = Survey::from_nodes(&lonely).keyboard_count();
        assert_eq!(boards, 1, "fixture precondition: only the panel is left");

        let mut with_bt = lonely.clone();
        with_bt.push(live(node(
            r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&3562C725&0&001BDC0F1FE7_C00000000",
            KEYBOARD_CLASS_GUID,
            "kbdhid",
            "@keyboard.inf,%hid.keyboarddevice%;Bluetooth Keyboard",
            None,
        )));
        assert_eq!(
            Survey::from_nodes(&with_bt).keyboard_count(),
            boards + 1,
            "a connected Bluetooth keyboard is a keyboard you can type on"
        );
    }

    /// **The trap, at the arithmetic that matters.** A paired-but-disconnected
    /// Bluetooth keyboard is PRESENT and must NOT be counted as the spare that
    /// licenses claiming the panel — someone reads "2 keyboards", claims their
    /// panel, and is locked out by a keyboard in a drawer with dead batteries.
    ///
    /// FAILS against counting rows in the keyboard class, which is what a
    /// registry-only survey does.
    #[test]
    fn a_disconnected_bluetooth_keyboard_never_licenses_claiming_the_panel() {
        let mut tree = healthy_cabinet_tree();
        tree.retain(|n| !n.instance_id.contains("VID_3434"));
        tree.push(
            node(
                r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&3562C725&0&001BDC0F1FE7_C00000000",
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;Bluetooth Keyboard",
                None,
            )
            .with_status(NodeStatus {
                started: false,
                problem: CM_PROB_DEVICE_NOT_CONNECTED,
            }),
        );
        let survey = Survey::from_nodes(&tree);

        assert!(
            survey
                .keyboards
                .iter()
                .any(|kb| kb.node.enumerator.eq_ignore_ascii_case("BTHENUM")),
            "it stays LISTED — hiding it would be its own lie"
        );
        assert_eq!(
            survey.keyboard_count(),
            1,
            "…and it is not counted: it cannot type the command that undoes a claim"
        );
        let err = plan_claim(&survey, "PID_0430&MI_00", &inf_dir())
            .expect_err("the panel is the only keyboard that can type");
        assert_eq!(err.code(), "last-keyboard");
    }

    /// One physical Bluetooth device wears several service nodes; it is ONE
    /// candidate. Two rows for one keyboard would put the same device twice in
    /// a picker and twice in the keyboard arithmetic.
    #[test]
    fn several_service_nodes_of_one_bluetooth_keyboard_are_one_candidate() {
        let mut tree = healthy_cabinet_tree();
        for suffix in ["_C00000000", "_C00000001"] {
            tree.push(live(node(
                &format!(
                    r"BTHENUM\{{00001124-0000-1000-8000-00805F9B34FB}}_VID&0002045E_PID&0800\7&3562C725&0&001BDC0F1FE7{suffix}"
                ),
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;Bluetooth Keyboard",
                None,
            )));
        }
        let survey = Survey::from_nodes(&tree);
        assert_eq!(
            survey
                .candidates
                .iter()
                .filter(|c| c.transport == Transport::Bluetooth)
                .count(),
            1,
            "one keyboard, one row"
        );
    }

    /// A Bluetooth device that is not a keyboard — a speaker, a controller —
    /// is not a claim candidate at all. The DEVICE LIST still shows it
    /// (`ksx_capture::bluetooth`); this survey answers a narrower question.
    #[test]
    fn a_bluetooth_device_with_no_keyboard_is_not_a_claim_candidate() {
        let mut tree = healthy_cabinet_tree();
        tree.push(live(node(
            r"BTHENUM\DEV_24C4069CF1F4\7&3562C725&0&BLUETOOTHDEVICE_24C4069CF1F4",
            "{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}",
            "BthEnum",
            "@bth.inf,%token%;ULT TOWER 9",
            None,
        )));
        let survey = Survey::from_nodes(&tree);
        assert!(survey
            .candidates
            .iter()
            .all(|c| c.transport == Transport::Usb));
    }

    /// Both measured spellings of a Bluetooth address, and the all-zero
    /// address that is NOT one.
    ///
    /// FAILS against `len() == 12 && all hex`: the local radio's own service
    /// nodes all spell `…&0&000000000000_0000000n`, and accepting that files
    /// three unrelated pseudo-devices under one identity.
    #[test]
    fn a_bluetooth_address_is_read_from_either_spelling_and_never_zero() {
        let of = |id: &str| bd_addr(&DeviceNode::new(id, None, None, None, None));
        assert_eq!(
            of(r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&02E0\7&3562C725&0&EC8350B88A10_C00000000")
                .as_deref(),
            Some("EC8350B88A10")
        );
        assert_eq!(
            of(r"BTHENUM\DEV_EC8350B88A10\7&3562C725&0&BLUETOOTHDEVICE_EC8350B88A10").as_deref(),
            Some("EC8350B88A10"),
            "the DEV_ node and the service node name one device"
        );
        assert_eq!(
            of(r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&02E0\2&21119A68&0&F2424BE71501")
                .as_deref(),
            Some("F2424BE71501")
        );
        assert_eq!(
            of(
                r"BTHENUM\{E836F634-9100-4CA8-95EE-F7B6E9115185}_LOCALMFG&0000\7&3562C725&0&000000000000_00000008"
            ),
            None,
            "the local radio's zero address is not a device"
        );
        assert_eq!(of(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000"), None);
    }

    /// The release order is load-bearing, and the plan says so.
    ///
    /// `/delete-driver` sits between removing the devnode and rescanning
    /// because the ksx INF outranks the in-box `input.inf` on the same hardware
    /// id: rescan with it still in the store and WinUSB binds straight back on.
    /// This pins the order so a future edit cannot quietly reorder the step
    /// that makes the release stick.
    #[test]
    fn the_ksx_inf_is_deleted_before_the_rescan_not_after() {
        let commands = release_commands(
            r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000",
            Some("ksx-usb-vid_d209-pid_0430-mi_00.inf"),
        );
        let verbs: Vec<&str> = commands
            .iter()
            .filter_map(|c| c.args.first().map(String::as_str))
            .collect();
        assert_eq!(
            verbs,
            vec![
                "/remove-device",
                "/enum-drivers",
                "/delete-driver",
                "/scan-devices"
            ],
            "a rescan before the INF is deleted re-claims the board"
        );
    }

    /// The recovery text is part of the contract, not decoration: this error
    /// only happens with the devnode already removed, so the user is holding a
    /// board bound to nothing and needs the three commands that finish the job.
    #[test]
    fn an_unconfirmed_release_says_what_state_the_machine_is_in() {
        let err = ApplyError::ReleaseUnconfirmed {
            reason: "pnputil /enum-drivers failed: access denied".to_owned(),
            instance_id: r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000".to_owned(),
        };
        let message = err.to_string();
        assert!(
            message.contains("stopping before the rescan"),
            "say that it stopped, and where: {message}"
        );
        let recovery = err.recovery().expect("this error must carry a way out");
        assert!(recovery.contains("/delete-driver"), "{recovery}");
        assert!(recovery.contains("/scan-devices"), "{recovery}");
        assert!(
            recovery.contains("goes straight back to WinUSB"),
            "the user must be told why the order matters: {recovery}"
        );
    }

    /// Add a second I-PAC and a claim must refuse, because the INF it would
    /// generate cannot tell the two apart.
    ///
    /// This is the shape that made the bug invisible: the last-keyboard
    /// refusal is correct *about boards* — subtract this board and one is
    /// left — while the artifact it approves is scoped to the hardware id,
    /// which both boards carry. The safety check and the thing it authorises
    /// were talking about different objects, so the answer was "you still have
    /// a keyboard" about a command that would take every keyboard on the
    /// machine.
    #[test]
    fn a_second_identical_board_refuses_the_claim_rather_than_taking_both() {
        let mut tree = healthy_cabinet_tree();
        // A second I-PAC 4X, same model, different port.
        tree.push(live(node(
            r"USB\VID_D209&PID_0430\9",
            COMPOSITE_CLASS,
            "usbccgp",
            "@usb.inf,%usb\\composite.devicedesc%;USB Composite Device",
            Some("7&99999999&0"),
        )));
        tree.push(live(node(
            r"USB\VID_D209&PID_0430&MI_00\7&99999999&0&0000",
            HID_CLASS,
            "HidUsb",
            "@input.inf,%hid.devicedesc%;USB Input Device",
            Some("8&deadbeef&0"),
        )));
        tree.push(keyboard_node(
            r"HID\VID_D209&PID_0430&MI_00\8&deadbeef&0&0000",
        ));

        let survey = Survey::from_nodes(&tree);
        let err = plan_claim(
            &survey,
            r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000",
            Path::new(r"C:\tmp"),
        )
        .expect_err("a claim that would take both boards must refuse");

        assert_eq!(err.code(), "shared-hardware-id");
        let Refusal::SharedHardwareId {
            hardware_id,
            siblings,
            ..
        } = &err
        else {
            panic!("expected SharedHardwareId, got {err:?}");
        };
        assert_eq!(hardware_id, r"USB\VID_D209&PID_0430&MI_00");
        assert_eq!(
            siblings,
            &[r"USB\VID_D209&PID_0430&MI_00\7&99999999&0&0000".to_owned()],
            "the refusal must name the board that would also be taken"
        );
        assert!(
            err.advice().contains("claim this board on its own"),
            "a refusal with no way forward is just an error message: {}",
            err.advice()
        );
    }

    /// The single-board case — the 99% one — must keep working exactly as it
    /// did. The guard above is about *twins*, and a board's own MI_01/MI_02
    /// carry different hardware ids, so they must not trip it.
    #[test]
    fn one_board_of_its_model_still_claims_cleanly() {
        let survey = Survey::from_nodes(&healthy_cabinet_tree());
        let plan = plan_claim(
            &survey,
            r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000",
            Path::new(r"C:\tmp"),
        )
        .expect("one I-PAC plus a spare keyboard is a legal claim");
        assert_eq!(plan.hardware_id, r"USB\VID_D209&PID_0430&MI_00");
    }

    /// A keyboard whose interface is **already bound to `winusb.sys`** is not a
    /// keyboard. Windows leaves the HID child in the tree until it
    /// re-enumerates, and counting it means "you have a spare" about a board
    /// that has already left the keyboard stack — ksx's own claim, or anyone
    /// else's.
    #[test]
    fn a_keyboard_already_claimed_through_winusb_does_not_count() {
        let mut tree = healthy_cabinet_tree();
        for n in &mut tree {
            if n.instance_id == r"USB\VID_3434&PID_0630&MI_00\7&1a8050ae&0&0000" {
                n.service = Some("WinUSB".into());
            }
        }
        let survey = Survey::from_nodes(&tree);
        assert_eq!(
            survey.keyboards.len(),
            2,
            "the stale HID child is still in the tree"
        );
        assert_eq!(survey.keyboard_count(), 1, "...and it cannot type");
        let spare = survey
            .keyboards
            .iter()
            .find(|k| k.node.device_key.contains("VID_3434"))
            .expect("the spare's node");
        assert!(spare.unusable.unwrap().contains("winusb.sys"), "{spare:?}");

        assert_eq!(
            plan_claim(&survey, "D209&PID_0430&MI_00", &inf_dir())
                .unwrap_err()
                .code(),
            "last-keyboard"
        );
    }

    /// A keyboard that is **present but not connected** does not count either.
    /// A paired Bluetooth keyboard sitting in a drawer with no batteries is
    /// present in the device tree all day (`CM_PROB_DEVICE_NOT_CONNECTED`), and
    /// it will not type the release command.
    #[test]
    fn a_paired_but_disconnected_bluetooth_keyboard_does_not_count() {
        let bt = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&2A0B8CBA&0&001BDC0F1FE7_C00000000";
        let mut tree: Vec<DeviceNode> = healthy_cabinet_tree()
            .into_iter()
            .filter(|n| !n.device_key.contains("VID_3434"))
            .collect();
        tree.push(with_status(
            node(
                bt,
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;Bluetooth Keyboard",
                None,
            ),
            false,
            CM_PROB_DEVICE_NOT_CONNECTED,
        ));
        let survey = Survey::from_nodes(&tree);
        assert_eq!(survey.keyboard_count(), 1, "the panel, and nothing else");
        assert_eq!(
            plan_claim(&survey, "PID_0430&MI_00", &inf_dir())
                .unwrap_err()
                .code(),
            "last-keyboard"
        );

        // ...and the moment it is switched on, the claim is allowed: the
        // refusal must not become superstition about Bluetooth.
        let connected: Vec<DeviceNode> = tree
            .into_iter()
            .map(|n| if n.instance_id == bt { live(n) } else { n })
            .collect();
        let survey = Survey::from_nodes(&connected);
        assert_eq!(survey.keyboard_count(), 2);
        assert!(plan_claim(&survey, "PID_0430&MI_00", &inf_dir()).is_ok());
    }

    /// Disabled, driverless, or stopped keyboards are the same story.
    #[test]
    fn a_disabled_or_driverless_keyboard_does_not_count() {
        for (started, problem, service) in [
            (false, CM_PROB_DISABLED, "kbdhid"),
            (false, 0, "kbdhid"),
            (true, 0, ""), // present, started, no function driver at all
        ] {
            let mut tree: Vec<DeviceNode> = healthy_cabinet_tree()
                .into_iter()
                .filter(|n| !n.instance_id.starts_with(r"HID\VID_3434"))
                .collect();
            tree.push(with_status(
                node(
                    r"HID\VID_3434&PID_0630&MI_00\8&13507575&0&0000",
                    KEYBOARD_CLASS_GUID,
                    service,
                    "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
                    None,
                ),
                started,
                problem,
            ));
            let survey = Survey::from_nodes(&tree);
            assert_eq!(
                survey.keyboard_count(),
                1,
                "started={started} problem={problem} service={service:?}"
            );
            assert_eq!(
                plan_claim(&survey, "PID_0430&MI_00", &inf_dir())
                    .unwrap_err()
                    .code(),
                "last-keyboard",
                "started={started} problem={problem} service={service:?}"
            );
        }
    }

    /// A node nobody asked the PnP manager about is not evidence of a fault:
    /// every fixture and every pre-`NodeStatus` caller builds nodes that way,
    /// and reading them as broken would refuse every claim on the machine.
    #[test]
    fn a_node_with_no_status_is_treated_as_working() {
        let survey = cabinet(); // no `with_status` anywhere
        assert_eq!(survey.keyboard_count(), 2);
        assert!(survey.keyboards.iter().all(KeyboardNode::is_usable));
    }

    /// With a spare plugged in, the same claim goes through and the count is
    /// reported honestly.
    #[test]
    fn claiming_with_a_spare_keyboard_present_is_allowed_and_counts_are_reported() {
        let plan = plan_claim(&cabinet(), "PID_0430&MI_00", &inf_dir()).unwrap();
        assert_eq!(plan.keyboards_before, 2);
        assert_eq!(plan.keyboards_after, 1);
        assert_eq!(plan.hardware_id, r"USB\VID_D209&PID_0430&MI_00");
        assert_eq!(
            plan.ksx_device_id, r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000",
            "the claim must name the id the config file already uses"
        );
    }

    #[test]
    fn claiming_a_non_keyboard_or_an_already_claimed_interface_is_refused() {
        let survey = cabinet();
        assert_eq!(
            plan_claim(&survey, "MI_01", &inf_dir()).unwrap_err().code(),
            "not-a-keyboard"
        );
        let mut tree = cabinet_tree();
        tree[1].service = Some("WinUSB".into());
        let claimed = Survey::from_nodes(&tree);
        assert_eq!(
            plan_claim(&claimed, "D209&PID_0430&MI_00", &inf_dir())
                .unwrap_err()
                .code(),
            "already-claimed"
        );
    }

    // -----------------------------------------------------------------
    // INF + commands
    // -----------------------------------------------------------------

    #[test]
    fn the_inf_filename_is_derived_from_the_hardware_id() {
        assert_eq!(
            inf_file_name(r"USB\VID_D209&PID_0430&MI_00"),
            "ksx-winusb-vid-d209-pid-0430-mi-00.inf"
        );
    }

    #[test]
    fn the_inf_matches_on_the_exact_interface_and_pulls_winusb_from_the_inbox_inf() {
        let plan = plan_claim(&cabinet(), "PID_0430&MI_00", &inf_dir()).unwrap();
        let inf = &plan.inf_text;
        // The model line must name the interface, not the composite parent —
        // matching USB\VID_D209&PID_0430 would claim every interface at once.
        assert!(
            inf.contains(r"%DeviceName% = USB_Install, USB\VID_D209&PID_0430&MI_00"),
            "{inf}"
        );
        assert!(!inf.contains("USB\\VID_D209&PID_0430\n"), "{inf}");
        // No driver binary of ours: winusb.sys comes from the in-box INF.
        for needle in [
            "Include = winusb.inf",
            "Needs   = WINUSB.NT",
            "Needs   = WINUSB.NT.Services",
        ] {
            assert!(inf.contains(needle), "missing '{needle}' in:\n{inf}");
        }
        assert!(inf.contains("PnpLockdown = 1"), "{inf}");
        assert!(inf.contains(USB_DEVICE_CLASS_GUID), "{inf}");
        assert!(inf.contains(KSX_DEVICE_INTERFACE_GUID), "{inf}");
        // Both architectures, so an ARM64 machine fails loudly rather than
        // installing an INF that matches nothing.
        assert!(inf.contains("[ksxDevice.NTamd64]"), "{inf}");
        assert!(inf.contains("[ksxDevice.NTarm64]"), "{inf}");
        // And it says out loud what it does.
        assert!(inf.contains("leaves the keyboard stack"), "{inf}");
    }

    /// Byte-identical across runs: an INF with a timestamp in it cannot be
    /// diffed against what is actually installed.
    #[test]
    fn the_inf_is_deterministic() {
        let a = plan_claim(&cabinet(), "PID_0430&MI_00", &inf_dir()).unwrap();
        let b = plan_claim(&cabinet(), "PID_0430&MI_00", &inf_dir()).unwrap();
        assert_eq!(a.inf_text, b.inf_text);
        assert!(a.inf_text.contains(DRIVER_VER));
    }

    #[test]
    fn the_claim_commands_are_add_driver_then_rescan_with_an_absolute_pnputil() {
        let plan = plan_claim(&cabinet(), "PID_0430&MI_00", &inf_dir()).unwrap();
        assert_eq!(plan.commands.len(), 2);
        let first = plan.commands[0].command_line();
        assert!(first.contains("pnputil.exe"), "{first}");
        assert!(
            first.to_lowercase().contains(r"system32\pnputil.exe"),
            "pnputil must be resolved absolutely, not through PATH: {first}"
        );
        assert!(first.contains("/add-driver"), "{first}");
        assert!(first.contains("/install"), "{first}");
        assert!(
            first.contains("ksx-winusb-vid-d209-pid-0430-mi-00.inf"),
            "{first}"
        );
        assert!(plan.commands[1].command_line().contains("/scan-devices"));
    }

    /// Signing is the step that stops everyone, so a dry run must lead with it
    /// rather than let a user discover it from a pnputil error code.
    #[test]
    fn the_dry_run_states_the_signing_prerequisite_and_rejects_test_signing() {
        let plan = plan_claim(&cabinet(), "PID_0430&MI_00", &inf_dir()).unwrap();
        let text = plan.render_human(true);
        assert!(text.contains("SIGNING (required"), "{text}");
        assert!(text.contains("signtool"), "{text}");
        assert!(text.contains("Zadig"), "{text}");
        assert!(
            text.contains("testsigning") && text.contains("What NOT to do"),
            "test-signing must be named and rejected: {text}"
        );
        assert!(text.contains("DRY RUN"), "{text}");
        assert!(text.contains("second keyboard"), "{text}");
        assert!(text.contains("RECOVERY.md"), "{text}");
    }

    // -----------------------------------------------------------------
    // Release
    // -----------------------------------------------------------------

    /// The rollback is only correct if the INF leaves the driver store too:
    /// ours matches on hardware id, the in-box input.inf only on compatible id,
    /// so a bare remove-device + rescan re-binds WinUSB straight back.
    #[test]
    fn release_removes_the_device_deletes_the_ksx_inf_and_rescans_in_that_order() {
        let mut tree = cabinet_tree();
        tree[1].service = Some("WinUSB".into());
        tree.remove(2);
        let survey = Survey::from_nodes(&tree);
        let plan = plan_release(&survey, "PID_0430&MI_00", false).unwrap();

        let lines: Vec<String> = plan.commands.iter().map(|c| c.command_line()).collect();
        assert!(lines[0].contains("/remove-device"), "{lines:?}");
        assert!(
            lines[0].contains(r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("/delete-driver")),
            "{lines:?}"
        );
        assert!(lines.last().unwrap().contains("/scan-devices"), "{lines:?}");
        // The delete step must be justified where a user will read it.
        let why = plan
            .commands
            .iter()
            .find(|c| c.args.iter().any(|a| a == "/delete-driver"))
            .unwrap()
            .why;
        assert!(why.contains("re-bind"), "{why}");
    }

    #[test]
    fn releasing_something_that_is_not_claimed_is_refused_unless_forced() {
        let survey = cabinet();
        let err = plan_release(&survey, "PID_0430&MI_00", false).unwrap_err();
        assert_eq!(err.code(), "not-claimed");
        assert!(plan_release(&survey, "PID_0430&MI_00", true).is_ok());
    }

    /// The runbook prints this sequence for a user whose ksx will not start.
    #[test]
    fn release_commands_are_available_without_a_survey() {
        let cmds = release_commands(
            r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000",
            Some("ksx-winusb-vid-d209-pid-0430-mi-00.inf"),
        );
        assert_eq!(cmds.len(), 4);
        let cmds = release_commands(r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000", None);
        assert_eq!(cmds.len(), 2, "no INF to delete, so no lookup either");
    }

    #[test]
    fn release_dry_run_explains_the_device_manager_route() {
        let mut tree = cabinet_tree();
        tree[1].service = Some("WinUSB".into());
        let survey = Survey::from_nodes(&tree);
        let text = plan_release(&survey, "PID_0430&MI_00", false)
            .unwrap()
            .render_human(true);
        assert!(text.contains("Device Manager"), "{text}");
        assert!(text.contains("UNCHECKED"), "{text}");
        assert!(text.contains("Scan for"), "{text}");
        assert!(text.contains("DRY RUN"), "{text}");
    }

    /// Plenty of machines already have WinUSB-bound interfaces that ksx never
    /// touched (this cabinet has two: an ASUS device and an NZXT one). `release`
    /// will happily plan against them — the survey cannot tell whose claim it
    /// is — so the plan has to say out loud that the /delete-driver step only
    /// removes ksx's own INF, or a user follows it and wonders why the rescan
    /// put WinUSB straight back.
    #[test]
    fn release_says_the_delete_step_only_removes_ksx_own_inf() {
        let nzxt = [
            node(
                r"USB\VID_1E71&PID_300E&MI_00\7&8fbf878&0&0000",
                USB_DEVICE_CLASS_GUID,
                "WINUSB",
                "@ksx.inf,%devicename%;WinUsb Device",
                None,
            ),
            node(
                r"HID\VID_3434&PID_0630&MI_00\8&13507575&0&0000",
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
                None,
            ),
        ];
        let survey = Survey::from_nodes(&nzxt);
        let text = plan_release(&survey, "VID_1E71", false)
            .unwrap()
            .render_human(true);
        assert!(text.contains("ksx's OWN INF"), "{text}");
        assert!(text.contains("Zadig"), "{text}");
        assert!(text.contains("skip that step"), "{text}");
    }

    // -----------------------------------------------------------------
    // pnputil output parsing
    // -----------------------------------------------------------------

    #[test]
    fn enum_drivers_output_parses_into_published_and_original_names() {
        let text = "\
Microsoft PnP Utility

Published Name:     oem41.inf
Original Name:      ksx-winusb-vid-d209-pid-0430-mi-00.inf
Provider Name:      ksx
Class Name:         Universal Serial Bus devices
Driver Version:     01/01/2026 1.0.0.0

Published Name:     oem12.inf
Original Name:      vigembus.inf
Provider Name:      Nefarius Software Solutions e.U.
Class Name:         System devices
Driver Version:     04/22/2020 1.17.333.0
";
        let drivers = parse_enum_drivers(text);
        assert_eq!(drivers.len(), 2);
        assert_eq!(drivers[0].published_name, "oem41.inf");
        assert_eq!(
            drivers[0].original_name,
            "ksx-winusb-vid-d209-pid-0430-mi-00.inf"
        );
        let ours = store_drivers_matching(&drivers, "ksx-winusb-vid-d209-pid-0430-mi-00.inf");
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].published_name, "oem41.inf");
        assert!(store_drivers_matching(&drivers, "nothing.inf").is_empty());
    }

    /// pnputil's labels are localised; the rollback must still work on a
    /// non-English machine, so parsing keys on value shape, not label text.
    #[test]
    fn enum_drivers_parsing_survives_localised_labels() {
        let text = "\
Veröffentlichter Name:  oem41.inf
Ursprünglicher Name:    ksx-winusb-vid-d209-pid-0430-mi-00.inf
Anbietername:           ksx
";
        let drivers = parse_enum_drivers(text);
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].published_name, "oem41.inf");
        assert_eq!(
            store_drivers_matching(&drivers, "KSX-WINUSB-VID-D209-PID-0430-MI-00.INF").len(),
            1,
            "INF names are case-insensitive on Windows"
        );
    }

    // -----------------------------------------------------------------
    // Failure hints
    // -----------------------------------------------------------------

    #[test]
    fn the_unsigned_inf_failure_is_recognised_and_explained() {
        let err = ApplyError::Failed {
            command: "pnputil".into(),
            code: 1,
            output: "Adding driver package failed: the third-party INF does not contain \
                     digital signature information."
                .into(),
        };
        let hint = err.hint().expect("recognised");
        assert!(hint.contains("trusted catalog"), "{hint}");
        assert!(hint.contains("Zadig"), "{hint}");
    }

    #[test]
    fn the_access_denied_failure_says_elevate() {
        let err = ApplyError::Failed {
            command: "pnputil".into(),
            code: 5,
            output: "Access is denied.".into(),
        };
        assert!(err.hint().unwrap().contains("elevated"));
    }

    // -----------------------------------------------------------------
    // Refusal codes are a scripting contract
    // -----------------------------------------------------------------

    #[test]
    fn every_refusal_has_a_stable_code_and_actionable_advice() {
        let refusals = [
            Refusal::UnknownDevice {
                requested: "x".into(),
                known: vec![],
            },
            Refusal::Ambiguous {
                requested: "x".into(),
                matches: vec!["a".into(), "b".into()],
            },
            Refusal::NotAKeyboard {
                instance_id: "x".into(),
            },
            Refusal::AlreadyClaimed {
                instance_id: "x".into(),
            },
            Refusal::NotClaimed {
                instance_id: "x".into(),
                driver: "HidUsb".into(),
            },
            Refusal::LastKeyboard {
                instance_id: "x".into(),
            },
            Refusal::NeedsElevation,
        ];
        let mut codes = std::collections::HashSet::new();
        for r in &refusals {
            assert!(codes.insert(r.code()), "duplicate code {}", r.code());
            assert!(!r.advice().is_empty(), "{r:?} has no advice");
            assert!(r.to_json()["refused"].as_bool().unwrap());
        }
    }

    /// **The one vendor branch on a live path, locked to being additive.**
    ///
    /// `docs/DEVICE-IDENTITY.md` §6: a vendor id may choose a display string; it
    /// **may not** gate capture, claiming, refusal or backend selection. This
    /// advice reads the VID — for a good reason, because it used to give every
    /// user I-PAC interface numbers regardless of what they owned — and that
    /// makes it the one place the rule could erode without anyone noticing.
    ///
    /// So the boundary is asserted rather than described: same refusal, same
    /// code, same generic paragraph, and the recognised board gets *one extra
    /// paragraph* on the end. A vendor id that ever decides whether to refuse,
    /// or which code to refuse with, fails here.
    ///
    /// Breaks against: moving any part of the generic advice inside the `if`,
    /// giving the Ultimarc case its own code, or refusing only for one vendor.
    #[test]
    fn the_ultimarc_hint_only_adds_a_paragraph_to_an_already_issued_refusal() {
        let logitech = Refusal::NotAKeyboard {
            instance_id: r"USB\VID_046D&PID_C31C&MI_01\7&1A2B3C4D&0&0001".into(),
        };
        let ultimarc = Refusal::NotAKeyboard {
            instance_id: r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001".into(),
        };

        assert_eq!(
            logitech.code(),
            ultimarc.code(),
            "the vendor may not change which refusal this is"
        );

        let (generic, specific) = (logitech.advice(), ultimarc.advice());
        assert!(
            !generic.is_empty(),
            "an unrecognised board still gets usable advice — the whole point of \
             the fix that produced this branch"
        );
        assert!(
            specific.starts_with(&generic),
            "the recognised board's advice must be the generic advice PLUS \
             something, never instead of it:\n--- generic ---\n{generic}\n--- \
             specific ---\n{specific}"
        );
        assert!(
            !generic.contains("MI_00") && specific.contains("MI_00"),
            "and the board-specific part must be board-specific"
        );
    }

    /// A survey is JSON-shaped for scripts, and the fields the runbook quotes
    /// must be there.
    #[test]
    fn the_status_json_carries_the_instance_paths_the_runbook_needs() {
        let json = cabinet().to_json();
        assert_eq!(json["keyboard_count"], 2);
        let candidates = json["candidates"].as_array().unwrap();
        let ipac = candidates
            .iter()
            .find(|c| {
                c["instance_id"]
                    .as_str()
                    .unwrap()
                    .contains("D209&PID_0430&MI_00")
            })
            .unwrap();
        assert_eq!(ipac["state"], "claimable");
        assert_eq!(ipac["claimable"], true);
        assert_eq!(ipac["driver"], "HidUsb");
        assert_eq!(ipac["vid"], "D209");
        assert_eq!(ipac["pid"], "0430");
        assert_eq!(ipac["interface"], 0);
        // Was `"ultimarc": bool` — a yes/no about one vendor. The board's name
        // says strictly more, and says something different for the SpinTrak.
        assert_eq!(ipac["vendor"], "Ultimarc I-PAC 4X");
        assert_eq!(
            ipac["ksx_device_id"],
            r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000"
        );
    }
}
