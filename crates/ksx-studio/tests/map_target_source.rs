//! Authored mapper destination contracts.
//!
//! Generated Forma assets are intentionally not inspected here: `map.ts` is
//! the source of the next release bundle, and these regressions must fail
//! before regeneration if an async write starts consulting the selected tab
//! again.

const MAP: &str = include_str!("../../../studio-ui/src/map.ts");

fn between(start: &str, end: &str) -> &'static str {
    let (_, tail) = MAP
        .split_once(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    tail.split_once(end)
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("missing {end} after {start}"))
}

#[test]
fn every_json_writer_requires_an_immutable_action_target() {
    assert!(MAP.contains("interface WriteTarget"));
    assert!(MAP.contains("function targetFields(target: WriteTarget)"));
    assert!(
        !MAP.contains("function targetFields():"),
        "a no-argument destination helper can silently follow a later tab switch"
    );

    for (start, end) in [
        (
            "async function bindOnce(",
            "/** Write a control's WHOLE key list",
        ),
        (
            "async function bindKeys(",
            "/** The number typed into the learn modal",
        ),
        ("async function macroWrite(", "/** The TOGGLE request"),
        (
            "async function macroSetEnabled(",
            "/** A refusal, in one sentence",
        ),
    ] {
        let writer = between(start, end);
        assert!(writer.contains("target: WriteTarget"), "{start}");
        assert!(writer.contains("targetFields(target)"), "{start}");
        assert!(!writer.contains("currentSlot()"), "{start}");
        assert!(!writer.contains("editingStage()"), "{start}");
    }
}

#[test]
fn delayed_and_multi_action_writes_keep_the_captured_target() {
    let learn = between("async function startLearn(", "async function cancelLearn(");
    assert!(learn.contains("learnWrite = {"));
    assert!(learn.contains("write.target"));
    assert!(learn.contains("write.before"));

    let batch = between("async function mapAll(", "/** Clear every selected control");
    assert!(batch.contains("bindOnce(target, fn, key, true)"));
    assert!(batch.contains("readWriteTarget(target)"));
    assert!(!batch.contains("bindOnce(slot.preset"));

    let clear = between(
        "async function clearSelectedBindings(",
        "/** Clear one control",
    );
    assert!(clear.contains("bindOnce(target, fn, null, false)"));
    assert!(
        !clear.contains("undo:"),
        "a partially successful multi-write cannot promise one-click Undo"
    );

    let conflict = between("async function saveBinding(", "/** FEATURE 2's write");
    assert!(conflict.contains("pendingWrite = { target, fn, before }"));
    assert!(conflict.contains("saveBinding(fn, key, true, target, before)"));
    assert!(
        conflict.contains("outcome.conflicts.every((c) => c.scope === \"preset\")"),
        "macro-trigger conflicts must reach the explicit decision dialog"
    );
}

#[test]
fn hostile_redirect_and_hydrated_toast_text_can_only_reach_authored_fallbacks() {
    let safe = between("function safeDetail(", "/** The daemon's chord advisory");
    assert!(safe.contains("return fallback;"), "{safe}");
    for hostile in [
        r"C:\\Users\\Victor\\secret",
        r"HID\\VID_D209&PID_0430",
        r"HKLM\\SYSTEM\\CurrentControlSet",
        "expected a sequence at line 4 column 9",
        r#"{\"verb\":\"map\",\"key\":\"A\"}"#,
    ] {
        assert!(
            !safe.contains(hostile),
            "the presentation boundary must not special-case one known diagnostic: {hostile}"
        );
    }

    let form_follow = between(
        "async function submitNoJsForm(",
        "// ── v11/v12: the macro editor's controls",
    );
    assert!(form_follow.contains("safeDetail(flash"), "{form_follow}");

    let hydration = between(
        "const flash = (query.get(\"flash\") ?? \"\").trim();",
        "wire(el);",
    );
    assert!(hydration.contains("safeDetail(flash"), "{hydration}");

    let chord = between("function chordAdvisory(", "/** Put one control back");
    assert!(chord.contains("one key in this chord"), "{chord}");
    for splice in ["slice(", "substring(", "split(", "${message}", "${note}"] {
        assert!(
            !chord.contains(splice),
            "chord feedback must not splice provider text via {splice}: {chord}"
        );
    }
}
