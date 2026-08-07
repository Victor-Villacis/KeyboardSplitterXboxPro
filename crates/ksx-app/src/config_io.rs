//! `ksx config export` / `ksx config import` — JSON in and out of the config
//! root, with TOML staying canonical on disk.
//!
//! # Why there are two formats at all
//!
//! **TOML is canonical because it carries COMMENTS.** ksx config files are
//! annotated — `mouse_move_deadzone = 5  # 0..12`, why a cabinet's
//! `launcher_grace_ms` is 20 s, which panel a `[[device]]` id belongs to — and
//! those notes are what makes the file maintainable a year later, by a person
//! *or* by an AI reading it (the intent travels with the value instead of
//! having to be inferred). JSON has no syntax that could keep them.
//!
//! **JSON exists for machines**: preset sharing (M7), AI-generated configs
//! (E5 — the AI-drivable CLI is the point of this verb), and anything that
//! wants a schema. Both formats go through the SAME serde types in
//! `ksx_config::interop`, so they cannot drift: a field added to a config type
//! is in both the moment it compiles.
//!
//! # What import actually writes
//!
//! Canonical TOML, unless the target file is already a `.json` (the store
//! preserves the format a file had). So the normal round trip is
//! JSON in → TOML on disk, and **the comments in an overwritten file do not
//! survive** — the same trade `ksx map` makes for atomic validated writes,
//! which is exactly why every overwritten file is copied to
//! `<file>.bak-YYYYMMDD-HHMMSS` first.
//!
//! # Consent
//!
//! `import` is a **DRY RUN unless `--yes`**, the same shape as
//! `ksx install-drivers` and `ksx winusb claim|release`: it reads, validates
//! and prints exactly what it would do, and changes nothing. `--dry-run` says
//! so explicitly (and wins over `--yes`).
//!
//! # Exit codes
//!
//! | 0 | exported, imported, or a dry-run report |
//! | 1 | error (I/O, a config root that will not load) |
//! | 2 | refused — validation faults, an unreadable document, an unknown `--preset`, a `--what` the document cannot satisfy. NOTHING was written |
//! | 3 | some files were written and then a write failed (the report names both halves) |

use std::path::{Path, PathBuf};

use ksx_config::interop::{self, Bundle, Format, JsonStyle, Part};
use ksx_config::{ConfigError, ConfigRoot, Issue, PresetFile, Store, Warning};

/// Refused — nothing was written.
pub const EXIT_REFUSED: i32 = 2;
/// Some files were written, then a write failed.
pub const EXIT_PARTIAL_WRITE: i32 = 3;

/// Where an exported document goes.
pub enum Destination {
    /// The default: the document IS stdout, so `ksx config export | jq` works
    /// and the summary goes to stderr.
    Stdout,
    File(PathBuf),
}

/// Where an imported document comes from.
pub enum Origin {
    Stdin,
    File(PathBuf),
}

pub struct ExportOptions {
    /// Empty = every part.
    pub what: Vec<Part>,
    /// Export only this preset (by its `name` field, not its file name).
    pub preset: Option<String>,
    pub out: Destination,
    pub style: JsonStyle,
    /// Machine-readable SUMMARY. The document itself is always JSON.
    pub json: bool,
}

pub struct ImportOptions {
    pub source: Origin,
    /// Empty = whatever the document carries.
    pub what: Vec<Part>,
    /// Report and stop, explicitly.
    pub dry_run: bool,
    /// Actually write. Without it this verb is a report.
    pub yes: bool,
    /// Write even though validation found faults (advisories never block).
    pub force: bool,
    pub json: bool,
}

// ---------------------------------------------------------------------------
// The reusable halves
//
// Everything below this banner and above `export` is the part of these two
// verbs that has NO console in it: read a root into a bundle, read a document
// into a bundle, work out what writing it would do, do it. The CLI entry
// points are thin wrappers that add printing and exit codes.
//
// Split out because there is a second front door now — Studio's `/setup` page
// reaches the identical machinery through
// `ksx_api::MachineSource::{config_export, config_import}` — and the CLI's own
// answer shape cannot be reused from a request handler: `refuse` below is
// `-> !` and calls `std::process::exit`, which inside axum would take the whole
// server down instead of returning a 303. One reader, one writer, two front
// doors (docs/SURFACES.md §1).
// ---------------------------------------------------------------------------

/// A refusal that has not yet been acted on: the stable `--json` code and the
/// sentence. The CLI hands one to [`refuse`]; a library caller turns it into a
/// `ksx_api::Refusal`.
pub(crate) enum Fault {
    Refused { code: &'static str, message: String },
    Error(anyhow::Error),
}

impl Fault {
    pub(crate) fn refused(code: &'static str, message: impl Into<String>) -> Self {
        Self::Refused {
            code,
            message: message.into(),
        }
    }
}

impl From<ConfigError> for Fault {
    fn from(err: ConfigError) -> Self {
        Self::Error(err.into())
    }
}

impl From<anyhow::Error> for Fault {
    fn from(err: anyhow::Error) -> Self {
        Self::Error(err)
    }
}

/// A config root read into one bundle.
pub(crate) struct Gathered {
    pub(crate) bundle: Bundle,
    pub(crate) warnings: Vec<Warning>,
}

/// The bundle an export of `want` (optionally one named preset) would produce.
pub(crate) fn gather(
    store: &Store,
    want: &[Part],
    preset: Option<&str>,
) -> Result<Gathered, Fault> {
    if preset.is_some() && !want.contains(&Part::Presets) {
        return Err(Fault::refused(
            "bad-selection",
            "--preset selects one preset, so --what must include presets",
        ));
    }

    let mut bundle = Bundle::default();
    let mut warnings: Vec<Warning> = Vec::new();

    if want.contains(&Part::Config) {
        let loaded = store.load_config()?;
        warnings.extend(loaded.warnings);
        bundle.config = Some(loaded.value);
    }
    if want.contains(&Part::Games) {
        let loaded = store.load_games()?;
        warnings.extend(loaded.warnings);
        bundle.games = Some(loaded.value);
    }
    if want.contains(&Part::Presets) {
        let loaded = store.load_presets()?;
        warnings.extend(loaded.warnings);
        let mut presets = loaded.value;
        if let Some(name) = preset {
            presets.retain(|preset| preset.name.eq_ignore_ascii_case(name));
            if presets.is_empty() {
                return Err(Fault::refused(
                    "unknown-preset",
                    format!(
                        "no preset named \"{name}\" in {} — `ksx config export --what presets` \
                         lists what is there",
                        store.root().presets_dir().display()
                    ),
                ));
            }
        }
        bundle.presets = Some(presets);
    }
    Ok(Gathered { bundle, warnings })
}

/// A document read into one bundle, narrowed to `what`.
pub(crate) struct Incoming {
    pub(crate) bundle: Bundle,
    pub(crate) warnings: Vec<Warning>,
}

/// Parse an interop document and narrow it — the "what IS this file" half of
/// [`import`], with the same refusal to guess.
pub(crate) fn read_bundle(label: &Path, text: &str, what: &[Part]) -> Result<Incoming, Fault> {
    // A BARE document (no `ksx_interop`) needs to be told what it is, and only
    // an unambiguous --what can tell it. Anything else is refused rather than
    // guessed — importing the wrong file over the wrong file is the failure
    // this whole verb exists to avoid.
    let bare = match what {
        [only] => Some(*only),
        _ => None,
    };
    let loaded = interop::parse_document(label, text, bare)
        .map_err(|err| Fault::refused(parse_error_code(&err), err.to_string()))?;
    let mut bundle = loaded.value;
    if !what.is_empty() {
        bundle.narrow(what);
    }
    if bundle.is_empty() {
        let asked: Vec<&str> = what.iter().map(|p| p.as_str()).collect();
        return Err(Fault::refused(
            "empty-selection",
            format!(
                "{} carries nothing to import{}",
                label.display(),
                if asked.is_empty() {
                    String::new()
                } else {
                    format!(" for --what {}", asked.join(","))
                }
            ),
        ));
    }
    Ok(Incoming {
        bundle,
        warnings: loaded.warnings,
    })
}

/// What would be WRONG with the state an import produces. Reads only — nothing
/// here writes, and nothing here plans a write either: the fault check has to
/// come first, so that a document which cannot pass validation is refused with
/// "nothing was written" rather than with whatever the planner tripped over.
pub(crate) struct Examination {
    pub(crate) issues: Vec<Issue>,
    pub(crate) warnings: Vec<Warning>,
}

impl Examination {
    /// Validation faults — the ones that refuse the write without `--force`.
    pub(crate) fn faults(&self) -> Vec<&Issue> {
        self.issues.iter().filter(|i| !i.is_advisory()).collect()
    }
}

/// Validate the state the import WOULD PRODUCE, not the document in isolation:
/// a preset bundle is only sound against the config that will reference it, and
/// a config is only sound against the presets that will be on disk when it is
/// read.
pub(crate) fn examine(store: &Store, bundle: &Bundle) -> Result<Examination, ConfigError> {
    let disk_config = store.load_config()?;
    let disk_games = store.load_games()?;
    let disk_presets = store.load_presets()?;
    let mut warnings = Vec::new();
    warnings.extend(disk_config.warnings);
    warnings.extend(disk_games.warnings);
    warnings.extend(disk_presets.warnings);

    let config_after = bundle.config.clone().unwrap_or(disk_config.value);
    let games_after = bundle.games.clone().unwrap_or(disk_games.value);
    let presets_after = merged_presets(disk_presets.value, bundle.presets.as_deref());

    let mut issues = ksx_config::validate(&config_after, &presets_after);
    issues.extend(ksx_config::validate_games(&games_after, &presets_after));

    Ok(Examination { issues, warnings })
}

/// What an applied import really did.
pub(crate) struct Applied {
    pub(crate) written: Vec<PathBuf>,
    pub(crate) backups: Vec<PathBuf>,
    /// The write that stopped it. The paths above ARE on disk either way.
    pub(crate) failure: Option<ConfigError>,
}

/// Write a plan. Every overwrite is backed up FIRST, and the loop stops at the
/// first failure with both halves named — a half-applied import that lies
/// about it is worse than one that says where it stopped.
pub(crate) fn apply_writes(store: &Store, bundle: &Bundle, plan: &[PlannedWrite]) -> Applied {
    let mut applied = Applied {
        written: Vec::new(),
        backups: Vec::new(),
        failure: None,
    };
    for step in plan {
        let outcome = store
            .backup(&step.path)
            .and_then(|backup| {
                if let Some(backup) = backup {
                    applied.backups.push(backup);
                }
                write_part(store, bundle, step)
            })
            .map(|path| applied.written.push(path));
        if let Err(err) = outcome {
            applied.failure = Some(err);
            break;
        }
    }
    applied
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

pub fn export(options: ExportOptions) -> anyhow::Result<()> {
    let store = Store::new(ConfigRoot::discover()?);
    let want = resolve_parts(&options.what, options.preset.is_some());
    let Gathered { bundle, warnings } = match gather(&store, &want, options.preset.as_deref()) {
        Ok(gathered) => gathered,
        Err(Fault::Refused { code, message }) => refuse(options.json, code, &message, &[]),
        Err(Fault::Error(err)) => return Err(err),
    };

    let mut text = bundle.to_json(options.style)?;
    text.push('\n');

    let destination = match &options.out {
        Destination::Stdout => {
            print!("{text}");
            "-".to_owned()
        }
        Destination::File(path) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, text.as_bytes())?;
            path.display().to_string()
        }
    };

    // The SUMMARY goes to stderr, always: stdout is reserved for the document
    // so `ksx config export > cabinet.json` and `| jq` both just work.
    let preset_count = bundle.presets.as_ref().map_or(0, Vec::len);
    if options.json {
        eprintln!(
            "{}",
            serde_json::json!({
                "ok": true,
                "what": bundle.parts().iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                "presets": preset_count,
                "bytes": text.len(),
                "out": destination,
                "style": match options.style { JsonStyle::Pretty => "pretty", JsonStyle::Compact => "compact" },
                "ksx_interop": bundle.ksx_interop,
                "schema_version": bundle.schema_version,
                "warnings": warnings,
            })
        );
    } else {
        let parts: Vec<&str> = bundle.parts().iter().map(|p| p.as_str()).collect();
        eprintln!(
            "exported {} ({preset_count} preset(s)) to {destination} — {} bytes",
            parts.join(", "),
            text.len()
        );
        for warning in &warnings {
            eprintln!("[WARN] {warning}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

/// One file the import would touch.
pub(crate) struct PlannedWrite {
    pub(crate) part: Part,
    pub(crate) path: PathBuf,
    pub(crate) format: Format,
    /// A file is already there: it gets a timestamped backup before the write.
    pub(crate) overwrite: bool,
}

impl PlannedWrite {
    pub(crate) fn action(&self) -> &'static str {
        if self.overwrite {
            "overwrite"
        } else {
            "create"
        }
    }
}

pub fn import(options: ImportOptions) -> anyhow::Result<()> {
    let store = Store::new(ConfigRoot::discover()?);
    let (label, text) = read_source(&options.source)?;

    let Incoming {
        bundle,
        mut warnings,
    } = match read_bundle(&label, &text, &options.what) {
        Ok(incoming) => incoming,
        Err(Fault::Refused { code, message }) => refuse(options.json, code, &message, &[]),
        Err(Fault::Error(err)) => return Err(err),
    };

    let examined = examine(&store, &bundle)?;
    let faults = examined.faults().len();
    warnings.extend(examined.warnings);
    let issues = examined.issues;

    if faults > 0 && !options.force {
        refuse(
            options.json,
            "validation-failed",
            &format!(
                "refusing to import {}: {faults} validation fault(s) in the configuration it \
                 would produce — nothing was written (--force writes anyway)",
                label.display(),
            ),
            &issues,
        );
    }

    let plan = plan_writes(&store, &bundle)?;
    let apply = options.yes && !options.dry_run;
    if !apply {
        report_plan(&options, &label, &bundle, &plan, &issues, &warnings);
        return Ok(());
    }

    let applied = apply_writes(&store, &bundle, &plan);
    report_applied(
        &options,
        &label,
        &bundle,
        &applied.written,
        &applied.backups,
        &issues,
        &warnings,
        &applied.failure,
    );
    if applied.failure.is_some() {
        std::process::exit(EXIT_PARTIAL_WRITE);
    }
    Ok(())
}

/// Disk presets with the imported ones layered ON TOP by name: an import of
/// two presets replaces those two and leaves every other preset file alone,
/// which is what the per-file storage already implies.
fn merged_presets(mut disk: Vec<PresetFile>, imported: Option<&[PresetFile]>) -> Vec<PresetFile> {
    let Some(imported) = imported else {
        return disk;
    };
    for preset in imported {
        match disk
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&preset.name))
        {
            Some(existing) => *existing = preset.clone(),
            None => disk.push(preset.clone()),
        }
    }
    disk
}

/// Which files an import touches, and how. Called AFTER [`examine`]'s fault
/// check, never before it — see that type's docs.
pub(crate) fn plan_writes(
    store: &Store,
    bundle: &Bundle,
) -> Result<Vec<PlannedWrite>, ConfigError> {
    let mut plan = Vec::new();
    let mut push = |part: Part, source: ksx_config::Source| {
        plan.push(PlannedWrite {
            part,
            overwrite: source.path.is_file(),
            path: source.path,
            format: source.format,
        });
    };
    if bundle.config.is_some() {
        push(Part::Config, store.config_source());
    }
    if bundle.games.is_some() {
        push(Part::Games, store.games_source());
    }
    for preset in bundle.presets.iter().flatten() {
        push(Part::Presets, store.preset_source(&preset.name)?);
    }
    Ok(plan)
}

fn write_part(store: &Store, bundle: &Bundle, step: &PlannedWrite) -> Result<PathBuf, ConfigError> {
    match step.part {
        Part::Config => store.save_config(bundle.config.as_ref().expect("planned")),
        Part::Games => store.save_games(bundle.games.as_ref().expect("planned")),
        Part::Presets => {
            let preset = bundle
                .presets
                .iter()
                .flatten()
                .find(|preset| {
                    store
                        .preset_source(&preset.name)
                        .is_ok_and(|source| source.path == step.path)
                })
                .expect("planned");
            store.save_preset(preset)
        }
    }
}

// ---------------------------------------------------------------------------
// reporting
// ---------------------------------------------------------------------------

fn report_plan(
    options: &ImportOptions,
    label: &Path,
    bundle: &Bundle,
    plan: &[PlannedWrite],
    issues: &[Issue],
    warnings: &[Warning],
) {
    if options.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "applied": false,
                "dry_run": true,
                "source": label.display().to_string(),
                "what": bundle.parts().iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                "writes": plan.iter().map(plan_json).collect::<Vec<_>>(),
                "issues": issues,
                "faults": issues.iter().filter(|i| !i.is_advisory()).count(),
                "advisories": issues.iter().filter(|i| i.is_advisory()).count(),
                "warnings": warnings,
                "written": Vec::<String>::new(),
                "backups": Vec::<String>::new(),
            })
        );
        return;
    }
    println!(
        "DRY RUN — nothing written. Re-run with --yes to apply. ({} would be read into {})",
        label.display(),
        plan.first()
            .and_then(|step| step.path.parent())
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "the config root".to_owned())
    );
    for step in plan {
        println!(
            "  {:<8} {} ({}, {})",
            step.part.as_str(),
            step.path.display(),
            step.format,
            step.action()
        );
    }
    if plan.iter().any(|step| step.overwrite) {
        println!(
            "each overwritten file is copied to <file>.bak-YYYYMMDD-HHMMSS first; \
             hand-written COMMENTS in it do not survive the rewrite"
        );
    }
    print_findings(issues, warnings);
}

#[allow(clippy::too_many_arguments)]
fn report_applied(
    options: &ImportOptions,
    label: &Path,
    bundle: &Bundle,
    written: &[PathBuf],
    backups: &[PathBuf],
    issues: &[Issue],
    warnings: &[Warning],
    failure: &Option<ConfigError>,
) {
    if options.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": failure.is_none(),
                "applied": true,
                "dry_run": false,
                "source": label.display().to_string(),
                "what": bundle.parts().iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                "written": written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "backups": backups.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "issues": issues,
                "faults": issues.iter().filter(|i| !i.is_advisory()).count(),
                "advisories": issues.iter().filter(|i| i.is_advisory()).count(),
                "forced": options.force,
                "warnings": warnings,
                "error": failure.as_ref().map(ToString::to_string),
            })
        );
        return;
    }
    println!(
        "imported {} from {}",
        bundle
            .parts()
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        label.display()
    );
    for path in written {
        println!("  wrote {}", path.display());
    }
    for path in backups {
        println!("  backed up {}", path.display());
    }
    print_findings(issues, warnings);
    match failure {
        Some(err) => eprintln!(
            "STOPPED after {} file(s): {err} — the files listed above ARE written; \
             the rest were not",
            written.len()
        ),
        None => {
            println!("a running session applies it after `ksx session reload` (or the next start)")
        }
    }
}

fn print_findings(issues: &[Issue], warnings: &[Warning]) {
    for warning in warnings {
        println!("[WARN] {warning}");
    }
    for issue in issues {
        let tag = if issue.is_advisory() { "WARN" } else { "FAIL" };
        println!("[{tag}] {issue}");
    }
}

fn plan_json(step: &PlannedWrite) -> serde_json::Value {
    serde_json::json!({
        "part": step.part.as_str(),
        "path": step.path.display().to_string(),
        "format": step.format.extension(),
        "action": step.action(),
    })
}

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

pub(crate) fn resolve_parts(what: &[Part], preset_named: bool) -> Vec<Part> {
    if !what.is_empty() {
        return what.to_vec();
    }
    // `--preset X` on its own means presets: asking for one preset and getting
    // the whole cabinet would be a surprise, not a convenience.
    if preset_named {
        vec![Part::Presets]
    } else {
        Part::ALL.to_vec()
    }
}

fn read_source(source: &Origin) -> anyhow::Result<(PathBuf, String)> {
    match source {
        Origin::Stdin => Ok((
            PathBuf::from("<stdin>"),
            std::io::read_to_string(std::io::stdin())?,
        )),
        Origin::File(path) => {
            let text = std::fs::read_to_string(path)?;
            Ok((path.clone(), text))
        }
    }
}

/// Stable `--json` `code` for every refusal — one place, so the CLI and
/// docs/CONTROL-SURFACE.md cannot drift (same rule as `map::error_code`).
fn parse_error_code(err: &ConfigError) -> &'static str {
    match err {
        ConfigError::UntaggedJson { .. } => "untagged-json",
        ConfigError::UnsupportedInteropVersion { .. } => "unsupported-interop",
        ConfigError::UnsupportedSchemaVersion { .. } | ConfigError::MissingSchemaVersion { .. } => {
            "unsupported-schema"
        }
        _ => "bad-json",
    }
}

/// Print the refusal and exit 2. Never returns — every caller is a point where
/// nothing has been written yet, and that is the property the exit code
/// promises.
fn refuse(json: bool, code: &str, message: &str, issues: &[Issue]) -> ! {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": false,
                "code": code,
                "error": message,
                "issues": issues,
            })
        );
    } else {
        eprintln!("{message}");
        for issue in issues {
            let tag = if issue.is_advisory() { "WARN" } else { "FAIL" };
            eprintln!("[{tag}] {issue}");
        }
    }
    std::process::exit(EXIT_REFUSED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::{ConfigFile, GamesFile};

    /// A config root under the scratch dir — never `%APPDATA%\ksx`.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-config-io-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("presets")).unwrap();
            Self(dir)
        }

        fn store(&self) -> Store {
            Store::new(ConfigRoot::at(&self.0))
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn seeded(root: &TempRoot) -> Store {
        let store = root.store();
        store.save_config(&ConfigFile::default()).unwrap();
        store.save_games(&GamesFile::default()).unwrap();
        store
            .save_preset(&PresetFile::from_core(&ksx_core::Preset::builtin_default()))
            .unwrap();
        store
    }

    /// The whole point of the verb, end to end and without a process: a config
    /// root becomes one JSON document and comes back byte-identical.
    #[test]
    fn a_root_survives_export_and_import_as_json() {
        let root = TempRoot::new("round-trip");
        let store = seeded(&root);
        let before_config = store.load_config().unwrap().value;
        let before_games = store.load_games().unwrap().value;
        let before_presets = store.load_presets().unwrap().value;

        let bundle = Bundle {
            config: Some(before_config.clone()),
            games: Some(before_games.clone()),
            presets: Some(before_presets.clone()),
            ..Bundle::default()
        };
        let json = bundle.to_json(JsonStyle::Pretty).unwrap();

        // Wipe and re-import through the same writers `import` uses.
        let back = interop::parse_document(Path::new("<test>"), &json, None).unwrap();
        let plan = plan_writes(&store, &back.value).unwrap();
        assert_eq!(plan.len(), 3);
        assert!(plan.iter().all(|step| step.overwrite));
        assert!(plan.iter().all(|step| step.format == Format::Toml));
        for step in &plan {
            write_part(&store, &back.value, step).unwrap();
        }

        assert_eq!(store.load_config().unwrap().value, before_config);
        assert_eq!(store.load_games().unwrap().value, before_games);
        assert_eq!(store.load_presets().unwrap().value, before_presets);
    }

    /// Import writes CANONICAL TOML even though the document was JSON — the
    /// interop format never becomes the storage format by accident.
    #[test]
    fn importing_json_writes_canonical_toml() {
        let root = TempRoot::new("canonical");
        let store = root.store();
        let bundle = Bundle {
            presets: Some(vec![PresetFile::from_core(
                &ksx_core::Preset::builtin_empty(),
            )]),
            ..Bundle::default()
        };
        for step in plan_writes(&store, &bundle).unwrap() {
            assert_eq!(step.format, Format::Toml);
            assert!(!step.overwrite);
            write_part(&store, &bundle, &step).unwrap();
        }
        let written = store.preset_path("empty").unwrap();
        assert_eq!(written.extension().unwrap(), "toml");
        assert!(std::fs::read_to_string(&written)
            .unwrap()
            .contains("name ="));
    }

    /// ...but a preset that already lives as `.json` keeps living as `.json`.
    #[test]
    fn an_existing_json_file_keeps_its_format() {
        let root = TempRoot::new("keep-json");
        let store = root.store();
        std::fs::write(
            root.0.join("presets").join("empty.json"),
            interop::to_json(
                &PresetFile::from_core(&ksx_core::Preset::builtin_empty()),
                JsonStyle::Pretty,
            )
            .unwrap(),
        )
        .unwrap();
        let mut bundle = Bundle {
            presets: Some(vec![PresetFile::from_core(
                &ksx_core::Preset::builtin_default(),
            )]),
            ..Bundle::default()
        };
        // A DIFFERENT preset name: this one is born canonical.
        let plan = plan_writes(&store, &bundle).unwrap();
        assert_eq!(plan[0].format, Format::Toml);

        bundle.presets = Some(vec![PresetFile::from_core(
            &ksx_core::Preset::builtin_empty(),
        )]);
        let plan = plan_writes(&store, &bundle).unwrap();
        assert_eq!(plan[0].format, Format::Json);
        assert!(plan[0].overwrite);
    }

    /// Every overwrite leaves a road home, with the mapper's name shape so
    /// `ksx map --list-backups` finds it.
    #[test]
    fn overwrites_are_backed_up_first() {
        let root = TempRoot::new("backup");
        let store = seeded(&root);
        let path = store.preset_path("default").unwrap();
        let backup = store.backup(&path).unwrap().expect("a file was there");
        assert!(backup.to_string_lossy().contains(".toml.bak-"));
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            std::fs::read_to_string(&path).unwrap()
        );
        // Nothing there = nothing to copy, and that is not an error.
        assert_eq!(store.backup(&root.0.join("nope.toml")).unwrap(), None);
    }

    /// Imported presets layer over the ones on disk by name; the rest survive.
    #[test]
    fn merge_replaces_by_name_and_keeps_the_others() {
        let mut a = PresetFile::from_core(&ksx_core::Preset::builtin_default());
        a.name = "Keep Me".into();
        let mut b = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        b.name = "Replace Me".into();
        let mut replacement = PresetFile::from_core(&ksx_core::Preset::builtin_default());
        replacement.name = "replace me".into(); // case-insensitive on purpose
        let mut fresh = PresetFile::from_core(&ksx_core::Preset::builtin_empty());
        fresh.name = "Brand New".into();

        let merged = merged_presets(
            vec![a.clone(), b],
            Some(&[replacement.clone(), fresh.clone()]),
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0], a);
        assert_eq!(merged[1], replacement);
        assert_eq!(merged[2], fresh);

        // Nothing imported = nothing changed.
        assert_eq!(merged_presets(vec![a.clone()], None), vec![a]);
    }

    #[test]
    fn part_selection_defaults_are_the_documented_ones() {
        assert_eq!(resolve_parts(&[], false), Part::ALL.to_vec());
        assert_eq!(resolve_parts(&[], true), vec![Part::Presets]);
        assert_eq!(resolve_parts(&[Part::Games], false), vec![Part::Games]);
    }

    #[test]
    fn refusal_codes_are_stable() {
        let path = PathBuf::from("x.json");
        assert_eq!(
            parse_error_code(&ConfigError::UntaggedJson { path: path.clone() }),
            "untagged-json"
        );
        assert_eq!(
            parse_error_code(&ConfigError::UnsupportedInteropVersion {
                path: path.clone(),
                found: 9,
                supported: 1,
            }),
            "unsupported-interop"
        );
        assert_eq!(
            parse_error_code(&ConfigError::MissingSchemaVersion { path: path.clone() }),
            "unsupported-schema"
        );
        assert_eq!(
            parse_error_code(&ConfigError::Parse {
                path,
                message: "boom".into()
            }),
            "bad-json"
        );
    }
}
