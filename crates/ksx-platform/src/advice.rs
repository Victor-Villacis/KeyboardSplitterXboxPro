//! Health verdicts derived from a [`DriverReport`]. Pure — fixture-tested.

use serde::Serialize;

use crate::report::{BusDriverReport, CiPolicyMode, DriverReport, ServiceState, SignatureStatus};

#[derive(Debug, Clone, Serialize)]
pub struct Advice {
    pub severity: Severity,
    /// Stable machine-readable code — scripts key off this, never the message.
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Turn a report into human warnings, most severe first. Deterministic:
/// same report, same advice in the same order.
pub fn summarize(report: &DriverReport) -> Vec<Advice> {
    let mut out = Vec::new();

    summarize_vigembus(&report.vigembus, &mut out);
    summarize_scpvbus(&report.scpvbus, &mut out);
    summarize_interception(report, &mut out);
    summarize_code_integrity(report, &mut out);

    out.sort_by_key(|a| std::cmp::Reverse(a.severity));
    out
}

fn summarize_vigembus(v: &BusDriverReport, out: &mut Vec<Advice>) {
    if !v.installed {
        out.push(Advice {
            severity: Severity::Critical,
            code: "vigembus-missing",
            message: "ViGEmBus is not installed: ksx pads will not work. \
                      Run `ksx install-drivers` (bundled ViGEmBus 1.22.0 installer)."
                .into(),
        });
        return;
    }
    if v.driver_file.is_none() {
        out.push(Advice {
            severity: Severity::Warning,
            code: "vigembus-file-missing",
            message: "ViGEmBus service is registered but ViGEmBus.sys is missing from \
                      System32\\drivers: broken install. Re-run `ksx install-drivers`."
                .into(),
        });
        return;
    }
    match v.service.as_ref().map(|s| s.state) {
        Some(ServiceState::Running) => {}
        Some(state) => out.push(Advice {
            severity: Severity::Warning,
            code: "vigembus-not-running",
            message: format!(
                "ViGEmBus is installed but not running (state: {state:?}). \
                 Reboot, or start the service, before plugging pads."
            ),
        }),
        None => out.push(Advice {
            severity: Severity::Warning,
            code: "vigembus-state-unknown",
            message: "ViGEmBus is installed but its service state could not be queried.".into(),
        }),
    }
}

fn summarize_scpvbus(s: &BusDriverReport, out: &mut Vec<Advice>) {
    if s.installed {
        let ver = s
            .driver_file
            .as_ref()
            .and_then(|f| f.file_version.as_deref())
            .unwrap_or("unknown version");
        out.push(Advice {
            severity: Severity::Info,
            code: "scpvbus-present",
            message: format!(
                "Legacy ScpVBus ({ver}) is still installed. Harmless alongside ViGEmBus \
                 (the legacy KeyboardSplitter keeps working); uninstall only after full \
                 migration to ksx."
            ),
        });
    }
}

fn summarize_interception(report: &DriverReport, out: &mut Vec<Advice>) {
    let kbd = &report.interception.keyboard;
    let file_present = kbd.driver_file.is_some();

    if !kbd.filter_active && !file_present {
        out.push(Advice {
            severity: Severity::Warning,
            code: "interception-missing",
            message: "Interception driver is not installed: the `interception` capture \
                      backend (M3) is unavailable. The WinUSB backend (M6) will not need it."
                .into(),
        });
        return;
    }
    if file_present && !kbd.filter_active {
        out.push(Advice {
            severity: Severity::Warning,
            code: "interception-filter-inactive",
            message: "keyboard.sys is on disk but 'keyboard' is not in the keyboard-class \
                      UpperFilters: Interception is installed but not hooked. Reinstall it \
                      or reboot."
                .into(),
        });
    }

    let sig = kbd.driver_file.as_ref().and_then(|f| f.signature.as_ref());
    match sig.map(|s| s.status) {
        Some(SignatureStatus::Untrusted) | Some(SignatureStatus::Expired) => {
            out.push(Advice {
                severity: Severity::Critical,
                code: "interception-signature-untrusted",
                message: "Windows no longer trusts the Interception driver signature: \
                          cross-signed-trust enforcement may have landed. The driver can \
                          stop loading on any boot. See docs/RECOVERY.md; the M6 WinUSB \
                          backend removes this dependency."
                    .into(),
            });
        }
        Some(SignatureStatus::ValidExpiredCert) => {
            let policy = report.code_integrity.cross_cert_policy.as_ref();
            let message = match policy {
                Some(p) => {
                    let name = p.name.as_deref().unwrap_or("cross-signed-trust removal");
                    let id = p.policy_id.as_deref().unwrap_or("unknown id");
                    format!(
                        "Interception is on borrowed time: its driver is cross-signed with \
                         a certificate that expired 2012-10-21, and the '{name}' CI policy \
                         ({guid}, id {id}) is active on this machine in \
                         {mode:?} mode. When Windows flips it to enforcement the driver is \
                         blocked from loading. Do NOT take Windows feature updates on the \
                         cabinet; the M6 WinUSB backend removes this dependency.",
                        guid = p.guid,
                        mode = p.mode,
                    )
                }
                None => "Interception's driver is cross-signed with a certificate that \
                         expired 2012-10-21; Microsoft's 2026 servicing removes trust for \
                         cross-signed drivers. Do NOT take Windows feature updates on the \
                         cabinet; the M6 WinUSB backend removes this dependency."
                    .to_string(),
            };
            out.push(Advice {
                severity: Severity::Warning,
                code: if policy.is_some() {
                    "interception-borrowed-time"
                } else {
                    "interception-legacy-signature"
                },
                message,
            });
        }
        _ => {}
    }
}

fn summarize_code_integrity(report: &DriverReport, out: &mut Vec<Advice>) {
    if let Some(p) = &report.code_integrity.cross_cert_policy {
        if p.mode == CiPolicyMode::Enforce {
            out.push(Advice {
                severity: Severity::Critical,
                code: "ci-policy-enforced",
                message: format!(
                    "Cross-signed-trust removal is ENFORCED on this machine (CI policy \
                     {}). Cross-signed drivers (Interception) will not load. Use the \
                     WinUSB backend; see docs/RECOVERY.md.",
                    p.guid
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::*;

    fn file(sig_status: SignatureStatus, cert_expired: bool) -> DriverFileReport {
        DriverFileReport {
            path: "X:\\drivers\\x.sys".into(),
            file_version: Some("1.0.0.0".into()),
            file_version_string: None,
            company: None,
            description: None,
            signature: Some(SignatureInfo {
                status: sig_status,
                signer: Some("Test Signer".into()),
                cert_expired: Some(cert_expired),
                ..SignatureInfo::unknown()
            }),
        }
    }

    fn bus(installed: bool, state: Option<ServiceState>, with_file: bool) -> BusDriverReport {
        BusDriverReport {
            installed,
            service: state.map(|state| ServiceInfo {
                start_type: StartType::Demand,
                image_path: None,
                display_name: None,
                state,
            }),
            driver_file: with_file.then(|| file(SignatureStatus::Valid, false)),
        }
    }

    fn filters(active: bool, with_file: Option<DriverFileReport>) -> ClassFilterReport {
        ClassFilterReport {
            upper_filters: if active {
                vec!["keyboard".into(), "kbdclass".into()]
            } else {
                vec!["kbdclass".into()]
            },
            filter_active: active,
            driver_file: with_file,
        }
    }

    /// This cabinet as verified live on 2026-08-03.
    fn cabinet_report() -> DriverReport {
        DriverReport {
            vigembus: bus(true, Some(ServiceState::Running), true),
            scpvbus: bus(true, Some(ServiceState::Running), true),
            interception: InterceptionReport {
                installed: true,
                keyboard: filters(true, Some(file(SignatureStatus::ValidExpiredCert, true))),
                mouse: filters(true, Some(file(SignatureStatus::ValidExpiredCert, true))),
            },
            code_integrity: CodeIntegrityReport {
                cross_cert_policy: Some(CiPolicyReport {
                    guid: "{784C4414-79F4-4C32-A6A5-F0FB42A51D0D}".into(),
                    file_path: "…".into(),
                    name: Some(
                        "Microsoft Windows Cross Certificates for Code Integrity \
                         Exceptions Audit Policy"
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

    fn codes(advice: &[Advice]) -> Vec<&'static str> {
        advice.iter().map(|a| a.code).collect()
    }

    #[test]
    fn cabinet_state_warns_borrowed_time_only() {
        let advice = summarize(&cabinet_report());
        let codes = codes(&advice);
        assert!(codes.contains(&"interception-borrowed-time"));
        assert!(codes.contains(&"scpvbus-present"));
        assert!(!codes.contains(&"vigembus-missing"));
        assert!(!codes.contains(&"ci-policy-enforced"));
        let bt = advice
            .iter()
            .find(|a| a.code == "interception-borrowed-time")
            .unwrap();
        assert_eq!(bt.severity, Severity::Warning);
        assert!(bt.message.contains("784C4414"));
        assert!(bt.message.contains("10.29611.0.0"));
        assert!(bt.message.contains("feature updates"));
    }

    #[test]
    fn missing_vigembus_is_critical() {
        let mut r = cabinet_report();
        r.vigembus = bus(false, None, false);
        let advice = summarize(&r);
        let a = advice
            .iter()
            .find(|a| a.code == "vigembus-missing")
            .unwrap();
        assert_eq!(a.severity, Severity::Critical);
        assert!(a.message.contains("ksx install-drivers"));
        // Critical sorts before the borrowed-time warning.
        assert_eq!(advice[0].code, "vigembus-missing");
    }

    #[test]
    fn vigembus_stopped_warns() {
        let mut r = cabinet_report();
        r.vigembus = bus(true, Some(ServiceState::Stopped), true);
        assert!(codes(&summarize(&r)).contains(&"vigembus-not-running"));
    }

    #[test]
    fn vigembus_registered_without_file_is_broken_install() {
        let mut r = cabinet_report();
        r.vigembus = bus(true, Some(ServiceState::Stopped), false);
        let advice = summarize(&r);
        assert!(codes(&advice).contains(&"vigembus-file-missing"));
        assert!(!codes(&advice).contains(&"vigembus-not-running"));
    }

    #[test]
    fn interception_absent_is_only_a_warning() {
        let mut r = cabinet_report();
        r.interception = InterceptionReport {
            installed: false,
            keyboard: filters(false, None),
            mouse: filters(false, None),
        };
        let advice = summarize(&r);
        assert!(codes(&advice).contains(&"interception-missing"));
        assert!(!codes(&advice).contains(&"interception-borrowed-time"));
    }

    #[test]
    fn interception_unhooked_file_present() {
        let mut r = cabinet_report();
        r.interception.keyboard =
            filters(false, Some(file(SignatureStatus::ValidExpiredCert, true)));
        let advice = summarize(&r);
        assert!(codes(&advice).contains(&"interception-filter-inactive"));
        // Still warns about the signature cliff.
        assert!(codes(&advice).contains(&"interception-borrowed-time"));
    }

    #[test]
    fn enforced_policy_and_untrusted_signature_are_critical() {
        let mut r = cabinet_report();
        r.code_integrity.cross_cert_policy.as_mut().unwrap().mode = CiPolicyMode::Enforce;
        r.interception.keyboard = filters(true, Some(file(SignatureStatus::Untrusted, true)));
        let advice = summarize(&r);
        let codes = codes(&advice);
        assert!(codes.contains(&"ci-policy-enforced"));
        assert!(codes.contains(&"interception-signature-untrusted"));
        assert_eq!(advice[0].severity, Severity::Critical);
    }

    #[test]
    fn expired_cert_without_policy_still_warns() {
        let mut r = cabinet_report();
        r.code_integrity.cross_cert_policy = None;
        assert!(codes(&summarize(&r)).contains(&"interception-legacy-signature"));
    }

    #[test]
    fn clean_future_machine_is_quiet() {
        // Post-M6 dream state: ViGEm healthy, no ScpVBus, no Interception.
        let r = DriverReport {
            vigembus: bus(true, Some(ServiceState::Running), true),
            scpvbus: bus(false, None, false),
            interception: InterceptionReport {
                installed: false,
                keyboard: filters(false, None),
                mouse: filters(false, None),
            },
            code_integrity: CodeIntegrityReport {
                cross_cert_policy: None,
                active_policy_count: Some(6),
                whql_evaluation: None,
            },
        };
        let advice = summarize(&r);
        assert_eq!(codes(&advice), vec!["interception-missing"]);
        assert_eq!(advice[0].severity, Severity::Warning);
    }

    #[test]
    fn report_and_advice_serialize_to_stable_json() {
        let r = cabinet_report();
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            v.pointer("/vigembus/installed"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            v.pointer("/vigembus/service/state"),
            Some(&serde_json::json!("running"))
        );
        assert_eq!(
            v.pointer("/interception/keyboard/driver_file/signature/status"),
            Some(&serde_json::json!("valid_expired_cert"))
        );
        assert_eq!(
            v.pointer("/code_integrity/cross_cert_policy/mode"),
            Some(&serde_json::json!("audit"))
        );
        let advice = serde_json::to_value(summarize(&r)).unwrap();
        assert_eq!(
            advice.get(0).and_then(|a| a.get("severity")),
            Some(&serde_json::json!("warning"))
        );
    }
}
