//! End-to-end collection against the live machine. The default test asserts
//! only internal consistency so CI stays green with any (or no) drivers
//! installed; `--features cab-tests` adds this cabinet's known ground truth.

#![cfg(windows)]

use ksx_platform::{collect, summarize};

#[test]
fn collect_runs_and_serializes() {
    let report = collect();
    let json = serde_json::to_value(&report).expect("report serializes");

    // Shape: the four sections are always present.
    for section in ["vigembus", "scpvbus", "interception", "code_integrity"] {
        assert!(json.get(section).is_some(), "missing section {section}");
    }

    // Consistency: a bus reported installed has service info; not-installed has none.
    for bus in [&report.vigembus, &report.scpvbus] {
        assert_eq!(bus.installed, bus.service.is_some());
    }
    // filter_active must agree with the raw UpperFilters list it derives from.
    let kbd = &report.interception.keyboard;
    assert_eq!(
        kbd.filter_active,
        kbd.upper_filters
            .iter()
            .any(|f| f.eq_ignore_ascii_case("keyboard"))
    );
    // installed = hooked + file on disk, by definition.
    assert_eq!(
        report.interception.installed,
        kbd.filter_active && kbd.driver_file.is_some()
    );

    // Verdicts derive without panicking and serialize.
    let advice = summarize(&report);
    serde_json::to_value(&advice).expect("advice serializes");
}

/// Ground truth on the cabinet, verified by hand 2026-08-03 (see
/// docs/research/keyboard-capture-2026.md §0). Never run in CI.
#[cfg(feature = "cab-tests")]
mod cab {
    use ksx_platform::{collect, summarize, CiPolicyMode, ServiceState, SignatureStatus};

    #[test]
    fn cabinet_ground_truth() {
        let report = collect();

        // ViGEmBus 1.22.0 (driver binary 1.21.442.0) installed and running.
        assert!(report.vigembus.installed);
        assert_eq!(
            report.vigembus.service.as_ref().unwrap().state,
            ServiceState::Running
        );
        assert!(report.vigembus.driver_file.is_some());

        // Legacy ScpVBus still registered for the C# app.
        assert!(report.scpvbus.installed);

        // Interception live in the keyboard class stack, cross-signed cert
        // expired 2012 but chain still verifies (timestamp countersignature).
        let kbd = &report.interception.keyboard;
        assert!(kbd.filter_active);
        let sig = kbd
            .driver_file
            .as_ref()
            .unwrap()
            .signature
            .as_ref()
            .unwrap();
        assert_eq!(sig.status, SignatureStatus::ValidExpiredCert);
        assert!(sig
            .signer
            .as_deref()
            .unwrap()
            .contains("Francisco Lopes da Silva"));
        assert_eq!(sig.cert_expired, Some(true));
        assert_eq!(sig.not_after_utc.as_deref(), Some("2012-10-21T15:44:05Z"));

        // The cross-signed-trust-removal audit policy is deployed and being
        // evaluated (WhqlOnlyEvaluation counters accumulating).
        let ci = &report.code_integrity;
        let policy = ci.cross_cert_policy.as_ref().unwrap();
        assert_eq!(policy.mode, CiPolicyMode::Audit);
        assert!(policy
            .name
            .as_deref()
            .unwrap()
            .contains("Cross Certificates"));
        assert!(policy.policy_id.is_some());
        assert!(ci.whql_evaluation.is_some());

        let codes: Vec<_> = summarize(&report).iter().map(|a| a.code).collect();
        assert!(codes.contains(&"interception-borrowed-time"));
        assert!(codes.contains(&"scpvbus-present"));
        assert!(!codes.contains(&"vigembus-missing"));
    }
}
