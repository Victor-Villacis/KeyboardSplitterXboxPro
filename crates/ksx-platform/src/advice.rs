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
    summarize_virtual_pads(report, &mut out);
    summarize_scpvbus(&report.scpvbus, &mut out);
    summarize_interception(report, &mut out);
    summarize_code_integrity(report, &mut out);
    summarize_hidmaestro(report, &mut out);

    out.sort_by_key(|a| std::cmp::Reverse(a.severity));
    out
}

/// The personas ksx cannot build, and — separately — whether HIDMaestro is
/// installed.
///
/// **Two facts, deliberately not one.** The old version reported only the
/// second and let it stand for the first: it said "HIDMaestro is not installed,
/// so the dualsense/switchpro/xboxseries personas are unavailable", and it went
/// completely silent once the driver appeared. Both halves were wrong. ksx has
/// no code that can create a HIDMaestro pad on any machine, so the install was
/// never the reason and its arrival never a fix — a user who followed that
/// advice installed a second driver and got exactly nothing.
///
/// Everything here stays **Info, never Warning** (except a half-finished
/// install, which is a real defect on the machine): a cabinet running Xbox 360
/// and PlayStation slots — every configuration ksx ships today — is unaffected,
/// and warning about it would train users to ignore the warning column.
fn summarize_hidmaestro(report: &DriverReport, out: &mut Vec<Advice>) {
    let hm = &report.hidmaestro;
    let gated = crate::report::HidMaestroReport::gated_personas();
    if !gated.is_empty() {
        out.push(Advice {
            severity: Severity::Info,
            code: "personas-not-implemented",
            message: format!(
                "The {} personas cannot be plugged by this build of ksx: it has no code \
                 that creates a device over HIDMaestro's shared section, only the protocol \
                 layers above it. Installing HIDMaestro does not enable them. Use \
                 playstation (HID/DirectInput — ✕○△□ prompts, read by MAME/RetroArch/Steam \
                 Input) or xbox360 instead.",
                gated.join("/"),
            ),
        });
    }
    if hm.installed {
        return;
    }
    if hm.service_key {
        // A half-install is a real defect on the machine and worth a warning on
        // its own terms — even though no persona depends on fixing it today.
        out.push(Advice {
            severity: Severity::Warning,
            code: "hidmaestro-partial",
            message: format!(
                "HIDMaestro's service key exists but its UMDF driver is missing \
                 (checked {}): a broken or half-removed install. Reinstall or remove it.",
                hm.looked_for.join(", "),
            ),
        });
        return;
    }
    out.push(Advice {
        severity: Severity::Info,
        code: "hidmaestro-missing",
        message: "HIDMaestro is not installed. Nothing depends on it today — ksx cannot \
                  drive it either way — and xbox360/playstation slots run on ViGEmBus."
            .into(),
    });
}

/// **`ksx doctor`'s ViGEmBus judgement, on its own.**
///
/// The same function [`summarize`] calls, exposed because more than one caller
/// now needs to know whether a pad can be plugged and every one of them must
/// reach that answer the same way. `/start` asks it before it offers Play, and
/// `ksx doctor` prints it: two surfaces, one verdict, one set of codes.
///
/// Empty means healthy — a service key, `ViGEmBus.sys` on disk, and a running
/// service. It never means "could not tell": that state is
/// `vigembus-state-unknown`, which is a warning like the others precisely
/// because a caller must not be able to mistake it for silence.
pub fn vigembus_advice(v: &BusDriverReport) -> Vec<Advice> {
    let mut out = Vec::new();
    summarize_vigembus(v, &mut out);
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

fn summarize_virtual_pads(report: &DriverReport, out: &mut Vec<Advice>) {
    let pads = &report.virtual_pads;
    if pads.count == 0 {
        return;
    }
    if let Some(owner) = pads.owners.first() {
        // Info, not a fault — but NOT the reassurance this used to print.
        //
        // The owner check matches on process NAME, and the tray daemon is
        // `ksx.exe` whether or not it has a session. On a cabinet the daemon
        // runs all day, so there is always an "owner", so `is_ghost_suspect`
        // essentially never fires on the one machine it exists to protect.
        // This said "Expected while it runs" about pads no live handle owned,
        // and 15 of them accumulated on the reference cabinet unremarked.
        //
        // So it now states what is actually known, and names the check that
        // settles it.
        out.push(Advice {
            severity: Severity::Info,
            code: "virtual-pads-in-use",
            message: format!(
                "{} virtual pad(s) are on the bus and a splitter process is running \
                 ({} pid {}). That process is ASSUMED to own them — this matches on \
                 process name, and the tray daemon is `{}` whether or not it is running a \
                 session. If `ksx session status` says stopped, these pads outlived \
                 whatever made them and can be cleared with `ksx pads --prune`.",
                pads.count, owner.name, owner.pid, owner.name
            ),
        });
        return;
    }
    // The fix needs the bus devnode id; pads can only be counted via that
    // devnode, so it is present whenever count > 0 — the fallback is for
    // hand-built reports only.
    let bus = pads
        .bus_instance_id
        .as_deref()
        .unwrap_or("<ViGEmBus instance id>");
    out.push(Advice {
        severity: Severity::Warning,
        code: "ghost-pads",
        message: format!(
            "{} virtual pad(s) are on the bus but no known splitter process ({}) is \
             running. Unless another ViGEm client (DS4Windows and similar feeders \
             use the same bus) created them, these are ghosts left by a killed or \
             wedged session: they sit in joy.cpl, can hold XInput slots and confuse \
             games. Close whatever created them if it is still alive; otherwise run \
             `ksx pads --prune` (a dry run; add --yes from an elevated prompt), which \
             restarts the bus device — the same thing as \
             pnputil /restart-device \"{bus}\" — or reboot.",
            pads.count,
            crate::virtual_pads::SPLITTER_PROCESS_NAMES.join(", "),
        ),
    });
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
            virtual_pads: crate::virtual_pads::VirtualPadReport::empty(),
            // Live state 2026-08-06: probed and absent.
            hidmaestro: crate::report::HidMaestroReport::absent(vec![
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\HIDMaestro".into(),
                "C:\\Windows\\System32\\drivers\\UMDF\\HIDMaestro.dll".into(),
            ]),
        }
    }

    fn ghost_pads(owners: Vec<crate::virtual_pads::OwnerProcess>) -> crate::VirtualPadReport {
        crate::VirtualPadReport::from_bus_children(
            Some("ROOT\\SYSTEM\\0002".into()),
            [
                "USB\\VID_045E&PID_028E\\2&2B7A94F5&0&01",
                "USB\\VID_045E&PID_028E\\2&2B7A94F5&0&02",
            ],
            owners,
        )
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
    fn unowned_pads_are_ghosts_with_the_restart_fix() {
        let mut r = cabinet_report();
        r.virtual_pads = ghost_pads(Vec::new());
        let advice = summarize(&r);
        let ghost = advice.iter().find(|a| a.code == "ghost-pads").unwrap();
        assert_eq!(ghost.severity, Severity::Warning);
        assert!(ghost.message.contains("2 virtual pad(s)"));
        assert!(ghost.message.contains("ghosts"));
        // The verdict must hedge: the owner check only knows the splitter
        // process names, so it may claim "no known splitter", never "no owner
        // exists" — a third-party ViGEm feeder (DS4Windows) is invisible to it.
        assert!(ghost.message.contains("no known splitter process"));
        assert!(ghost.message.contains("ksx.exe, KeyboardSplitter.exe"));
        assert!(ghost.message.contains("ViGEm client"));
        // The fix, verbatim enough to paste: restart the named bus devnode.
        assert!(ghost
            .message
            .contains("pnputil /restart-device \"ROOT\\SYSTEM\\0002\""));
        assert!(ghost.message.contains("reboot"));
    }

    #[test]
    fn pads_with_a_live_splitter_are_info_not_ghosts() {
        let mut r = cabinet_report();
        r.virtual_pads = ghost_pads(vec![crate::OwnerProcess {
            pid: 4242,
            name: "ksx.exe".into(),
        }]);
        let advice = summarize(&r);
        let codes = codes(&advice);
        assert!(!codes.contains(&"ghost-pads"));
        let in_use = advice
            .iter()
            .find(|a| a.code == "virtual-pads-in-use")
            .unwrap();
        assert_eq!(in_use.severity, Severity::Info);
        assert!(in_use.message.contains("ksx.exe pid 4242"));
    }

    #[test]
    fn zero_pads_say_nothing() {
        let advice = summarize(&cabinet_report());
        let codes = codes(&advice);
        assert!(!codes.contains(&"ghost-pads"));
        assert!(!codes.contains(&"virtual-pads-in-use"));
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
            virtual_pads: crate::VirtualPadReport::empty(),
            hidmaestro: crate::report::HidMaestroReport::absent(vec!["<probe>".into()]),
        };
        let advice = summarize(&r);
        // Sorted most-severe-first, so the two Info-level notes land last —
        // behind anything that is actually broken.
        assert_eq!(
            codes(&advice),
            vec![
                "interception-missing",
                "personas-not-implemented",
                "hidmaestro-missing"
            ]
        );
        assert_eq!(advice[0].severity, Severity::Warning);
    }

    /// The advice that used to be a wrong-turn sign.
    ///
    /// "HIDMaestro is not installed, so those personas are unavailable" reads
    /// as an instruction, and following it costs a driver install and buys
    /// nothing. The two facts are now separate, and the one that gates the
    /// personas must not mention the install as its cause — nor go quiet when
    /// the driver turns up.
    #[test]
    fn installing_hidmaestro_does_not_change_which_personas_are_offered() {
        let mut absent = cabinet_report();
        absent.hidmaestro = crate::report::HidMaestroReport::absent(vec!["<probe>".into()]);
        let mut present = absent.clone();
        present.hidmaestro.installed = true;
        present.hidmaestro.service_key = true;

        for (label, report) in [("absent", &absent), ("installed", &present)] {
            let advice = summarize(report);
            let gate = advice
                .iter()
                .find(|a| a.code == "personas-not-implemented")
                .unwrap_or_else(|| {
                    panic!("{label}: the gate note must survive: {:?}", codes(&advice))
                });
            for persona in crate::report::HidMaestroReport::gated_personas() {
                assert!(gate.message.contains(persona), "{label}: {}", gate.message);
            }
            assert!(
                gate.message
                    .contains("Installing HIDMaestro does not enable them"),
                "{label}: {}",
                gate.message
            );
            assert_eq!(gate.severity, Severity::Info, "{label}");
        }
        // ...and the install note itself only appears when it is true.
        assert!(codes(&summarize(&absent)).contains(&"hidmaestro-missing"));
        assert!(!codes(&summarize(&present)).contains(&"hidmaestro-missing"));
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
