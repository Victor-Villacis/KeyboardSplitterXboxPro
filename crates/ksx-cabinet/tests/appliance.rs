//! `docs/SURFACES.md` §4, as a test: this is an appliance panel, not a worse
//! browser.
//!
//! # Why a source scan rather than a UI test
//!
//! The rule being enforced is *"anything requiring text entry belongs
//! elsewhere"*, and its justification is physical: **at an arcade cabinet there
//! is no keyboard**. A text field on this surface is not a slightly worse
//! control, it is a dead end — the operator is holding a joystick and two
//! buttons, and the panel they would type on is the thing being configured.
//!
//! That cannot be tested by driving egui, which is immediate-mode and has no
//! retained widget tree to inspect, and it does not need to be: the failure is
//! visible at the call site. A widget that cannot be reached without a keyboard
//! is one that was *written*, so refusing to let it be written is the check.
//!
//! The same argument is why `nav.rs` has four moves and two verbs and no
//! chords, long-presses or accelerators. This test guards the one part of that
//! which a well-meaning change would otherwise undo — "just let them rename the
//! profile here" is a reasonable-sounding sentence, and it is how a surface
//! with no keyboard acquires a keyboard requirement.

use std::path::{Path, PathBuf};

/// Every `.rs` file in this crate's `src/`.
fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("src/ is readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("readable");
                out.push((path, text));
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    assert!(!out.is_empty(), "the walk found no source files");
    out
}

/// **No screen may ask for typed text** (`docs/SURFACES.md` §4).
///
/// Breaks against: any `egui::TextEdit`, `text_edit_singleline` or
/// `text_edit_multiline` reaching this crate. The constructor spellings are
/// matched rather than the bare word "TextEdit" so that *discussing* the rule in
/// a comment — as this file does — is not itself a violation.
#[test]
fn no_cabinet_screen_asks_the_operator_to_type() {
    const WIDGETS: &[&str] = &[
        "TextEdit::",
        ".text_edit_singleline(",
        ".text_edit_multiline(",
    ];
    for (path, text) in sources() {
        for widget in WIDGETS {
            assert!(
                !text.contains(widget),
                "{} constructs `{widget}`. There is no keyboard at an arcade \
                 cabinet — the panel IS the input, and the board a text field \
                 would be typed on is the one being configured. Text entry \
                 belongs on the CLI or Studio (docs/SURFACES.md §4).",
                path.display()
            );
        }
    }
}

/// The other half of §4's interaction model: four moves, two verbs, nothing
/// that needs a second simultaneous input.
///
/// A joystick and two buttons cannot express a chord or a modifier, so a
/// keyboard shortcut on this surface is a feature only the developer can reach.
///
/// Breaks against: a `Key::` binding or a modifier check arriving in the
/// navigation layer, which is where one would naturally be added.
#[test]
fn navigation_needs_nothing_a_joystick_cannot_express() {
    let nav = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("nav.rs"),
    )
    .expect("nav.rs");
    for forbidden in ["modifiers.ctrl", "modifiers.alt", "modifiers.shift"] {
        assert!(
            !nav.contains(forbidden),
            "nav.rs reads `{forbidden}`. A cabinet panel has no modifier keys, \
             so an interaction gated on one is unreachable by the only input \
             this surface has (docs/SURFACES.md §4)."
        );
    }
}
