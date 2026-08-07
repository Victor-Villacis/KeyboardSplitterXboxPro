//! The design docs, as tests.
//!
//! # Why a test crate reads Markdown
//!
//! `docs/` is not reference material here — several files are *normative*, and
//! the code cites them by section number to explain why it refuses something.
//! That arrangement has two failure modes that no compiler catches:
//!
//! 1. **A renumber.** `docs/DEVICE-IDENTITY.md` is cited from twelve source
//!    files, around twenty-five times, by section number. Delete a section or
//!    insert one and every citation below it starts pointing at the wrong
//!    argument — silently, and only for the person reading the code at 2 a.m.
//! 2. **A doc drifting into a memo.** `docs/SURFACES.md` was written as a peer
//!    of `docs/CONTROL-SURFACE.md` and, at its first audit, had **zero**
//!    references anywhere in the repository while its peers had 8 to 106. A
//!    design document nothing points at is not consulted when the code it
//!    governs is written, and its rules get broken without anyone deciding to
//!    break them — which is exactly what that audit found.
//!
//! Neither test checks that a doc is *true*. Nothing can. They check the two
//! mechanical properties that keep a doc reachable, which is the precondition
//! for anyone noticing it is not true.
//!
//! The third test guards a specific promise rather than a mechanism, and it is
//! here because it is a documentation rule with a user-visible cost: the setup
//! docs must not teach someone to hand-author a device id.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The repository root: this crate is `<root>/crates/ksx-app`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ksx-app is two levels below the repo root")
        .to_path_buf()
}

/// This file, which is excluded from every scan below.
///
/// Not tidiness — correctness. These tests name the documents they check, in
/// prose, so a scan that included this file would find `SURFACES.md` cited by
/// the very test asserting that something cites it. It would pass for a
/// repository in exactly the state that motivated it.
const SELF: &str = "crates/ksx-app/tests/docs.rs";

/// Every `.rs` file under `crates/`, with its repo-relative path.
///
/// A hand-rolled walk rather than a dev-dependency: this crate's dev-deps are
/// part of what `docs/GATES.md` checks, and a directory walk is eleven lines.
fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string()
                    .replace('\\', "/");
                if name == SELF {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((name, text));
                }
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    walk(&root.join("crates"), &root, &mut out);
    assert!(
        out.len() > 50,
        "the source walk found {} files, which means it walked the wrong tree",
        out.len()
    );
    out
}

fn read_doc(name: &str) -> String {
    let path = repo_root().join("docs").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()))
}

/// Every `§N` cited within a few characters of `doc`'s filename, and who cites
/// it.
///
/// The window is deliberately tiny. Citations are written exactly two ways —
/// ``docs/DEVICE-IDENTITY.md` §6`` and `docs/DEVICE-IDENTITY.md §6)` — so three
/// characters is enough to find the `§`, and short enough that a section number
/// belonging to a *different* document mentioned in the same sentence cannot be
/// picked up by accident.
fn citations(doc: &str) -> BTreeMap<u32, BTreeSet<String>> {
    let mut found: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (file, text) in source_files() {
        for (at, _) in text.match_indices(doc) {
            let tail = &text[at + doc.len()..];
            let head: String = tail.chars().take(3).collect();
            let Some(rel) = head.find('§') else { continue };
            let digits: String = tail[rel + '§'.len_utf8()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(section) = digits.parse::<u32>() {
                found.entry(section).or_default().insert(file.clone());
            }
        }
    }
    found
}

/// **The renumber guard.**
///
/// `docs/DEVICE-IDENTITY.md`'s own status block calls its section numbers
/// "load-bearing" and used to name one file as the reason. It is twelve files.
/// Renumbering, or deleting a section, leaves every citation below it pointing
/// at an argument that is no longer there — and a citation is the only thing
/// explaining why several refusals are worded the way they are.
///
/// Breaks against: deleting a cited section, renumbering the file, or demoting
/// a `## §N` heading to plain text.
#[test]
fn every_section_number_the_code_cites_still_exists_in_device_identity() {
    let doc = read_doc("DEVICE-IDENTITY.md");
    let headings: BTreeSet<u32> = doc
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("## §")?;
            rest.split_whitespace()
                .next()
                .and_then(|n| n.parse::<u32>().ok())
        })
        .collect();
    assert!(
        headings.len() >= 5,
        "the heading scan found {headings:?}, which means the format changed"
    );

    let cited = citations("DEVICE-IDENTITY.md");
    assert!(
        cited.len() >= 5,
        "the citation scan found {cited:?} — if the code really stopped citing \
         this document, that is the finding, not a passing test"
    );
    for (section, files) in &cited {
        assert!(
            headings.contains(section),
            "code cites docs/DEVICE-IDENTITY.md §{section}, which has no `## §{section}` \
             heading. Cited from:\n  {}\nHeadings present: {headings:?}",
            files.iter().cloned().collect::<Vec<_>>().join("\n  ")
        );
    }
}

/// **The memo guard.**
///
/// Every doc listed here states rules the code is supposed to follow. A rule
/// nobody is pointed at while writing the code is not enforced by anything —
/// and `SURFACES.md` proved it: written as a peer of `CONTROL-SURFACE.md`,
/// cited nowhere, and by its first audit four cells of its capability matrix
/// described capabilities that did not exist.
///
/// One citation is a low bar on purpose. This is not "cite the doc everywhere";
/// it is "a governing document must have at least one hook into the code it
/// governs", which is the difference between a design doc and a memo.
///
/// Breaks against: adding a normative doc with no call-site reference, or
/// deleting the last reference to one — the state `SURFACES.md` shipped in.
#[test]
fn every_governing_doc_is_cited_from_the_code_it_governs() {
    // Normative documents only. Runbooks and narrative (QUICKSTART, RECOVERY,
    // PLAYBOOK) are addressed to a human and are not on trial here.
    const GOVERNING: &[&str] = &[
        "ARCHITECTURE.md",
        "CONTROL-SURFACE.md",
        "DEVICE-IDENTITY.md",
        "ENHANCEMENTS.md",
        "INPUT-TRANSFORMS.md",
        "M9-DECISION.md",
        "MAPPER-UX.md",
        "SURFACES.md",
    ];

    let sources = source_files();
    for doc in GOVERNING {
        assert!(
            repo_root().join("docs").join(doc).exists(),
            "docs/{doc} is listed as governing but does not exist"
        );
        let citing: Vec<&str> = sources
            .iter()
            .filter(|(_, text)| text.contains(doc))
            .map(|(file, _)| file.as_str())
            .take(3)
            .collect();
        assert!(
            !citing.is_empty(),
            "no source file mentions docs/{doc}. A design document with no hook \
             into the code it governs is a memo: nobody reads it while writing \
             the thing it constrains, and its rules get broken without anyone \
             deciding to break them. Cite it where it applies, or move it out \
             of docs/ and say it is history."
        );
    }
}

/// **The one documentation rule with a user-visible cost.**
///
/// `docs/DEVICE-IDENTITY.md` §1: every path that teaches a user to paste a raw
/// device instance path "teaches them to hand-author a value they have no way
/// to know" — it is one specific board, in one specific socket, on one specific
/// machine. `ksx device pick` exists precisely so nobody has to, and it landed
/// while both of these pages still showed a devnode with no mention of it.
///
/// The rule is deliberately not "no devnode may appear". A migration page has
/// to show the id it is migrating *from*, and every config written before
/// selectors existed holds one. The rule is that the page cannot show one
/// without also naming the command that writes it for you.
///
/// Breaks against: the state both files shipped in, and against a future edit
/// that adds a hand-pasted example to either.
#[test]
fn the_setup_docs_name_the_picker_wherever_they_show_a_devnode() {
    for name in ["QUICKSTART.md", "MIGRATION-WINUSB.md"] {
        let text = read_doc(name);
        let shows_a_devnode = text
            .lines()
            .any(|line| line.contains("id ") && line.contains('=') && line.contains(r"VID_"));
        if !shows_a_devnode {
            continue;
        }
        assert!(
            text.contains("ksx device pick"),
            "docs/{name} shows a literal device instance path as an `id` but never \
             names `ksx device pick`. That teaches a reader to hand-author a value \
             they cannot look up — it is one person's board in one socket \
             (docs/DEVICE-IDENTITY.md §1)."
        );
        assert!(
            text.contains("usb:d209:0430:00"),
            "docs/{name} should show the `usb:` selector beside the devnode, since \
             that is what the picker writes and what a reader should copy"
        );
    }
}
