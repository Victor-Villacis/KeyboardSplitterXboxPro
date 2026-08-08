//! `ksx devices` — every keyboard ksx could capture, on either backend, and
//! what stands between it and being captured.
//!
//! Read-only and safe on a production keyboard stack, on both halves:
//! constructing an `InterceptionBackend` never sets a class filter (see its
//! `new` docs), and the WinUSB half only enumerates — `ksx_capture::winusb::
//! enumerate` opens nothing and claims nothing (see its module docs). Running
//! this command mid-session cannot disturb a keystroke.
//!
//! Exit codes (documented in `--help`): 0 = listed, 1 = error,
//! [`EXIT_DRIVER_MISSING`] (2) = **nothing** could be enumerated — neither the
//! Interception driver nor the USB tree. A missing Interception driver on its
//! own is no longer fatal here: after the M6 rebind the whole point is that
//! ksx runs with Interception uninstalled, and a command that refused to list
//! anything in that state would be useless exactly when it is needed most.
//!
//! The health line is the *static* slot-exhaustion check: budget usage plus
//! any keyboard sitting outside the 1..=10 slot range. Id-climb detection
//! needs observation history (two identical boards legitimately occupy two
//! slots), so the climb detector runs only inside a live capture session —
//! a single enumeration reporting climbs would false-positive on twin boards.

// Off Windows only the stub `run` is reachable outside tests; the pure report
// + JSON helpers stay compiled (and tested) but would trip dead_code.
#![cfg_attr(not(windows), allow(dead_code))]

use ksx_capture::{DeviceInfo, DeviceKind, MAX_KEYBOARD_SLOT};
use ksx_config::Backend;
use ksx_core::{DeviceFacts, DeviceId};

/// Exit code when no enumeration path worked at all (documented in `--help`).
/// Same value as `ksx pads`' missing-ViGEmBus code: 2 always means "a required
/// driver is not there".
pub const EXIT_DRIVER_MISSING: i32 = 2;

/// The vendor/board name to tag a hardware id with, if ksx knows one.
///
/// Replaces an `is_ipac()` that matched on `VID_D209` alone and therefore
/// labelled **every** Ultimarc product `[I-PAC]` — including the SpinTrak
/// trackball on the reference cabinet, which is not an I-PAC and said so in its
/// own product string. A vendor id is enough to name a *vendor*; naming a
/// *board* needs the product id too, which is why this reads both.
///
/// Returns a name, never a bool: a bool is the shape that invites a branch, and
/// `docs/DEVICE-IDENTITY.md` §6 is explicit that no capture, claim or refusal
/// path may branch on a vendor id.
pub fn vendor_tag(hwid: &str) -> Option<&'static str> {
    let upper = hwid.to_ascii_uppercase();
    let vid = hex_field(&upper, "VID_")?;
    // A hardware id without a PID still identifies a vendor.
    let pid = hex_field(&upper, "PID_").unwrap_or_default();
    ksx_core::vendors::name_for(vid, pid)
}

/// Read `<key>XXXX` as hex out of a device id, e.g. `VID_D209` -> `0xD209`.
fn hex_field(upper: &str, key: &str) -> Option<u16> {
    let at = upper.find(key)? + key.len();
    let digits: String = upper[at..].chars().take(4).collect();
    u16::from_str_radix(&digits, 16).ok()
}

/// Hardware ids reported by more than one connected **keyboard**, sorted and
/// deduplicated.
///
/// Two boards of the same model share one Interception hardware id (risk review
/// R2): the driver offers nothing else to tell them apart. Anything that binds
/// such an id to a slot is ambiguous by construction — "capture this device"
/// captures both boards, and either one drives every slot bound to the id — so
/// `ksx run` refuses to start and `ksx devices` calls it out.
///
/// The WinUSB backend has no equivalent problem: its ids are per-interface
/// device instance paths, so two identical boards on different ports are two
/// different ids (`docs/USE-CASES.md` T4, `docs/MIGRATION-WINUSB.md`).
pub fn duplicate_hardware_ids(devices: &[DeviceInfo]) -> Vec<DeviceId> {
    let mut ids: Vec<&DeviceId> = devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Keyboard)
        .map(|d| &d.id)
        .collect();
    ids.sort_unstable();
    let mut out: Vec<DeviceId> = Vec::new();
    for pair in ids.windows(2) {
        if pair[0] == pair[1] && out.last() != Some(pair[0]) {
            out.push(pair[0].clone());
        }
    }
    out
}

/// What `config.toml` says about one device, if anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfiguredDevices {
    /// `(id, alias, backend)` from every `[[device]]` entry, the id as written.
    pub entries: Vec<(ksx_core::DeviceRef, String, Backend)>,
}

impl ConfiguredDevices {
    pub fn from_config(config: &ksx_config::ConfigFile) -> Self {
        Self {
            entries: config
                .devices
                .iter()
                .map(|d| (d.id.clone(), d.alias.clone(), d.backend))
                .collect(),
        }
    }

    /// The `[[device]]` entry that names this connected interface.
    ///
    /// Byte-exact first — an Interception hardware id and a legacy full
    /// instance path both land there — then the selector, so a config holding
    /// `usb:d209:0430:00` still shows its alias and its backend against the
    /// board it names. Without that second step, switching a config to the
    /// replug-proof spelling would blank the alias and backend columns of
    /// `ksx devices` and turn the "configured for WinUSB but not rebound"
    /// warning off, which is the single most useful line the command prints.
    ///
    /// A model-rung entry with twins connected matches BOTH rows. That is the
    /// honest report — the entry really does name both, and `ksx run` refuses
    /// with the pair listed rather than picking one.
    fn find(&self, id: &DeviceId, facts: Option<&DeviceFacts>) -> Option<&Entry> {
        if let Some(hit) = self
            .entries
            .iter()
            .find(|(entry, _, _)| entry.raw() == id.as_str())
        {
            return Some(hit);
        }
        // Enumerated facts when the caller has them, because those carry the
        // descriptor's serial and a path does not — a `sn=` selector can only
        // be honoured against the real thing.
        let derived;
        let facts = match facts {
            Some(facts) => facts,
            None => {
                derived = DeviceFacts::from_instance_path(id.as_str())?;
                &derived
            }
        };
        self.entries
            .iter()
            .find(|(entry, _, _)| entry.selector().matches(facts))
    }

    /// Which backend would drive this device: what config says, or the default
    /// for an unconfigured one.
    pub fn backend_for(&self, id: &DeviceId) -> Backend {
        self.find(id, None).map(|(_, _, b)| *b).unwrap_or_default()
    }

    pub fn alias_for(&self, id: &DeviceId) -> Option<&str> {
        self.find(id, None).map(|(_, alias, _)| alias.as_str())
    }

    /// [`Self::backend_for`] for a row that was enumerated, so a `sn=` selector
    /// is answered against the serial the descriptor actually reports.
    pub fn backend_for_facts(&self, facts: &DeviceFacts) -> Backend {
        self.find(&facts.id, Some(facts))
            .map(|(_, _, b)| *b)
            .unwrap_or_default()
    }

    /// [`Self::alias_for`], likewise.
    pub fn alias_for_facts(&self, facts: &DeviceFacts) -> Option<&str> {
        self.find(&facts.id, Some(facts))
            .map(|(_, alias, _)| alias.as_str())
    }

    /// Entries that ask for the WinUSB backend, as written.
    pub fn winusb_ids(&self) -> Vec<&ksx_core::DeviceRef> {
        self.entries
            .iter()
            .filter(|(_, _, b)| *b == Backend::Winusb)
            .map(|(id, _, _)| id)
            .collect()
    }
}

/// One `[[device]]` row: what it names, what the user calls it, how it is
/// captured.
type Entry = (ksx_core::DeviceRef, String, Backend);

/// One WinUSB-side row: a USB interface plus what config wants from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbRow {
    pub candidate: ksx_capture::UsbCandidate,
    /// The `[[device]]` alias bound to this id, if any.
    pub alias: Option<String>,
    /// `true` when a `[[device]]` entry selects `backend = "winusb"` for it.
    pub selected: bool,
}

/// One Bluetooth-side row: a paired device plus what config wants from it.
///
/// The Bluetooth twin of [`UsbRow`], and it exists for the same reason: the
/// enumerator says what is attached, and config says what ksx has been told to
/// do with it. The two are joined here, once, rather than at each surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtRow {
    pub candidate: ksx_capture::BtCandidate,
    /// The `[[device]]` alias bound to this device, if any.
    pub alias: Option<String>,
    /// `true` when a `[[device]]` entry selects `backend = "winusb"` for it —
    /// which, on this transport, is a config that can never work. It is
    /// reported rather than corrected: ksx does not silently rewrite a file.
    pub selected_winusb: bool,
}

impl BtRow {
    /// The id a `[[device]]` entry for this device holds: the keyboard-class
    /// devnode, else the service node ksx found it through.
    pub fn config_id(&self) -> &DeviceId {
        self.candidate
            .keyboard_id
            .as_ref()
            .unwrap_or(&self.candidate.id)
    }
}

/// The `[[device]] id` `ksx device pick` would write for each row, index-aligned
/// with `rows`.
///
/// **One function, so no two surfaces can answer differently.** `ksx devices`,
/// `ksx device scan` and the typed [`ksx_api::DevicesView`] all read this, and
/// it is `DeviceSelector::strongest_for` — literally the call `plan_pick` makes,
/// against the same enumeration — so what a surface *shows* is what the writer
/// *writes* (`docs/SURFACES.md` §1, `docs/DEVICE-IDENTITY.md` §5).
///
/// Deriving it per surface was the alternative, and it is how the mapper's
/// timing arithmetic ended up existing three times.
///
/// The whole enumeration is the second argument on purpose: the rung depends on
/// what else is plugged in. One board of a model gets `usb:d209:0430:00`; while
/// its twin is connected the same board gets a `port=` pin, which is both the
/// honest answer and the ambiguity marker a picker needs.
pub fn suggested_selectors(rows: &[UsbRow]) -> Vec<String> {
    let connected: Vec<DeviceFacts> = rows.iter().map(|r| r.candidate.facts()).collect();
    connected
        .iter()
        .map(|facts| ksx_core::DeviceSelector::strongest_for(facts, &connected).to_string())
        .collect()
}

impl UsbRow {
    /// Is this row ready to be captured — configured for WinUSB *and* rebound?
    pub fn ready(&self) -> bool {
        self.selected && self.candidate.binding.is_winusb()
    }

    /// Configured for WinUSB but still on the keyboard stack: `ksx run` will
    /// refuse. This is the single most useful line this command prints.
    pub fn needs_rebind(&self) -> bool {
        self.selected && !self.candidate.binding.is_winusb()
    }
}

/// Pure, fixture-testable view over one enumeration pass.
pub struct DevicesReport {
    /// Keyboards the Interception driver sees, sorted by slot. Empty when the
    /// driver is not installed — which is the *expected* end state of M6.
    pub keyboards: Vec<DeviceInfo>,
    /// Was the Interception driver available at all?
    pub interception_available: bool,
    /// Mice visible to the driver — listed as a count only; ksx never touches
    /// the mouse filter.
    pub mice_visible: usize,
    /// HID-class USB interfaces, claimable or not.
    pub usb: Vec<UsbRow>,
    /// Was USB enumeration possible?
    pub usb_available: bool,
    /// Paired Bluetooth devices — the other half of "what is attached".
    ///
    /// A separate field from [`Self::usb`] because they come from separate
    /// passes with separate failure modes; they are UNIFIED at the view, which
    /// is where a human reads them, and never here.
    pub bluetooth: Vec<BtRow>,
    /// Was Bluetooth enumeration possible?
    pub bluetooth_available: bool,
    /// `[[device]]` entries, for the backend column.
    pub configured: ConfiguredDevices,
}

impl DevicesReport {
    /// Interception-only report (the shape M3–M5 produced).
    ///
    /// Test-only since M6: the real command always has both halves, and a
    /// constructor that silently claims "no USB" would be a way to build a
    /// report that cannot happen.
    #[cfg(test)]
    pub fn new(devices: Vec<DeviceInfo>) -> Self {
        Self::build(
            devices,
            true,
            Vec::new(),
            false,
            Vec::new(),
            false,
            ConfiguredDevices::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build(
        mut devices: Vec<DeviceInfo>,
        interception_available: bool,
        usb: Vec<UsbRow>,
        usb_available: bool,
        bluetooth: Vec<BtRow>,
        bluetooth_available: bool,
        configured: ConfiguredDevices,
    ) -> Self {
        let mice_visible = devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Mouse)
            .count();
        devices.retain(|d| d.kind == DeviceKind::Keyboard);
        devices.sort_by_key(|d| d.interception_slot);
        Self {
            keyboards: devices,
            interception_available,
            mice_visible,
            usb,
            usb_available,
            bluetooth,
            bluetooth_available,
            configured,
        }
    }

    /// Bluetooth devices that are keyboards — the ones with a backend.
    pub fn bt_keyboards(&self) -> impl Iterator<Item = &BtRow> {
        self.bluetooth.iter().filter(|r| r.candidate.is_keyboard)
    }

    /// `[[device]] backend = "winusb"` entries that resolved to a BLUETOOTH
    /// device.
    ///
    /// A different fault from [`Self::unmatched_winusb_config`] and it must not
    /// be folded into it: that one means "the board is not here or the id is
    /// wrong", and the fix is to plug it in or re-pick. This one means the
    /// entry names the right device and asks for a backend that transport can
    /// never offer, and the fix is to change the entry — no claim, replug or
    /// future release does anything for it.
    pub fn winusb_on_bluetooth(&self) -> Vec<&BtRow> {
        self.bluetooth
            .iter()
            .filter(|r| r.selected_winusb)
            .collect()
    }

    pub fn slots_used(&self) -> usize {
        self.keyboards.len()
    }

    pub fn highest_slot(&self) -> Option<u8> {
        self.keyboards
            .iter()
            .filter_map(|d| d.interception_slot)
            .max()
    }

    /// Hardware ids shared by two or more connected keyboards. Binding one of
    /// these to a slot makes `ksx run` refuse to start.
    pub fn duplicates(&self) -> Vec<DeviceId> {
        duplicate_hardware_ids(&self.keyboards)
    }

    /// How many keyboards report `id`.
    pub fn count_of(&self, id: &DeviceId) -> usize {
        self.keyboards.iter().filter(|d| &d.id == id).count()
    }

    /// A keyboard outside the 1..=10 budget means the driver's slot table is
    /// exhausted/corrupt for that device — reboot required (risk review R2).
    pub fn reboot_required(&self) -> bool {
        self.keyboards.iter().any(|d| {
            d.interception_slot
                .is_some_and(|s| !(1..=MAX_KEYBOARD_SLOT as u8).contains(&s))
        })
    }

    /// HID interfaces only — the ones that could ever carry keyboard reports.
    pub fn hid_rows(&self) -> impl Iterator<Item = &UsbRow> {
        self.usb
            .iter()
            .filter(|r| r.candidate.is_keyboard_candidate())
    }

    /// Rows a run would refuse on: configured for WinUSB, not rebound.
    pub fn pending_rebinds(&self) -> Vec<&UsbRow> {
        self.usb.iter().filter(|r| r.needs_rebind()).collect()
    }

    /// `[[device]] backend = "winusb"` entries with no matching USB interface —
    /// a config pointing at a board that is not plugged in, or (much more
    /// likely, and the reason this exists) a config still holding an
    /// **Interception hardware id** after being switched to `winusb`.
    /// See `docs/MIGRATION-WINUSB.md`.
    pub fn unmatched_winusb_config(&self) -> Vec<&ksx_core::DeviceRef> {
        self.configured
            .winusb_ids()
            .into_iter()
            .filter(|entry| {
                !self.usb.iter().any(|r| {
                    r.candidate.id.as_str() == entry.raw()
                        || entry.selector().matches(&r.candidate.facts())
                })
            })
            .collect()
    }
}

/// `YYYY-MM-DD HH:MM:SS UTC`, the stamp every ksx view carries.
///
/// Spelled here rather than borrowed from `crate::sources`, which only exists
/// under the `studio`/`cabinet` features. Gated to the same features as its
/// only caller: without a UI nothing asks for a `DevicesView`, and an
/// ungated helper is dead code the default build refuses at `-D warnings`.
#[cfg(windows)]
fn stamp_utc() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// The report as the typed surface every front end reads
/// ([`ksx_api::MachineSource::devices`]).
///
/// A translation, not a second collector: same pass, same facts, shaped for a
/// screen instead of a terminal. The cabinet and Studio therefore cannot
/// disagree with `ksx devices` about what is plugged in.
///
/// Gated to the UI features because that is who reads it — a default build has
/// no surface to render a `DevicesView` and would carry this as dead code.
#[cfg(windows)]
pub fn to_view(report: &DevicesReport) -> ksx_api::DevicesView {
    use ksx_capture::winusb::Binding;

    let keyboards = report
        .keyboards
        .iter()
        .map(|d| ksx_api::KeyboardRow {
            slot: u16::from(d.interception_slot.unwrap_or(0)),
            hardware_id: d.id.as_str().to_owned(),
            alias: report.configured.alias_for(&d.id).map(str::to_owned),
            backend: backend_name(report.configured.backend_for(&d.id)).to_owned(),
            detail: match (d.friendly.as_deref(), vendor_tag(d.id.as_str())) {
                (Some(f), Some(v)) => format!("{f} ({v})"),
                (Some(f), None) => f.to_owned(),
                (None, Some(v)) => v.to_owned(),
                (None, None) => String::new(),
            },
        })
        .collect();

    let selectors = suggested_selectors(&report.usb);
    let mut usb: Vec<ksx_api::UsbRow> = report
        .usb
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let c = &row.candidate;
            // The vocabulary the CLI already prints, so a screen and a terminal
            // describe the same interface with the same words.
            let (state, verdict) = if !c.is_keyboard_candidate() {
                (
                    "not-a-keyboard",
                    "not a keyboard interface; ksx leaves it alone".to_owned(),
                )
            } else {
                match &c.binding {
                    Binding::WinUsb => (
                        "claimed",
                        "bound to winusb.sys — ksx can capture this".to_owned(),
                    ),
                    Binding::HidUsb => (
                        "claimable",
                        "on the keyboard stack; ksx could claim it".to_owned(),
                    ),
                    Binding::Other(service) => (
                        "foreign-driver",
                        format!("{service} owns this interface; ksx will not touch it"),
                    ),
                    Binding::None => (
                        "foreign-driver",
                        "nothing is driving this devnode (mid-rescan?)".to_owned(),
                    ),
                }
            };
            // Which backends can reach this interface — decided by
            // `ksx_core::Reach`, never here. `claimed` is the binding, and a
            // claimed interface is off the Windows keyboard stack by
            // construction, so it cannot type either.
            let reach = ksx_core::Reach {
                transport: ksx_core::Transport::Usb,
                keyboard: c.is_keyboard_candidate(),
                claimed: c.binding.is_winusb(),
                can_type: !c.binding.is_winusb(),
            };
            let eligibility = reach.eligibility();
            ksx_api::UsbRow {
                instance_id: c.id.as_str().to_owned(),
                description: c.friendly().unwrap_or_default().to_owned(),
                transport: ksx_core::Transport::Usb.code().to_owned(),
                state: state.to_owned(),
                verdict,
                alias: row.alias.clone(),
                selected: row.selected,
                ready: row.ready(),
                vendor: ksx_core::vendors::name_for(c.vendor_id, c.product_id).map(str::to_owned),
                // The composite parent: every interface of one physical board
                // shares it, which is what lets a picker group three devnodes
                // into "I-PAC 4X — 3 interfaces".
                board: Some(c.parent_id.clone()),
                // HID class 3, subclass 1 (boot), protocol 1 (keyboard). The
                // one positive signal available without claiming; see the
                // field's docs for why `claimable` alone is not enough for a
                // menu.
                boot_keyboard: c.interface_class == 0x03
                    && c.interface_subclass == 1
                    && c.interface_protocol == 1,
                // Computed once, in the backend, by the same call the writer
                // makes — see `suggested_selectors`.
                selector: selectors.get(i).cloned(),
                interception_eligible: eligibility.interception,
                winusb_eligible: eligibility.winusb,
                backends: eligibility.line,
                can_type: reach.can_type,
                cannot_type_reason: if reach.can_type {
                    String::new()
                } else {
                    ksx_core::transport::INTERCEPTION_NEEDS_THE_STACK.to_owned()
                },
            }
        })
        .collect();

    // ── the Bluetooth half, into the SAME list ────────────────────────────
    //
    // One list is the whole point. Two lists is what shipped, and it meant the
    // detailed view (`ksx device scan`) could not see a Bluetooth keyboard at
    // all while the other one saw it and said nothing useful about it.
    for row in &report.bluetooth {
        let c = &row.candidate;
        let reach = c.reach();
        let eligibility = reach.eligibility();
        let (state, verdict) = if !c.is_keyboard {
            (
                "not-a-keyboard",
                "not a keyboard — ksx leaves it alone".to_owned(),
            )
        } else if !c.can_type {
            (
                "interception-only",
                format!(
                    "a Bluetooth keyboard, but it {} — pairing puts it in the device tree; \
                     connecting it is what makes it type",
                    c.trouble.unwrap_or("cannot deliver a keystroke right now")
                ),
            )
        } else {
            (
                "interception-only",
                "a Bluetooth keyboard on the Windows input stack — ksx can capture it through \
                 Interception and split it into virtual pads"
                    .to_owned(),
            )
        };
        usb.push(ksx_api::UsbRow {
            instance_id: c.id.as_str().to_owned(),
            description: c.name.clone(),
            transport: ksx_core::Transport::Bluetooth.code().to_owned(),
            state: state.to_owned(),
            verdict,
            alias: row.alias.clone(),
            // `selected` means "config asks for winusb here". On this transport
            // that is a config that can never work, and it is REPORTED rather
            // than quietly treated as interception — ksx does not decide that a
            // user meant something other than what their file says.
            selected: row.selected_winusb,
            // A Bluetooth device is never rebound, so it is never READY in the
            // WinUSB sense. Saying otherwise would put it in the "ksx run will
            // capture this" column of a backend that cannot see it.
            ready: false,
            vendor: None,
            board: Some(c.device.clone()),
            // The positive keyboard signal on this transport is the
            // keyboard-class devnode, not a HID boot protocol byte — there is
            // no USB interface descriptor to read one from.
            boot_keyboard: c.is_keyboard,
            // What `ksx device pick` would write: the keyboard devnode's path,
            // byte-exact. No `usb:` selector can name a device with no USB
            // interface, and inventing one would be a suggestion the writer
            // could not honour.
            selector: c.keyboard_id.as_ref().map(|id| id.as_str().to_owned()),
            interception_eligible: eligibility.interception,
            winusb_eligible: eligibility.winusb,
            backends: eligibility.line,
            can_type: c.can_type,
            cannot_type_reason: c.trouble.unwrap_or_default().to_owned(),
        });
    }

    // Notes are the things a LIST cannot say, and every one of them is a
    // condition a user would otherwise diagnose by reading rows carefully.
    let mut notes = Vec::new();
    if !report.interception_available && !report.usb_available {
        notes.push(
            "neither the Interception driver nor USB enumeration is available — \
             run `ksx doctor`"
                .to_owned(),
        );
    } else if !report.interception_available {
        notes.push(
            "the Interception driver is not installed. After M6 that is the expected \
             state, not a fault."
                .to_owned(),
        );
    }
    for id in report.duplicates() {
        notes.push(format!(
            "two keyboards report the hardware id {id} — Interception cannot tell them \
             apart, so capturing one captures both"
        ));
    }
    for row in report.pending_rebinds() {
        notes.push(format!(
            "{} is configured for winusb but is still on the keyboard stack; \
             `ksx run` will refuse until it is claimed",
            row.candidate.id.as_str()
        ));
    }
    for id in report.unmatched_winusb_config() {
        notes.push(format!(
            "config names {id} for winusb, but no such interface is present"
        ));
    }
    // Not folded into the note above. "No such interface is present" sends a
    // user to plug something in or re-pick; this entry names the right device
    // and asks for a backend its transport can never offer, so the only fix is
    // to edit the entry.
    for row in report.winusb_on_bluetooth() {
        notes.push(format!(
            "config names {} for winusb, but it is a Bluetooth device: {} Set backend = \
             \"interception\" for it.",
            row.config_id(),
            ksx_core::transport::WINUSB_NEEDS_A_USB_INTERFACE
        ));
    }
    if !report.bluetooth_available {
        notes.push(
            "Bluetooth enumeration failed — any paired device is MISSING from this list, and its \
             absence here is not evidence that it is unpaired"
                .to_owned(),
        );
    }
    if report.reboot_required() {
        notes.push("a rebind is pending a reboot before it takes effect".to_owned());
    }

    ksx_api::DevicesView {
        generated_at: stamp_utc(),
        keyboards,
        interception_available: report.interception_available,
        usb,
        usb_available: report.usb_available,
        bluetooth_available: report.bluetooth_available,
        notes,
    }
}

pub fn devices_json(report: &DevicesReport) -> serde_json::Value {
    let keyboards: Vec<serde_json::Value> = report
        .keyboards
        .iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id.as_str(),
                "slot": d.interception_slot,
                "friendly": d.friendly,
                // Was `"ipac": bool` — one vendor's product name as the shape
                // of the schema, and wrong for every other Ultimarc board.
                "vendor": vendor_tag(d.id.as_str()),
                "alias": report.configured.alias_for(&d.id),
                "backend": backend_name(report.configured.backend_for(&d.id)),
            })
        })
        .collect();
    let duplicates: Vec<serde_json::Value> = report
        .duplicates()
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id.as_str(),
                "count": report.count_of(id),
            })
        })
        .collect();
    let usb: Vec<serde_json::Value> = report
        .hid_rows()
        .map(|row| {
            let c = &row.candidate;
            serde_json::json!({
                "id": c.id.as_str(),
                "vendor_id": format!("{:04X}", c.vendor_id),
                "product_id": format!("{:04X}", c.product_id),
                "interface": c.interface_number,
                "boot_keyboard": c.is_boot_keyboard(),
                "friendly": c.friendly(),
                "vendor": ksx_core::vendors::name_for(c.vendor_id, c.product_id),
                "bound_to": c.binding.label(),
                "winusb_rebind_present": c.binding.is_winusb(),
                "alias": row.alias,
                "selected_backend": if row.selected { "winusb" } else { "interception" },
                "ready": row.ready(),
                "needs_rebind": row.needs_rebind(),
            })
        })
        .collect();
    let bluetooth: Vec<serde_json::Value> = report
        .bluetooth
        .iter()
        .map(|row| {
            let c = &row.candidate;
            let eligibility = c.reach().eligibility();
            serde_json::json!({
                "id": c.id.as_str(),
                "config_id": row.config_id().as_str(),
                "device": c.device,
                "address": c.address,
                "friendly": c.name,
                "transport": ksx_core::Transport::Bluetooth.code(),
                "keyboard": c.is_keyboard,
                // PRESENT and TYPING are different questions for a paired
                // device, so both are in the payload and neither is inferred.
                "can_type": c.can_type,
                "cannot_type_reason": c.trouble,
                "alias": row.alias,
                "backends": {
                    "interception": eligibility.interception,
                    "winusb": eligibility.winusb,
                },
                "winusb_reason": eligibility.winusb_reason,
                "selected_backend": if row.selected_winusb { "winusb" } else { "interception" },
            })
        })
        .collect();
    serde_json::json!({
        // BACKENDS are what ksx captures WITH; TRANSPORTS are how a device is
        // attached. Keeping them in separate objects is the schema saying the
        // thing this whole list exists to teach: `bluetooth` is not a third
        // backend that ksx has not written, it is a transport one of the two
        // backends can never reach.
        "backends": {
            "interception": { "available": report.interception_available },
            "winusb": { "available": report.usb_available },
        },
        "transports": {
            // Two enumerations, two flags. A consumer that read an empty
            // device array as "nothing attached" without checking these would
            // be making the exact assertion a failed read cannot support.
            "usb": { "available": report.usb_available },
            "bluetooth": { "available": report.bluetooth_available },
        },
        "keyboards": keyboards,
        "mice_visible": report.mice_visible,
        "usb_candidates": usb,
        "bluetooth_devices": bluetooth,
        "health": {
            "keyboard_slots_used": report.slots_used(),
            "highest_keyboard_slot": report.highest_slot(),
            "slot_budget": MAX_KEYBOARD_SLOT,
            "reboot_required": report.reboot_required(),
            // Ids shared by several boards: unusable as a slot binding, because
            // Interception cannot tell those boards apart.
            "duplicate_hardware_ids": duplicates,
            "pending_rebinds": report
                .pending_rebinds()
                .iter()
                .map(|r| r.candidate.id.as_str())
                .collect::<Vec<_>>(),
            "unmatched_winusb_config": report
                .unmatched_winusb_config()
                .iter()
                .map(|entry| entry.raw())
                .collect::<Vec<_>>(),
            // Deliberately its own key. `unmatched_winusb_config` means "not
            // here, or the id is wrong" and is fixed by plugging in or
            // re-picking; this means the entry names the right device and asks
            // for a backend its transport can never offer.
            "winusb_on_bluetooth": report
                .winusb_on_bluetooth()
                .iter()
                .map(|row| row.config_id().as_str())
                .collect::<Vec<_>>(),
        },
    })
}

fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Interception => "interception",
        Backend::Winusb => "winusb",
    }
}

/// Grouped human report. Pure: same report, same text, any platform.
pub fn render_human(report: &DevicesReport) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();

    // -- Interception half ------------------------------------------------
    if !report.interception_available {
        let _ = writeln!(
            out,
            "interception backend: driver not installed (expected once every \
             board is on WinUSB)"
        );
    } else if report.keyboards.is_empty() {
        let _ = writeln!(out, "no keyboards visible to the Interception driver");
    } else {
        let _ = writeln!(out, "keyboards (interception backend):");
        for d in &report.keyboards {
            let slot = d
                .interception_slot
                .map_or_else(|| "?".to_string(), |s| s.to_string());
            let friendly = d.friendly.as_deref().unwrap_or("n/a");
            let tag = vendor_tag(d.id.as_str()).map_or_else(String::new, |n| format!("  [{n}]"));
            // A device configured for winusb still shows up here until the
            // rebind happens — saying so is the whole point of the column.
            let note = match report.configured.backend_for(&d.id) {
                Backend::Winusb => "  -> configured backend: winusb (not rebound yet)",
                Backend::Interception => "",
            };
            let _ = writeln!(
                out,
                "  slot {slot:<2} {}  \"{friendly}\"{tag}{note}",
                d.id.as_str()
            );
        }
    }
    if report.mice_visible > 0 {
        let _ = writeln!(
            out,
            "mice: {} visible (unused — ksx never sets the mouse filter)",
            report.mice_visible
        );
    }

    // -- WinUSB half ------------------------------------------------------
    if !report.usb_available {
        let _ = writeln!(out, "usb enumeration unavailable");
    } else {
        // Index-aligned with `report.usb`, so the filter below has to carry the
        // selector along rather than re-deriving it from the surviving rows —
        // the rung depends on what ELSE is plugged in, including the rows this
        // filter drops.
        let selectors = suggested_selectors(&report.usb);
        let rows: Vec<(&UsbRow, &String)> = report
            .usb
            .iter()
            .zip(&selectors)
            .filter(|(row, _)| row.candidate.is_keyboard_candidate())
            .collect();
        if rows.is_empty() {
            let _ = writeln!(out, "no HID USB interfaces found");
        } else {
            let _ = writeln!(out, "usb interfaces (winusb backend candidates):");
            for (row, selector) in rows {
                let c = &row.candidate;
                let friendly = c.friendly().unwrap_or("n/a");
                let tag = ksx_core::vendors::name_for(c.vendor_id, c.product_id)
                    .map_or_else(String::new, |n| format!("  [{n}]"));
                let state = if row.ready() {
                    "  [READY]"
                } else if row.needs_rebind() {
                    "  [NEEDS REBIND]"
                } else {
                    ""
                };
                // The `id = '…'` line is not decoration. `ResolveError::Missing`
                // tells a user, by name, that "`ksx devices` prints the id it
                // has now, and the `usb:` selector that names the board either
                // way" — and for a long time this command printed no selector
                // at all, so a refusal sent people to a command that could not
                // answer it (`docs/DEVICE-IDENTITY.md` §5).
                let _ = writeln!(
                    out,
                    "  {}  \"{friendly}\"{tag}\n      bound to {} | interface MI_{:02X} | \
                     backend {}{state}\n      id = '{selector}'   <- what `ksx device pick` \
                     would write",
                    c.id.as_str(),
                    c.binding.label(),
                    c.interface_number,
                    if row.selected {
                        "winusb"
                    } else {
                        "interception"
                    },
                );
            }
        }
    }

    // -- Bluetooth half ---------------------------------------------------
    //
    // The same story `ksx device scan` tells, in this command's vocabulary. A
    // Bluetooth keyboard used to be visible here only as an Interception
    // hardware id in the list above, with nothing saying which backends could
    // reach it — so the one fact a user needs (WinUSB never can, and that is
    // the transport rather than a gap) was nowhere on the screen.
    if !report.bluetooth_available {
        let _ = writeln!(
            out,
            "bluetooth enumeration unavailable — any paired device is MISSING below, and its \
             absence is not evidence that it is unpaired"
        );
    } else {
        let paired: Vec<&BtRow> = report.bt_keyboards().collect();
        if paired.is_empty() {
            let _ = writeln!(out, "no Bluetooth keyboards paired");
        } else {
            let _ = writeln!(out, "bluetooth keyboards (interception backend only):");
            for row in paired {
                let c = &row.candidate;
                let _ = writeln!(
                    out,
                    "  {}  \"{}\"\n      {}\n      id = '{}'   <- what `ksx device pick` would \
                     write",
                    c.id.as_str(),
                    c.name,
                    c.reach().eligibility().line,
                    row.config_id().as_str(),
                );
                // PRESENT is not TYPING. A paired keyboard with dead batteries
                // is in the tree all day, and someone reading this list to find
                // a spare before claiming their panel has to see the
                // difference.
                if let Some(trouble) = c.trouble {
                    let _ = writeln!(
                        out,
                        "      [!] {trouble} — it is paired and present and CANNOT type right \
                         now, so it does not count as the spare keyboard a claim needs"
                    );
                }
            }
        }
    }

    // -- Findings ---------------------------------------------------------
    for row in report.winusb_on_bluetooth() {
        let _ = writeln!(
            out,
            "[WARN] config selects backend = \"winusb\" for {}, which is a Bluetooth device. {} \
             No claim, replug or future ksx release changes that — set backend = \
             \"interception\" for this entry, which CAN capture it.",
            row.config_id(),
            ksx_core::transport::WINUSB_NEEDS_A_USB_INTERFACE
        );
    }
    for id in report.duplicates() {
        let _ = writeln!(
            out,
            "[WARN] {} keyboards report the hardware id {id} — the Interception driver cannot \
             tell them apart. `ksx run` refuses to start while a slot is bound to it; move one \
             board to the WinUSB backend, whose ids are per-port instance paths (docs/MIGRATION-WINUSB.md).",
            report.count_of(&id)
        );
    }
    for row in report.pending_rebinds() {
        let _ = writeln!(
            out,
            "[WARN] {} is configured for the winusb backend but is bound to {}. ksx never \
             rebinds a device itself — perform the supervised rebind (docs/MIGRATION-WINUSB.md) with a \
             spare keyboard plugged in, or set backend = \"interception\" for now.",
            row.candidate.id.as_str(),
            row.candidate.binding.label()
        );
    }
    for id in report.unmatched_winusb_config() {
        let _ = writeln!(
            out,
            "[WARN] config selects backend = \"winusb\" for {id}, but no USB interface has that \
             instance path. If that looks like an Interception hardware id (it starts with \
             HID\\ and has no instance suffix), it is: replace it with the USB\\ id listed \
             above — the alias keeps every [[slot]] working (docs/MIGRATION-WINUSB.md)."
        );
    }
    if report.reboot_required() {
        let _ = writeln!(
            out,
            "health: [FAIL] REBOOT REQUIRED — a keyboard sits outside the 1..={} slot \
             budget (Interception slot exhaustion)",
            MAX_KEYBOARD_SLOT
        );
    } else if report.interception_available {
        let highest = report
            .highest_slot()
            .map_or_else(|| "-".to_string(), |s| s.to_string());
        let _ = writeln!(
            out,
            "health: [OK]   {}/{} keyboard slots in use (highest slot {highest}); \
             no exhaustion detected",
            report.slots_used(),
            MAX_KEYBOARD_SLOT
        );
    }
    out
}

/// One enumeration pass, shared by `ksx devices` and by
/// [`ksx_api::MachineSource::devices`].
///
/// Extracted so the cabinet cannot grow a second collector. The whole point of
/// the M9 typed surface is that a screen and a CLI verb answer from the same
/// facts; two collectors would be two answers to "what is plugged in", and the
/// one on screen would be the one nobody tested.
///
/// Read-only on both halves — see the module docs. Never exits: a machine with
/// neither backend is a *report*, and the caller decides what that means (the
/// CLI exits 2; a UI renders the note).
#[cfg(windows)]
pub fn collect() -> DevicesReport {
    use ksx_capture::{CaptureBackend as _, InterceptionBackend};

    // Config is advisory here: a machine with no config still lists hardware.
    let configured = ksx_config::ConfigRoot::discover()
        .ok()
        .and_then(|root| ksx_config::Store::new(root).load_config().ok())
        .map(|loaded| ConfiguredDevices::from_config(&loaded.value))
        .unwrap_or_default();

    // Interception half. A missing driver is a *fact to report*, not a failure:
    // after M6 that is the target state. Creating the context sets no filter.
    let (keyboards, interception_available) = match InterceptionBackend::new() {
        Ok(mut backend) => (backend.devices(), true),
        Err(_) => (Vec::new(), false),
    };

    // WinUSB half. Enumeration only — nothing is opened or claimed.
    let (usb, usb_available) = match ksx_capture::usb_candidates() {
        Ok(found) => {
            let rows = found
                .into_iter()
                .map(|candidate| {
                    // The enumerated facts, not the id: a `sn=` selector can
                    // only be answered against the serial the descriptor
                    // reports, and `selected` is what turns on the "configured
                    // for WinUSB but still on the keyboard stack" warning.
                    let facts = candidate.facts();
                    UsbRow {
                        alias: configured.alias_for_facts(&facts).map(str::to_owned),
                        selected: configured.backend_for_facts(&facts) == Backend::Winusb,
                        candidate,
                    }
                })
                .collect();
            (rows, true)
        }
        Err(err) => {
            tracing::warn!("USB enumeration failed: {err}");
            (Vec::new(), false)
        }
    };

    // Bluetooth half. Enumeration only — the same read-only PnP walk.
    let (bluetooth, bluetooth_available) = match ksx_capture::bt_candidates() {
        Ok(found) => {
            let rows = found
                .into_iter()
                .map(|candidate| {
                    // A Bluetooth device is named in config by the instance
                    // path of its keyboard devnode, so that is the id config is
                    // looked up by. Byte-exact: no `usb:` selector can name a
                    // device with no USB interface.
                    let id = candidate
                        .keyboard_id
                        .clone()
                        .unwrap_or_else(|| candidate.id.clone());
                    BtRow {
                        alias: configured.alias_for(&id).map(str::to_owned),
                        selected_winusb: configured.backend_for(&id) == Backend::Winusb,
                        candidate,
                    }
                })
                .collect();
            (rows, true)
        }
        Err(err) => {
            // Reported as a FAILED READ, never as "nothing is paired" — the
            // note this raises is the difference between "your keyboard is not
            // paired" and "ksx could not look".
            tracing::warn!("Bluetooth enumeration failed: {err}");
            (Vec::new(), false)
        }
    };

    DevicesReport::build(
        keyboards,
        interception_available,
        usb,
        usb_available,
        bluetooth,
        bluetooth_available,
        configured,
    )
}

#[cfg(windows)]
pub fn run(json: bool) -> anyhow::Result<()> {
    let report = collect();
    let interception_available = report.interception_available;
    let usb_available = report.usb_available;

    if !interception_available && !usb_available {
        let message = "neither the Interception driver nor USB enumeration is available; \
                       run `ksx doctor` for driver diagnostics"
            .to_owned();
        if json {
            println!(
                "{}",
                crate::pads::error_json("no-capture-backend", &message)
            );
        } else {
            eprintln!("error: {message}");
        }
        std::process::exit(EXIT_DRIVER_MISSING);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&devices_json(&report))?);
    } else {
        print!("{}", render_human(&report));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx devices` is Windows-only (it enumerates USB and the Interception driver)")
}

#[cfg(test)]
mod tests {
    use ksx_capture::winusb::Binding;
    use ksx_core::DeviceId;

    use super::*;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const LOGI: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";
    const MOUSE: &str = "HID\\VID_046D&PID_C077&REV_7200";
    const IPAC_USB: &str = "USB\\VID_D209&PID_0430&MI_00\\7&1A2B3C4D&0&0000";
    const IPAC_USB_B: &str = "USB\\VID_D209&PID_0430&MI_00\\7&5E6F7A8B&0&0000";

    fn keyboard(id: &str, slot: u8, friendly: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::from(id),
            interception_slot: Some(slot),
            friendly: friendly.map(Into::into),
            kind: DeviceKind::Keyboard,
        }
    }

    /// This cabinet's expected shape: the I-PAC plus a desk keyboard and a
    /// mouse the driver also sees. Deliberately fed out of slot order.
    fn fixture() -> Vec<DeviceInfo> {
        vec![
            keyboard(LOGI, 2, None),
            keyboard(IPAC, 1, Some("I-PAC Arcade Control Interface")),
            DeviceInfo {
                id: DeviceId::from(MOUSE),
                interception_slot: Some(11),
                friendly: None,
                kind: DeviceKind::Mouse,
            },
        ]
    }

    fn usb(id: &str, binding: Binding) -> ksx_capture::UsbCandidate {
        ksx_capture::UsbCandidate {
            id: DeviceId::from(id),
            parent_id: "USB\\VID_D209&PID_0430\\4".into(),
            vendor_id: 0xD209,
            product_id: 0x0430,
            interface_number: 0,
            interface_class: 0x03,
            interface_subclass: 1,
            interface_protocol: 1,
            interface_string: None,
            serial: None,
            product: Some("I-PAC Ultimate I/O".into()),
            device_desc: Some("HID Keyboard Device".into()),
            port_chain: vec![1, 4],
            bus_id: "1".into(),
            binding,
        }
    }

    /// A report from a machine whose Bluetooth walk ANSWERED and found nothing
    /// paired — not one where it failed. The fixtures below are about the USB
    /// half, and defaulting the flag to `false` would quietly put every one of
    /// them on a half-blind machine.
    fn usb_only_report(
        keyboards: Vec<DeviceInfo>,
        interception_available: bool,
        usb: Vec<UsbRow>,
        usb_available: bool,
        configured: ConfiguredDevices,
    ) -> DevicesReport {
        DevicesReport::build(
            keyboards,
            interception_available,
            usb,
            usb_available,
            Vec::new(),
            true,
            configured,
        )
    }

    fn config(entries: &[(&str, &str, Backend)]) -> ConfiguredDevices {
        ConfiguredDevices {
            entries: entries
                .iter()
                .map(|(id, alias, b)| (id.parse().expect(id), (*alias).to_owned(), *b))
                .collect(),
        }
    }

    fn row(id: &str, binding: Binding, configured: &ConfiguredDevices) -> UsbRow {
        let candidate = usb(id, binding);
        UsbRow {
            alias: configured.alias_for(&candidate.id).map(str::to_owned),
            selected: configured.backend_for(&candidate.id) == Backend::Winusb,
            candidate,
        }
    }

    /// **What a surface shows must be what the writer writes.**
    ///
    /// `plan_pick` chooses an id with `DeviceSelector::strongest_for` against
    /// the live enumeration. `ksx devices`, `ksx device scan` and the typed
    /// view all print this function's answer, so a suggestion `pick` would not
    /// commit is a lie in the one place a user is deciding what to commit.
    ///
    /// Breaks against the obvious surface-local re-derivation — building facts
    /// out of the instance path, which is all a rendered row carries.
    /// `DeviceFacts::from_instance_path` cannot read a serial (it lives in the
    /// descriptor, not the path), so the third case below would climb to a
    /// port pin where `pick` writes `sn=` — telling a user their board is now
    /// socket-specific when it is not.
    #[test]
    fn the_suggested_id_is_the_one_pick_would_write_and_it_reads_the_whole_room() {
        let none = ConfiguredDevices::default();

        // One board of a model: the weakest rung, which survives a replug.
        let alone = vec![row(IPAC_USB, Binding::HidUsb, &none)];
        assert_eq!(suggested_selectors(&alone), vec!["usb:d209:0430:00"]);

        // Its twin arrives, and neither reports a serial (the cheap-HID case):
        // the port pin is the only thing left, and BOTH boards get one.
        let twins = vec![
            row(IPAC_USB, Binding::HidUsb, &none),
            row(IPAC_USB_B, Binding::HidUsb, &none),
        ];
        assert_eq!(
            suggested_selectors(&twins),
            vec![
                "usb:d209:0430:00:port=7&1A2B3C4D&0&0000",
                "usb:d209:0430:00:port=7&5E6F7A8B&0&0000",
            ],
            "the rung depends on what else is plugged in, so the whole \
             enumeration has to be the input"
        );

        // Twins whose firmware serials DO differ: the serial rung separates
        // them and still survives a replug, so nothing gets pinned to a socket.
        let mut a = row(IPAC_USB, Binding::HidUsb, &none);
        a.candidate.serial = Some("A".into());
        let mut b = row(IPAC_USB_B, Binding::HidUsb, &none);
        b.candidate.serial = Some("B".into());
        assert_eq!(
            suggested_selectors(&[a, b]),
            vec!["usb:d209:0430:00:sn=A", "usb:d209:0430:00:sn=B"],
            "a serial read from the descriptor is invisible in the instance \
             path, and this is where a path-derived re-derivation goes wrong"
        );
    }

    /// The refusal a user reads names this command: *"`ksx devices` prints the
    /// id it has now, and the `usb:` selector that names the board either
    /// way."* It printed no selector at all, so the advice led nowhere.
    ///
    /// Breaks against that version of `render_human`.
    #[test]
    fn the_human_report_prints_the_usb_selector_the_missing_refusal_promises() {
        let report = usb_only_report(
            fixture(),
            true,
            vec![row(
                IPAC_USB,
                Binding::HidUsb,
                &ConfiguredDevices::default(),
            )],
            true,
            ConfiguredDevices::default(),
        );
        let text = render_human(&report);
        assert!(
            text.contains("id = 'usb:d209:0430:00'"),
            "the selector that names the board either way:\n{text}"
        );
        assert!(
            text.contains(IPAC_USB),
            "beside the id it has right now:\n{text}"
        );
    }

    #[test]
    fn the_vendor_tag_reads_the_product_id_not_just_the_vendor() {
        assert_eq!(vendor_tag(IPAC), Some("Ultimarc I-PAC 4X"));
        assert_eq!(
            vendor_tag(&IPAC.to_ascii_lowercase()),
            Some("Ultimarc I-PAC 4X")
        );
        assert_eq!(vendor_tag(LOGI), None);
        assert_eq!(vendor_tag(""), None);
    }

    /// The bug this replaced, as seen on the reference cabinet:
    ///
    /// ```text
    ///   USB\VID_D209&PID_15A2\6  "SpinTrak"  [I-PAC]
    /// ```
    ///
    /// A SpinTrak is a trackball. `is_ipac` matched Ultimarc's vendor id alone,
    /// so every product that vendor makes claimed to be the one board the
    /// author owned — while the device's own product string said otherwise.
    #[test]
    fn a_spintrak_is_never_tagged_as_an_ipac() {
        let tag = vendor_tag(r"USB\VID_D209&PID_15A2\6").expect("Ultimarc is a known vendor");
        assert_eq!(tag, "Ultimarc SpinTrak");
        assert!(
            !tag.contains("I-PAC"),
            "a trackball must not be labelled as the keyboard encoder: {tag}"
        );
    }

    #[test]
    fn report_splits_kinds_and_sorts_by_slot() {
        let report = DevicesReport::new(fixture());
        assert_eq!(report.keyboards.len(), 2);
        assert_eq!(report.keyboards[0].id, DeviceId::from(IPAC));
        assert_eq!(report.keyboards[1].id, DeviceId::from(LOGI));
        assert_eq!(report.mice_visible, 1);
        assert_eq!(report.slots_used(), 2);
        assert_eq!(report.highest_slot(), Some(2));
        assert!(!report.reboot_required());
    }

    #[test]
    fn keyboard_slot_out_of_budget_flags_reboot() {
        let report = DevicesReport::new(vec![keyboard(IPAC, 11, None)]);
        assert!(report.reboot_required());
        let text = render_human(&report);
        assert!(text.contains("REBOOT REQUIRED"), "{text}");
        let v = devices_json(&report);
        assert_eq!(
            v.pointer("/health/reboot_required"),
            Some(&serde_json::json!(true))
        );
    }

    /// Two identical I-PACs: same hardware id, different slots. The driver
    /// cannot distinguish them, so this has to be visible before someone binds
    /// that id to a slot and gets both boards captured (risk review R2 / §3.3).
    #[test]
    fn two_identical_boards_are_reported_as_a_duplicate_id() {
        let report = DevicesReport::new(vec![
            keyboard(IPAC, 1, Some("I-PAC Arcade Control Interface")),
            keyboard(IPAC, 2, Some("I-PAC Arcade Control Interface")),
            keyboard(LOGI, 3, None),
        ]);
        assert_eq!(report.duplicates(), vec![DeviceId::from(IPAC)]);
        assert_eq!(report.count_of(&DeviceId::from(IPAC)), 2);
        assert_eq!(report.count_of(&DeviceId::from(LOGI)), 1);

        let text = render_human(&report);
        assert!(
            text.contains("2 keyboards report the hardware id") && text.contains(IPAC),
            "{text}"
        );
        let v = devices_json(&report);
        assert_eq!(
            v.pointer("/health/duplicate_hardware_ids/0/id"),
            Some(&serde_json::json!(IPAC))
        );
        assert_eq!(
            v.pointer("/health/duplicate_hardware_ids/0/count"),
            Some(&serde_json::json!(2))
        );
    }

    /// T4, structurally fixed: the same two boards on the WinUSB side are two
    /// distinct ids, so both can be bound and neither is ambiguous.
    #[test]
    fn two_identical_boards_are_distinct_on_the_winusb_side() {
        let cfg = config(&[
            (IPAC_USB, "P1 I-PAC", Backend::Winusb),
            (IPAC_USB_B, "P2 I-PAC", Backend::Winusb),
        ]);
        let report = usb_only_report(
            Vec::new(),
            false,
            vec![
                row(IPAC_USB, Binding::WinUsb, &cfg),
                row(IPAC_USB_B, Binding::WinUsb, &cfg),
            ],
            true,
            cfg,
        );
        assert!(report.duplicates().is_empty());
        assert!(report.pending_rebinds().is_empty());
        assert!(report.unmatched_winusb_config().is_empty());
        assert_eq!(report.hid_rows().filter(|r| r.ready()).count(), 2);

        let text = render_human(&report);
        assert_eq!(text.matches("[READY]").count(), 2, "{text}");
        assert!(!text.contains("cannot tell them apart"));
    }

    #[test]
    fn distinct_boards_and_mice_are_never_duplicates() {
        // A mouse sharing an id with a keyboard is not an ambiguity for us: ksx
        // never captures mice, so only keyboards are compared.
        let report = DevicesReport::new(vec![
            keyboard(IPAC, 1, None),
            keyboard(LOGI, 2, None),
            DeviceInfo {
                id: DeviceId::from(IPAC),
                interception_slot: Some(11),
                friendly: None,
                kind: DeviceKind::Mouse,
            },
        ]);
        assert!(report.duplicates().is_empty());
        assert!(!render_human(&report).contains("cannot tell them apart"));
        assert_eq!(
            devices_json(&report).pointer("/health/duplicate_hardware_ids"),
            Some(&serde_json::json!([]))
        );
    }

    /// The state a user is in for the whole middle of the migration: config
    /// says winusb, the board is still a keyboard. `ksx run` would refuse, so
    /// this must be impossible to miss.
    #[test]
    fn a_selected_but_unrebound_board_is_called_out() {
        let cfg = config(&[(IPAC_USB, "P1 I-PAC", Backend::Winusb)]);
        let report = usb_only_report(
            vec![keyboard(IPAC, 1, Some("I-PAC"))],
            true,
            vec![row(IPAC_USB, Binding::HidUsb, &cfg)],
            true,
            cfg,
        );
        assert_eq!(report.pending_rebinds().len(), 1);
        let text = render_human(&report);
        assert!(text.contains("[NEEDS REBIND]"), "{text}");
        assert!(text.contains("ksx never rebinds a device itself"), "{text}");
        let v = devices_json(&report);
        assert_eq!(
            v.pointer("/health/pending_rebinds/0"),
            Some(&serde_json::json!(IPAC_USB))
        );
        assert_eq!(
            v.pointer("/usb_candidates/0/winusb_rebind_present"),
            Some(&serde_json::json!(false))
        );
    }

    /// The migration mistake worth catching by name: `backend` flipped to
    /// winusb while `id` is still the Interception hardware id.
    #[test]
    fn an_interception_id_left_on_a_winusb_entry_is_diagnosed() {
        let cfg = config(&[(IPAC, "P1 I-PAC", Backend::Winusb)]);
        let report = usb_only_report(
            Vec::new(),
            false,
            vec![row(IPAC_USB, Binding::WinUsb, &cfg)],
            true,
            cfg,
        );
        assert_eq!(
            report
                .unmatched_winusb_config()
                .iter()
                .map(|entry| entry.raw())
                .collect::<Vec<_>>(),
            vec![IPAC]
        );
        let text = render_human(&report);
        assert!(
            text.contains("no USB interface has that instance path"),
            "{text}"
        );
        assert!(
            text.contains("the alias keeps every [[slot]] working"),
            "{text}"
        );
    }

    /// The M6 exit state: Interception uninstalled, everything on WinUSB. The
    /// command must still work — it is how you check the machine survived.
    /// **A backend and a transport are different kinds of thing, and `--json`
    /// says so with its shape.**
    ///
    /// `bluetooth` is not a third backend ksx has not got round to writing; it
    /// is a transport that one of the two backends can never reach. A payload
    /// that listed it beside `interception` and `winusb` would teach a script
    /// author the same wrong thing this whole list exists to correct.
    ///
    /// Breaks against the obvious first shape — a flat `bluetooth_available`
    /// beside `backends` — which also duplicated `backends.winusb.available`
    /// under a second name, so two keys could disagree about one fact.
    #[test]
    fn the_json_keeps_backends_and_transports_apart() {
        let report = DevicesReport::build(
            Vec::new(),
            true,
            Vec::new(),
            true,
            Vec::new(),
            false,
            ConfiguredDevices::default(),
        );
        let v = devices_json(&report);
        assert_eq!(
            v.pointer("/transports/bluetooth/available"),
            Some(&serde_json::json!(false)),
            "a failed Bluetooth walk is reported, not hidden behind the USB one"
        );
        assert_eq!(
            v.pointer("/transports/usb/available"),
            Some(&serde_json::json!(true))
        );
        assert!(
            v.pointer("/backends/bluetooth").is_none(),
            "bluetooth is a transport, never a backend: {v}"
        );
        assert!(
            v.pointer("/bluetooth_available").is_none(),
            "one fact, one key — a flat duplicate can disagree with the nested \
             one: {v}"
        );
    }

    #[test]
    fn listing_works_with_the_interception_driver_gone() {
        let cfg = config(&[(IPAC_USB, "P1 I-PAC", Backend::Winusb)]);
        let report = usb_only_report(
            Vec::new(),
            false,
            vec![row(IPAC_USB, Binding::WinUsb, &cfg)],
            true,
            cfg,
        );
        let text = render_human(&report);
        assert!(text.contains("driver not installed"), "{text}");
        assert!(text.contains("[READY]"), "{text}");
        assert!(
            !text.contains("health:"),
            "no slot-budget line without a driver to budget: {text}"
        );
        let v = devices_json(&report);
        assert_eq!(
            v.pointer("/backends/interception/available"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            v.pointer("/backends/winusb/available"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn non_hid_interfaces_are_not_listed_as_candidates() {
        let cfg = ConfiguredDevices::default();
        let mut vendor = row(IPAC_USB, Binding::None, &cfg);
        vendor.candidate.interface_class = 0xFF;
        let report = usb_only_report(Vec::new(), false, vec![vendor], true, cfg);
        assert_eq!(report.hid_rows().count(), 0);
        assert!(render_human(&report).contains("no HID USB interfaces"));
    }

    #[test]
    fn configured_devices_answers_backend_alias_and_selection() {
        let cfg = config(&[
            (IPAC_USB, "P1 I-PAC", Backend::Winusb),
            (LOGI, "Desk", Backend::Interception),
        ]);
        assert_eq!(cfg.backend_for(&DeviceId::from(IPAC_USB)), Backend::Winusb);
        assert_eq!(
            cfg.backend_for(&DeviceId::from(LOGI)),
            Backend::Interception
        );
        // An unconfigured device gets the schema default.
        assert_eq!(
            cfg.backend_for(&DeviceId::from("USB\\NOPE")),
            Backend::Interception
        );
        assert_eq!(cfg.alias_for(&DeviceId::from(IPAC_USB)), Some("P1 I-PAC"));
        assert_eq!(cfg.alias_for(&DeviceId::from("USB\\NOPE")), None);
        assert_eq!(
            cfg.winusb_ids()
                .iter()
                .map(|entry| entry.raw())
                .collect::<Vec<_>>(),
            vec![IPAC_USB]
        );
    }

    /// **The spelling `ksx device pick` writes has to light up the same
    /// columns.** A config that names a board rather than a socket must still
    /// show its alias and its backend against that board — and must still turn
    /// on the "configured for WinUSB but still on the keyboard stack" line,
    /// which is the single most useful thing this command prints.
    #[test]
    fn a_usb_selector_still_names_the_board_it_matches() {
        let cfg = config(&[("usb:d209:0430:00", "P1 I-PAC", Backend::Winusb)]);
        let candidate = usb(IPAC_USB, Binding::HidUsb);
        let facts = candidate.facts();

        assert_eq!(cfg.alias_for_facts(&facts), Some("P1 I-PAC"));
        assert_eq!(cfg.backend_for_facts(&facts), Backend::Winusb);

        let report = usb_only_report(
            Vec::new(),
            false,
            vec![UsbRow {
                alias: cfg.alias_for_facts(&facts).map(str::to_owned),
                selected: cfg.backend_for_facts(&facts) == Backend::Winusb,
                candidate,
            }],
            true,
            cfg,
        );
        assert_eq!(report.pending_rebinds().len(), 1, "the rebind is still due");
        assert!(
            report.unmatched_winusb_config().is_empty(),
            "the entry DOES match a connected interface — through its selector"
        );
    }

    #[test]
    fn devices_json_snapshot() {
        let report = DevicesReport::new(fixture());
        insta::assert_snapshot!(serde_json::to_string_pretty(&devices_json(&report)).unwrap());
    }

    #[test]
    fn render_human_snapshot() {
        let report = DevicesReport::new(fixture());
        insta::assert_snapshot!(render_human(&report));
    }

    #[test]
    fn mixed_backend_snapshot() {
        // The realistic mid-migration cabinet: one board rebound and ready, one
        // still on the keyboard stack, plus the desk keyboard on Interception.
        let cfg = config(&[
            (IPAC_USB, "P1 I-PAC", Backend::Winusb),
            (IPAC_USB_B, "P2 I-PAC", Backend::Winusb),
            (LOGI, "Desk", Backend::Interception),
        ]);
        let report = usb_only_report(
            vec![keyboard(LOGI, 1, Some("Logitech Keyboard"))],
            true,
            vec![
                row(IPAC_USB, Binding::WinUsb, &cfg),
                row(IPAC_USB_B, Binding::HidUsb, &cfg),
            ],
            true,
            cfg,
        );
        insta::assert_snapshot!(render_human(&report));
    }

    #[test]
    fn empty_enumeration_renders_cleanly() {
        let report = DevicesReport::new(vec![]);
        let text = render_human(&report);
        assert!(text.contains("no keyboards"), "{text}");
        assert!(!report.reboot_required());
        assert_eq!(report.highest_slot(), None);
    }

    /// The typed surface a screen reads must carry what the terminal prints.
    ///
    /// Until this existed, `MachineSource::devices()` fell through to the
    /// trait's REFUSAL, so the cabinet could not list devices at all — and the
    /// vendor fix that stopped calling a SpinTrak an I-PAC lived only in CLI
    /// output where no UI could reach it.
    #[test]
    #[cfg(windows)]
    fn the_view_carries_the_vendor_name_and_groups_by_board() {
        let ipac_kb = usb(IPAC_USB, Binding::WinUsb);
        let mut ipac_mouse = usb(
            r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001",
            Binding::HidUsb,
        );
        ipac_mouse.interface_number = 1;
        // Vendor-specific class, not HID. `is_keyboard_candidate` is
        // deliberately just `is_hid()` — a rebound interface stops describing
        // itself as a keyboard, and NKRO firmware often reports protocol 0, so
        // guessing harder there would only produce confident wrong answers.
        // The real proof is the report descriptor at claim time.
        ipac_mouse.interface_class = 0xFF;
        ipac_mouse.interface_protocol = 0;
        let mut spintrak = usb(r"USB\VID_D209&PID_15A2\6", Binding::HidUsb);
        spintrak.product_id = 0x15A2;
        spintrak.parent_id = r"USB\VID_D209&PID_15A2\6".into();
        spintrak.product = Some("SpinTrak".into());

        let report = usb_only_report(
            Vec::new(),
            false,
            vec![
                UsbRow {
                    candidate: ipac_kb,
                    alias: None,
                    selected: true,
                },
                UsbRow {
                    candidate: ipac_mouse,
                    alias: None,
                    selected: false,
                },
                UsbRow {
                    candidate: spintrak,
                    alias: None,
                    selected: false,
                },
            ],
            true,
            ConfiguredDevices::default(),
        );
        let view = to_view(&report);

        assert_eq!(view.usb.len(), 3);
        // The regression the vendors table fixed, now reaching a screen.
        assert_eq!(view.usb[0].vendor.as_deref(), Some("Ultimarc I-PAC 4X"));
        assert_eq!(view.usb[2].vendor.as_deref(), Some("Ultimarc SpinTrak"));
        assert_ne!(
            view.usb[2].vendor, view.usb[0].vendor,
            "a trackball must not be labelled as the keyboard encoder"
        );

        // Grouping: the I-PAC's two interfaces are ONE board; the SpinTrak is
        // another. This is what lets a picker offer boards, not devnodes.
        assert_eq!(view.usb[0].board, view.usb[1].board);
        assert_ne!(view.usb[0].board, view.usb[2].board);

        // The verdict vocabulary a screen renders is the CLI's own.
        assert_eq!(view.usb[0].state, "claimed");
        assert_eq!(view.usb[1].state, "not-a-keyboard");
        assert_eq!(view.usb[2].state, "claimable");
        assert!(view.usb.iter().all(|r| !r.verdict.is_empty()));

        // A missing Interception driver is a NOTE, not an empty list with no
        // explanation — it is the expected end state after M6.
        assert!(
            view.notes.iter().any(|n| n.contains("expected state")),
            "{:?}",
            view.notes
        );
    }
}
