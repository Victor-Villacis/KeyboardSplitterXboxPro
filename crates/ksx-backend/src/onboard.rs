//! **First run, and the configuration in and out — with no path in the
//! middle.**
//!
//! Three reads and one write, all of them the ones `ksx config` and `ksx setup`
//! already own. What is new here is only the SHAPE of the answer: a value a
//! surface can render, instead of a console it has to be.
//!
//! # Why the export verb hands back bytes and not a file name
//!
//! `ksx config export` writes to stdout or to a `--out` path, which is exactly
//! right for a shell. It is exactly wrong for a person who asked a page to give
//! them their configuration: a path is a thing they then have to go and find,
//! and putting the filesystem back in front of them is the failure this whole
//! screen exists to avoid. So [`export`] returns the document itself and a
//! suggested file name, and the surface decides what a "download" means there.
//! The bundle is built by the same `config_io::gather` the CLI uses — there is
//! no second serializer and no second set of warnings.
//!
//! # Why import stays a dry run
//!
//! Unchanged from the CLI, deliberately (`crate::config_io` module docs):
//! [`ImportRequest::apply`] is what writes, every overwritten file is copied to
//! `<file>.bak-YYYYMMDD-HHMMSS` first, and validation faults refuse the write
//! unless `force` says otherwise. Replacing a cabinet's whole configuration is
//! not a one-click action, and a web form is the last place to make it one.
//!
//! # Why the CHECKLIST is computed here
//!
//! Because "which step is next" is a decision about configuration, and
//! docs/SURFACES.md §1 puts decisions in the backend. [`plan_steps`] is pure
//! over three counts, which is what makes the rule testable without a config
//! root — and what stops the egui and Studio from growing two answers to the
//! same question.

use std::path::Path;

use ksx_api::{
    codes, setup_states, setup_steps, ConfigExport, ExportRequest, ImportReport, ImportRequest,
    ImportWrite, Refusal, SetupDeviceRow, SetupSlotRow, SetupStep, SetupView,
};
use ksx_config::interop::{JsonStyle, Part};
use ksx_config::{ConfigFile, ConfigRoot, GamesFile, Store};

use crate::config_io::{self, Fault};

/// What the document a surface hands us is called in every message about it.
/// It is not a path and must never look like one.
const DOCUMENT_LABEL: &str = "the pasted document";

// ---------------------------------------------------------------------------
// the checklist
// ---------------------------------------------------------------------------

/// How far a config root has got, as the three counts the checklist turns on.
///
/// Counts rather than the rows themselves, so [`plan_steps`] is pure, total and
/// testable with no store, no filesystem and no platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// `[[device]]` entries — boards this cabinet has been told about.
    pub devices: usize,
    /// Wired slots, from `config.toml` and from every games.toml profile.
    pub slots: usize,
    /// Presets on disk.
    pub presets: usize,
}

/// The first-run checklist for one [`Progress`].
///
/// **Exactly one step is `now`, always.** The last step can never be `done` —
/// nothing on disk records that a human pressed a button and watched ksx name
/// it — so there is always a next thing, and the page never has to render an
/// empty "what now?".
pub fn plan_steps(progress: Progress) -> Vec<SetupStep> {
    let board_done = progress.devices > 0;
    // A games.toml profile persists device ids directly, so a cabinet can have
    // wired slots and no named board. The board step still comes first: a slot
    // that names a raw instance path is the thing `ksx device pick` exists to
    // replace.
    let slot_done = progress.slots > 0;

    vec![
        SetupStep {
            id: setup_steps::BOARD.to_owned(),
            title: "Find your board and name it".to_owned(),
            detail: match progress.devices {
                0 => "Nothing is named yet. ksx needs to know which keyboard or \
                      encoder is the panel before it can listen to one."
                    .to_owned(),
                1 => "One board is named. Add another if this cabinet has a \
                      second panel."
                    .to_owned(),
                n => format!("{n} boards are named."),
            },
            state: if board_done {
                setup_states::DONE
            } else {
                setup_states::NOW
            }
            .to_owned(),
        },
        SetupStep {
            id: setup_steps::SLOT.to_owned(),
            title: "Wire a slot".to_owned(),
            detail: match (progress.slots, progress.presets) {
                (0, 0) => "No slot is wired and there are no presets to point one \
                           at. Import a configuration, or run `ksx preset new`."
                    .to_owned(),
                (0, _) => "No slot is wired yet. A slot is one player: a preset, \
                           and the pad it presents itself as."
                    .to_owned(),
                (1, _) => "One slot is wired — this cabinet is a one-player \
                           cabinet until a second one is."
                    .to_owned(),
                (n, _) => format!("{n} slots are wired."),
            },
            state: if slot_done {
                setup_states::DONE
            } else if board_done {
                setup_states::NOW
            } else {
                setup_states::LATER
            }
            .to_owned(),
        },
        SetupStep {
            id: setup_steps::PROVE.to_owned(),
            title: "Press a button and watch it land".to_owned(),
            // Never `done`: ksx cannot tick this one for you, and a checkbox it
            // ticked would be a claim it has no way to make.
            detail: if board_done && slot_done {
                "Start the listener, press a button on the panel, and ksx names \
                 the key it saw and the device it came from. This is the only \
                 step that proves the wiring rather than describing it."
                    .to_owned()
            } else {
                "Once a board is named and a slot is wired, press a button here \
                 and ksx will name the key it saw."
                    .to_owned()
            },
            state: if board_done && slot_done {
                setup_states::NOW
            } else {
                setup_states::LATER
            }
            .to_owned(),
        },
    ]
}

// ---------------------------------------------------------------------------
// the read
// ---------------------------------------------------------------------------

/// What the config root already holds, plus the checklist derived from it.
///
/// Nothing here needs a daemon, and nothing here is cached — the same rule as
/// every other collector in `crate::sources`.
pub fn state() -> Result<SetupView, Refusal> {
    let root = discover()?;
    let store = Store::new(root.clone());
    let mut notes: Vec<String> = Vec::new();

    let config = match store.load_config() {
        Ok(loaded) => {
            notes.extend(loaded.warnings.iter().map(ToString::to_string));
            loaded.value
        }
        Err(err) => {
            // A config that will not parse is a FACT to render, not a reason to
            // show the user nothing: the import panel below it is very likely
            // the way out of exactly this state.
            notes.push(format!("the config file could not be read: {err}"));
            ConfigFile::default()
        }
    };
    let games = match store.load_games() {
        Ok(loaded) => {
            notes.extend(loaded.warnings.iter().map(ToString::to_string));
            loaded.value
        }
        Err(err) => {
            notes.push(format!("the games profiles could not be read: {err}"));
            GamesFile::default()
        }
    };
    let presets: Vec<String> = match store.load_presets() {
        Ok(loaded) => {
            notes.extend(loaded.warnings.iter().map(ToString::to_string));
            loaded.value.into_iter().map(|file| file.name).collect()
        }
        Err(err) => {
            notes.push(format!("the presets folder could not be read: {err}"));
            Vec::new()
        }
    };

    let devices: Vec<SetupDeviceRow> = config
        .devices
        .iter()
        .map(|device| SetupDeviceRow {
            alias: device.alias.clone(),
            // The RAW spelling, which is what is in the file — never the
            // canonicalised one (docs/DEVICE-IDENTITY.md: a config that says
            // one thing and a screen that says another is a support call).
            id: device.id.raw().to_owned(),
            backend: match device.backend {
                ksx_config::Backend::Interception => "interception",
                ksx_config::Backend::Winusb => "winusb",
            }
            .to_owned(),
        })
        .collect();

    let mut slots: Vec<SetupSlotRow> = config
        .slots
        .iter()
        .map(|slot| SetupSlotRow {
            number: slot.number,
            device: slot.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
            preset: slot.preset.clone(),
            persona: slot.persona.label().to_owned(),
            source: "config.toml".to_owned(),
        })
        .collect();
    for game in &games.games {
        slots.extend(game.slots.iter().map(|slot| SetupSlotRow {
            number: slot.number,
            device: slot.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
            preset: slot.preset.clone(),
            persona: slot.persona.label().to_owned(),
            source: game.title.clone(),
        }));
    }

    let progress = Progress {
        devices: devices.len(),
        slots: slots.len(),
        presets: presets.len(),
    };

    Ok(SetupView {
        generated_at: now_utc(),
        config_root: root.dir().display().to_string(),
        config_exists: store.config_source().path.is_file(),
        devices,
        slots,
        presets,
        profiles: games.games.iter().map(|g| g.title.clone()).collect(),
        steps: plan_steps(progress),
        notes,
        // The one place this number may live is `ksx_core` (task #17). A
        // surface renders the menu; it does not get to decide how long it is.
        max_slots: ksx_core::MAX_SLOTS,
    })
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/// The config root as one interop JSON document, in memory.
pub fn export(request: &ExportRequest) -> Result<ConfigExport, Refusal> {
    let store = Store::new(discover()?);
    let want = config_io::resolve_parts(&parts_of(&request.what)?, false);
    let gathered = config_io::gather(&store, &want, None).map_err(refusal_of)?;

    let mut document = gathered.bundle.to_json(JsonStyle::Pretty).map_err(|err| {
        Refusal::new(
            codes::REFUSED,
            format!("the configuration could not be rendered as JSON: {err}"),
        )
    })?;
    document.push('\n');

    Ok(ConfigExport {
        filename: format!("ksx-config-{}.json", stamp()),
        bytes: document.len(),
        parts: gathered
            .bundle
            .parts()
            .iter()
            .map(|part| part.as_str().to_owned())
            .collect(),
        presets: gathered.bundle.presets.as_ref().map_or(0, Vec::len),
        warnings: gathered.warnings.iter().map(ToString::to_string).collect(),
        document,
    })
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

/// One import attempt. **Dry run unless `request.apply`.**
pub fn import(request: &ImportRequest) -> Result<ImportReport, Refusal> {
    let store = Store::new(discover()?);
    let want = parts_of(&request.what)?;
    let label = Path::new(DOCUMENT_LABEL);

    let incoming = config_io::read_bundle(label, &request.document, &want).map_err(refusal_of)?;
    let examined = config_io::examine(&store, &incoming.bundle).map_err(|err| {
        Refusal::new(
            codes::REFUSED,
            format!("the configuration on disk could not be read to check against: {err}"),
        )
    })?;

    let faults: Vec<String> = examined.faults().iter().map(ToString::to_string).collect();
    let advisories: Vec<String> = examined
        .issues
        .iter()
        .filter(|issue| issue.is_advisory())
        .map(ToString::to_string)
        .collect();
    let mut warnings: Vec<String> = incoming.warnings.iter().map(ToString::to_string).collect();
    warnings.extend(examined.warnings.iter().map(ToString::to_string));
    let parts: Vec<String> = incoming
        .bundle
        .parts()
        .iter()
        .map(|part| part.as_str().to_owned())
        .collect();
    let mut report = ImportReport {
        ok: true,
        applied: false,
        summary: String::new(),
        error: None,
        parts,
        writes: Vec::new(),
        faults,
        advisories,
        warnings,
        written: Vec::new(),
        backups: Vec::new(),
    };

    // Faults refuse the write, and refuse it for a dry run too — a "this is
    // fine" report that a later Apply would reject is worse than no report.
    // Checked BEFORE anything is planned, so "nothing was written" is a
    // statement about a decision rather than about how far it got.
    if !report.faults.is_empty() && !request.force {
        let faults = report.faults.len();
        // The COUNT is the fact; the faults themselves ride in
        // `report.faults`, which every surface already receives. This sentence
        // used to end "`ksx config import --dry-run` lists them", which handed
        // a web page's user to a shell to read a list the page was holding.
        return Ok(ImportReport {
            ok: false,
            summary: format!(
                "refused: importing this would leave {faults} validation fault(s) in your \
                 configuration — nothing was written."
            ),
            ..report
        });
    }

    let plan = config_io::plan_writes(&store, &incoming.bundle).map_err(|err| {
        Refusal::new(
            codes::REFUSED,
            format!("the files this import would write could not be worked out: {err}"),
        )
    })?;
    report.writes = plan
        .iter()
        .map(|step| ImportWrite {
            part: step.part.as_str().to_owned(),
            path: step.path.display().to_string(),
            action: step.action().to_owned(),
            format: step.format.extension().to_owned(),
        })
        .collect();

    if !request.apply {
        return Ok(ImportReport {
            summary: dry_run_summary(&report),
            ..report
        });
    }

    let applied = config_io::apply_writes(&store, &incoming.bundle, &plan);
    let written: Vec<String> = applied
        .written
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    let backups: Vec<String> = applied
        .backups
        .iter()
        .map(|path| path.display().to_string())
        .collect();

    let summary = match &applied.failure {
        Some(err) => format!(
            "STOPPED after {} file(s): {err} — the files it names were written, the rest \
             were not",
            written.len()
        ),
        None => format!(
            "imported {} — {} file(s) written{}",
            parts_in_words(&report),
            written.len(),
            if backups.is_empty() {
                String::new()
            } else {
                format!(", {} copied first so there is a way back", backups.len())
            }
        ),
    };

    Ok(ImportReport {
        ok: applied.failure.is_none(),
        applied: true,
        summary,
        error: applied.failure.as_ref().map(ToString::to_string),
        written,
        backups,
        ..report
    })
}

/// The dry run's one sentence: what it WOULD bring in, what it would replace,
/// and the fact that it did nothing.
///
/// It talks about PARTS, not paths. "config.toml, games.toml and
/// presets/IPAC P1.toml" is a list of files, and a person importing a
/// configuration is not thinking in files — "your settings, boards and slots"
/// is the same fact in the vocabulary they are actually working in. The count
/// of REPLACED files rides along, because "replace" is the word that needs a
/// number beside it.
///
/// **And it names no control.** This sentence used to end `Tick "write it" and
/// import again to apply.` — the label on ONE checkbox on ONE web page, baked
/// into the shared backend every `MachineSource` consumer reads, cabinet egui
/// included. The backend states the fact (`applied: false`, and a plan in
/// `writes`); each surface names its own consent control on top of it.
fn dry_run_summary(report: &ImportReport) -> String {
    let overwrites = report
        .writes
        .iter()
        .filter(|write| write.action == "overwrite")
        .count();
    let mut line = format!(
        "nothing written yet — this would bring in {}",
        parts_in_words(report)
    );
    if overwrites > 0 {
        line.push_str(&format!(
            ", replacing {overwrites} file(s) — each one copied first"
        ));
    }
    if !report.advisories.is_empty() {
        line.push_str(&format!(
            ", with {} advisory note(s)",
            report.advisories.len()
        ));
    }
    line.push_str(". Nothing was written.");
    line
}

/// The parts a document carries, in the words a person uses for them.
fn parts_in_words(report: &ImportReport) -> String {
    let presets = report
        .writes
        .iter()
        .filter(|write| write.part == "presets")
        .count();
    let words: Vec<String> = report
        .parts
        .iter()
        .map(|part| match part.as_str() {
            "config" => "your settings, boards and slots".to_owned(),
            "games" => "your game profiles".to_owned(),
            "presets" => match presets {
                1 => "1 preset".to_owned(),
                n => format!("{n} presets"),
            },
            // A part a newer ksx knows and this one does not: name it as it
            // came, rather than dropping it out of the sentence.
            other => other.to_owned(),
        })
        .collect();
    match words.as_slice() {
        [] => "nothing".to_owned(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

fn discover() -> Result<ConfigRoot, Refusal> {
    ConfigRoot::discover().map_err(|err| {
        Refusal::with_remedy(
            codes::REFUSED,
            format!("the config root could not be found: {err}"),
            "run `ksx doctor`",
        )
    })
}

/// `["config", "presets"]` → the typed parts, refusing an unknown word rather
/// than silently exporting everything.
fn parts_of(what: &[String]) -> Result<Vec<Part>, Refusal> {
    what.iter()
        .filter(|word| !word.trim().is_empty())
        .map(|word| {
            Part::parse(word).ok_or_else(|| {
                Refusal::new(
                    codes::BAD_REQUEST,
                    format!("\"{word}\" is not a config part (config | games | presets)"),
                )
            })
        })
        .collect()
}

/// A `config_io` fault as the refusal a surface renders.
///
/// The CODE travels: these are the same stable words `ksx config import --json`
/// prints (`untagged-json`, `validation-failed`, …), which is the opposite of
/// inventing one per caller — flattening them all to `refused` would throw away
/// the only machine-readable half of the answer.
fn refusal_of(fault: Fault) -> Refusal {
    match fault {
        Fault::Refused { code, message } => Refusal::new(code, message),
        Fault::Error(err) => Refusal::new(codes::REFUSED, err.to_string()),
    }
}

fn now_utc() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// `YYYYMMDD-HHMMSS` — the same stamp shape the store's backups use, so an
/// exported document sorts next to them.
fn stamp() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the page is written against: there is ALWAYS exactly one
    /// next step, whatever state the machine is in. A checklist with two
    /// "do this now" rows asks the user to choose, which is the thing a first
    /// run exists to spare them.
    #[test]
    fn exactly_one_step_is_next_for_every_state_of_the_machine() {
        for devices in 0..3 {
            for slots in 0..3 {
                for presets in 0..3 {
                    let progress = Progress {
                        devices,
                        slots,
                        presets,
                    };
                    let steps = plan_steps(progress);
                    let now = steps
                        .iter()
                        .filter(|s| s.state == setup_states::NOW)
                        .count();
                    assert_eq!(now, 1, "{progress:?} produced {now} next steps: {steps:?}");
                    assert_eq!(steps.len(), 3, "{progress:?}");
                }
            }
        }
    }

    /// The order is the order a person performs them in, and the ids are what a
    /// surface routes on — a rename here is a page that links nowhere.
    #[test]
    fn the_checklist_ids_are_the_documented_ones_in_order() {
        let steps = plan_steps(Progress::default());
        let ids: Vec<&str> = steps.iter().map(|step| step.id.as_str()).collect();
        assert_eq!(
            ids,
            [setup_steps::BOARD, setup_steps::SLOT, setup_steps::PROVE]
        );
        // Every step says something. A blank row in a checklist is a row the
        // user cannot act on.
        for step in &steps {
            assert!(!step.title.trim().is_empty(), "{step:?}");
            assert!(!step.detail.trim().is_empty(), "{step:?}");
        }
    }

    /// A fresh machine is told to find its board, and nothing else is offered
    /// as actionable.
    #[test]
    fn a_fresh_machine_is_pointed_at_the_board_step() {
        let steps = plan_steps(Progress::default());
        assert_eq!(steps[0].id, setup_steps::BOARD);
        assert_eq!(steps[0].state, setup_states::NOW);
        assert_eq!(steps[1].state, setup_states::LATER);
        assert_eq!(steps[2].state, setup_states::LATER);
    }

    /// Proving is never ticked off for you — see [`plan_steps`].
    #[test]
    fn the_prove_step_is_never_done() {
        for devices in 0..3 {
            for slots in 0..3 {
                let steps = plan_steps(Progress {
                    devices,
                    slots,
                    presets: 1,
                });
                assert_ne!(steps[2].state, setup_states::DONE);
            }
        }
    }

    /// A cabinet whose slots live in a games.toml profile has wired slots and
    /// no named board. The next step is still the board — a slot that names a
    /// raw instance path is what `ksx device pick` replaces.
    #[test]
    fn wired_slots_without_a_named_board_still_ask_for_the_board() {
        let steps = plan_steps(Progress {
            devices: 0,
            slots: 2,
            presets: 1,
        });
        assert_eq!(steps[0].state, setup_states::NOW);
        assert_eq!(steps[1].state, setup_states::DONE);
        assert_eq!(steps[2].state, setup_states::LATER);
    }

    /// An unknown `--what` word is refused rather than quietly meaning
    /// "everything" — importing the wrong part over the wrong part is the
    /// failure this verb exists to prevent.
    #[test]
    fn an_unknown_part_is_refused_by_name() {
        let refusal = parts_of(&["presets".to_owned(), "keybindings".to_owned()]).unwrap_err();
        assert_eq!(refusal.code, codes::BAD_REQUEST);
        assert!(refusal.message.contains("keybindings"), "{refusal}");
        // Blank entries are what an empty form field sends; they mean nothing,
        // not an error.
        assert_eq!(
            parts_of(&["".to_owned(), " ".to_owned()]).unwrap(),
            Vec::<Part>::new()
        );
        assert_eq!(
            parts_of(&["config".to_owned()]).unwrap(),
            vec![Part::Config]
        );
    }

    /// The dry-run sentence has to survive a 300-character `?flash=` cap
    /// (ksx-studio `urlencode`) and still say the two things that matter: that
    /// nothing was written, and how to make it write.
    fn write(part: &str, action: &str) -> ImportWrite {
        ImportWrite {
            part: part.to_owned(),
            // A real path, so the test would catch one leaking into the
            // sentence. This summary is what a `?flash=` carries, and the whole
            // point of it is that it is not a list of files.
            path: format!("C:\\cfg\\ksx\\{part}.toml"),
            action: action.to_owned(),
            format: "toml".to_owned(),
        }
    }

    /// The one sentence that has to survive a 300-character `?flash=` cap
    /// (ksx-studio `urlencode`) AND say the three things that matter: what it
    /// would bring in, that nothing happened, and how to make it happen.
    #[test]
    fn the_dry_run_summary_fits_in_a_flash_and_says_nothing_happened() {
        let report = ImportReport {
            parts: vec![
                "config".to_owned(),
                "games".to_owned(),
                "presets".to_owned(),
            ],
            writes: vec![
                write("config", "overwrite"),
                write("games", "create"),
                write("presets", "overwrite"),
                write("presets", "create"),
            ],
            advisories: vec!["slot 2 has no Start".to_owned()],
            ..ImportReport::default()
        };
        let summary = dry_run_summary(&report);
        assert!(summary.contains("nothing written yet"), "{summary}");
        assert!(
            summary.contains("your settings, boards and slots"),
            "{summary}"
        );
        assert!(summary.contains("your game profiles"), "{summary}");
        assert!(summary.contains("2 presets"), "{summary}");
        assert!(summary.contains("replacing 2 file(s)"), "{summary}");
        assert!(summary.contains("copied first"), "{summary}");
        assert!(summary.contains("1 advisory note(s)"), "{summary}");
        assert!(summary.contains("Nothing was written."), "{summary}");
        // …and it names no CONTROL. A checkbox label ("write it") belongs to
        // one web form; this sentence is read by the cabinet egui and by any
        // other MachineSource consumer, none of which has that box.
        for surface_only in ["Tick", "tick", "box", "checkbox", "click", "button"] {
            assert!(
                !summary.contains(surface_only),
                "the shared backend named a surface's own control ({surface_only:?}): {summary}"
            );
        }
        assert!(
            summary.chars().count() <= 300,
            "{} chars, and the flash truncates at 300: {summary}",
            summary.chars().count()
        );
        // …and no path in it. This page does not hand people directories.
        assert!(!summary.contains("C:\\cfg"), "{summary}");
        assert!(!summary.contains(".toml"), "{summary}");
    }

    /// One part reads as one phrase, not as a list with a stray "and".
    #[test]
    fn one_part_reads_as_one_phrase() {
        let report = ImportReport {
            parts: vec!["presets".to_owned()],
            writes: vec![write("presets", "create")],
            ..ImportReport::default()
        };
        assert_eq!(parts_in_words(&report), "1 preset");
        let summary = dry_run_summary(&report);
        assert!(summary.contains("bring in 1 preset."), "{summary}");
        assert!(
            !summary.contains("replacing"),
            "nothing is replaced: {summary}"
        );
    }
}
