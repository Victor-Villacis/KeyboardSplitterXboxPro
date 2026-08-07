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
use ksx_core::DeviceId;

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
    /// `(id, alias, backend)` from every `[[device]]` entry.
    pub entries: Vec<(DeviceId, String, Backend)>,
}

impl ConfiguredDevices {
    pub fn from_config(config: &ksx_config::ConfigFile) -> Self {
        Self {
            entries: config
                .devices
                .iter()
                .map(|d| (DeviceId::new(d.id.clone()), d.alias.clone(), d.backend))
                .collect(),
        }
    }

    fn find(&self, id: &DeviceId) -> Option<&(DeviceId, String, Backend)> {
        self.entries.iter().find(|(entry, _, _)| entry == id)
    }

    /// Which backend would drive this device: what config says, or the default
    /// for an unconfigured one.
    pub fn backend_for(&self, id: &DeviceId) -> Backend {
        self.find(id).map(|(_, _, b)| *b).unwrap_or_default()
    }

    pub fn alias_for(&self, id: &DeviceId) -> Option<&str> {
        self.find(id).map(|(_, alias, _)| alias.as_str())
    }

    /// Entries that ask for the WinUSB backend.
    pub fn winusb_ids(&self) -> Vec<&DeviceId> {
        self.entries
            .iter()
            .filter(|(_, _, b)| *b == Backend::Winusb)
            .map(|(id, _, _)| id)
            .collect()
    }
}

/// One WinUSB-side row: a USB interface plus what config wants from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbRow {
    pub candidate: ksx_capture::UsbCandidate,
    /// The `[[device]]` alias bound to this id, if any.
    pub alias: Option<String>,
    /// `true` when a `[[device]]` entry selects `backend = "winusb"` for it.
    pub selected: bool,
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
            ConfiguredDevices::default(),
        )
    }

    pub fn build(
        mut devices: Vec<DeviceInfo>,
        interception_available: bool,
        usb: Vec<UsbRow>,
        usb_available: bool,
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
            configured,
        }
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
    pub fn unmatched_winusb_config(&self) -> Vec<&DeviceId> {
        self.configured
            .winusb_ids()
            .into_iter()
            .filter(|id| !self.usb.iter().any(|r| &r.candidate.id == *id))
            .collect()
    }
}

/// `YYYY-MM-DD HH:MM:SS UTC`, the stamp every ksx view carries.
///
/// Spelled here rather than borrowed from `crate::sources`, which only exists
/// under the `studio`/`cabinet` features. Gated to the same features as its
/// only caller: without a UI nothing asks for a `DevicesView`, and an
/// ungated helper is dead code the default build refuses at `-D warnings`.
#[cfg(all(windows, feature = "cabinet"))]
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
#[cfg(all(windows, feature = "cabinet"))]
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

    let usb = report
        .usb
        .iter()
        .map(|row| {
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
            ksx_api::UsbRow {
                instance_id: c.id.as_str().to_owned(),
                description: c.friendly().unwrap_or_default().to_owned(),
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
            }
        })
        .collect();

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
    if report.reboot_required() {
        notes.push("a rebind is pending a reboot before it takes effect".to_owned());
    }

    ksx_api::DevicesView {
        generated_at: stamp_utc(),
        keyboards,
        interception_available: report.interception_available,
        usb,
        usb_available: report.usb_available,
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
    serde_json::json!({
        "backends": {
            "interception": { "available": report.interception_available },
            "winusb": { "available": report.usb_available },
        },
        "keyboards": keyboards,
        "mice_visible": report.mice_visible,
        "usb_candidates": usb,
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
                .map(|id| id.as_str())
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
        let rows: Vec<&UsbRow> = report.hid_rows().collect();
        if rows.is_empty() {
            let _ = writeln!(out, "no HID USB interfaces found");
        } else {
            let _ = writeln!(out, "usb interfaces (winusb backend candidates):");
            for row in rows {
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
                let _ = writeln!(
                    out,
                    "  {}  \"{friendly}\"{tag}\n      bound to {} | interface MI_{:02X} | \
                     backend {}{state}",
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

    // -- Findings ---------------------------------------------------------
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
                .map(|candidate| UsbRow {
                    alias: configured.alias_for(&candidate.id).map(str::to_owned),
                    selected: configured.backend_for(&candidate.id) == Backend::Winusb,
                    candidate,
                })
                .collect();
            (rows, true)
        }
        Err(err) => {
            tracing::warn!("USB enumeration failed: {err}");
            (Vec::new(), false)
        }
    };

    DevicesReport::build(
        keyboards,
        interception_available,
        usb,
        usb_available,
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

    fn config(entries: &[(&str, &str, Backend)]) -> ConfiguredDevices {
        ConfiguredDevices {
            entries: entries
                .iter()
                .map(|(id, alias, b)| (DeviceId::from(*id), (*alias).to_owned(), *b))
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
        let report = DevicesReport::build(
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
        let report = DevicesReport::build(
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
        let report = DevicesReport::build(
            Vec::new(),
            false,
            vec![row(IPAC_USB, Binding::WinUsb, &cfg)],
            true,
            cfg,
        );
        assert_eq!(
            report.unmatched_winusb_config(),
            vec![&DeviceId::from(IPAC)]
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
    #[test]
    fn listing_works_with_the_interception_driver_gone() {
        let cfg = config(&[(IPAC_USB, "P1 I-PAC", Backend::Winusb)]);
        let report = DevicesReport::build(
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
        let report = DevicesReport::build(Vec::new(), false, vec![vendor], true, cfg);
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
        assert_eq!(cfg.winusb_ids(), vec![&DeviceId::from(IPAC_USB)]);
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
        let report = DevicesReport::build(
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
    #[cfg(all(windows, feature = "cabinet"))]
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

        let report = DevicesReport::build(
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
