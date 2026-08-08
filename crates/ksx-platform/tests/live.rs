//! End-to-end collection against the live machine. The default test asserts
//! only internal consistency so CI stays green with any (or no) drivers
//! installed; `--features cab-tests` adds this cabinet's known ground truth.

#![cfg(windows)]

use ksx_platform::{collect, summarize};

#[test]
fn collect_runs_and_serializes() {
    let report = collect();
    let json = serde_json::to_value(&report).expect("report serializes");

    // Shape: the five sections are always present.
    for section in [
        "vigembus",
        "scpvbus",
        "interception",
        "code_integrity",
        "virtual_pads",
    ] {
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

    // The ghost-pad section: count mirrors the rows, and a pad without a bus
    // devnode to hang off is impossible by construction.
    let pads = &report.virtual_pads;
    assert_eq!(pads.count, pads.pads.len());
    if pads.count > 0 {
        assert!(
            pads.bus_instance_id.is_some(),
            "children can only be enumerated via the bus devnode"
        );
    }

    // Verdicts derive without panicking and serialize.
    let advice = summarize(&report);
    serde_json::to_value(&advice).expect("advice serializes");
}

/// The WinUSB survey against the live device tree.
///
/// Read-only by construction: `CM_Get_Device_ID_List` plus registry reads.
/// Nothing here opens a device handle, and a claim is never planned — this test
/// exists to prove the *enumerator* works on a real machine, which no synthetic
/// tree can.
#[test]
fn winusb_survey_runs_and_is_internally_consistent() {
    use ksx_platform::winusb::{self, ClaimState};

    let survey = winusb::survey();

    // Every candidate's state must agree with the evidence it was derived from.
    for c in &survey.candidates {
        match c.state {
            ClaimState::Claimed => {
                assert!(
                    c.interface.service_is(winusb::WINUSB_SERVICE),
                    "{} reported claimed but its service is {:?}",
                    c.interface.instance_id,
                    c.interface.service
                );
            }
            ClaimState::Claimable => {
                assert!(
                    c.keyboard.is_some(),
                    "{} is claimable with no keyboard child",
                    c.interface.instance_id
                );
                assert!(c.keyboard.as_ref().unwrap().is_keyboard_class());
            }
            ClaimState::NotAKeyboard | ClaimState::ForeignDriver => {
                assert!(c.keyboard.is_none(), "{}", c.interface.instance_id);
            }
            // A Bluetooth keyboard: it HAS a keyboard node (that is the whole
            // point — Interception can capture it) and no USB interface for a
            // claim to bind.
            ClaimState::InterceptionOnly => {
                assert!(
                    c.keyboard.is_some(),
                    "{} is interception-only with no keyboard node",
                    c.interface.instance_id
                );
                assert_eq!(c.transport, winusb::Transport::Bluetooth);
                assert!(!c.transport.can_winusb());
            }
        }
        match c.transport {
            winusb::Transport::Usb => {
                assert!(c.interface.enumerator.eq_ignore_ascii_case("USB"))
            }
            winusb::Transport::Bluetooth => {
                assert!(c.interface.enumerator.eq_ignore_ascii_case("BTHENUM"))
            }
        }
        assert!(!c.ksx_device_id().is_empty());
    }

    // Every keyboard a candidate claims must be in the machine's keyboard list.
    for c in &survey.candidates {
        if let Some(kb) = &c.keyboard {
            assert!(survey.keyboards.iter().any(|k| &k.node == kb));
        }
    }
    // A keyboard cannot be attributed to two interfaces.
    let mut attributed: Vec<&str> = survey
        .candidates
        .iter()
        .filter_map(|c| c.keyboard.as_ref().map(|k| k.instance_id.as_str()))
        .collect();
    let before = attributed.len();
    attributed.sort_unstable();
    attributed.dedup();
    assert_eq!(before, attributed.len(), "a keyboard was double-attributed");

    // JSON shape, for `ksx winusb status --json` consumers.
    let json = survey.to_json();
    assert!(json["candidates"].is_array());
    // The count is usable BOARDS, not rows: a claimed, disabled or
    // paired-but-disconnected keyboard is present and cannot type, and two
    // collections of one board are one keyboard (see `Survey::keyboard_count`).
    assert_eq!(
        json["keyboard_count"].as_u64().unwrap() as usize,
        survey.keyboard_count()
    );
    assert!(
        survey.keyboard_count() <= survey.keyboards.len(),
        "the refusal must never count more keyboards than the machine has rows for"
    );
}

/// The bundled ViGEmBus installer, verified the way `ksx install-drivers` does
/// it: sealed handle, both pins, real `WinVerifyTrust`.
///
/// This is the accepted-with-timestamp case in the flesh. Its certificate
/// expired 2025-02-16 and it is still installable, because a countersignature
/// dates the signature to 2023 — inside the window. Nothing here executes
/// anything; `verify_sealed` only reads.
///
/// The last assertion is the load-bearing one. Anyone can write a test that
/// says "the bundle passes"; it would keep passing if the timestamp check were
/// deleted and expired certificates were waved through wholesale. So the same
/// signature is re-judged with the countersignature removed, and must flip to
/// refused. That is what proves the acceptance came from the timestamp.
#[test]
fn the_bundled_installer_is_accepted_because_of_its_timestamp() {
    use ksx_platform::installer::{self, SignatureVerdict};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../drivers")
        .join(installer::INSTALLER_FILE_NAME);
    if !path.is_file() {
        // A source checkout without the release asset. `install-drivers`
        // reports `installer-missing` for this, which is its own tested state.
        return;
    }

    let (_sealed, verification) = installer::seal_and_verify(&path).expect("the bundle seals");
    assert!(
        verification.is_trusted(),
        "the committed bundle must be installable: {:?}",
        verification.failures()
    );
    assert_eq!(
        verification.signature_verdict.code(),
        "expired-timestamp-verified",
        "{:?}",
        verification.signature_verdict
    );

    let signature = verification.signature.clone().expect("Authenticode ran");
    assert_eq!(signature.cert_expired, Some(true));
    let timestamp = signature.timestamp.clone().expect("a countersignature");
    assert!(timestamp.is_verified(), "{timestamp:?}");

    // Re-derive the window comparison from the reported instants rather than
    // trusting the boolean that was computed from them. RFC 3339 UTC at fixed
    // width sorts lexicographically, so string ordering is date ordering.
    let signed_at = timestamp.signed_at_utc.as_deref().expect("a signing time");
    let not_before = signature.not_before_utc.as_deref().expect("NotBefore");
    let not_after = signature.not_after_utc.as_deref().expect("NotAfter");
    assert!(
        not_before <= signed_at && signed_at <= not_after,
        "signed {signed_at} outside [{not_before}, {not_after}]"
    );

    // And the proof that the timestamp is what carried it: take it away, and
    // the very same signature is refused.
    let mut undated = signature;
    undated.timestamp = None;
    let verdict = installer::judge_signature(Some(&undated));
    assert!(!verdict.accepted(), "{verdict:?}");
    assert!(
        matches!(verdict, SignatureVerdict::ExpiredNoValidTimestamp { .. }),
        "{verdict:?}"
    );
}

/// Ground truth on the cabinet, verified by hand 2026-08-03 (see
/// docs/research/keyboard-capture-2026.md §0). Never run in CI.
#[cfg(feature = "cab-tests")]
mod cab {
    use ksx_platform::{collect, summarize, CiPolicyMode, ServiceState, SignatureStatus};

    /// The I-PAC4 as this machine enumerates it, read-only. Pins the exact
    /// instance paths docs/RECOVERY.md quotes — if a board moves ports those
    /// paths change, and the runbook must be re-derived rather than trusted.
    #[test]
    fn the_ipac_keyboard_interface_is_visible_and_claimable() {
        use ksx_platform::winusb::{self, ClaimState};

        let survey = winusb::survey();
        let ipac = survey
            .resolve(r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000")
            .expect("the I-PAC keyboard interface is present");
        assert_eq!(ipac.state, ClaimState::Claimable);
        assert!(ipac.is_ultimarc());
        assert_eq!(ipac.interface.interface_number(), Some(0));
        assert_eq!(
            ipac.ksx_device_id(),
            r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000"
        );

        // MI_01 (system/consumer/mouse) and MI_02 (vendor) must never be
        // claimable: claiming MI_01 would take the trackball's mouse collection
        // with it.
        for iface in ["&MI_01\\", "&MI_02\\"] {
            let c = survey.resolve(iface).expect(iface);
            assert_eq!(c.state, ClaimState::NotAKeyboard, "{iface}");
        }

        // The refusal only holds if there really is another keyboard.
        assert!(
            survey.keyboard_count() >= 2,
            "the cabinet must keep a spare keyboard plugged in; found {}",
            survey.keyboard_count()
        );
    }

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
