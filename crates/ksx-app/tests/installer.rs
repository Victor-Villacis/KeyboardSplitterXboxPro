//! `packaging/ksx.iss` and the release workflow, as tests.
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
//! # And why it also reads `.github/workflows/`
//!
//! Moment 2 is only reachable through moment 1: "a `.exe` from the releases
//! page. One file." The installer this file guards is built by
//! `build-installer.yml` and published by `release.yml`, and **both of those
//! run on a runner that no local command reproduces** — `release.yml` fires on
//! a tag push and on nothing else, so an ordinary branch push never executes a
//! line of it. Its first execution is a real release, of a real version number,
//! for real customers, and a version number spent on a failed run is spent.
//!
//! So the parts of it that can be checked without running it, are:
//!
//! - the version in `ksx.iss` and the version in `Cargo.toml` agree, which is
//!   the precondition the tag has to satisfy;
//! - the trigger really is a pushed tag, in a pattern this repo's own version
//!   can produce;
//! - the file that gets attached is the installer `ksx.iss` actually emits;
//! - the release body says how to verify the download, and what the unsigned
//!   installer's SmartScreen dialog is.
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

fn read(relative: &str) -> String {
    let path = repo_root().join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} could not be read: {err}", path.display()))
}

fn workflow(name: &str) -> String {
    read(&format!(".github/workflows/{name}"))
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

/// The value of a `#define NAME "value"` line.
fn define(text: &str, name: &str) -> String {
    text.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("#define")?.trim_start();
            let rest = rest.strip_prefix(name)?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            Some(rest.trim().trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| panic!("ksx.iss has no `#define {name}`"))
}

/// `{#AppName}-{#AppVersion}-setup` → `ksx-0.1.0-setup`.
fn expand(text: &str, value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find("{#") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        let end = tail
            .find('}')
            .unwrap_or_else(|| panic!("unterminated `{{#` in ksx.iss: {value}"));
        out.push_str(&define(text, &tail[..end]));
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The value of one `Key=Value` line in `[Setup]`, with `{#defines}` expanded.
fn setup_value(text: &str, key: &str) -> String {
    let line = section(text, "[Setup]")
        .into_iter()
        .find(|line| {
            line.split_once('=')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        })
        .unwrap_or_else(|| panic!("[Setup] has no {key}"));
    let (_, value) = line.split_once('=').expect("matched above");
    expand(text, value.trim())
}

/// `[workspace.package] version` from the workspace manifest.
fn workspace_version() -> String {
    let manifest = read("Cargo.toml");
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[workspace.package]";
            continue;
        }
        if inside {
            if let Some(value) = line
                .strip_prefix("version")
                .and_then(|rest| rest.trim_start().strip_prefix('='))
            {
                return value.trim().trim_matches('"').to_owned();
            }
        }
    }
    panic!("Cargo.toml has no [workspace.package] version")
}

// ---------------------------------------------------------------------------
// A YAML subset, which is not a YAML parser
// ---------------------------------------------------------------------------
//
// Block mappings, block sequences, and `- key: value` step lists: everything
// the three workflow files in this repository use, and nothing else. It is here
// so the tests below can assert STRUCTURE — "the only trigger is a tag push" —
// rather than substrings — "the file contains the word tags". The second kind
// passes against a workflow that mentions tags in a comment and publishes on
// every push to master.
//
// A dev-dependency on a YAML crate would be the other way to get this. It is
// forty lines against a new dependency in the crate `docs/GATES.md` watches, on
// a file format three files use, so: forty lines.

/// A line's leading-space count.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// One line with any trailing `# comment` removed. Quotes are tracked, so a `#`
/// inside a string stays.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    for (at, byte) in bytes.iter().enumerate() {
        match byte {
            b'\'' | b'"' => match quote {
                Some(open) if open == *byte => quote = None,
                None => quote = Some(*byte),
                _ => {}
            },
            b'#' if quote.is_none() && (at == 0 || bytes[at - 1] == b' ') => {
                return line[..at].trim_end();
            }
            _ => {}
        }
    }
    line.trim_end()
}

/// A workflow's meaningful lines: blanks and comments gone, indentation kept.
fn yaml_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(strip_comment)
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// The lines nested under a key path, e.g. `["on", "push"]`.
fn yaml_block(lines: &[String], path: &[&str]) -> Vec<String> {
    let mut current = lines.to_vec();
    for key in path {
        let base = current
            .iter()
            .map(|line| indent_of(line))
            .min()
            .unwrap_or_else(|| panic!("nothing is nested under {path:?}"));
        let at = current
            .iter()
            .position(|line| {
                indent_of(line) == base
                    && line
                        .trim_start()
                        .strip_prefix(key)
                        .is_some_and(|rest| rest.starts_with(':'))
            })
            .unwrap_or_else(|| panic!("no `{key}:` where {path:?} expects one"));
        current = current[at + 1..]
            .iter()
            // A sequence may sit at its key's own indentation, so `- ` counts as
            // inside the block too.
            .take_while(|line| {
                indent_of(line) > base
                    || (indent_of(line) == base && line.trim_start().starts_with("- "))
            })
            .cloned()
            .collect();
    }
    current
}

/// The keys of the mapping at the top level of `lines`.
fn yaml_keys(lines: &[String]) -> Vec<String> {
    let Some(base) = lines.iter().map(|line| indent_of(line)).min() else {
        return Vec::new();
    };
    lines
        .iter()
        .filter(|line| indent_of(line) == base)
        .filter_map(|line| {
            let line = line.trim_start();
            if line.starts_with("- ") {
                return None;
            }
            Some(line.split_once(':')?.0.trim().to_owned())
        })
        .collect()
}

/// The items of the sequence at the top level of `lines`, unquoted.
fn yaml_items(lines: &[String]) -> Vec<String> {
    let Some(base) = lines.iter().map(|line| indent_of(line)).min() else {
        return Vec::new();
    };
    lines
        .iter()
        .filter(|line| indent_of(line) == base)
        .filter_map(|line| line.trim_start().strip_prefix("- "))
        .map(|item| item.trim().trim_matches(['\'', '"']).to_owned())
        .collect()
}

/// `(artifact name, path)` for every `actions/<verb>-artifact` step in a
/// workflow. `verb` is `upload` or `download`.
fn artifact_steps(text: &str, verb: &str) -> Vec<(String, String)> {
    let lines = yaml_lines(text);
    let marker = format!("uses: actions/{verb}-artifact");
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(&marker))
        .map(|(at, line)| {
            let base = indent_of(line);
            let body: Vec<&String> = lines[at + 1..]
                .iter()
                .take_while(|line| indent_of(line) > base)
                .collect();
            let value = |key: &str| -> String {
                body.iter()
                    .find_map(|line| {
                        line.trim_start()
                            .strip_prefix(key)?
                            .strip_prefix(':')
                            .map(|value| value.trim().trim_matches(['\'', '"']).to_owned())
                    })
                    .unwrap_or_default()
            };
            (value("name"), value("path"))
        })
        .collect()
}

/// `*` matches any run of characters. Nothing else is special — these are two
/// patterns out of two files, not a shell.
fn glob_matches(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => text.strip_prefix(head).is_some_and(|rest| {
            (0..=rest.len())
                .any(|at| rest.is_char_boundary(at) && glob_matches(tail, &rest[at..]))
        }),
    }
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

/// **Moment 7 has a driver under it, and the wizard asks.**
///
/// Fails against every version before this one, where the `[Tasks]` section had
/// two entries and neither was the driver. The bundled ViGEmBus setup shipped
/// to `{app}\drivers` and was never executed, so on a machine that has never
/// had ViGEmBus a first-run user reached Play, pressed it, and nothing plugged
/// — and the documented fix was `ksx install-drivers` from an elevated shell,
/// which `docs/FIRST-RUN.md` §7 rules out as an answer.
///
/// It fails in the other direction too. Installing a kernel driver without
/// asking is what `docs/DRIVERS.md` refuses, so this asserts a checkbox exists
/// AND that its label says what it does: "install drivers" is a phrase a
/// first-time user cannot rank, and a checkbox nobody understands is not
/// consent.
#[test]
fn the_bundled_driver_is_offered_checked_and_the_label_says_what_it_is() {
    let text = script();
    let tasks = section(&text, "[Tasks]");
    let task = tasks
        .iter()
        .find(|line| field(line.as_str(), "Name").as_deref() == Some("vigembus"))
        .unwrap_or_else(|| {
            panic!("no `vigembus` task: nothing in this installer installs the driver: {tasks:?}")
        });

    assert!(
        !field(task, "Flags")
            .unwrap_or_default()
            .contains("unchecked"),
        "the driver box is ticked by default — the whole point is that a first run does \
         not have to know it needs one: {task}"
    );

    let description = field(task, "Description").expect("the task must carry a Description");
    assert!(
        description.contains("ViGEmBus"),
        "the label names the driver, so somebody who already has one can recognise it: \
         {description}"
    );
    assert!(
        description.to_ascii_lowercase().contains("controller"),
        "and says what it is FOR, in a word a first-run user has (`controller`), not one \
         only we have: {description}"
    );
}

/// **The install goes through the verb that verifies it.**
///
/// `drivers\ViGEmBus_1.22.0_x64_x86_arm64.exe` is sitting in `{app}` and a
/// one-line `[Run]` entry could execute it. That version would pass every other
/// test in this file and would throw away `docs/DRIVERS.md`'s entire
/// guarantee: the protected-directory search, the sealed handle, the SHA-256
/// and the Authenticode chain all live in `ksx install-drivers`, and none of
/// them becomes optional because Inno is doing the running.
///
/// So this asserts the bundled file name appears only as a payload to COPY,
/// never as something to execute, and that the driver step names the verb.
#[test]
fn nothing_executes_the_bundled_setup_directly() {
    let text = script();
    let bundle = ksx_platform::installer::INSTALLER_FILE_NAME;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with("//") || !line.contains(bundle) {
            continue;
        }
        assert!(
            line.starts_with("Source:"),
            "{bundle} may only be COPIED by this installer. Running it directly skips \
             every check `ksx install-drivers` makes on it (docs/DRIVERS.md): {line}"
        );
    }

    let code = section(&text, "[Code]").join("\n");
    assert!(
        code.contains("install-drivers --yes"),
        "the driver step must run `ksx install-drivers --yes` — one code path owns the \
         hash pin, the signature pin and the sealed handle"
    );
}

/// **A driver that will not install must not fail the install.**
///
/// ksx without ViGEmBus still runs, still configures, still maps and still
/// saves; it just cannot plug a pad. Rolling the whole install back over that
/// would take away the nine tenths that work to punish the one tenth that did
/// not — and would leave the user with no ksx *and* no driver.
///
/// Fails against the obvious "fix" for a failed driver step: `Abort`,
/// `ExitSetupMsgBox` or a `RaiseException` in the driver path, any of which
/// turns a recoverable outcome into a rolled-back install.
///
/// It also pins the other half of the obligation — that a failure SAYS so and
/// names a way back — because a step that fails silently is the same bug as a
/// step that fails loudly and takes everything with it.
#[test]
fn a_failed_driver_install_reports_and_continues() {
    let text = script();
    let code = section(&text, "[Code]").join("\n");

    for wrecker in ["Abort", "ExitSetupMsgBox", "RaiseException"] {
        assert!(
            !code.contains(wrecker),
            "`{wrecker}` in [Code] would let a failed driver install take the whole \
             install with it. A machine with no ViGEmBus still wants ksx."
        );
    }

    // An EXCEPTION out of `CurStepChanged` rolls the install back just as
    // effectively as an `Abort`, and it is the failure mode ISCC cannot catch:
    // a constant that does not expand compiles perfectly and throws at run
    // time, on a shipped setup.exe, on somebody else's machine.
    assert!(
        code.contains("try") && code.contains("except"),
        "the driver step must be wrapped in try..except — CI proves this file \
         COMPILES and proves nothing about what it does when run"
    );

    // The retry the user can actually perform comes first: they are looking at
    // the installer that offers it. `FIRST-RUN.md` §6 puts "the only way out of
    // a mistake is a shell command" on the list of things that must never
    // happen, so the command is named as well, never instead.
    assert!(
        code.contains("run this installer again"),
        "a failure must name the no-terminal retry — this installer, with the box ticked"
    );
    assert!(
        code.contains("ksx install-drivers --yes"),
        "...and the command, for somebody who has a terminal open anyway"
    );
}

/// **Every byte the user can see is ASCII.**
///
/// `ksx.iss` has no UTF-8 BOM, so ISCC reads it in the system code page: one
/// byte above 127 in a wizard checkbox, a shortcut tooltip or a message box
/// becomes mojibake on somebody else's machine, and it renders correctly on
/// the machine that wrote it. This is the one class of mistake in this file
/// that cannot be caught by looking at it, and the number of user-visible
/// strings just tripled.
///
/// Comment lines are exempt and stay exempt — ISCC discards them, and this
/// repository's prose uses en dashes and section marks everywhere.
#[test]
fn no_user_visible_string_carries_a_byte_above_127() {
    for (number, line) in script().lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with(';') || trimmed.starts_with("//") {
            continue;
        }
        assert!(
            line.is_ascii(),
            "line {} has a byte above 127 outside a comment. ksx.iss has no BOM, so ISCC \
             reads it in the system code page and this reaches a user as mojibake — keep \
             it ASCII, or put the sentence in a comment: {line}",
            number + 1
        );
    }
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

/// **One version, spelled in two files, and a tag that has to equal both.**
///
/// `#define AppVersion` is the installer's filename, its `VersionInfoVersion`,
/// and the "ksx 0.1.0" row in Apps & Features — what the INSTALLED program says
/// about itself. `[workspace.package] version` is what `ksx --version` prints.
/// `.github/workflows/build-installer.yml` refuses to build a release unless the
/// tag equals both, and it is the release that makes the disagreement expensive:
/// a tag is a public name you cannot reuse, so learning ten minutes into a
/// release run that two files disagree costs a deleted tag and a burned version
/// number.
///
/// Fails against the ordinary broken tree: someone bumps `Cargo.toml` to 0.2.0
/// and `ksx.iss` keeps 0.1.0. Nothing else in this repository notices — the
/// build is fine, the tests are green, the installer compiles — until the tag.
#[test]
fn the_installer_version_and_the_workspace_version_cannot_drift() {
    let installer = define(&script(), "AppVersion");
    let workspace = workspace_version();
    assert_eq!(
        installer, workspace,
        "packaging/ksx.iss says AppVersion {installer} and Cargo.toml's \
         [workspace.package] says {workspace}. A release tag has to equal both \
         (.github/workflows/build-installer.yml, \"Version agreement\"), so one \
         of these two files is wrong right now — decide which before tagging."
    );
}

/// **The release is a pushed tag, and nothing else is a release.**
///
/// Fails against three broken versions, each of which has a plausible author:
///
/// 1. `on: release: types: [published]` — the belief that publishing requires
///    creating a Release in the web UI first. It does not: the UI works only
///    *because* it creates a tag. Under that trigger `git push origin v0.1.0`
///    does nothing at all, which is indistinguishable from a broken workflow
///    and is how a repo ends up with tags and an empty releases page.
/// 2. `branches:` added beside `tags:`, which cuts a release on every push.
/// 3. A pattern this repository's own version cannot produce (`release-v*`,
///    `v*.*.*-*`): the workflow then exists, is valid, and never fires.
#[test]
fn the_release_is_triggered_by_pushing_a_version_tag() {
    let lines = yaml_lines(&workflow("release.yml"));
    let triggers = yaml_keys(&yaml_block(&lines, &["on"]));
    assert_eq!(
        triggers,
        vec!["push"],
        "the only trigger may be a push. A `release:` trigger would wait for a \
         human to create a Release in the browser, and then a CLI-pushed tag \
         publishes nothing; a `workflow_dispatch` would run the publish job on a \
         branch, where there is no tag to attach a release to."
    );

    let push = yaml_block(&lines, &["on", "push"]);
    assert_eq!(
        yaml_keys(&push),
        vec!["tags"],
        "a `branches:` filter beside `tags:` would publish a release on every \
         branch push: {push:?}"
    );

    let patterns = yaml_items(&yaml_block(&lines, &["on", "push", "tags"]));
    assert!(!patterns.is_empty(), "no tag patterns in release.yml");
    let tag = format!("v{}", workspace_version());
    assert!(
        patterns.iter().any(|pattern| glob_matches(pattern, &tag)),
        "this repository is at version {}, so the tag to push is `{tag}` — and \
         none of release.yml's patterns {patterns:?} match it. A workflow that \
         cannot fire for the version in the tree is a workflow that never fires.",
        workspace_version()
    );
}

/// **What gets attached is the installer, under the name `ksx.iss` emits.**
///
/// `docs/FIRST-RUN.md` §1 moment 1 is "a `.exe` from the releases page. One
/// file" — and that file is the setup.exe. The chain from the Inno script to the
/// release asset runs through three files, and every link is a string:
///
/// ```text
///   ksx.iss OutputDir + OutputBaseFilename
///     -> build-installer.yml upload path glob
///       -> artifact name
///         -> release.yml download
///           -> gh release create <asset>
/// ```
///
/// Fails against:
///
/// - `OutputDir=dist` in `ksx.iss` (one word; ISCC compiles it happily) — the
///   upload glob then matches nothing;
/// - an artifact renamed in one file and not the other, which fails the publish
///   AFTER a ten-minute build, on a tag that is already public;
/// - a release that attaches only `ksx.exe`. A bare console binary with no
///   driver folder beside it is not what moment 1 means by "one file".
#[test]
fn the_release_attaches_the_installer_that_ksx_iss_actually_produces() {
    let iss = script();
    // OutputDir is relative to the .iss, which lives in packaging/.
    let produced = format!(
        "packaging/{}/{}.exe",
        setup_value(&iss, "OutputDir"),
        setup_value(&iss, "OutputBaseFilename")
    );

    let build = workflow("build-installer.yml");
    let uploads = artifact_steps(&build, "upload");
    assert!(!uploads.is_empty(), "build-installer.yml uploads nothing");
    let (installer_artifact, glob) = uploads
        .iter()
        .find(|(_, path)| glob_matches(path, &produced))
        .unwrap_or_else(|| {
            panic!(
                "ksx.iss writes {produced}, and no upload in build-installer.yml \
                 collects it: {uploads:?}"
            )
        });
    assert!(
        glob.contains("setup"),
        "the installer upload should still name the installer: {glob}"
    );

    let release = workflow("release.yml");
    let downloads = artifact_steps(&release, "download");
    let names: Vec<&str> = downloads.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        names.contains(&installer_artifact.as_str()),
        "build-installer.yml uploads the installer as `{installer_artifact}` and \
         release.yml downloads {names:?} — the publish would fail after the build, \
         with the tag already pushed."
    );
    for (name, _) in &downloads {
        assert!(
            uploads.iter().any(|(uploaded, _)| uploaded == name),
            "release.yml downloads an artifact `{name}` that build-installer.yml \
             never uploads: {uploads:?}"
        );
    }

    // The assets, as data rather than as a command line, so this can read them.
    let assets = release
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("$assets") && line.contains("@("))
        .expect(
            "release.yml's publish step must name its release assets in one \
             `$assets = @(...)` line, so this test can see what gets attached",
        );
    assert!(
        assets.contains("SETUP_NAME"),
        "the installer must be a release asset (FIRST-RUN.md §1 moment 1): {assets}"
    );
    assert!(
        assets.contains("ksx.exe"),
        "the bare ksx.exe rides along for people who want it: {assets}"
    );
}

/// **The release body explains the scary dialog, and lets a download be
/// checked.**
///
/// Two sentences carry the whole weight of moment 1 and neither can be
/// generated:
///
/// - The installer is unsigned, so Windows shows "Windows protected your PC"
///   with only a *Don't run* button visible. A first-time user who meets an
///   unexplained warning stops there, and no later screen gets a turn. The body
///   has to name the dialog and the two clicks through it.
/// - The SHA-256 and the commit are what make "click Run anyway" checkable
///   rather than a request for trust. Both already exist — the build computes
///   them — so the only failure mode is not printing them.
///
/// Fails against a body that drops either, and against a template that grows a
/// placeholder `release.yml` does not substitute: `{{SIZE}}` would then appear
/// on a public page as five literal characters.
#[test]
fn the_release_body_carries_the_hash_the_commit_and_the_smartscreen_step() {
    let notes = read("packaging/release-notes.md");
    for placeholder in ["{{SETUP_NAME}}", "{{SETUP_SHA256}}", "{{COMMIT}}"] {
        assert!(
            notes.contains(placeholder),
            "the release body must carry {placeholder}: a download nobody can \
             verify against the run that built it is a download nobody can verify"
        );
    }
    for phrase in ["Windows protected your PC", "More info", "Run anyway"] {
        assert!(
            notes.contains(phrase),
            "the release body must say \"{phrase}\". The installer is not \
             code-signed; SmartScreen's dialog shows a single `Don't run` button, \
             and a first-time user who is not told about it does not reach moment 2."
        );
    }

    // Every placeholder the template uses is one the workflow fills in.
    let release = workflow("release.yml");
    let mut rest = notes.as_str();
    while let Some(at) = rest.find("{{") {
        let tail = &rest[at + 2..];
        let end = tail
            .find("}}")
            .unwrap_or_else(|| panic!("unterminated `{{{{` in packaging/release-notes.md"));
        let placeholder = format!("{{{{{}}}}}", &tail[..end]);
        assert!(
            release.contains(&placeholder),
            "packaging/release-notes.md uses {placeholder} and \
             .github/workflows/release.yml never substitutes it — it would reach \
             the releases page as literal braces"
        );
        rest = &tail[end + 2..];
    }
}
