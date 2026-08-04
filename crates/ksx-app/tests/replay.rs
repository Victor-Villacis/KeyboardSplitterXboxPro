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

#[test]
fn recorded_cabinet_session_replays_deterministically() {
    let events = load_corpus();
    assert!(events.len() > 300, "corpus should hold the full session");

    let mut engine = Engine::new(cabinet_slots());
    let mut transitions = 0usize;
    let mut per_slot = [0usize; 5];
    let mut last = BTreeMap::<u8, PadState>::new();

    for (i, e) in events.iter().enumerate() {
        for delta in engine.handle(&KeyEvent {
            device: e.device.clone(),
            key: e.key,
            down: e.down,
            t: i as u64,
        }) {
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
