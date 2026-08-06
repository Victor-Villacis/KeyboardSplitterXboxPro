//! Replay oracle (docs/PLAYBOOK.md "Test oracle strategy").
//!
//! Feeds a *recorded real hardware session* — the cabinet's I-PAC4 driving all
//! four player panels, captured by `ksx monitor --record` — through the real
//! presets imported from the legacy XML, and pins the resulting pad-state
//! transitions. Any future change to the engine, key vocabulary, or importer
//! that alters what the cabinet would have felt fails here, in CI, with no
//! hardware attached.
//!
//! Corpus: `crates/ksx-capture/tests/corpus/ipac-session-001.jsonl`
//! (392 events, 312 from the I-PAC, 51 distinct keys, downs == ups).

use std::collections::BTreeMap;
use std::path::PathBuf;

use ksx_core::{DeviceId, Engine, Key, KeyEvent, PadState, ResolvedSlot, SlotSpec};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

struct Event {
    device: DeviceId,
    key: Key,
    down: bool,
}

fn load_corpus() -> Vec<Event> {
    let path = repo_root().join("crates/ksx-capture/tests/corpus/ipac-session-001.jsonl");
    let text = std::fs::read_to_string(&path).expect("corpus present");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            // Deliberately hand-parsed: the corpus format is a stable contract
            // (documented in ksx-app/src/monitor.rs), not a serde type.
            let field = |name: &str| -> String {
                let key = format!("\"{name}\":");
                let rest = &line[line.find(&key).expect("field") + key.len()..];
                let rest = rest.trim_start();
                if let Some(stripped) = rest.strip_prefix('"') {
                    let end = stripped.find('"').expect("closing quote");
                    stripped[..end].replace("\\\\", "\\")
                } else {
                    rest.split([',', '}']).next().unwrap().trim().to_string()
                }
            };
            Event {
                device: DeviceId::from(field("device")),
                key: Key::from_name(&field("key")).expect("key name in ksx-core vocabulary"),
                down: field("down") == "true",
            }
        })
        .collect()
}

/// The cabinet's real slot layout: one I-PAC feeding four slots with the
/// disjoint IPAC P1..P4 presets (games.toml, imported from splitter_games.xml).
fn cabinet_slots() -> Vec<ResolvedSlot> {
    let fixtures = repo_root().join("crates/ksx-legacy-import/tests/fixtures");
    let import = ksx_legacy_import::import_dir(&fixtures).expect("import fixtures");
    assert!(
        import.report.warnings.is_empty(),
        "fixture import must stay warning-free: {:?}",
        import.report.warnings
    );
    let ipac = DeviceId::from("HID\\VID_D209&PID_0430&REV_0056&MI_00");
    let by_name: BTreeMap<String, _> = import
        .presets
        .iter()
        .map(|p| (p.name.clone(), p.to_core().expect("preset converts")))
        .collect();

    (1..=4u8)
        .map(|n| {
            let preset = by_name
                .get(&format!("IPAC P{n}"))
                .unwrap_or_else(|| panic!("preset IPAC P{n} in fixtures"))
                .clone();
            ResolvedSlot {
                spec: SlotSpec::new(n, Some(ipac.clone()), None, preset.name.clone())
                    .expect("valid slot"),
                preset,
            }
        })
        .collect()
}

/// FNV-1a over the exact XInput wire fields of the whole transition sequence.
///
/// Hand-rolled on purpose: `DefaultHasher` is explicitly not stable across Rust
/// releases, so it cannot back a golden value that has to survive toolchain
/// upgrades.
fn digest(transitions: &[(u8, PadState)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |byte: u8| {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for (slot, s) in transitions {
        eat(*slot);
        s.buttons
            .bits()
            .to_le_bytes()
            .iter()
            .copied()
            .for_each(&mut eat);
        eat(s.lt);
        eat(s.rt);
        for axis in [s.lx, s.ly, s.rx, s.ry] {
            axis.to_le_bytes().iter().copied().for_each(&mut eat);
        }
    }
    h
}

/// The golden: what the cabinet actually felt during the recorded session.
///
/// This is the "byte-identical pad-state sequences forever after" half of the
/// oracle promised in `docs/PLAYBOOK.md`. The structural assertions below
/// (fan-out, diffing, balanced end) all survive a change that swaps two button
/// bits or retargets a key — verified by mutation: swapping `XButton::A` and
/// `XButton::B` in `XButton::flag()` left the entire workspace test suite green.
/// This constant is what actually notices.
///
/// It pins *current* behaviour, so it is a drift detector, not a proof of
/// correctness-vs-legacy. If a deliberate mapping change lands, re-derive it and
/// say why in the commit message — never "just update the number".
///
/// Re-derived once, when `AXIS_MIN` became -32767 and `ksx-config` started
/// folding a literal `-32768` to it at the file boundary (`ksx_core::AXIS_MIN`
/// explains why `i16::MIN` is not wanted). The cabinet XML stores
/// `value="-32768"`, so 76 of the 311 recorded transitions now carry -32767
/// where they carried -32768. That is the *entire* delta: mapping -32767 back
/// to `i16::MIN` across the sequence reproduces the previous digest
/// (16_837_667_763_229_975_859) byte for byte — same transition count, same
/// order, same buttons, same triggers. One LSB less deflection, nothing else.
const SESSION_DIGEST: u64 = 1_218_103_173_477_976_965;
const SESSION_TRANSITIONS: usize = 311;

#[test]
fn recorded_cabinet_session_replays_deterministically() {
    let events = load_corpus();
    assert!(events.len() > 300, "corpus should hold the full session");

    let mut engine = Engine::new(cabinet_slots());
    let mut transitions = 0usize;
    let mut per_slot = [0usize; 5];
    let mut last = BTreeMap::<u8, PadState>::new();
    let mut sequence: Vec<(u8, PadState)> = Vec::new();

    for (i, e) in events.iter().enumerate() {
        for delta in engine.handle(&KeyEvent {
            device: e.device.clone(),
            key: e.key,
            down: e.down,
            t: i as u64,
        }) {
            sequence.push((delta.slot, delta.state));
            // Diffing contract: the engine never emits an unchanged state.
            assert_ne!(
                last.get(&delta.slot),
                Some(&delta.state),
                "slot {} re-emitted an identical state at event {i}",
                delta.slot
            );
            last.insert(delta.slot, delta.state);
            per_slot[delta.slot as usize] += 1;
            transitions += 1;
        }
    }

    // Every player panel drove its own pad — the one-keyboard-to-many-slots
    // fan-out this whole project exists for.
    for slot in 1..=4u8 {
        assert!(
            per_slot[slot as usize] > 0,
            "slot {slot} saw no transitions; fan-out is broken"
        );
    }
    assert!(
        transitions > 100,
        "expected a busy session, got {transitions}"
    );

    // The oracle proper: the exact wire-level sequence the cabinet produced.
    assert_eq!(
        (transitions, digest(&sequence)),
        (SESSION_TRANSITIONS, SESSION_DIGEST),
        "the recorded session no longer replays to the same pad states. Something \
         changed what the cabinet would feel: a key mapping, a button bit, the \
         all-keys-up release rule, the opposite-axis snap, or the importer. If the \
         change was deliberate, re-derive the constants and justify it in the commit \
         message."
    );

    // The session ended with every key released, so every pad must be neutral.
    let ipac = DeviceId::from("HID\\VID_D209&PID_0430&REV_0056&MI_00");
    let residual = engine.release_device(&ipac);
    assert!(
        residual.is_empty(),
        "keys were left stuck after a balanced session: {residual:?}"
    );
    for (slot, state) in &last {
        assert_eq!(
            *state,
            PadState::default(),
            "slot {slot} did not return to neutral"
        );
    }
}

/// M6: the recorded corpus must stay replayable on the **WinUSB** backend too.
///
/// The oracle above proves the engine still feels the same given `KeyEvent`s.
/// It says nothing about whether the WinUSB capture path can still *produce*
/// those events — that path reaches `Key` by a completely different route
/// (HID usage → set-1 scancode → `corrected_key`) than Interception does.
///
/// A key that this cabinet really pressed but that no HID usage maps to would
/// be silently unreachable after the rebind: the button would go dead, the
/// preset bound to it would never fire, and nothing in CI would notice —
/// exactly the "presets break on one backend only" failure the usage table was
/// written to prevent. So: every key in the recorded session must be the image
/// of some keyboard-page usage.
#[test]
fn every_key_the_cabinet_really_pressed_is_reachable_from_a_hid_usage() {
    use ksx_capture::hid::usage_to_scancode;
    use ksx_capture::keymap::{corrected_key, KEY_DOWN};

    // Every `Key` the WinUSB backend can ever report.
    let reachable: std::collections::BTreeSet<Key> = (0u8..=0xFF)
        .filter_map(usage_to_scancode)
        .map(|(code, prefix)| corrected_key(code, prefix | KEY_DOWN))
        .collect();

    let recorded: std::collections::BTreeSet<Key> = load_corpus().iter().map(|e| e.key).collect();
    assert!(!recorded.is_empty(), "corpus loaded");

    let unreachable: Vec<&Key> = recorded.difference(&reachable).collect();
    assert!(
        unreachable.is_empty(),
        "these keys were pressed on the real cabinet but no HID usage maps to \
         them, so they would go dead on the WinUSB backend: {unreachable:?}"
    );
}
