//! **The surface-parity guard** — `docs/SURFACES.md` §1 and §3, as a test.
//!
//! # The two failures this exists for, and they are the same failure
//!
//! `ksx device pick` shipped as a verb with no web face and nobody noticed
//! until the owner opened the UI and found he could not list a board. That is
//! the obvious half. `docs/SURFACES.md` §1 records the other half, which is the
//! same shape pointed the other way: `MachineSource::devices()` was built for a
//! cabinet screen nobody ever wrote, and sat with **zero callers**, compiled
//! behind `#[cfg(all(windows, feature = "cabinet"))]` for a consumer that never
//! asks. A backend verb with no face is not finished — it is untested against a
//! real caller and its shape is a guess.
//!
//! §3's matrix is where those two are supposed to be reconciled. It is prose,
//! so it drifts: at its first audit **four** of its cells described capabilities
//! that did not exist, and two of them said "view" for a surface that renders
//! nothing at all — which reads as shipped. This file makes the matrix a claim
//! the tree either supports or does not.
//!
//! # What the guard checks against what
//!
//! Three evidence sources, none of them a list maintained by hand:
//!
//! | surface | evidence | read from |
//! |---|---|---|
//! | CLI | the clap command tree | the built binary, walked with `--help` |
//! | egui | `Screen::ALL` + every `Ask` variant | `ksx-cabinet/src/{nav,app}.rs`, by source text |
//! | Studio | every `.route()` inside `Router::new()` | `ksx-studio/src/server.rs`, by source text |
//!
//! The two source-text readers work the way `ksx-app/src/run/plan.rs`'s
//! source-guard test does, and for the same reason: the fact being checked is a
//! *structural* one that no value reachable at runtime reports. In Studio's case
//! the structure is load-bearing twice over — the scan stops at the guard layer
//! (`crate::guard::same_origin`), so a route declared after it, outside the CSRF
//! and DNS-rebinding checks, is invisible here and reads as a missing face.
//!
//! # Why the CLI is walked as a subprocess and not `Cli::command()`
//!
//! `ksx-app` has a `[[bin]]` and no `[lib]`. An integration test is its own
//! crate and can see a package's library types, never a binary's — so `Cli`,
//! `Command` and the derive output are not nameable from `tests/`. Walking the
//! binary's own `--help` is the same tree rendered by the same clap, which is
//! the property that matters: nothing here re-types a verb name. Give this
//! package a `[lib]` holding the CLI types and `walk_clap_tree` becomes eight
//! lines against `clap::CommandFactory`; until then this is the honest reading.
//!
//! The walk runs against a COPY of the binary in a temp directory with a
//! `ksx.toml` beside it, because `ksx` initialises logging before it parses
//! argv: the portable marker makes that temp directory the config root
//! (`ksx_config::ConfigRoot::resolve`), so forty `--help` runs write forty log
//! lines into a directory this test then deletes, instead of into the config
//! root of whoever is running the tests.
//!
//! # What the guard cannot see
//!
//! An `EXEMPT` verb, a cell with no anchor, and a face built under a name the
//! anchor table does not guess. The "nothing is there" direction is a tripwire
//! on the obvious name, not a proof — `/devices/pick` catches a device picker,
//! `/pick-a-board` would not. The "a face exists" direction is exact: the
//! anchors are read out of the tree, so a claim of a shipped face fails the
//! moment the thing backing it is gone.
//!
//! It also cannot see a verb a row does not name. §10 records one live example:
//! `/profiles/new` creates a games.toml profile and there is no `ksx games new`,
//! so §2's build order ran 1 → 3 with the CLI skipped. The row says "owns" and
//! the guard agrees, because the row names `slot assign` and `config import` and
//! both exist. A guard cannot check a claim nobody wrote down.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Reading the tree
// ---------------------------------------------------------------------------

/// The repository root: this crate is `<root>/crates/ksx-app`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ksx-app is two levels below the repo root")
        .to_path_buf()
}

/// A repo file, with line endings normalised.
///
/// Not cosmetic: two of the readers below slice on a literal `"\n}"`, and this
/// repository has already shipped one test that passed or failed on how the
/// checkout was configured (`.gitattributes`, commit c5f2532).
fn read_repo(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()))
        .replace("\r\n", "\n")
}

// ---------------------------------------------------------------------------
// Evidence 1: the CLI, from the clap command tree
// ---------------------------------------------------------------------------

/// The binary cargo built for this integration test.
const KSX: &str = env!("CARGO_BIN_EXE_ksx");

/// Every runnable verb path, space-separated: `pads`, `device pick`,
/// `macro trace`.
fn cli_verbs() -> &'static BTreeSet<String> {
    static VERBS: LazyLock<BTreeSet<String>> = LazyLock::new(walk_clap_tree);
    &VERBS
}

fn walk_clap_tree() -> BTreeSet<String> {
    let dir = std::env::temp_dir().join(format!("ksx-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
    let name = Path::new(KSX)
        .file_name()
        .expect("the built binary has a file name");
    let exe = dir.join(name);
    std::fs::copy(KSX, &exe).unwrap_or_else(|e| panic!("copying {KSX}: {e}"));
    // The portable marker (`ksx_config::paths::PORTABLE_MARKER`). Its presence
    // beside the exe makes this directory the config root, so the logging
    // `ksx` sets up before `Cli::parse()` lands here and is deleted with it.
    std::fs::write(dir.join("ksx.toml"), "schema_version = 1\n").expect("portable marker");

    let mut out = BTreeSet::new();
    walk(&exe, &[], &mut out, 0);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        out.len() >= 20,
        "the clap walk found {} verbs ({out:?}), which means it walked nothing — \
         the help format changed, or the binary did not run",
        out.len()
    );
    out
}

fn walk(exe: &Path, path: &[String], out: &mut BTreeSet<String>, depth: usize) {
    assert!(
        depth < 4,
        "the clap tree is deeper than this walk expects at {path:?}; raise the cap \
         deliberately rather than by accident"
    );
    let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
    args.push("--help");
    let output = Command::new(exe)
        .args(&args)
        .output()
        .unwrap_or_else(|e| panic!("running {} {args:?}: {e}", exe.display()));
    assert!(
        output.status.success(),
        "{} {args:?} exited {:?}",
        exe.display(),
        output.status.code()
    );
    // stdout only: `logging::announce` writes its line to stderr, which is
    // where it belongs and out of the way here.
    let help = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    assert!(
        help.contains("Usage:"),
        "{} {args:?} printed no usage line:\n{help}",
        exe.display()
    );

    // A node whose usage line demands `<COMMAND>` is a group and nothing else.
    // `ksx macro` is BOTH — it takes `--preset`/`--name` and has a `trace`
    // subcommand — so the test is the usage line, not the presence of children.
    if !path.is_empty() && !usage_demands_a_subcommand(&help) {
        out.insert(path.join(" "));
    }
    for child in subcommands(&help) {
        let mut next = path.to_vec();
        next.push(child);
        walk(exe, &next, out, depth + 1);
    }
}

fn usage_demands_a_subcommand(help: &str) -> bool {
    help.lines()
        .find(|line| line.starts_with("Usage:"))
        .is_some_and(|line| line.contains("<COMMAND>"))
}

/// The names in clap's `Commands:` block. Entries sit at exactly two spaces of
/// indent; wrapped descriptions are indented to the description column, which
/// is much deeper, and the name shape is checked besides.
fn subcommands(help: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in help.lines() {
        if line.starts_with("Commands:") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if !line.starts_with("  ") || line.trim().is_empty() {
            break;
        }
        if line.as_bytes().get(2) == Some(&b' ') {
            continue;
        }
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        // clap's own `help` verb prints this message; it is not a ksx verb.
        if name == "help" {
            continue;
        }
        if name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            out.push(name.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Evidence 2: Studio, from the routes inside the guarded router
// ---------------------------------------------------------------------------

/// Every path registered inside `Router::new()`, up to the guard layer.
///
/// The cut is at `crate::guard::same_origin`'s `.layer()` and not at the first
/// `.layer()` in the chain, because `/setup/import` carries its own
/// `DefaultBodyLimit` — cutting at the first one would silently drop the six
/// routes that follow it and report six missing faces.
fn studio_routes() -> &'static BTreeSet<String> {
    static ROUTES: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
        const SERVER: &str = "crates/ksx-studio/src/server.rs";
        let text = read_repo(SERVER);
        let start = text
            .find("Router::new()")
            .unwrap_or_else(|| panic!("{SERVER} no longer builds its router with Router::new()"));
        let guard = text[start..]
            .find(".layer(axum::middleware::from_fn")
            .unwrap_or_else(|| {
                panic!(
                    "{SERVER} no longer ends its route chain with the guard layer. \
                     Every mutating route goes inside Router::new() BEFORE that \
                     .layer() — if the guard moved, this reader has to move with it."
                )
            });
        let chain = &text[start..start + guard];

        let mut out = BTreeSet::new();
        for (at, _) in chain.match_indices(".route(\"") {
            let rest = &chain[at + ".route(\"".len()..];
            if let Some(end) = rest.find('"') {
                out.insert(rest[..end].to_string());
            }
        }
        assert!(
            out.len() >= 30 && out.contains("/") && out.contains("/map"),
            "the route scan found {} paths ({out:?}), which means it read the wrong \
             span of {SERVER}",
            out.len()
        );
        out
    });
    &ROUTES
}

// ---------------------------------------------------------------------------
// Evidence 3: the egui, from its screens and its verbs
// ---------------------------------------------------------------------------

/// `Screen::<Name>` for every tab, and `Ask::<Name>` for every write the
/// cabinet can perform.
///
/// Two sources because the surface has two halves and the matrix claims both:
/// "primary" for `"Press a button, see it light"` is a SCREEN (it decides
/// nothing), "slot→preset only" for config editing is an ASK.
fn cabinet_surface() -> &'static BTreeSet<String> {
    static SURFACE: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
        let mut out = BTreeSet::new();

        let nav = read_repo("crates/ksx-cabinet/src/nav.rs");
        let all = between(
            &nav,
            "pub const ALL: [Screen;",
            "];",
            "nav.rs's Screen::ALL",
        );
        for (at, _) in all.match_indices("Screen::") {
            let ident = leading_ident(&all[at + "Screen::".len()..]);
            if !ident.is_empty() {
                out.insert(format!("Screen::{ident}"));
            }
        }

        let app = read_repo("crates/ksx-cabinet/src/app.rs");
        let asks = between(&app, "pub enum Ask {", "\n}", "app.rs's Ask enum");
        for line in asks.lines() {
            let line = line.trim_start();
            if line.starts_with("//") || line.starts_with("#[") {
                continue;
            }
            let ident = leading_ident(line);
            if ident.is_empty() || !ident.starts_with(|c: char| c.is_ascii_uppercase()) {
                continue;
            }
            // A variant, not a struct-variant field: what follows the name is
            // `,` (unit), `(` (tuple) or `{` (struct).
            if line[ident.len()..]
                .trim_start()
                .starts_with([',', '(', '{'])
            {
                out.insert(format!("Ask::{ident}"));
            }
        }

        assert!(
            out.len() >= 10 && out.contains("Screen::ButtonCheck") && out.contains("Ask::Assign"),
            "the cabinet scan found {out:?}, which means one of the two readers \
             matched nothing"
        );
        out
    });
    &SURFACE
}

fn between<'a>(text: &'a str, open: &str, close: &str, what: &str) -> &'a str {
    let start = text
        .find(open)
        .unwrap_or_else(|| panic!("{what}: no `{open}` — the shape this reader assumes is gone"))
        + open.len();
    let len = text[start..]
        .find(close)
        .unwrap_or_else(|| panic!("{what}: `{open}` is never closed by `{close}`"));
    &text[start..start + len]
}

fn leading_ident(text: &str) -> &str {
    let end = text
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(text.len());
    &text[..end]
}

// ---------------------------------------------------------------------------
// The claim: docs/SURFACES.md §3
// ---------------------------------------------------------------------------

/// One matrix row, cells as written.
#[derive(Debug)]
struct Row {
    capability: String,
    cli: String,
    egui: String,
    studio: String,
}

/// What a cell asserts about whether a face exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    /// A face is there today: `owns`, `primary`, `view`, `convenience`.
    Shipped,
    /// Nothing is there: `—`, `planned …`, `never …`. §3 spells the first one
    /// out — **"planned" = nothing is there** — because the previous version of
    /// the table used "view" for two cells that render nothing at all.
    Absent,
}

fn classify(cell: &str) -> Claim {
    let plain = cell.replace("**", "").replace('`', "");
    let plain = plain.trim();
    // The FIRST word decides, and the qualifier that follows never does.
    // "planned primary" is a plan; "input only (ksx monitor)" is a shipped
    // verb; "**primary** (§3a)" is a shipped page. Reading from the other end
    // would classify the first of those three backwards.
    match plain.split_whitespace().next().unwrap_or("") {
        "owns" | "primary" | "view" | "convenience" | "input" => Claim::Shipped,
        "slot→preset" => Claim::Shipped,
        "planned" | "never" | "—" | "-" => Claim::Absent,
        other => panic!(
            "docs/SURFACES.md §3 has a cell starting `{other}` ({cell:?}) that this guard \
             does not know how to read. Every cell has to say whether a face EXISTS — \
             teach `classify` the new word and decide which side it falls on, because a \
             word nobody classified is a claim nobody checks."
        ),
    }
}

/// Normalised capability text, for matching a row to its anchors.
fn key(capability: &str) -> String {
    capability
        .replace("**", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn matrix() -> &'static Vec<Row> {
    static MATRIX: LazyLock<Vec<Row>> = LazyLock::new(|| {
        let doc = read_repo("docs/SURFACES.md");
        let mut rows = Vec::new();
        let mut inside = false;
        let mut header = 0;
        for line in doc.lines() {
            if line.starts_with("## §3 ") {
                inside = true;
                continue;
            }
            if inside && line.starts_with('#') {
                break;
            }
            let line = line.trim();
            if !inside || !line.starts_with('|') {
                continue;
            }
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(
                cells.len(),
                4,
                "docs/SURFACES.md §3's matrix row {line:?} has {} columns, not the four \
                 this guard reads (capability, CLI, egui, Studio). A new column is a new \
                 kind of claim and needs a reader.",
                cells.len()
            );
            if cells[0] == "Capability" {
                header += 1;
                continue;
            }
            if cells.iter().all(|c| c.chars().all(|ch| ch == '-')) {
                continue;
            }
            rows.push(Row {
                capability: key(cells[0]),
                cli: cells[1].to_string(),
                egui: cells[2].to_string(),
                studio: cells[3].to_string(),
            });
        }
        assert_eq!(
            header, 1,
            "docs/SURFACES.md §3 should hold exactly one matrix; found {header} headers"
        );
        assert!(
            rows.len() >= 8,
            "the matrix parser found {} rows, which means the table moved or changed shape",
            rows.len()
        );
        rows
    });
    &MATRIX
}

// ---------------------------------------------------------------------------
// The binding: which evidence would make each cell true
// ---------------------------------------------------------------------------

/// A row of §3's matrix, bound to the things that would make its cells true.
///
/// This table is the guard, and it is a SPECIFICATION, not a mirror of the
/// implementation: it says what each capability is made of on each surface. The
/// lists are read in both directions — every name must exist where a cell says
/// the face is shipped, and none may exist where a cell says nothing is there.
///
/// `cli` doubles as the coverage list: a verb no row names and no `EXEMPT`
/// entry covers is a verb with no face and no reason to lack one.
struct Anchors {
    capability: &'static str,
    cli: &'static [&'static str],
    egui: &'static [&'static str],
    studio: &'static [&'static str],
}

const ANCHORS: &[Anchors] = &[
    Anchors {
        capability: "First run: stage a setup, save or play",
        // `planned` — and it is the reason the bookkeeping test below stopped
        // demanding that every CLI anchor resolve. `ksx stage` does not exist;
        // saying so IS the claim, exactly as `Screen::Mapper` below says the
        // cabinet has no mapper. §3c records why the build order ran backwards
        // here.
        cli: &["stage"],
        egui: &["Screen::FirstRun", "Ask::Stage"],
        // The two acts §2 requires be separable, plus the page they live on.
        // Naming both is deliberate: a `/start` that could only save, or only
        // play, would satisfy a one-anchor row while failing the requirement
        // the row exists for.
        studio: &["/start", "/start/save", "/start/play"],
    },
    Anchors {
        capability: "Author presets / key mappings",
        cli: &[
            "setup",
            "preset list",
            "preset new",
            "map",
            "macro",
            "macro trace",
        ],
        // The names a cabinet mapper would take. Neither exists, which is what
        // makes the egui cell's "—" honest.
        egui: &["Screen::Mapper", "Ask::Bind"],
        studio: &["/map", "/map/bind"],
    },
    Anchors {
        capability: "Edit config, profiles",
        cli: &["slot list", "slot assign", "config export", "config import"],
        // `Ask::Assign` IS the "slot→preset only" cell: the Presets screen
        // builds one, and `assign_destination` decides which file it lands in.
        egui: &["Ask::Assign"],
        studio: &["/setup/slot", "/profiles/new"],
    },
    Anchors {
        capability: "Device pick / remove",
        cli: &["device scan", "device pick", "device remove"],
        egui: &["Screen::Devices", "Ask::DevicePick"],
        studio: &["/devices/pick", "/devices/remove"],
    },
    Anchors {
        capability: "WinUSB claim / release",
        cli: &["winusb status", "winusb claim", "winusb release"],
        egui: &["Ask::WinusbClaim"],
        // "never (needs elevation)" — server.rs says the same thing at the
        // /devices routes: "Claiming is NOT here and never will be."
        studio: &["/winusb/claim"],
    },
    Anchors {
        capability: "\"Press a button, see it light\"",
        cli: &["monitor"],
        egui: &["Screen::ButtonCheck"],
        // §8: "no feed on AppState, no frame type, no handler". The feed is the
        // load-bearing half — a page without one is a layout.
        studio: &["/buttons", "/api/feed"],
    },
    Anchors {
        capability: "Is it working: pads, drivers",
        cli: &["doctor", "devices", "session status"],
        egui: &["Screen::Status"],
        studio: &["/", "/api/status"],
    },
    Anchors {
        capability: "Spawn test pads / prune the bus",
        // One verb, both halves: `ksx pads --count N` and `ksx pads --prune`.
        cli: &["pads"],
        egui: &["Ask::PadsSpawn", "Ask::PadsPrune"],
        studio: &["/pads/spawn", "/pads/prune"],
    },
    Anchors {
        capability: "Record / replay a session",
        // One capability, both halves: `ksx monitor --record` writes the
        // timeline and `ksx play` drives the pipeline from it. `monitor` is
        // named by the button-check row as well, which is correct — a verb can
        // be part of two capabilities, and this one genuinely is.
        cli: &["monitor", "play"],
        // The names an attract-mode button would take (§3b). Neither exists,
        // which is what makes the egui cell's "planned" honest.
        egui: &["Screen::Replay", "Ask::Play"],
        studio: &["/play", "/recordings"],
    },
    Anchors {
        capability: "Start / stop / switch profile",
        cli: &["run", "session start", "session stop", "session reload"],
        egui: &["Screen::Session", "Ask::Start", "Ask::Stop"],
        studio: &["/session/start", "/session/stop", "/profiles/switch"],
    },
];

/// A verb that is right to leave CLI-only, and why.
///
/// The reason is a field rather than a comment so that it reaches the failure
/// message and so that an entry cannot land without one. "Nobody got round to
/// it" is not on this list — that is the thing the guard is for.
///
/// The test for membership is *what makes the other surfaces the wrong place*,
/// and it is a narrower test than "feels like plumbing". `ksx doctor` looks like
/// an exemption and is not one: `StatusSnapshot` carries `vigem`,
/// `interception`, `pads` and `autostart`, the egui's Status screen renders
/// them and so does `/api/status`, which is precisely §3's "Is it working:
/// pads, drivers" row. It is claimed there, not excused here.
struct Exempt {
    verb: &'static str,
    /// The cargo feature that has to be on for this verb to exist at all.
    gate: Option<&'static str>,
    why: &'static str,
}

const EXEMPT: &[Exempt] = &[
    Exempt {
        verb: "import-legacy",
        gate: None,
        why: "A one-way migration off the old C# splitter's XML, run once on a machine \
              that still has those files. There is nothing to operate afterwards: the \
              RESULT is the config every other surface then edits.",
    },
    Exempt {
        verb: "daemon",
        gate: None,
        why: "Not a capability — the process the capabilities run inside, and its face is \
              the tray icon it installs. Whether it is up is already on both other \
              surfaces (`StatusSnapshot::daemon_running`). What is not is STARTING it, \
              and a button that starts the thing serving the button is a bootstrap \
              problem, not a surface gap.",
    },
    Exempt {
        verb: "install-drivers",
        gate: None,
        why: "Needs an administrator token and ksx never self-elevates, so this sits on \
              the same line §3a draws for WinUSB claim: a page could only ever print the \
              command. §3 marks that column `never` for a reason.",
    },
    Exempt {
        verb: "autostart",
        gate: None,
        why: "Its STATE is already on both other surfaces (`StatusSnapshot::autostart`); \
              what is CLI-only is the write, which registers a per-user Task Scheduler \
              task once, when a cabinet is commissioned. A machine-lifecycle step, not \
              something anyone performs while ksx runs.",
    },
    Exempt {
        verb: "studio",
        gate: Some("studio"),
        why: "It LAUNCHES a surface. Studio cannot be the place you go to open Studio.",
    },
    Exempt {
        verb: "cabinet",
        gate: Some("cabinet"),
        why: "Same, and §6 already fixed the direction: the egui opens Studio, never the \
              reverse — a browser would need a registered ksx:// protocol to do it.",
    },
    Exempt {
        verb: "open",
        gate: Some("studio"),
        why: "It IS the product's front door — start the daemon if needed, then show Studio \
              in its own window — so it is the same class as `studio` and `cabinet`: a verb \
              that launches a surface cannot have a face on the surface it launches. It is \
              what the installer's shortcut and the Start menu entry both run, which is \
              precisely the point at which no other surface exists yet.",
    },
];

/// **The guard's own blind spot, stated so it is not rediscovered.**
///
/// `EXEMPT` gates and the two feature-gated verbs above are only visible when
/// the test binary was built with that feature on, and CI's test step is
/// `cargo test --workspace` with default features — where `studio` and
/// `cabinet` are off (`CLAUDE.md`: "the default build compiles neither"). So
/// `ksx open`, `ksx studio` and `ksx cabinet` are checked by this file only
/// when somebody runs it with the feature enabled. `ksx open` sat unaccounted
/// for exactly that reason until 2026-08-08 and CI stayed green throughout.
///
/// The fix is a feature-enabled test job, not a change here; it is written down
/// rather than done because widening the CI matrix is its own change with its
/// own runtime cost, and a note that names the hole is better than a guard that
/// quietly checks less than it claims.
#[cfg(test)]
const _FEATURE_GATED_VERBS_ARE_ONLY_CHECKED_WITH_THE_FEATURE_ON: () = ();

fn gate_is_on(feature: &str) -> bool {
    match feature {
        "studio" => cfg!(feature = "studio"),
        "cabinet" => cfg!(feature = "cabinet"),
        other => panic!(
            "EXEMPT names cargo feature `{other}`, which this guard cannot evaluate — \
             `cfg!(feature = ..)` needs a literal, so add it beside the other two"
        ),
    }
}

fn anchors() -> BTreeMap<String, &'static Anchors> {
    ANCHORS.iter().map(|a| (key(a.capability), a)).collect()
}

/// Which evidence set a surface's anchors are looked up in.
fn evidence(surface: &str, name: &str) -> bool {
    match surface {
        "CLI" => cli_verbs().contains(name),
        "egui" => cabinet_surface().contains(name),
        "Studio" => studio_routes().contains(name),
        other => unreachable!("no evidence source for {other}"),
    }
}

fn source_of(surface: &str) -> &'static str {
    match surface {
        "CLI" => "the clap tree (`ksx --help`, walked)",
        "egui" => "crates/ksx-cabinet/src/{nav,app}.rs (Screen::ALL, enum Ask)",
        "Studio" => "crates/ksx-studio/src/server.rs (Router::new(), guard side)",
        other => unreachable!("no evidence source for {other}"),
    }
}

/// The three cells of a row, paired with the anchors that would make them true.
fn cells<'a>(
    row: &'a Row,
    a: &'a Anchors,
) -> [(&'static str, &'a str, &'static [&'static str]); 3] {
    [
        ("CLI", row.cli.as_str(), a.cli),
        ("egui", row.egui.as_str(), a.egui),
        ("Studio", row.studio.as_str(), a.studio),
    ]
}

// ---------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------

/// **A cell that claims a shipped face must have one.**
///
/// This is the direction that caught `ksx device pick`: a verb in the tree, a
/// row in the table, and no page anywhere. It is also the direction that the
/// first audit of §3 walked by hand and found four wrong cells in, including
/// two saying "view" for surfaces that render nothing.
///
/// Breaks against: the tree as it was before task #22, where §3's device row
/// claimed a Studio face and `server.rs` had no `/devices*` route at all;
/// against deleting `ksx winusb claim` while the row still says "owns"; and
/// against moving a mutating route outside the `.layer()` guard, which takes it
/// out of the router this reader can see.
#[test]
fn every_cell_claiming_a_shipped_face_has_one() {
    let anchors = anchors();
    let mut problems = Vec::new();

    for row in matrix() {
        let Some(a) = anchors.get(&row.capability) else {
            continue; // the bookkeeping test below is what reports this
        };
        for (surface, cell, names) in cells(row, a) {
            if classify(cell) != Claim::Shipped {
                continue;
            }
            let missing: Vec<&str> = names
                .iter()
                .copied()
                .filter(|n| !evidence(surface, n))
                .collect();
            if missing.is_empty() {
                continue;
            }
            problems.push(format!(
                "docs/SURFACES.md §3 — {capability}\n  \
                 the {surface} cell says {cell:?}, which means the face is THERE.\n  \
                 missing: {missing:?}\n  \
                 looked in: {source}\n  \
                 Fix it one of three ways: BUILD the face; change the cell to `planned` \
                 (§3: \"planned\" = nothing is there); or, if the face shipped under \
                 another name, update this row's anchors in tests/parity.rs.",
                capability = row.capability,
                source = source_of(surface),
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} capability cell(s) claim a face that is not in the tree:\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// **A cell that says nothing is there must be right about that.**
///
/// The symmetric failure, and the one §1 records: `MachineSource::devices()`
/// was written for a screen nobody had, so the backend grew a shape no caller
/// ever checked. Read forwards it catches the cheaper mistake — a face ships
/// and the table still calls it planned, which is how a matrix stops being
/// worth reading.
///
/// Breaks against the tree as committed at def1c31, where two Studio cells
/// still said "planned" after `/devices/pick`, `/devices/remove`, `/setup/slot`
/// and `/profiles/new` had all shipped. It also breaks the day someone adds
/// `Screen::Devices` to the cabinet without touching §3 row 3.
///
/// The limit is stated where the anchors are: this is a tripwire on the name a
/// face would obviously take, not a proof that no face exists.
#[test]
fn every_cell_claiming_nothing_is_there_is_right() {
    let anchors = anchors();
    let mut problems = Vec::new();

    for row in matrix() {
        let Some(a) = anchors.get(&row.capability) else {
            continue;
        };
        for (surface, cell, names) in cells(row, a) {
            if classify(cell) != Claim::Absent {
                continue;
            }
            let present: Vec<&str> = names
                .iter()
                .copied()
                .filter(|n| evidence(surface, n))
                .collect();
            if present.is_empty() {
                continue;
            }
            problems.push(format!(
                "docs/SURFACES.md §3 — {capability}\n  \
                 the {surface} cell says {cell:?}, and §3 spells that out: \
                 \"planned\" = nothing is there.\n  \
                 but this IS there: {present:?}\n  \
                 found in: {source}\n  \
                 The face shipped and the table did not notice. Update the cell to what \
                 the surface now does — and if the row's shape changed with it, say so in \
                 §3's corrections list the way the other four are recorded.",
                capability = row.capability,
                source = source_of(surface),
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} capability cell(s) say nothing is there when something is:\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// **Every verb is in the matrix, or exempt with a reason.**
///
/// The task list's #26 in one sentence: no backend verb ships without a face.
/// Four verbs did, and the finding was made by a human opening the UI.
///
/// Breaks against the tree at the moment `ksx device pick` landed: a new verb
/// in the clap tree, no row in §3 naming it, no exemption. It also breaks if a
/// verb is quietly deleted, because `EXEMPT` and the anchor lists are checked
/// against the live tree by the bookkeeping test below.
#[test]
fn every_cli_verb_is_claimed_by_a_row_or_exempt_with_a_reason() {
    let claimed: BTreeSet<&str> = ANCHORS.iter().flat_map(|a| a.cli.iter().copied()).collect();
    let exempt: BTreeMap<&str, &Exempt> = EXEMPT.iter().map(|e| (e.verb, e)).collect();

    let orphans: Vec<&String> = cli_verbs()
        .iter()
        .filter(|v| !claimed.contains(v.as_str()) && !exempt.contains_key(v.as_str()))
        .collect();

    assert!(
        orphans.is_empty(),
        "{} CLI verb(s) exist with nothing in docs/SURFACES.md §3 accounting for them: \
         {orphans:?}\n\n\
         A backend verb with no face is not finished — it is untested against a real \
         caller and its shape is a guess (§1). Pick one:\n  \
         1. ADD A ROW to §3's matrix saying which surface performs it, then name the verb \
         in that row's `cli` anchors in tests/parity.rs;\n  \
         2. NAME IT in an existing row, if it is part of a capability already in the \
         table;\n  \
         3. ADD AN EXEMPTION to `EXEMPT` in tests/parity.rs with a `why` that says what \
         makes CLI-only the RIGHT answer — a migration run once, a step that needs an \
         administrator token, a verb that launches a surface. Not \"no UI yet\".",
        orphans.len(),
    );
}

/// **The guard's own bookkeeping.**
///
/// Everything above compares two lists; this checks that both lists are still
/// about the same repository. Without it the guard rots quietly in the one way
/// that matters — an anchor row for a capability the doc no longer has, or an
/// exemption for a verb that was deleted, both of which make the guard pass by
/// checking less.
///
/// Breaks against: renaming a §3 capability without touching `ANCHORS`, adding
/// a matrix row with no anchors (which would otherwise be skipped in silence),
/// deleting a verb an exemption still names, and an exemption whose `why` is a
/// placeholder.
#[test]
fn the_guard_is_still_bound_to_the_documents_and_the_tree_it_reads() {
    let mut problems = Vec::new();

    let anchored: BTreeSet<String> = ANCHORS.iter().map(|a| key(a.capability)).collect();
    let documented: BTreeSet<String> = matrix().iter().map(|r| r.capability.clone()).collect();

    for capability in documented.difference(&anchored) {
        problems.push(format!(
            "docs/SURFACES.md §3 has a row `{capability}` that tests/parity.rs does not \
             bind. An unbound row is checked by NOTHING. Add an `Anchors` entry naming, \
             per surface, the verbs / screens / routes that would make its cells true."
        ));
    }
    for capability in anchored.difference(&documented) {
        problems.push(format!(
            "tests/parity.rs binds `{capability}`, which is not a row in docs/SURFACES.md \
             §3 any more. Either the row was renamed — match the new wording — or the \
             capability left the table, and its anchors should leave with it."
        ));
    }

    let verbs = cli_verbs();
    let claimed: BTreeSet<&str> = ANCHORS.iter().flat_map(|a| a.cli.iter().copied()).collect();
    // Only where the ROW SAYS THE VERB IS THERE. A `planned` CLI cell anchored
    // on a name that does not resolve is not a stale anchor — it is the claim
    // (§3c). The egui and Studio columns have always worked this way:
    // `Screen::Mapper` and `/buttons` name nothing, which is exactly what makes
    // their cells honest, and the CLI column was the odd one out until the
    // first row with a genuinely planned CLI half arrived.
    //
    // Nothing is lost by narrowing it. A `owns` cell whose verb was renamed or
    // deleted still fails, one test up, in
    // `every_cell_claiming_a_shipped_face_has_one` — with a better message.
    let shipped_cli: BTreeSet<&str> = matrix()
        .iter()
        .filter(|row| classify(&row.cli) == Claim::Shipped)
        .filter_map(|row| anchored.contains(&row.capability).then(|| row))
        .filter_map(|row| ANCHORS.iter().find(|a| key(a.capability) == row.capability))
        .flat_map(|a| a.cli.iter().copied())
        .collect();
    for verb in &shipped_cli {
        if !verbs.contains(*verb) {
            problems.push(format!(
                "tests/parity.rs anchors a capability on `ksx {verb}`, whose §3 cell says the \
                 CLI OWNS it, and it is not in the clap tree. Either the verb was renamed and \
                 the anchor is stale, or it was deleted and §3 still promises it."
            ));
        }
    }

    for e in EXEMPT {
        if let Some(feature) = e.gate {
            if !gate_is_on(feature) {
                continue; // this build does not compile the verb at all
            }
        }
        if !verbs.contains(e.verb) {
            problems.push(format!(
                "`ksx {}` is on the EXEMPT list and is not in the clap tree. An exemption \
                 for a verb that does not exist excuses nothing; delete it.",
                e.verb
            ));
        }
        if claimed.contains(e.verb) {
            problems.push(format!(
                "`ksx {}` is BOTH exempt and named by a capability row. It cannot be both \
                 CLI-only-on-purpose and part of a capability with a face; pick one.",
                e.verb
            ));
        }
        assert!(
            e.why.len() >= 60,
            "the EXEMPT entry for `ksx {}` has a {}-character reason. The reason is the \
             whole point of the list — it has to say what makes CLI-only RIGHT, not that \
             nobody has built the face yet.",
            e.verb,
            e.why.len()
        );
    }

    assert!(
        problems.is_empty(),
        "the parity guard has drifted from what it reads:\n\n{}",
        problems.join("\n\n")
    );
}
