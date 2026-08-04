//! `ksx devices` — keyboards as the Interception driver currently sees them.
//!
//! Read-only and safe on a production keyboard stack: constructing an
//! `InterceptionBackend` never sets a class filter (see its `new` docs), so
//! enumeration cannot affect the machine's keyboards in any way.
//!
//! Exit codes (documented in `--help`): 0 = listed, 1 = error,
//! [`EXIT_DRIVER_MISSING`] (2) = the Interception driver is unavailable —
//! the hint points at `ksx doctor`; `--json` puts `{"error":{code,message}}`
//! on stdout instead (same convention as `ksx pads`).
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
use ksx_core::DeviceId;

/// Exit code when the Interception driver is unavailable (documented in
/// `--help`). Same value as `ksx pads`' missing-ViGEmBus code: 2 always means
/// "a required driver is not there".
pub const EXIT_DRIVER_MISSING: i32 = 2;

/// Ultimarc's USB vendor id — every I-PAC board enumerates with it. The exact
/// known cabinet encoder is `HID\VID_D209&PID_0430&REV_0056&MI_00`.
const IPAC_VID_TAG: &str = "VID_D209";

/// Is this hardware id an Ultimarc I-PAC family board — the device this whole
/// program exists for?
pub fn is_ipac(hwid: &str) -> bool {
    hwid.to_ascii_uppercase().contains(IPAC_VID_TAG)
}

/// Hardware ids reported by more than one connected **keyboard**, sorted and
/// deduplicated.
///
/// Two boards of the same model share one Interception hardware id (risk review
/// R2): the driver offers nothing else to tell them apart. Anything that binds
/// such an id to a slot is ambiguous by construction — "capture this device"
/// captures both boards, and either one drives every slot bound to the id — so
/// `ksx run` refuses to start and `ksx devices` calls it out.
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

/// Pure, fixture-testable view over one enumeration pass.
pub struct DevicesReport {
    /// Keyboards only, sorted by Interception slot.
    pub keyboards: Vec<DeviceInfo>,
    /// Mice visible to the driver — listed as a count only; M3 never touches
    /// the mouse filter.
    pub mice_visible: usize,
}

impl DevicesReport {
    pub fn new(mut devices: Vec<DeviceInfo>) -> Self {
        let mice_visible = devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Mouse)
            .count();
        devices.retain(|d| d.kind == DeviceKind::Keyboard);
        devices.sort_by_key(|d| d.interception_slot);
        Self {
            keyboards: devices,
            mice_visible,
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
}

/// The single `--json` success object:
/// `{backend, keyboards: [{id, slot, friendly, ipac}], mice_visible, health}`.
pub fn devices_json(report: &DevicesReport) -> serde_json::Value {
    let keyboards: Vec<serde_json::Value> = report
        .keyboards
        .iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id.as_str(),
                "slot": d.interception_slot,
                "friendly": d.friendly,
                "ipac": is_ipac(d.id.as_str()),
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
    serde_json::json!({
        "backend": "interception",
        "keyboards": keyboards,
        "mice_visible": report.mice_visible,
        "health": {
            "keyboard_slots_used": report.slots_used(),
            "highest_keyboard_slot": report.highest_slot(),
            "slot_budget": MAX_KEYBOARD_SLOT,
            "reboot_required": report.reboot_required(),
            // Ids shared by several boards: unusable as a slot binding, because
            // Interception cannot tell those boards apart.
            "duplicate_hardware_ids": duplicates,
        },
    })
}

/// Grouped human report. Pure: same report, same text, any platform.
pub fn render_human(report: &DevicesReport) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if report.keyboards.is_empty() {
        let _ = writeln!(out, "no keyboards visible to the Interception driver");
    } else {
        let _ = writeln!(out, "keyboards (interception backend):");
        for d in &report.keyboards {
            let slot = d
                .interception_slot
                .map_or_else(|| "?".to_string(), |s| s.to_string());
            let friendly = d.friendly.as_deref().unwrap_or("n/a");
            let tag = if is_ipac(d.id.as_str()) {
                "  [I-PAC]"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  slot {slot:<2} {}  \"{friendly}\"{tag}",
                d.id.as_str()
            );
        }
    }
    if report.mice_visible > 0 {
        let _ = writeln!(
            out,
            "mice: {} visible (unused — ksx never sets the mouse filter in M3)",
            report.mice_visible
        );
    }
    for id in report.duplicates() {
        let _ = writeln!(
            out,
            "[WARN] {} keyboards report the hardware id {id} — the Interception driver cannot \
             tell them apart. `ksx run` refuses to start while a slot is bound to it; use \
             different models/ports (or the WinUSB backend in M6).",
            report.count_of(&id)
        );
    }
    if report.reboot_required() {
        let _ = writeln!(
            out,
            "health: [FAIL] REBOOT REQUIRED — a keyboard sits outside the 1..={} slot \
             budget (Interception slot exhaustion)",
            MAX_KEYBOARD_SLOT
        );
    } else {
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

#[cfg(windows)]
pub fn run(json: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_capture::{CaptureBackend as _, CaptureError, InterceptionBackend};

    // Safe: creating the context sets no filter; keyboards are untouched.
    let mut backend = match InterceptionBackend::new() {
        Ok(backend) => backend,
        Err(err @ CaptureError::DriverUnavailable) => {
            let message = format!("{err}; run `ksx doctor` for driver diagnostics");
            if json {
                println!(
                    "{}",
                    crate::pads::error_json("interception-unavailable", &message)
                );
            } else {
                eprintln!("error: {message}");
            }
            std::process::exit(EXIT_DRIVER_MISSING);
        }
        Err(err) => return Err(err).context("creating the Interception context"),
    };

    let report = DevicesReport::new(backend.devices());
    if json {
        println!("{}", serde_json::to_string_pretty(&devices_json(&report))?);
    } else {
        print!("{}", render_human(&report));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx devices` is Windows-only (it enumerates via the Interception driver)")
}

#[cfg(test)]
mod tests {
    use ksx_core::DeviceId;

    use super::*;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const LOGI: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";
    const MOUSE: &str = "HID\\VID_046D&PID_C077&REV_7200";

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

    #[test]
    fn ipac_tag_matches_vid_d209_case_insensitively() {
        assert!(is_ipac(IPAC));
        assert!(is_ipac(&IPAC.to_ascii_lowercase()));
        assert!(!is_ipac(LOGI));
        assert!(!is_ipac(""));
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

    #[test]
    fn distinct_boards_and_mice_are_never_duplicates() {
        // A mouse sharing an id with a keyboard is not an ambiguity for us: M4
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
    fn empty_enumeration_renders_cleanly() {
        let report = DevicesReport::new(vec![]);
        let text = render_human(&report);
        assert!(text.contains("no keyboards"), "{text}");
        assert!(!report.reboot_required());
        assert_eq!(report.highest_slot(), None);
    }
}
