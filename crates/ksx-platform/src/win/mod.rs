//! Live driver-health collection (Windows only). Read-only: registry queries,
//! file metadata, WinVerifyTrust. Never elevates, never errors — a machine with
//! no drivers at all still yields a complete `DriverReport`.

pub(crate) mod devices;
mod filever;
// `crate::app_paths` reads a different hive through the same wrappers rather
// than opening a second registry binding beside this one.
pub(crate) mod registry;
mod services;
pub(crate) mod signature;
mod vigem_pads;

use std::path::{Path, PathBuf};

use crate::parse::{ci_mode_from_name, filetime_to_rfc3339, parse_cip_meta, resolve_image_path};
use crate::report::{
    BusDriverReport, CiPolicyReport, ClassFilterReport, CodeIntegrityReport, DriverFileReport,
    DriverReport, HidMaestroReport, InterceptionReport, ServiceInfo, StartType,
    WhqlEvaluationReport,
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

/// HIDMaestro's service name and UMDF driver binary.
///
/// HIDMaestro is UMDF2, so its driver is a DLL under `System32\drivers\UMDF`
/// and **not** a `.sys` in `System32\drivers` — [`bus_driver`] would look in
/// the wrong place, which is why this has its own collector.
///
/// Kept in sync with `ksx_hidmaestro::driver::PROBE_TARGETS`, which ksx-output
/// consults at plug time. This crate deliberately depends on nothing, so the
/// two lists are duplicated rather than shared.
const HIDMAESTRO_SERVICE: &str = "HIDMaestro";
const HIDMAESTRO_UMDF_DLL: &str = r"System32\drivers\UMDF\HIDMaestro.dll";

/// Just the ViGEm bus's children, without the rest of the driver stack.
///
/// [`collect`] builds SIX reports — registry reads for two bus services, an
/// Interception class-filter walk, `dll.is_file()` probes, a CI-policy read
/// and a process snapshot — and a caller that wants the pad list pays for all
/// of it. Studio's /pads polls every 2 s and discards five sixths of that
/// report, which is what this exists to stop.
pub fn collect_virtual_pads() -> VirtualPadReport {
    virtual_pads()
}

/// Just the ViGEm bus's own health, without the rest of the driver stack.
///
/// Same reason as [`collect_virtual_pads`], for a different caller: `/start`
/// polls every 2 s and needs exactly one fact — can a pad be plugged right now
/// — while [`collect`] walks the Interception class filters, probes HIDMaestro,
/// reads the CI policy and takes a process snapshot. This is two registry
/// reads, one SCM query and one file-version read.
///
/// It is the SAME [`bus_driver`] call [`collect`] makes, so the judgement
/// [`crate::advice::vigembus_advice`] reaches from it is the judgement
/// `ksx doctor` reaches. A second, cheaper probe that answered differently
/// would be worse than the cost it saved.
pub fn collect_vigembus() -> BusDriverReport {
    bus_driver(VIGEMBUS_SERVICE, &windir())
}

/// Collect the full driver-health report from the live machine.
pub fn collect() -> DriverReport {
    let windir = windir();
    DriverReport {
        vigembus: bus_driver(VIGEMBUS_SERVICE, &windir),
        scpvbus: bus_driver("ScpVBus", &windir),
        interception: interception(&windir),
        code_integrity: code_integrity(&windir),
        virtual_pads: virtual_pads(),
        hidmaestro: hidmaestro(&windir),
    }
}

/// HIDMaestro install state. Never elevates, never errors: a machine without it
/// is a normal machine that simply cannot mount three of the five personas.
fn hidmaestro(windir: &str) -> HidMaestroReport {
    let service_key = format!("{SERVICES}\\{HIDMAESTRO_SERVICE}");
    let dll = PathBuf::from(format!("{windir}\\{HIDMAESTRO_UMDF_DLL}"));
    let looked_for = vec![format!("HKLM\\{service_key}"), dll.display().to_string()];
    HidMaestroReport {
        // The DLL, not the service key: an uninstall can leave the key behind,
        // and reporting that as "installed" would promise personas that fail
        // later at plug time instead of now at doctor time.
        installed: dll.is_file(),
        service_key: registry::key_exists(&service_key),
        driver_file: driver_file(dll),
        looked_for,
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
