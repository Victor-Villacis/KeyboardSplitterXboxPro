//! `ksx doctor` — driver-health report + advice.
//!
//! Rendering ([`render_human`], [`doctor_json`]) and the exit-code policy are
//! pure and cross-platform (fixture-tested); only the live collection
//! (`ksx_platform::collect`) is Windows-only.
//!
//! Exit codes (documented in `--help`): 0 = healthy or warnings only,
//! 1 = error, 2 = at least one `Critical` advice.

// Off Windows only the stub `run` is reachable outside tests; the pure render
// + JSON helpers stay compiled (and tested) but would trip dead_code.
#![cfg_attr(not(windows), allow(dead_code))]

use ksx_platform::{
    Advice, BusDriverReport, CiPolicyMode, ClassFilterReport, DriverFileReport, DriverReport,
    ServiceState, Severity, SignatureStatus, VirtualPadReport,
};

/// Exit code when any advice is `Critical` (documented in `--help`).
pub const EXIT_CRITICAL: i32 = 2;

/// Warnings and info stay exit 0 — only `Critical` flips the exit code.
pub fn exit_code(advice: &[Advice]) -> i32 {
    if advice.iter().any(|a| a.severity == Severity::Critical) {
        EXIT_CRITICAL
    } else {
        0
    }
}

/// The single `--json` object: `{report, advice}`.
pub fn doctor_json(report: &DriverReport, advice: &[Advice]) -> serde_json::Value {
    serde_json::json!({ "report": report, "advice": advice })
}

#[cfg(windows)]
pub fn run(json: bool) -> anyhow::Result<()> {
    let report = ksx_platform::collect();
    let advice = ksx_platform::summarize(&report);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&doctor_json(&report, &advice))?
        );
    } else {
        print!("{}", render_human(&report, &advice));
    }
    let code = exit_code(&advice);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(json: bool) -> anyhow::Result<()> {
    let _ = json;
    println!("ksx doctor: driver checks are Windows-only; nothing to check on this OS");
    Ok(())
}

/// `ksx doctor --latency`.
///
/// Latency is a property of a *running* pipeline, so there is nothing for a
/// one-shot diagnostic to sample: this explains where the number actually comes
/// from instead of pretending to measure it. Always exit 0 — it is help text,
/// not a verdict.
pub fn run_latency(json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&latency_json())?);
    } else {
        print!("{}", render_latency());
    }
    Ok(())
}

/// The single `--latency --json` object.
pub fn latency_json() -> serde_json::Value {
    serde_json::json!({
        "latency": {
            "measured_by": "ksx run",
            "live_flag": "ksx run --latency",
            "window": "capture QueryPerformanceCounter stamp -> ViGEm submit",
            "instrument": "hdrhistogram, 3 significant figures",
            "budget_p99_us": crate::run::latency::BUDGET_P99_US,
            "reported": ["p50_us", "p99_us", "max_us", "updates"],
        }
    })
}

/// Pure: same text on any platform.
pub fn render_latency() -> String {
    let mut doc = Doc(String::new());
    doc.line("ksx doctor — capture-to-submit latency");
    doc.blank();
    doc.line("  Latency is measured live, not by this command. Every `ksx run` session");
    doc.line("  times each keystroke from the capture thread's QueryPerformanceCounter");
    doc.line("  stamp to the ViGEm submit on the output thread, into an HDR histogram:");
    doc.blank();
    doc.line("    ksx run             prints p50 / p99 / max once, at shutdown");
    doc.line("    ksx run --latency   also prints a rolling summary every 5 s");
    doc.line("    ksx run --json      puts the same numbers in the final summary object");
    doc.blank();
    doc.line(format!(
        "  Budget: p99 < {} us (docs/ARCHITECTURE.md rule 5). Over budget means the",
        crate::run::latency::BUDGET_P99_US
    ));
    doc.line("  engine or output thread is being starved — check the dropped-event count");
    doc.line("  in the same summary, and `ksx doctor` for driver problems.");
    doc.0
}

/// Line-oriented string builder so the render code reads as the output does.
struct Doc(String);

impl Doc {
    fn line(&mut self, s: impl AsRef<str>) {
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }

    fn blank(&mut self) {
        self.0.push('\n');
    }
}

fn severity_marker(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "[FAIL]",
        Severity::Warning => "[WARN]",
        Severity::Info => "[INFO]",
    }
}

fn lower_debug(x: impl std::fmt::Debug) -> String {
    format!("{x:?}").to_lowercase()
}

/// Grouped human report. Pure: same report + advice, same text, any platform.
pub fn render_human(report: &DriverReport, advice: &[Advice]) -> String {
    let mut doc = Doc(String::new());
    doc.line("ksx doctor — driver health");
    doc.blank();

    doc.line("ViGEmBus (virtual pad bus)");
    render_vigembus(&mut doc, &report.vigembus);
    doc.blank();

    doc.line("Virtual pads (what the bus is exposing right now)");
    render_virtual_pads(&mut doc, &report.virtual_pads);
    doc.blank();

    doc.line("ScpVBus (legacy bus — used by the old C# splitter, never by ksx)");
    render_scpvbus(&mut doc, &report.scpvbus);
    doc.blank();

    doc.line("Interception (keyboard/mouse class filters — M3 capture backend)");
    render_interception(&mut doc, report);
    doc.blank();

    doc.line("HIDMaestro (M8 backend — not implemented in this build; see below)");
    render_hidmaestro(&mut doc, &report.hidmaestro);
    doc.blank();

    doc.line("Code integrity (2026 cross-signed-trust removal)");
    render_code_integrity(&mut doc, report);
    doc.blank();

    doc.line("Advice");
    if advice.is_empty() {
        doc.line("  [OK]   no issues detected");
    } else {
        for a in advice {
            doc.line(format!(
                "  {} {}: {}",
                severity_marker(a.severity),
                a.code,
                a.message
            ));
        }
    }
    doc.0
}

fn render_vigembus(doc: &mut Doc, bus: &BusDriverReport) {
    if !bus.installed {
        doc.line("  [FAIL] not installed — run `ksx install-drivers`");
        return;
    }
    match &bus.service {
        Some(s) if s.state == ServiceState::Running => doc.line(format!(
            "  [OK]   service running (start: {})",
            lower_debug(s.start_type)
        )),
        Some(s) => doc.line(format!(
            "  [WARN] service {} (start: {})",
            lower_debug(s.state),
            lower_debug(s.start_type)
        )),
        None => doc.line("  [WARN] service state unknown"),
    }
    match &bus.driver_file {
        Some(file) => render_driver_file(doc, file),
        None => doc.line("  [WARN] ViGEmBus.sys missing from System32\\drivers"),
    }
}

fn render_virtual_pads(doc: &mut Doc, pads: &VirtualPadReport) {
    if pads.count == 0 {
        doc.line("  [OK]   none — the bus has no child pads");
        return;
    }
    // Owned pads are the product working; unowned pads outlived their creator.
    let marker = if pads.is_ghost_suspect() {
        "[WARN]"
    } else {
        "[INFO]"
    };
    doc.line(format!(
        "  {marker} {} virtual pad(s) on the bus",
        pads.count
    ));
    for pad in &pads.pads {
        let label = pad.persona_guess.label();
        // Unknown personas print the id the guess was made from, so a new pad
        // type is evidence rather than a mystery.
        if pad.persona_guess == ksx_platform::PersonaGuess::Unknown {
            doc.line(format!(
                "  {marker}   {label} — {} (hardware id {})",
                pad.instance_id, pad.hardware_id
            ));
        } else {
            doc.line(format!("  {marker}   {label} — {}", pad.instance_id));
        }
    }
    match pads.owners.first() {
        Some(owner) => doc.line(format!(
            "  [INFO] a splitter is running ({} pid {}) — pads unplug when it exits",
            owner.name, owner.pid
        )),
        None => {
            doc.line(format!(
                "  [WARN] no known splitter process is running (checked {})",
                ksx_platform::virtual_pads::SPLITTER_PROCESS_NAMES.join(", ")
            ));
            doc.line(
                "  [WARN] unless another ViGEm client (e.g. DS4Windows) created them, \
                 these are ghosts (pads that survived their creator)",
            );
            let bus = pads
                .bus_instance_id
                .as_deref()
                .unwrap_or("<ViGEmBus instance id>");
            doc.line(format!(
                "  [WARN] fix: close whatever created them if it is still alive; else, \
                 as admin: pnputil /restart-device \"{bus}\" — or reboot"
            ));
        }
    }
}

fn render_scpvbus(doc: &mut Doc, bus: &BusDriverReport) {
    if !bus.installed {
        doc.line("  [OK]   not installed");
        return;
    }
    let state = bus
        .service
        .as_ref()
        .map_or_else(|| "state unknown".to_string(), |s| lower_debug(s.state));
    let version = bus
        .driver_file
        .as_ref()
        .and_then(|f| f.file_version.as_deref())
        .unwrap_or("unknown version");
    doc.line(format!(
        "  [INFO] installed ({state}, {version}) — harmless alongside ViGEmBus"
    ));
}

fn render_interception(doc: &mut Doc, report: &DriverReport) {
    let interception = &report.interception;
    let keyboard = &interception.keyboard;
    if !interception.installed && !keyboard.filter_active && keyboard.driver_file.is_none() {
        doc.line(
            "  [INFO] not installed — the M3 `interception` capture backend is \
             unavailable (M6 WinUSB will not need it)",
        );
        return;
    }
    render_filter(doc, "keyboard", keyboard);
    render_filter(doc, "mouse", &interception.mouse);
}

/// HIDMaestro's row — and, printed first, the row that actually decides
/// anything.
///
/// The install state used to *be* the verdict: absent read "those personas are
/// unavailable", present read "[OK] installed — personas available". The second
/// was a promise ksx could not keep, and it appeared the instant a user acted
/// on the first. Which personas ksx can build is a fact about this binary
/// ([`ksx_core::Persona::can_plug`]), so that is what the gate line reports,
/// and it says the same thing whatever the probe found.
///
/// The install state is still printed, with what was looked for, because a
/// half-finished install is worth knowing about. It is `[INFO]`, never `[FAIL]`:
/// nothing on the cabinet stops working without HIDMaestro.
fn render_hidmaestro(doc: &mut Doc, hm: &ksx_platform::HidMaestroReport) {
    let gated = ksx_platform::HidMaestroReport::gated_personas();
    if !gated.is_empty() {
        doc.line(format!(
            "  [INFO] {} cannot be plugged by this build of ksx — no HIDMaestro \
             transport is implemented, so installing the driver does not enable them",
            gated.join("/")
        ));
        doc.line(format!(
            "  [INFO]   use persona '{}' or '{}' instead",
            ksx_core::Persona::PlayStation,
            ksx_core::Persona::Xbox360
        ));
    }
    if !hm.installed {
        if hm.service_key {
            doc.line(
                "  [WARN] service key present but the UMDF driver is missing — broken install",
            );
        } else {
            doc.line("  [INFO] not installed (nothing depends on it today)");
        }
        for target in &hm.looked_for {
            doc.line(format!("  [INFO]   looked for {target}"));
        }
        return;
    }
    doc.line("  [INFO] installed");
    match &hm.driver_file {
        Some(file) => render_driver_file(doc, file),
        None => doc.line("  [WARN] driver file present but unreadable"),
    }
}

fn render_filter(doc: &mut Doc, name: &str, filter: &ClassFilterReport) {
    if filter.filter_active {
        doc.line(format!("  [OK]   {name}-class upper filter active"));
    } else {
        doc.line(format!(
            "  [WARN] {name}-class upper filter not in UpperFilters"
        ));
    }
    match &filter.driver_file {
        Some(file) => render_driver_file(doc, file),
        None => doc.line(format!(
            "  [WARN] {name}.sys missing from System32\\drivers"
        )),
    }
}

fn render_driver_file(doc: &mut Doc, file: &DriverFileReport) {
    let version = file.file_version.as_deref().unwrap_or("unknown version");
    doc.line(format!("  [OK]   {} — {version}", file.path));
    let Some(sig) = &file.signature else {
        doc.line("  [INFO] signature not checked");
        return;
    };
    let signer = sig.signer.as_deref().unwrap_or("unknown signer");
    match sig.status {
        SignatureStatus::Valid => doc.line(format!("  [OK]   signature valid — {signer}")),
        SignatureStatus::ValidExpiredCert => {
            let expired = sig.not_after_utc.as_deref().unwrap_or("in the past");
            doc.line(format!(
                "  [WARN] signature valid via timestamp — {signer}; signing cert expired {expired}"
            ));
        }
        SignatureStatus::Expired => doc.line(format!("  [FAIL] signature expired — {signer}")),
        SignatureStatus::Untrusted => doc.line(format!("  [FAIL] signature untrusted — {signer}")),
        SignatureStatus::Unsigned => doc.line("  [FAIL] unsigned driver"),
        SignatureStatus::Unknown => doc.line("  [INFO] signature state unknown"),
    }
}

fn render_code_integrity(doc: &mut Doc, report: &DriverReport) {
    let ci = &report.code_integrity;
    match &ci.cross_cert_policy {
        None => doc.line("  [OK]   cross-cert CI policy not deployed on this machine"),
        Some(p) => {
            let marker = match p.mode {
                CiPolicyMode::Enforce => "[FAIL]",
                CiPolicyMode::Audit => "[WARN]",
                CiPolicyMode::Unknown => "[INFO]",
            };
            let name = p.name.as_deref().unwrap_or("unnamed policy");
            doc.line(format!(
                "  {marker} {name} ({}) — {} mode",
                p.guid,
                lower_debug(p.mode)
            ));
        }
    }
    if let Some(n) = ci.active_policy_count {
        doc.line(format!("  [INFO] {n} active CI policies"));
    }
    if let Some(whql) = &ci.whql_evaluation {
        let boots = whql
            .num_boot_sessions
            .map_or_else(|| "?".to_string(), |n| n.to_string());
        let uptime = whql
            .system_uptime_secs
            .map_or_else(|| "?".to_string(), |n| n.to_string());
        doc.line(format!(
            "  [INFO] WHQL-only evaluation counters present ({boots} boot sessions, \
             {uptime}s uptime) — enforcement not flipped yet"
        ));
    }
}

#[cfg(test)]
mod tests {
    use ksx_platform::{
        summarize, CiPolicyReport, CodeIntegrityReport, InterceptionReport, ServiceInfo,
        SignatureInfo, StartType, WhqlEvaluationReport,
    };

    use super::*;

    fn file(status: SignatureStatus, signer: &str, not_after: Option<&str>) -> DriverFileReport {
        DriverFileReport {
            path: "C:\\Windows\\System32\\drivers\\test.sys".into(),
            file_version: Some("1.21.442.0".into()),
            file_version_string: None,
            company: None,
            description: None,
            signature: Some(SignatureInfo {
                status,
                signer: Some(signer.into()),
                not_after_utc: not_after.map(Into::into),
                cert_expired: Some(matches!(status, SignatureStatus::ValidExpiredCert)),
                ..SignatureInfo::unknown()
            }),
        }
    }

    fn bus(
        installed: bool,
        state: Option<ServiceState>,
        file: Option<DriverFileReport>,
    ) -> BusDriverReport {
        BusDriverReport {
            installed,
            service: state.map(|state| ServiceInfo {
                start_type: StartType::Demand,
                image_path: None,
                display_name: None,
                state,
            }),
            driver_file: file,
        }
    }

    /// This cabinet as verified live on 2026-08-03: ViGEmBus + ScpVBus both
    /// running, Interception hooked with the 2012-expired cross-signed cert,
    /// `{784C4414-…}` CI policy present in audit mode.
    fn cabinet_report() -> DriverReport {
        let interception_file = || {
            file(
                SignatureStatus::ValidExpiredCert,
                "Francisco Lopes da Silva",
                Some("2012-10-21T15:38:52Z"),
            )
        };
        DriverReport {
            vigembus: bus(
                true,
                Some(ServiceState::Running),
                Some(file(
                    SignatureStatus::Valid,
                    "Nefarius Software Solutions e.U.",
                    None,
                )),
            ),
            scpvbus: bus(
                true,
                Some(ServiceState::Running),
                Some(file(SignatureStatus::Valid, "Scarlet.Crush Productions", None)),
            ),
            interception: InterceptionReport {
                installed: true,
                keyboard: ClassFilterReport {
                    upper_filters: vec!["keyboard".into(), "kbdclass".into()],
                    filter_active: true,
                    driver_file: Some(interception_file()),
                },
                mouse: ClassFilterReport {
                    upper_filters: vec!["mouse".into(), "mouclass".into()],
                    filter_active: true,
                    driver_file: Some(interception_file()),
                },
            },
            code_integrity: CodeIntegrityReport {
                cross_cert_policy: Some(CiPolicyReport {
                    guid: "{784C4414-79F4-4C32-A6A5-F0FB42A51D0D}".into(),
                    file_path: "C:\\Windows\\System32\\CodeIntegrity\\CiPolicies\\Active\\{784C4414-79F4-4C32-A6A5-F0FB42A51D0D}.cip".into(),
                    name: Some(
                        "Microsoft Windows Cross Certificates for Code Integrity Exceptions Audit Policy"
                            .into(),
                    ),
                    policy_id: Some("10.29611.0.0".into()),
                    mode: CiPolicyMode::Audit,
                }),
                active_policy_count: Some(8),
                whql_evaluation: Some(WhqlEvaluationReport {
                    num_boot_sessions: Some(58),
                    latest_boot_id: Some(185),
                    status_event_time_utc: Some("2026-08-04T00:36:58Z".into()),
                    system_uptime_secs: Some(889_951),
                }),
            },
            virtual_pads: VirtualPadReport::empty(),
            // Probed live 2026-08-06 on this machine: no service key, no UMDF
            // driver, no ROOT\HIDMAESTRO devnode, nothing in Program Files.
            hidmaestro: ksx_platform::HidMaestroReport::absent(vec![
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\HIDMaestro".into(),
                "C:\\Windows\\System32\\drivers\\UMDF\\HIDMaestro.dll".into(),
            ]),
        }
    }

    /// Two X360 ghosts and one id no persona matches, no owner process — the
    /// state a `taskkill /f` mid-session leaves behind.
    fn ghost_pads() -> VirtualPadReport {
        VirtualPadReport::from_bus_children(
            Some("ROOT\\SYSTEM\\0002".into()),
            [
                "USB\\VID_045E&PID_028E\\2&2B7A94F5&0&01",
                "USB\\VID_045E&PID_028E\\2&2B7A94F5&0&02",
                "USB\\VID_054C&PID_0CE6\\2&2B7A94F5&0&03",
            ],
            Vec::new(),
        )
    }

    #[test]
    fn render_human_cabinet_snapshot() {
        let report = cabinet_report();
        let advice = summarize(&report);
        insta::assert_snapshot!(render_human(&report, &advice));
    }

    /// The row that used to become a promise the moment a driver appeared.
    ///
    /// With HIDMaestro installed the old render printed
    /// "[OK] installed — dualsense/switchpro/xboxseries personas available",
    /// which was false on every build ksx has ever shipped, and false in the
    /// one state where a user would act on it.
    #[test]
    fn installing_hidmaestro_never_makes_doctor_offer_a_persona() {
        let mut report = cabinet_report();
        report.hidmaestro.installed = true;
        report.hidmaestro.service_key = true;
        let text = render_human(&report, &summarize(&report));

        assert!(
            !text.contains("personas available"),
            "doctor must never advertise a persona it cannot plug:\n{text}"
        );
        for persona in ksx_platform::HidMaestroReport::gated_personas() {
            assert!(text.contains(persona), "{persona} unmentioned:\n{text}");
        }
        assert!(
            text.contains("installing the driver does not enable them"),
            "{text}"
        );
        // The install is still reported — it is worth knowing, it just decides
        // nothing — and it must not be dressed up as a fault.
        assert!(text.contains("[INFO] installed"), "{text}");
        assert_ne!(exit_code(&summarize(&report)), EXIT_CRITICAL);
    }

    #[test]
    fn render_human_missing_vigembus_fails_loudly() {
        let mut report = cabinet_report();
        report.vigembus = bus(false, None, None);
        let advice = summarize(&report);
        let text = render_human(&report, &advice);
        assert!(text.contains("[FAIL] not installed"), "{text}");
        assert!(text.contains("vigembus-missing"), "{text}");
        assert_eq!(exit_code(&advice), EXIT_CRITICAL);
    }

    #[test]
    fn render_human_ghost_pads_name_the_fix() {
        let mut report = cabinet_report();
        report.virtual_pads = ghost_pads();
        let advice = summarize(&report);
        let text = render_human(&report, &advice);
        assert!(text.contains("3 virtual pad(s) on the bus"), "{text}");
        assert!(text.contains("Xbox 360 pad"), "{text}");
        // The unknown row shows the id the guess was made from.
        assert!(
            text.contains("unknown pad") && text.contains("USB\\VID_054C&PID_0CE6"),
            "{text}"
        );
        assert!(text.contains("ghosts"), "{text}");
        // Hedged, not absolute: only the known splitter names were checked, so
        // the render must not claim no owner exists (third-party ViGEm feeders
        // are invisible to the heuristic).
        assert!(text.contains("no known splitter process"), "{text}");
        assert!(
            text.contains("pnputil /restart-device \"ROOT\\SYSTEM\\0002\""),
            "{text}"
        );
        // Ghosts warn; they must not flip the exit code.
        assert_eq!(exit_code(&advice), 0);
    }

    #[test]
    fn render_human_owned_pads_are_not_ghosts() {
        let mut report = cabinet_report();
        report.virtual_pads = ghost_pads();
        report.virtual_pads.owners = vec![ksx_platform::OwnerProcess {
            pid: 4242,
            name: "ksx.exe".into(),
        }];
        let text = render_human(&report, &summarize(&report));
        assert!(!text.contains("ghosts"), "{text}");
        assert!(
            text.contains("a splitter is running (ksx.exe pid 4242)"),
            "{text}"
        );
    }

    #[test]
    fn render_human_no_advice_is_ok() {
        let report = cabinet_report();
        let text = render_human(&report, &[]);
        assert!(text.contains("[OK]   no issues detected"), "{text}");
    }

    #[test]
    fn doctor_json_shape() {
        let mut report = cabinet_report();
        report.virtual_pads = ghost_pads();
        let advice = summarize(&report);
        let v = doctor_json(&report, &advice);
        // The additive virtual-pads key: {count, pads:[{instance_id, persona_guess}]}.
        assert_eq!(
            v.pointer("/report/virtual_pads/count"),
            Some(&serde_json::json!(3))
        );
        assert_eq!(
            v.pointer("/report/virtual_pads/pads/0/persona_guess"),
            Some(&serde_json::json!("xbox360"))
        );
        assert_eq!(
            v.pointer("/report/virtual_pads/pads/0/instance_id"),
            Some(&serde_json::json!(
                "USB\\VID_045E&PID_028E\\2&2B7A94F5&0&01"
            ))
        );
        assert_eq!(
            v.pointer("/report/virtual_pads/pads/2/persona_guess"),
            Some(&serde_json::json!("unknown"))
        );
        assert_eq!(
            v.pointer("/report/virtual_pads/bus_instance_id"),
            Some(&serde_json::json!("ROOT\\SYSTEM\\0002"))
        );
        assert!(v
            .pointer("/advice")
            .and_then(|a| a.as_array())
            .unwrap()
            .iter()
            .any(|a| a.pointer("/code") == Some(&serde_json::json!("ghost-pads"))));
        assert_eq!(
            v.pointer("/report/vigembus/installed"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/report/vigembus/service/state"),
            Some(&serde_json::json!("running"))
        );
        assert_eq!(
            v.pointer("/report/interception/keyboard/driver_file/signature/status"),
            Some(&serde_json::json!("valid_expired_cert"))
        );
        assert_eq!(
            v.pointer("/report/code_integrity/cross_cert_policy/mode"),
            Some(&serde_json::json!("audit"))
        );
        // Advice objects carry the stable code + severity for scripting.
        let advice_arr = v.pointer("/advice").and_then(|a| a.as_array()).unwrap();
        assert!(!advice_arr.is_empty());
        assert!(advice_arr
            .iter()
            .any(|a| a.pointer("/code") == Some(&serde_json::json!("interception-borrowed-time"))));
    }

    #[test]
    fn exit_code_variants() {
        fn advice(severity: Severity) -> Advice {
            Advice {
                severity,
                code: "test-code",
                message: "test".into(),
            }
        }
        assert_eq!(exit_code(&[]), 0);
        assert_eq!(exit_code(&[advice(Severity::Info)]), 0);
        assert_eq!(
            exit_code(&[advice(Severity::Warning), advice(Severity::Info)]),
            0
        );
        assert_eq!(
            exit_code(&[advice(Severity::Warning), advice(Severity::Critical)]),
            EXIT_CRITICAL
        );
        // The cabinet fixture (warnings only) stays exit 0.
        assert_eq!(exit_code(&summarize(&cabinet_report())), 0);
    }
}
