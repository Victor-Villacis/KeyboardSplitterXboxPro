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
    ServiceState, Severity, SignatureStatus,
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

    doc.line("ScpVBus (legacy bus — used by the old C# splitter, never by ksx)");
    render_scpvbus(&mut doc, &report.scpvbus);
    doc.blank();

    doc.line("Interception (keyboard/mouse class filters — M3 capture backend)");
    render_interception(&mut doc, report);
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
                issuer: None,
                not_after_utc: not_after.map(Into::into),
                cert_expired: Some(matches!(status, SignatureStatus::ValidExpiredCert)),
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
        }
    }

    #[test]
    fn render_human_cabinet_snapshot() {
        let report = cabinet_report();
        let advice = summarize(&report);
        insta::assert_snapshot!(render_human(&report, &advice));
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
    fn render_human_no_advice_is_ok() {
        let report = cabinet_report();
        let text = render_human(&report, &[]);
        assert!(text.contains("[OK]   no issues detected"), "{text}");
    }

    #[test]
    fn doctor_json_shape() {
        let report = cabinet_report();
        let advice = summarize(&report);
        let v = doctor_json(&report, &advice);
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
