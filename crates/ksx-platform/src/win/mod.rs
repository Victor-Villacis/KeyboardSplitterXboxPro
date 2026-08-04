//! Live driver-health collection (Windows only). Read-only: registry queries,
//! file metadata, WinVerifyTrust. Never elevates, never errors — a machine with
//! no drivers at all still yields a complete `DriverReport`.

pub(crate) mod devices;
mod filever;
mod registry;
mod services;
pub(crate) mod signature;
mod vigem_pads;

use std::path::{Path, PathBuf};

use crate::parse::{ci_mode_from_name, filetime_to_rfc3339, parse_cip_meta, resolve_image_path};
use crate::report::{
    BusDriverReport, CiPolicyReport, ClassFilterReport, CodeIntegrityReport, DriverFileReport,
    DriverReport, InterceptionReport, ServiceInfo, StartType, WhqlEvaluationReport,
};
use crate::virtual_pads::{owner_candidates, VirtualPadReport};

const SERVICES: &str = "SYSTEM\\CurrentControlSet\\Services";
const KEYBOARD_CLASS: &str =
    "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e96b-e325-11ce-bfc1-08002be10318}";
const MOUSE_CLASS: &str =
    "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e96f-e325-11ce-bfc1-08002be10318}";
const WHQL_EVAL: &str = "SYSTEM\\CurrentControlSet\\Control\\CI\\WhqlOnlyEvaluation";
/// The 2026 cross-signed-trust-removal rollout policy
/// ("Microsoft Windows Cross Certificates for Code Integrity Exceptions Audit Policy").
const CROSS_CERT_POLICY_GUID: &str = "784C4414-79F4-4C32-A6A5-F0FB42A51D0D";

/// The bus service name — the single key both the service-health check and the
/// ghost-pad devnode lookup go through.
const VIGEMBUS_SERVICE: &str = "ViGEmBus";

/// Collect the full driver-health report from the live machine.
pub fn collect() -> DriverReport {
    let windir = windir();
    DriverReport {
        vigembus: bus_driver(VIGEMBUS_SERVICE, &windir),
        scpvbus: bus_driver("ScpVBus", &windir),
        interception: interception(&windir),
        code_integrity: code_integrity(&windir),
        virtual_pads: virtual_pads(),
    }
}

/// The bus's current child pads. The devnode is found by *service name* (the
/// same identity [`bus_driver`] reports on), never by pad VID/PIDs — a real
/// controller on a physical port hangs off a hub, not off ViGEmBus, and must
/// never be counted.
fn virtual_pads() -> VirtualPadReport {
    // The service filter lists devnodes *registered* to ViGEmBus, present or
    // not — an uninstalled or stopped bus leaves a registered id behind. Only
    // a devnode that locates as present (`child_instance_ids` → `Some`) may be
    // reported or named in the restart advice; a stale id would put a
    // non-restartable devnode in the `pnputil` command.
    let mut present_bus: Option<String> = None;
    let mut children: Vec<String> = Vec::new();
    for bus in vigem_pads::bus_instance_ids(VIGEMBUS_SERVICE) {
        if let Some(kids) = vigem_pads::child_instance_ids(&bus) {
            children.extend(kids);
            // A machine has one bus devnode; if several ever exist the
            // children are aggregated and the advice names the first present.
            present_bus.get_or_insert(bus);
        }
    }
    let owners = owner_candidates(&crate::process::snapshot(), std::process::id());
    VirtualPadReport::from_bus_children(present_bus, children, owners)
}

fn windir() -> String {
    std::env::var("SystemRoot")
        .or_else(|_| std::env::var("WINDIR"))
        .unwrap_or_else(|_| "C:\\Windows".to_string())
}

fn bus_driver(service: &str, windir: &str) -> BusDriverReport {
    let key = format!("{SERVICES}\\{service}");
    let installed = registry::key_exists(&key);
    if !installed {
        // No service key: still check for an orphaned driver file.
        return BusDriverReport {
            installed: false,
            service: None,
            driver_file: driver_file(default_sys_path(service, windir)),
        };
    }
    let image_path = registry::read_string(&key, "ImagePath");
    let resolved = image_path
        .as_deref()
        .map(|p| PathBuf::from(resolve_image_path(p, windir)))
        .unwrap_or_else(|| default_sys_path(service, windir));
    BusDriverReport {
        installed: true,
        service: Some(ServiceInfo {
            start_type: registry::read_u32(&key, "Start")
                .map(StartType::from_raw)
                .unwrap_or(StartType::Unknown),
            image_path,
            display_name: registry::read_string(&key, "DisplayName"),
            state: services::query_state(service),
        }),
        driver_file: driver_file(resolved),
    }
}

fn default_sys_path(service: &str, windir: &str) -> PathBuf {
    PathBuf::from(format!("{windir}\\System32\\drivers\\{service}.sys"))
}

fn driver_file(path: PathBuf) -> Option<DriverFileReport> {
    if !path.is_file() {
        return None;
    }
    let path_str = path.display().to_string();
    let ver = filever::query(&path_str);
    Some(DriverFileReport {
        file_version: ver.fixed_version,
        file_version_string: ver.file_version_string,
        company: ver.company,
        description: ver.description,
        signature: Some(signature::verify(&path_str)),
        path: path_str,
    })
}

fn interception(windir: &str) -> InterceptionReport {
    let keyboard = class_filter(KEYBOARD_CLASS, "keyboard", windir);
    let mouse = class_filter(MOUSE_CLASS, "mouse", windir);
    InterceptionReport {
        installed: keyboard.filter_active && keyboard.driver_file.is_some(),
        keyboard,
        mouse,
    }
}

fn class_filter(class_key: &str, filter_name: &str, windir: &str) -> ClassFilterReport {
    let upper_filters = registry::read_multi_string(class_key, "UpperFilters").unwrap_or_default();
    let filter_active = upper_filters
        .iter()
        .any(|f| f.eq_ignore_ascii_case(filter_name));
    ClassFilterReport {
        filter_active,
        driver_file: driver_file(default_sys_path(filter_name, windir)),
        upper_filters,
    }
}

fn code_integrity(windir: &str) -> CodeIntegrityReport {
    let active_dir = Path::new(windir).join("System32\\CodeIntegrity\\CiPolicies\\Active");
    let mut count = None;
    let mut cross_cert_policy = None;

    if let Ok(entries) = std::fs::read_dir(&active_dir) {
        let mut n = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".cip") {
                continue;
            }
            n += 1;
            if name.to_ascii_uppercase().contains(CROSS_CERT_POLICY_GUID) {
                cross_cert_policy = Some(cip_report(&entry.path()));
            }
        }
        count = Some(n);
    }

    CodeIntegrityReport {
        cross_cert_policy,
        active_policy_count: count,
        whql_evaluation: whql_evaluation(),
    }
}

fn cip_report(path: &Path) -> CiPolicyReport {
    // The .cip is PKCS#7-signed; name/id are scraped from embedded UTF-16
    // strings (see parse::parse_cip_meta) — authoritative flag parsing would
    // need elevated `CiTool --list-policies`.
    let meta = std::fs::read(path)
        .map(|b| parse_cip_meta(&b))
        .unwrap_or_default();
    CiPolicyReport {
        guid: format!("{{{CROSS_CERT_POLICY_GUID}}}"),
        file_path: path.display().to_string(),
        mode: ci_mode_from_name(meta.name.as_deref()),
        name: meta.name,
        policy_id: meta.id,
    }
}

fn whql_evaluation() -> Option<WhqlEvaluationReport> {
    if !registry::key_exists(WHQL_EVAL) {
        return None;
    }
    Some(WhqlEvaluationReport {
        num_boot_sessions: registry::read_u32(WHQL_EVAL, "NumBootSessions"),
        latest_boot_id: registry::read_u32(WHQL_EVAL, "LatestBootId"),
        status_event_time_utc: registry::read_u64(WHQL_EVAL, "StatusEventTimestamp")
            .and_then(filetime_to_rfc3339),
        system_uptime_secs: registry::read_u64(WHQL_EVAL, "SystemUptime").map(|t| t / 10_000_000),
    })
}
