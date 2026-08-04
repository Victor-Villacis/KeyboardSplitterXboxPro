//! Fuzz surface 3 (PLAYBOOK "M3/M6 fuzzing", the M6 target): the WinUSB HID
//! path — report-descriptor parsing and raw NKRO/boot report decoding.
//!
//! Entry points: `hid::descriptor::parse` (descriptor bytes come from a
//! `GET_DESCRIPTOR(Report)` control transfer — the device can say anything)
//! and `hid::ReportReader::feed` (interrupt-endpoint bytes — the threat model
//! is hardware-supplied). Both are pure already; no driver I/O is touched
//! here.
//!
//! Invariants: never panic; a `Rollover`/`Foreign` report emits nothing;
//! usage 0 and the error usages 0x01–0x03 are never emitted and never enter
//! the held-state set; each usage transitions at most once per report; press
//! events are bounded by the report's bit count; `release_all` empties the
//! state and is idempotent.

use ksx_capture::hid::usage::{is_error_usage, usage_to_scancode, usage_to_stroke};
use ksx_capture::hid::{descriptor, fixtures, ReportKind, ReportReader};
use ksx_fuzz::mutated_bytes;
use proptest::prelude::*;

/// The parser's own ceiling (`descriptor::MAX_PAYLOAD_LEN` = one USB
/// interrupt transfer, 1024 bytes) plus the report-ID byte. Asserted below:
/// `parse` must never hand back a format that asks for a bigger buffer.
const MAX_SANE_REPORT_LEN: usize = 1024 + 1;

fn descriptor_seeds() -> Vec<Vec<u8>> {
    [
        fixtures::BOOT_KEYBOARD_DESCRIPTOR,
        fixtures::NKRO_DESCRIPTOR,
        fixtures::KEYBOARD_PLUS_CONSUMER_DESCRIPTOR,
        fixtures::PUSH_POP_DESCRIPTOR,
        fixtures::MOUSE_DESCRIPTOR,
    ]
    .iter()
    .map(|d| d.to_vec())
    .collect()
}

fn report_seeds() -> Vec<Vec<u8>> {
    vec![
        fixtures::boot_report(0, &[0x04]).to_vec(),
        fixtures::boot_report(0xFF, &[0x04, 0x05, 0x06, 0x07, 0x08, 0x09]).to_vec(),
        fixtures::boot_rollover(0b0000_0001).to_vec(),
        fixtures::nkro_report(0xFF, &(0x04..=0x1D).collect::<Vec<_>>()),
        fixtures::nkro_report(0, &[]),
        vec![0xFF; 64],
    ]
}

proptest! {
    #![proptest_config(ksx_fuzz::persisting("regressions-hid-report.txt"))]

    /// Descriptor parsing is total: arbitrary bytes → some `KeyboardFormat`,
    /// possibly empty, never a panic — and a reader over whatever parsed
    /// survives a probe report.
    #[test]
    fn descriptor_parse_never_panics(bytes in mutated_bytes(descriptor_seeds(), 2048)) {
        let format = descriptor::parse(&bytes);
        prop_assert!(
            format.max_report_len() <= MAX_SANE_REPORT_LEN,
            "parse honored a report no keyboard can send ({} bytes)",
            format.max_report_len(),
        );
        let _ = format.is_empty();
        let mut reader = ReportReader::new(format);
        reader.feed(&[0u8; 64], |_, _| {});
    }

    /// Feed a stream of arbitrary reports through a reader built from an
    /// arbitrary (mutated-real) descriptor and check every output invariant.
    #[test]
    fn feed_never_panics_and_never_invents_keys(
        desc in mutated_bytes(descriptor_seeds(), 512),
        reports in proptest::collection::vec(mutated_bytes(report_seeds(), 128), 1..24),
    ) {
        let format = descriptor::parse(&desc);
        // No prop_assume guard: the parser itself caps declared report size
        // now, so every parse result must be cheap enough to fuzz through.
        prop_assert!(format.max_report_len() <= MAX_SANE_REPORT_LEN);
        let mut reader = ReportReader::new(format);

        for report in &reports {
            let mut events: Vec<(u8, bool)> = Vec::new();
            let kind = reader.feed(report, |usage, down| events.push((usage, down)));

            if matches!(kind, ReportKind::Rollover | ReportKind::Foreign) {
                prop_assert!(events.is_empty(), "{kind:?} must emit nothing");
            }
            let mut seen = [false; 256];
            let mut downs = 0usize;
            for &(usage, down) in &events {
                prop_assert!(usage != 0, "usage 0 ('no event') emitted as a key");
                prop_assert!(!is_error_usage(usage), "error usage {usage:#04x} emitted");
                prop_assert!(
                    !seen[usage as usize],
                    "usage {usage:#04x} transitioned twice in one report"
                );
                seen[usage as usize] = true;
                if down {
                    downs += 1;
                    // The downstream translation must be total as well.
                    let _ = usage_to_stroke(usage, down);
                }
            }
            prop_assert!(
                downs <= report.len().saturating_mul(8),
                "{downs} presses out of a {}-byte report",
                report.len()
            );

            // The held-state set obeys the same exclusions as the events.
            prop_assert!(!reader.state().contains(0));
            for code in 1..=3u8 {
                prop_assert!(!reader.state().contains(code));
            }
            prop_assert!(reader.state().len() <= 256);
        }

        // Unplug semantics: everything held is released exactly once.
        let held = reader.state().len();
        let mut released = Vec::new();
        reader.release_all(|usage, down| released.push((usage, down)));
        prop_assert_eq!(released.len(), held);
        prop_assert!(released.iter().all(|&(_, down)| !down));
        prop_assert!(reader.state().is_empty());
        let mut again = 0usize;
        reader.release_all(|_, _| again += 1);
        prop_assert_eq!(again, 0, "release_all must be idempotent");
    }
}

/// The usage → scancode tables are total over the whole page (exhaustive, not
/// probabilistic — 256 values is cheaper than one proptest case).
#[test]
fn usage_translation_is_total_over_the_whole_page() {
    for usage in 0..=u8::MAX {
        let _ = usage_to_scancode(usage);
        let _ = usage_to_stroke(usage, true);
        let _ = usage_to_stroke(usage, false);
    }
}

/// Regression for the descriptor-declared-size DoS found by this fuzz on
/// 2026-08-04: a 13-byte descriptor declaring a 16M-bit bitmap used to make
/// `max_report_len()` request a 2 MiB claim-time buffer (512 MiB at the
/// 2^32-bit maximum) and every `feed` walk the declared range (~98 ms debug
/// per report, measured). `parse` now refuses any report bigger than one USB
/// interrupt transfer — the format comes back empty and the claim path
/// refuses the device, same as any other malformed descriptor.
#[test]
fn giant_declared_reports_are_refused_not_honored() {
    // Usage Page (Keyboard), Usage Min (0), Report Size (1),
    // Report Count (0x01000000), Input (Data,Var,Abs).
    let descriptor = [
        0x05, 0x07, // Usage Page (Keyboard/Keypad)
        0x19, 0x00, // Usage Minimum (0)
        0x75, 0x01, // Report Size (1)
        0x97, 0x00, 0x00, 0x00, 0x01, // Report Count (16 777 216)
        0x81, 0x02, // Input (Data, Var, Abs)
    ];
    let format = descriptor::parse(&descriptor);
    assert!(
        format.is_empty(),
        "a report no keyboard can send must be dropped, got max_report_len {}",
        format.max_report_len(),
    );
    // And the boundary holds: a descriptor at exactly the transfer ceiling
    // (1024 bytes = 8192 one-bit fields) is still honored.
    let at_cap = [
        0x05, 0x07, // Usage Page (Keyboard/Keypad)
        0x19, 0x00, // Usage Minimum (0)
        0x75, 0x01, // Report Size (1)
        0x96, 0x00, 0x20, // Report Count (8192)
        0x81, 0x02, // Input (Data, Var, Abs)
    ];
    let format = descriptor::parse(&at_cap);
    assert_eq!(format.max_report_len(), 1024, "the cap is inclusive");
}
