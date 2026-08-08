//! `packaging/ksx.iss`, as tests.
//!
//! # Why a Rust test crate reads an Inno Setup script
//!
//! Two reasons, and neither is tidiness.
//!
//! 1. **The installer is moment 2 of `docs/FIRST-RUN.md` §1, and nothing else
//!    in this repository fails when it regresses.** §4 states four things the
//!    installer must do; each is one word or one flag on one line, each was
//!    WRONG at the audit that produced that file, and each is the kind of edit
//!    made in passing while fixing something else. ISCC compiles every broken
//!    version happily — a setup.exe that hands a first-run user a diagnostic
//!    is not a build failure, it is a working build of the wrong product.
//! 2. **ISCC does not run on the machine this is developed on.** The
//!    `release-binary` CI job is the only compile check the script gets, and it
//!    runs after the whole test suite. These are the part of that check that
//!    runs in milliseconds, on any platform, beside the code the installer
//!    launches.
//!
//! They do NOT re-encode the file. They assert the four sentences of §4 and one
//! landmine from `CLAUDE.md`. Everything else about the script — compression,
//! the driver payload, the uninstall hook, every comment — is free to change
//! without touching this file.
//!
//! Line endings: the script is CRLF in a Windows checkout and LF in a fresh
//! clone elsewhere, so every line is `trim()`ed before it is read. A test that
//! compared against `"\n"` would pass here and fail on CI, which this
//! repository has already paid for (`CLAUDE.md`, "Windows/CRLF").

use std::path::{Path, PathBuf};

/// The repository root: this crate is `<root>/crates/ksx-app`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ksx-app is two levels below the repo root")
        .to_path_buf()
}

fn script() -> String {
    let path = repo_root().join("packaging").join("ksx.iss");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} could not be read: {err}", path.display()))
}

/// Every meaningful line of one `[Section]`, in order: comments and blanks
/// dropped, each line trimmed.
///
/// A `;` comment in an `.iss` occupies a whole line — Inno has no trailing
/// comment form outside `[Code]` — so dropping lines that begin with one is the
/// entire parser this needs.
fn section(text: &str, want: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            inside = line.eq_ignore_ascii_case(want);
            continue;
        }
        if inside {
            out.push(line.to_owned());
        }
    }
    assert!(!out.is_empty(), "{want} is missing or empty in ksx.iss");
    out
}

/// One entry line's `Key: value` fields, split on the semicolons that are not
/// inside a quoted value.
fn fields(entry: &str) -> Vec<(String, String)> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in entry.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ';' if !quoted => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    parts.push(current);

    parts
        .iter()
        .filter_map(|part| {
            let (key, value) = part.split_once(':')?;
            let value = value.trim();
            // Inno doubles an embedded quote; the outer pair is the delimiter.
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            Some((key.trim().to_owned(), value.to_owned()))
        })
        .collect()
}

/// The value of one field, or `None` if the entry does not carry it.
fn field(entry: &str, key: &str) -> Option<String> {
    fields(entry)
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

/// **`docs/FIRST-RUN.md` §4 bullet 2.** The post-install offer hands over the
/// product.
///
/// Fails against the audited version, whose only `[Run]` line was
/// `Parameters: "doctor"; Description: "Check drivers and hardware now (ksx
/// doctor)"`. A user who ticked the single checkbox the installer offers got a
/// console of driver tables — a developer verb — as their first sight of ksx.
#[test]
fn the_post_install_offer_opens_ksx_and_runs_no_diagnostic() {
    let text = script();
    let run = section(&text, "[Run]");
    let offers: Vec<&String> = run
        .iter()
        .filter(|line| {
            field(line.as_str(), "Flags").is_some_and(|flags| flags.contains("postinstall"))
        })
        .collect();
    assert_eq!(
        offers.len(),
        1,
        "exactly one post-install offer, or a first-run user is ranking checkboxes again: {run:?}"
    );
    let offer = offers[0];
    assert_eq!(
        field(offer, "Parameters").as_deref(),
        Some("open"),
        "the hand-off must be `ksx open` (FIRST-RUN.md §4 bullet 2): {offer}"
    );
    for line in &run {
        assert_ne!(
            field(line, "Parameters").as_deref(),
            Some("doctor"),
            "no [Run] entry may run the diagnostic; doctor lives on the advanced \
             Start-menu folder now: {line}"
        );
    }
}

/// **`docs/FIRST-RUN.md` §4 bullets 1 and 4.** The desktop icon is on by
/// default; PATH is not.
///
/// Fails against the audited version in both directions at once: `desktopicon`
/// carried `Flags: unchecked` — so declining the launch prompt left nothing on
/// screen and a Start menu to hunt through — and `addtopath` carried no flag at
/// all, so every install edited a machine-wide environment variable to buy a
/// customer who never opens a shell precisely nothing.
#[test]
fn the_desktop_icon_is_default_and_path_is_not() {
    let text = script();
    let tasks = section(&text, "[Tasks]");
    let find = |name: &str| -> String {
        tasks
            .iter()
            .find(|line| field(line.as_str(), "Name").as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no `{name}` task in ksx.iss: {tasks:?}"))
            .clone()
    };

    let desktop = find("desktopicon");
    assert!(
        !field(&desktop, "Flags")
            .unwrap_or_default()
            .contains("unchecked"),
        "the desktop icon must be checked by default (FIRST-RUN.md §4 bullet 1): {desktop}"
    );

    let path = find("addtopath");
    assert!(
        field(&path, "Flags")
            .unwrap_or_default()
            .contains("unchecked"),
        "PATH must stay opt-in and unchecked (FIRST-RUN.md §4 bullet 4): {path}"
    );
}

/// **`docs/FIRST-RUN.md` §4 bullet 3.** One Start-menu entry, and it is the
/// product.
///
/// Fails against the audited version, which put five names at the top level of
/// the Start-menu group — `ksx`, `ksx daemon (tray only)`, `ksx Studio (serve
/// only)`, `ksx cabinet`, `ksx setup wizard` — and gave a new user no way to
/// rank them.
///
/// It fails against the other wrong fix too: deleting the four. They stay
/// reachable without a shell, so this asserts they moved one level down rather
/// than out of the installer.
#[test]
fn the_start_menu_offers_one_thing_and_keeps_the_rest_one_level_down() {
    let text = script();
    let icons = section(&text, "[Icons]");
    let group: Vec<(String, String)> = icons
        .iter()
        .filter_map(|line| {
            let name = field(line.as_str(), "Name")?;
            let inside = name.strip_prefix("{group}\\")?.to_owned();
            Some((inside, line.clone()))
        })
        .collect();

    let top: Vec<&(String, String)> = group.iter().filter(|(n, _)| !n.contains('\\')).collect();
    assert_eq!(
        top.len(),
        1,
        "exactly ONE Start-menu entry at the top level (FIRST-RUN.md §4 bullet 3): {:?}",
        top.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    let (name, line) = top[0];
    assert_eq!(
        name.as_str(),
        "{#AppName}",
        "the one entry is ksx itself, not a verb: {line}"
    );
    assert_eq!(
        field(line, "Parameters").as_deref(),
        Some("open"),
        "and it opens the app rather than starting a daemon behind a tray icon \
         (docs/M9-DECISION.md §4 item 1): {line}"
    );

    // The verbs are not deleted. Every other surface keeps a shortcut one level
    // down, which for a user who never opens a shell is the only place it
    // exists at all.
    let nested: Vec<&String> = group
        .iter()
        .filter(|(n, _)| n.contains('\\'))
        .map(|(_, line)| line)
        .collect();
    for verb in ["daemon", "studio", "cabinet", "setup"] {
        assert!(
            nested
                .iter()
                .any(|line| field(line.as_str(), "Parameters").as_deref() == Some(verb)),
            "`ksx {verb}` must keep a shortcut in the advanced folder, not lose one: {nested:?}"
        );
    }

    // And the desktop icon, when its task is taken, is the same app.
    let desktop = icons
        .iter()
        .find(|line| {
            field(line.as_str(), "Name").is_some_and(|name| name.starts_with("{autodesktop}"))
        })
        .expect("a desktop icon entry");
    assert_eq!(
        field(desktop, "Parameters").as_deref(),
        Some("open"),
        "the desktop icon and the Start-menu entry are the same act: {desktop}"
    );
    assert_eq!(
        field(desktop, "Tasks").as_deref(),
        Some("desktopicon"),
        "the desktop icon stays tied to its task: {desktop}"
    );
}

/// **The `CLAUDE.md` landmine, as an assertion.** No Pascal `{ }` comment in
/// `[Code]`.
///
/// Fails against the version that shipped one. Pascal Script ends a brace
/// comment at the FIRST `}`, so a comment explaining what `{app}` means closes
/// four characters in and the rest of the sentence is compiled as code. The
/// symptom is an ISCC syntax error pointing at prose, and it cost this file its
/// first compile.
///
/// The scan tracks state rather than banning the character, because both other
/// uses are legitimate and present: `{` inside a single-quoted string
/// (`ExpandConstant('{app}')`) and `{` inside a `//` comment — which is how the
/// note warning about all this is written.
#[test]
fn the_code_section_uses_line_comments_only() {
    let text = script();
    for line in section(&text, "[Code]") {
        let chars: Vec<char> = line.chars().collect();
        let mut in_string = false;
        for (index, c) in chars.iter().enumerate() {
            match c {
                '\'' => in_string = !in_string,
                // The rest of the line is a `//` comment: stop reading.
                '/' if !in_string && chars.get(index + 1) == Some(&'/') => break,
                '{' if !in_string => panic!(
                    "a `{{ }}` comment in [Code] ends at the FIRST `}}` — use `//` \
                     instead (CLAUDE.md; this has broken ksx.iss once): {line}"
                ),
                _ => {}
            }
        }
    }
}
