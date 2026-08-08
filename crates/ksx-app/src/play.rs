//! `ksx play` — replay a recorded session, live.
//!
//! `ksx monitor --record` writes a timeline; this plays it back into the real
//! pipeline. The session that results is an ordinary one in every respect —
//! same plan, same presets, same personas, same pads, same teardown order — and
//! the only difference is that its [`ksx_capture::CaptureBackend`] reads a file
//! instead of a driver. That is deliberate: a replay is worth having exactly
//! because it is *not* a special path. What it drives is what a player drives.
//!
//! # The one hard problem: a recording names devices that may not exist
//!
//! A recorded id is the id the board had **when the recording was made**. After
//! a replug it can be a different string for the same board; on another machine
//! it names nothing at all. So a recording is resolved before anything is
//! plugged, through the same selector path a session uses
//! ([`crate::run::resolve`], `docs/DEVICE-IDENTITY.md` §9), and `--as` remaps a
//! recorded device onto a configured one.
//!
//! The refusal rule mirrors the one `crate::run::resolve::drop_missing` argues
//! for, and for the same reason: a recorded device that drives no slot is
//! *noted and ignored*, because that is precisely what an unassigned keyboard
//! does in a live session — the cabinet corpus contains a desk keyboard that
//! was never bound to anything. But a recording where **nothing** drives a slot
//! is refused before the pads are plugged, naming every recorded id and the
//! flag that fixes it. Starting that session would plug four pads and move none
//! of them, which is the "reports success while the panel is dead" failure this
//! project has a landmine entry about.
//!
//! # Live input is suppressed while a recording plays
//!
//! The driven boards are captured exactly as `ksx run` captures them — so their
//! keystrokes do not reach Windows — and their events are discarded rather than
//! merged into the timeline ([`ksx_capture::Silenced`]). Mixing the two means
//! the player at the panel fights the recording, in the game, where the game
//! sees both. The emergency escapes still work, because the silenced backend is
//! a real one watching the real board and shares this session's latch.

use std::io::Write;
use std::path::PathBuf;

use ksx_capture::{Recording, RecordingError, ReplayProgress, Speed, SpeedError};
use ksx_config::DeviceEntry;
use ksx_core::{DeviceFacts, DeviceId, DeviceRef, DeviceSelector, Match};

use crate::run::plan::RunPlan;
use crate::run::supervisor::{HookStop, SessionHook};

/// Everything `ksx play` was asked for.
pub struct Options {
    pub file: PathBuf,
    /// `--as` specs, in the order given.
    pub remap: Vec<String>,
    pub speed: f64,
    pub looping: bool,
    pub game: Option<String>,
    pub no_launch: bool,
    pub dry_run: bool,
    pub latency: bool,
    pub json: bool,
}

// ---------------------------------------------------------------------------
// --as
// ---------------------------------------------------------------------------

/// One `--as` spec: `TARGET`, or `FROM=TARGET`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Remap {
    /// The recorded id to move. `None` means "the one device this recording
    /// names" — which is only unambiguous when it names exactly one.
    pub from: Option<String>,
    /// A `[[device]]` alias, or an id/selector spelled out.
    pub target: String,
}

impl Remap {
    pub(crate) fn parse(spec: &str) -> Result<Self, PlayError> {
        // Split on the FIRST `=`: a target is an alias or a selector
        // (`usb:d209:0430:00:port=7&25EEA38C&0&0000`), and a `port=` qualifier
        // has an `=` of its own. Splitting on the last one would cut a selector
        // in half and blame the user for it.
        let (from, target) = match spec.split_once('=') {
            // A bare `usb:…` target contains `=` only inside a qualifier, which
            // always follows a `:` — so a spec whose left half looks like a
            // selector rung is not a FROM at all.
            Some((left, right)) if !left.contains(':') => {
                (Some(left.trim().to_owned()), right.trim().to_owned())
            }
            _ => (None, spec.trim().to_owned()),
        };
        if target.is_empty() || from.as_ref().is_some_and(String::is_empty) {
            return Err(PlayError::BadRemap {
                spec: spec.to_owned(),
            });
        }
        Ok(Self { from, target })
    }
}

// ---------------------------------------------------------------------------
// The resolved recording
// ---------------------------------------------------------------------------

/// A recorded device that drives slots in this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Driven {
    pub id: DeviceId,
    pub events: usize,
    pub slots: Vec<u8>,
}

/// A recording, resolved against a plan and ready to play.
#[derive(Clone, Debug)]
pub(crate) struct PlayPlan {
    /// Device ids rewritten to what this session actually drives.
    pub recording: Recording,
    pub driving: Vec<Driven>,
    /// Recorded devices no slot is bound to: seen by the engine, routed
    /// nowhere — exactly like an unassigned keyboard in a live session.
    pub ignored: Vec<(DeviceId, usize)>,
    pub notes: Vec<String>,
}

impl PlayPlan {
    /// Events that will actually move a pad.
    pub fn driving_events(&self) -> usize {
        self.driving.iter().map(|d| d.events).sum()
    }
}

/// Why a replay was refused. Every variant happens **before** a pad is plugged
/// or a keyboard filter is armed: exit code 2, nothing touched.
#[derive(Clone, Debug)]
pub(crate) enum PlayError {
    Unreadable {
        path: String,
        detail: String,
    },
    Recording(RecordingError),
    Speed(SpeedError),
    BadRemap {
        spec: String,
    },
    /// `--as TARGET` on a recording that names several devices: which one?
    UnnamedRemap {
        target: String,
        recorded: Vec<DeviceId>,
    },
    /// `--as FROM=…` where `FROM` is not in the recording.
    RemapMatchedNothing {
        from: String,
        recorded: Vec<DeviceId>,
    },
    /// A `--as` target that is neither a `[[device]]` alias nor a parseable id.
    UnknownTarget {
        target: String,
        known: Vec<String>,
    },
    /// A `--as` target that parses but names no connected board.
    TargetMissing {
        target: String,
    },
    /// A `--as` target that names more than one.
    TargetAmbiguous {
        target: String,
        hits: Vec<DeviceId>,
    },
    /// The whole point of the resolution pass: this recording moves nothing.
    NothingDrives {
        file: String,
        recorded: Vec<(DeviceId, usize)>,
        /// `(id, alias if the config has one, slots)`.
        driven: Vec<(DeviceId, Option<String>, Vec<u8>)>,
    },
}

impl PlayError {
    /// Stable `--json` error code.
    pub fn code(&self) -> &'static str {
        match self {
            PlayError::Unreadable { .. } => "recording-unreadable",
            PlayError::Recording(_) => "recording-invalid",
            PlayError::Speed(_) => "speed-invalid",
            PlayError::BadRemap { .. }
            | PlayError::UnnamedRemap { .. }
            | PlayError::RemapMatchedNothing { .. } => "remap-invalid",
            PlayError::UnknownTarget { .. }
            | PlayError::TargetMissing { .. }
            | PlayError::TargetAmbiguous { .. } => "remap-target-unresolved",
            PlayError::NothingDrives { .. } => "recording-drives-nothing",
        }
    }
}

impl std::fmt::Display for PlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlayError::Unreadable { path, detail } => write!(
                f,
                "'{path}' could not be read ({detail}). `ksx monitor --record <FILE>` is what \
                 writes one"
            ),
            PlayError::Recording(err) => write!(f, "this is not a recording ksx can play: {err}"),
            PlayError::Speed(err) => write!(f, "{err}"),
            PlayError::BadRemap { spec } => write!(
                f,
                "`--as {spec}` is not a remap. Write `--as <alias|selector>` when the recording \
                 names one device, or `--as \"<recorded id>=<alias|selector>\"` to say which one"
            ),
            PlayError::UnnamedRemap { target, recorded } => {
                writeln!(
                    f,
                    "`--as {target}` does not say WHICH recorded device to remap, and this \
                     recording names {}:",
                    recorded.len()
                )?;
                for id in recorded {
                    writeln!(f, "  {id}")?;
                }
                write!(f, "Use `--as \"<one of those>={target}\"`")
            }
            PlayError::RemapMatchedNothing { from, recorded } => {
                writeln!(
                    f,
                    "`--as {from}=…` names a device this recording never mentions. It holds \
                     events from:"
                )?;
                for id in recorded {
                    writeln!(f, "  {id}")?;
                }
                write!(
                    f,
                    "The left side of a remap is the id AS RECORDED, copied from that list"
                )
            }
            PlayError::UnknownTarget { target, known } => {
                write!(
                    f,
                    "`--as …={target}`: '{target}' is neither a [[device]] alias in config.toml \
                     nor a device id or selector"
                )?;
                if known.is_empty() {
                    write!(
                        f,
                        ". config.toml has no [[device]] entries at all — paste the id \
                         `ksx devices` prints instead"
                    )
                } else {
                    write!(f, ". Known aliases: {}", known.join(", "))
                }
            }
            PlayError::TargetMissing { target } => write!(
                f,
                "`--as …={target}`: nothing connected answers to it. `ksx devices` prints what \
                 is actually there"
            ),
            PlayError::TargetAmbiguous { target, hits } => {
                writeln!(
                    f,
                    "`--as …={target}`: {} connected interfaces answer to it, so ksx cannot tell \
                     which board you mean — and will not guess:",
                    hits.len()
                )?;
                for id in hits {
                    writeln!(f, "  {id}")?;
                }
                write!(
                    f,
                    "Remap onto one of those ids, or onto a [[device]] alias that pins one"
                )
            }
            PlayError::NothingDrives {
                file,
                recorded,
                driven,
            } => render_nothing_drives(f, file, recorded, driven),
        }
    }
}

impl std::error::Error for PlayError {}

/// The refusal this whole module exists for.
///
/// It has to answer three questions at once, because a person who has just been
/// told "no" is holding all three: what did I record, what does this machine
/// have, and what do I type now.
fn render_nothing_drives(
    f: &mut std::fmt::Formatter<'_>,
    file: &str,
    recorded: &[(DeviceId, usize)],
    driven: &[(DeviceId, Option<String>, Vec<u8>)],
) -> std::fmt::Result {
    writeln!(
        f,
        "nothing in this recording drives a slot in this session, so playing it would plug \
         the pads and move none of them."
    )?;
    writeln!(f, "  the recording names:")?;
    for (id, events) in recorded {
        writeln!(f, "    {id}  ({events} event(s))")?;
    }
    if driven.is_empty() {
        return write!(
            f,
            "  ...and this session has no slot bound to a keyboard at all, so there is nothing \
             to point it at. `ksx run --dry-run` shows what the plan resolved to."
        );
    }
    writeln!(f, "  this session's slots are driven by:")?;
    for (id, alias, slots) in driven {
        match alias {
            Some(alias) => writeln!(f, "    {id}  -> slot(s) {slots:?}  (alias \"{alias}\")")?,
            None => writeln!(f, "    {id}  -> slot(s) {slots:?}")?,
        }
    }
    // Name the busiest recorded device: on a cabinet capture that is the panel,
    // and the desk keyboard that also got recorded is not what anyone means.
    let busiest = recorded
        .iter()
        .max_by_key(|(_, events)| *events)
        .map(|(id, _)| id.as_str())
        .unwrap_or("<recorded id>");
    let target = driven[0]
        .1
        .clone()
        .unwrap_or_else(|| driven[0].0.as_str().to_owned());
    writeln!(
        f,
        "A recording names a device by the id it had WHEN IT WAS RECORDED — after a replug, or \
         on another machine, that id can name nothing here. Point it at a board this session \
         drives:"
    )?;
    if recorded.len() == 1 {
        write!(f, "    ksx play \"{file}\" --as {target}")
    } else {
        write!(f, "    ksx play \"{file}\" --as \"{busiest}={target}\"")
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Turn a parsed recording into one this plan can actually play.
///
/// Pure: no filesystem, no drivers, no clock. `connected` is the live USB tree
/// as [`crate::run::resolve::connected`] reads it (empty when nothing needed
/// enumerating).
pub(crate) fn resolve_recording(
    file: &str,
    mut recording: Recording,
    remaps: &[Remap],
    plan: &RunPlan,
    devices: &[DeviceEntry],
    connected: &[DeviceFacts],
) -> Result<PlayPlan, PlayError> {
    let mut notes: Vec<String> = Vec::new();

    for remap in remaps {
        let recorded = recording.devices();
        let from = match &remap.from {
            Some(from) => recorded
                .iter()
                .find(|id| id.as_str().eq_ignore_ascii_case(from))
                .cloned()
                .ok_or_else(|| PlayError::RemapMatchedNothing {
                    from: from.clone(),
                    recorded: recorded.clone(),
                })?,
            None => {
                if recorded.len() != 1 {
                    return Err(PlayError::UnnamedRemap {
                        target: remap.target.clone(),
                        recorded,
                    });
                }
                recorded[0].clone()
            }
        };
        let to = resolve_target(&remap.target, plan, devices, connected)?;
        let moved = recording.remap(&from, &to);
        notes.push(format!(
            "[INFO] --as: {moved} event(s) recorded against {from} now drive {to}"
        ));
    }

    // Line every recorded id up with the plan's exact spelling. Windows reports
    // instance paths in whatever case it feels like, and everything downstream
    // of here compares ids byte-exactly (`plan.captureable.contains`), so a
    // difference of case would silently turn a driving device into an ignored
    // one — a replay that starts, reports success and moves nothing.
    for recorded in recording.devices() {
        let Some(id) = plan
            .captureable
            .iter()
            .find(|id| id.as_str().eq_ignore_ascii_case(recorded.as_str()))
        else {
            continue;
        };
        if id.as_str() != recorded.as_str() {
            recording.remap(&recorded, id);
            notes.push(format!(
                "[INFO] recorded device {recorded} is this session's {id} (same id, different case)"
            ));
        }
    }

    let mut driving: Vec<Driven> = Vec::new();
    let mut ignored: Vec<(DeviceId, usize)> = Vec::new();
    for id in recording.devices() {
        let events = recording.count_for(&id);
        if plan.captureable.contains(&id) {
            driving.push(Driven {
                slots: plan.slots_using(&id),
                id,
                events,
            });
        } else {
            ignored.push((id, events));
        }
    }

    if driving.is_empty() {
        return Err(PlayError::NothingDrives {
            file: file.to_owned(),
            recorded: ignored,
            driven: plan
                .captureable
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        alias_for(id, devices, connected),
                        plan.slots_using(id),
                    )
                })
                .collect(),
        });
    }

    for (id, events) in &ignored {
        notes.push(format!(
            "[INFO] recorded device {id} drives no slot in this plan, so its {events} event(s) \
             are ignored — which is exactly what an unassigned keyboard does in a live session"
        ));
    }

    Ok(PlayPlan {
        recording,
        driving,
        ignored,
        notes,
    })
}

/// A `--as` target → the concrete device it names, through the same selector
/// path a session's `[[device]]` entries take.
///
/// **The plan wins.** `crate::run::plan::resolve_as` has already turned every
/// configured spelling into the interface that answers to it, so a spelling
/// this session is *already driving* resolves to that, and only an unfamiliar
/// one is matched against the USB tree. Resolving a second time would be a
/// second opinion about the same selector, and the session acts on the first.
fn resolve_target(
    target: &str,
    plan: &RunPlan,
    devices: &[DeviceEntry],
    connected: &[DeviceFacts],
) -> Result<DeviceId, PlayError> {
    let driven = |written: &str| -> Option<DeviceId> {
        plan.captureable
            .iter()
            .find(|id| id.as_str().eq_ignore_ascii_case(written))
            .cloned()
    };

    if let Some(entry) = devices.iter().find(|d| d.alias == target) {
        if let Some(id) = driven(entry.id.raw()) {
            return Ok(id);
        }
        return concrete(entry.id.clone(), target, connected);
    }
    let Ok(reference) = DeviceRef::parse(target) else {
        return Err(PlayError::UnknownTarget {
            target: target.to_owned(),
            known: devices.iter().map(|d| d.alias.clone()).collect(),
        });
    };
    // An id copied straight out of the refusal above is the other thing people
    // type, and it has to work for the same reason.
    if let Some(id) = driven(reference.raw()) {
        return Ok(id);
    }
    concrete(reference, target, connected)
}

fn concrete(
    reference: DeviceRef,
    target: &str,
    connected: &[DeviceFacts],
) -> Result<DeviceId, PlayError> {
    // The same rule as `crate::run::resolve::needs_matching`: an Interception
    // hardware id names a devnode on the keyboard stack, which never appears in
    // a USB enumeration. Matching it could only ever produce "not connected".
    if matches!(reference.selector(), DeviceSelector::HardwareId(_)) {
        return Ok(reference.as_device_id());
    }
    match reference.selector().match_against(connected) {
        Match::One(facts) => Ok(facts.id.clone()),
        Match::None => Err(PlayError::TargetMissing {
            target: target.to_owned(),
        }),
        Match::Ambiguous(hits) => Err(PlayError::TargetAmbiguous {
            target: target.to_owned(),
            hits: hits.iter().map(|facts| facts.id.clone()).collect(),
        }),
    }
}

/// The `[[device]]` alias for a concrete id, if the config has one — advice
/// only, so a lookup that cannot be made simply yields `None`.
fn alias_for(id: &DeviceId, devices: &[DeviceEntry], connected: &[DeviceFacts]) -> Option<String> {
    devices
        .iter()
        .find(|entry| {
            entry.id.raw().eq_ignore_ascii_case(id.as_str())
                || matches!(entry.id.selector().match_against(connected),
                    Match::One(facts) if &facts.id == id)
        })
        .map(|entry| entry.alias.clone())
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

pub(crate) fn render_human(play: &PlayPlan, file: &str, speed: Speed, looping: bool) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let seconds = play.recording.duration_ms() as f64 / 1000.0 / speed.get();
    let _ = writeln!(
        out,
        "replaying {file}: {} event(s), {seconds:.1}s at {speed}{}",
        play.recording.len(),
        if looping { ", looping" } else { "" }
    );
    for driven in &play.driving {
        let _ = writeln!(
            out,
            "  drives  {} -> slot(s) {:?}  ({} event(s))",
            driven.id, driven.slots, driven.events
        );
    }
    for (id, events) in &play.ignored {
        let _ = writeln!(out, "  ignored {id}  ({events} event(s))");
    }
    let _ = writeln!(
        out,
        "  live input from the driven board(s) never reaches the pads while this plays"
    );
    for note in &play.notes {
        let _ = writeln!(out, "  {note}");
    }
    out
}

pub(crate) fn play_json(
    play: &PlayPlan,
    file: &str,
    speed: Speed,
    looping: bool,
) -> serde_json::Value {
    serde_json::json!({
        "file": file,
        "events": play.recording.len(),
        // What will actually move a pad. Not the same number as `events` the
        // moment the recording caught an unassigned keyboard too, and a caller
        // that reported the total would overstate what it is about to watch.
        "driving_events": play.driving_events(),
        "duration_ms": play.recording.duration_ms(),
        "speed": speed.get(),
        "looping": looping,
        "driving": play.driving.iter().map(|d| serde_json::json!({
            "device": d.id.as_str(),
            "events": d.events,
            "slots": d.slots,
        })).collect::<Vec<_>>(),
        "ignored": play.ignored.iter().map(|(id, events)| serde_json::json!({
            "device": id.as_str(),
            "events": events,
        })).collect::<Vec<_>>(),
        "notes": play.notes,
    })
}

// ---------------------------------------------------------------------------
// The hook
// ---------------------------------------------------------------------------

/// Ends the session when the recording runs out.
///
/// The replay's capture thread deliberately stays alive after the last event
/// (a capture thread that exits is a *failure* to the supervisor), so something
/// has to turn "finished" into a stop. A hook is that something, and wrapping
/// the `--game` hook rather than replacing it means `ksx play --game` still
/// stops when the game exits, whichever happens first.
pub(crate) struct ReplayHook {
    progress: ReplayProgress,
    inner: Box<dyn SessionHook>,
}

impl ReplayHook {
    pub(crate) fn new(progress: ReplayProgress, inner: Box<dyn SessionHook>) -> Self {
        Self { progress, inner }
    }
}

impl SessionHook for ReplayHook {
    fn started(&mut self, out: &mut dyn Write) -> Result<(), String> {
        self.inner.started(out)
    }

    /// The game is asked first: if it has gone, there is nobody watching the
    /// rest of the recording.
    fn poll(&mut self, out: &mut dyn Write) -> Option<HookStop> {
        if let Some(stop) = self.inner.poll(out) {
            return Some(stop);
        }
        self.progress.finished().then_some(HookStop::ReplayFinished)
    }

    fn finished(&mut self, out: &mut dyn Write) {
        let _ = writeln!(
            out,
            "replay: {} event(s) delivered over {} pass(es)",
            self.progress.events(),
            self.progress.passes()
        );
        self.inner.finished(out);
    }

    fn status_line(&self) -> Option<String> {
        self.inner.status_line()
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn run(options: Options) -> anyhow::Result<()> {
    let root = ksx_config::ConfigRoot::discover()?;
    let file = options.file.display().to_string();

    let speed = match Speed::new(options.speed) {
        Ok(speed) => speed,
        Err(err) => refuse(&PlayError::Speed(err), options.json),
    };
    let remaps: Vec<Remap> = {
        let mut out = Vec::with_capacity(options.remap.len());
        for spec in &options.remap {
            match Remap::parse(spec) {
                Ok(remap) => out.push(remap),
                Err(err) => refuse(&err, options.json),
            }
        }
        out
    };
    let text = match std::fs::read_to_string(&options.file) {
        Ok(text) => text,
        Err(err) => refuse(
            &PlayError::Unreadable {
                path: file.clone(),
                detail: err.to_string(),
            },
            options.json,
        ),
    };
    let recording = match Recording::parse(&text) {
        Ok(recording) => recording,
        Err(err) => refuse(&PlayError::Recording(err), options.json),
    };

    // The plan first, and through the call every session shares: `ksx play`
    // resolves `[[device]]` selectors exactly the way `ksx run` does, or the
    // ids a recording is matched against would be a different set from the ones
    // the session drives.
    let plan = match crate::run::plan::resolve_as(&root, options.game.as_deref(), "ksx play") {
        Ok(plan) => plan,
        Err(err) => {
            if options.json {
                println!(
                    "{}",
                    crate::pads::error_json("play-cannot-start", &err.to_string())
                );
            } else {
                eprintln!("error: {err}");
            }
            std::process::exit(crate::run::EXIT_CANNOT_START);
        }
    };

    let config = ksx_config::Store::new(root.clone()).load_config()?;
    // Only a `--as` target can need the USB tree; without one, nothing here
    // asks for an enumeration (same trade as `resolve::needs_enumeration`).
    let connected = if remaps.is_empty() {
        Vec::new()
    } else {
        crate::run::resolve::connected()?
    };

    let play = match resolve_recording(
        &file,
        recording,
        &remaps,
        &plan,
        &config.value.devices,
        &connected,
    ) {
        Ok(play) => play,
        Err(err) => refuse(&err, options.json),
    };

    // A launch that cannot work is refused here, with every keyboard still
    // normal and no pad plugged — the same order `ksx run` uses.
    let launch = match options.game.as_deref() {
        Some(title) if !options.no_launch => match crate::run::resolve_launch(&root, title) {
            Ok(spec) => Some(spec),
            Err(err) => {
                if options.json {
                    println!(
                        "{}",
                        crate::pads::error_json("game-not-launchable", &err.to_string())
                    );
                } else {
                    eprintln!("error: {err}");
                }
                std::process::exit(crate::run::EXIT_CANNOT_START);
            }
        },
        _ => None,
    };

    if options.dry_run {
        if options.json {
            let mut value = play_json(&play, &file, speed, options.looping);
            value["plan"] = crate::run::plan::plan_json(&plan);
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print!("{}", crate::run::plan::render_human(&plan));
            print!("{}", render_human(&play, &file, speed, options.looping));
            println!(
                "dry run: nothing was plugged, no keyboard filter was set, nothing was replayed"
            );
        }
        return Ok(());
    }

    play_live(PlayRequest {
        plan,
        play,
        file,
        speed,
        looping: options.looping,
        launch,
        games_toml: root.games_path(),
        latency: options.latency,
        json: options.json,
    })
}

/// Everything one live replay needs, so the platform split below is one
/// signature rather than nine arguments repeated twice.
struct PlayRequest {
    plan: RunPlan,
    play: PlayPlan,
    file: String,
    speed: Speed,
    looping: bool,
    launch: Option<ksx_games::LaunchSpec>,
    games_toml: PathBuf,
    latency: bool,
    json: bool,
}

#[cfg(windows)]
fn play_live(request: PlayRequest) -> anyhow::Result<()> {
    use ksx_capture::{CaptureBackend, CompositeBackend, Handles, ReplayBackend, Silenced};

    let PlayRequest {
        plan,
        play,
        file,
        speed,
        looping,
        launch,
        games_toml,
        latency,
        json,
    } = request;

    if !json {
        print!("{}", render_human(&play, &file, speed, looping));
    }

    let progress = ReplayProgress::default();
    let inner: Box<dyn crate::run::supervisor::SessionHook> = match launch {
        Some(spec) => Box::new(crate::run::game::GameHook::new(
            spec,
            ksx_games::RealHost::new(),
            games_toml,
        )),
        None => Box::new(crate::run::supervisor::NoHook),
    };
    let hook = Box::new(ReplayHook::new(progress.clone(), inner));

    let recording = play.recording;
    crate::run::live_session(
        &plan,
        move |plan| {
            // ONE set of handles across both backends: one health state, and —
            // the part that matters at a cabinet — one escape latch, so
            // `LeftCtrl x5` on the real panel still frees every keyboard in the
            // session even though the panel is being ignored.
            let handles = Handles::new();
            let live = crate::capture::build_with(plan, handles.clone())?;
            let replay = ReplayBackend::new(recording)
                .with_speed(speed)
                .looping(looping)
                .with_progress(progress)
                .with_handles(handles.clone());
            let children: Vec<Box<dyn CaptureBackend>> =
                vec![Box::new(replay), Box::new(Silenced::new(live))];
            Ok(Box::new(CompositeBackend::new(children, handles)) as Box<dyn CaptureBackend>)
        },
        hook,
        latency,
        json,
    )
}

#[cfg(not(windows))]
fn play_live(_request: PlayRequest) -> anyhow::Result<()> {
    anyhow::bail!(
        "`ksx play` is Windows-only (it drives the ViGEmBus and capture drivers); \
         `ksx play --dry-run` works everywhere"
    )
}

/// Print a refusal and exit 2. Nothing was plugged and no filter was armed.
fn refuse(err: &PlayError, json: bool) -> ! {
    if json {
        println!("{}", crate::pads::error_json(err.code(), &err.to_string()));
    } else {
        eprintln!("error: {err}");
    }
    std::process::exit(crate::run::EXIT_CANNOT_START);
}

#[cfg(test)]
mod tests {
    use ksx_capture::RecordedEvent;
    use ksx_config::{ConfigFile, GamesFile, PresetFile};
    use ksx_core::Key;

    use super::*;

    /// The cabinet's board, as `ksx monitor` recorded it.
    const RECORDED: &str = r"HID\VID_D209&PID_0430&REV_0056&MI_00";
    /// A desk keyboard that was also plugged in at record time and is bound to
    /// nothing — the corpus really does contain one.
    const DESK: &str = r"HID\VID_046D&PID_C31C&REV_6402&MI_00";
    /// The same panel after the WinUSB migration: a different string entirely.
    const LIVE: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";

    fn presets() -> Vec<PresetFile> {
        vec![toml::from_str("name = \"P1\"\n[bindings]\nA = \"S\"\n").unwrap()]
    }

    fn plan_for(config: &ConfigFile) -> RunPlan {
        crate::run::plan::build_plan(config, &GamesFile::default(), &presets(), None)
            .expect("plan builds")
    }

    fn config(id: &str) -> ConfigFile {
        toml::from_str(&format!(
            "schema_version = 1\n\n\
             [[device]]\nid = '{id}'\nalias = \"ipac\"\n\n\
             [[slot]]\nnumber = 1\nkeyboard = \"ipac\"\npreset = \"P1\"\n"
        ))
        .unwrap()
    }

    fn recording(devices: &[(&str, usize)]) -> Recording {
        let mut events = Vec::new();
        let mut t_ms = 0;
        for (device, count) in devices {
            for _ in 0..*count {
                events.push(RecordedEvent {
                    t_ms,
                    device: DeviceId::from(*device),
                    key: Key::A,
                    down: t_ms % 2 == 0,
                });
                t_ms += 1;
            }
        }
        Recording::from_events(events)
    }

    fn resolve(
        recording: Recording,
        remaps: &[Remap],
        config: &ConfigFile,
    ) -> Result<PlayPlan, PlayError> {
        let plan = plan_for(config);
        resolve_recording(
            "session.jsonl",
            recording,
            remaps,
            &plan,
            &config.devices,
            &[],
        )
    }

    /// The ordinary case: the board is still spelled the way it was recorded.
    #[test]
    fn a_recording_of_the_configured_board_plays_with_no_flags() {
        let play = resolve(recording(&[(RECORDED, 4)]), &[], &config(RECORDED)).expect("plays");
        assert_eq!(play.driving.len(), 1);
        assert_eq!(play.driving[0].id, DeviceId::from(RECORDED));
        assert_eq!(play.driving[0].slots, vec![1]);
        assert_eq!(play.driving_events(), 4);
        assert!(play.ignored.is_empty());
    }

    /// **The corpus's own shape.** A session recorded on a cabinet also catches
    /// the desk keyboard next to it. That device is bound to nothing, so it is
    /// ignored — refusing over it would make the real corpus unplayable, and an
    /// unassigned keyboard is routed nowhere in a live session too.
    ///
    /// Breaks against: a resolver that requires EVERY recorded device to drive a
    /// slot.
    #[test]
    fn an_unassigned_keyboard_in_the_recording_is_noted_and_ignored() {
        let play = resolve(
            recording(&[(RECORDED, 6), (DESK, 3)]),
            &[],
            &config(RECORDED),
        )
        .expect("the bound board still drives");

        assert_eq!(play.driving.len(), 1);
        assert_eq!(play.driving_events(), 6);
        assert_eq!(play.ignored, vec![(DeviceId::from(DESK), 3)]);
        assert!(
            play.notes
                .iter()
                .any(|n| n.contains(DESK) && n.contains("ignored")),
            "the ignored device is named out loud: {:?}",
            play.notes
        );
    }

    /// **The refusal this module exists for.** The recording names the board by
    /// the id it had before the WinUSB migration; the session drives a different
    /// string. Playing it would plug four pads and move none of them.
    ///
    /// Breaks against: a `ksx play` that starts anyway — the "reports success
    /// while the panel is dead" failure — and against a refusal that does not
    /// name the recorded id or the flag that fixes it.
    #[test]
    fn a_recording_whose_device_drives_nothing_refuses_before_starting() {
        let err = resolve(recording(&[(RECORDED, 5)]), &[], &config(LIVE))
            .expect_err("nothing this recording names is bound to a slot");
        assert_eq!(err.code(), "recording-drives-nothing");

        let text = err.to_string();
        assert!(text.contains(RECORDED), "name the RECORDED id: {text}");
        assert!(text.contains(LIVE), "and what this session drives: {text}");
        assert!(text.contains("--as"), "and the flag that fixes it: {text}");
        assert!(
            text.contains("--as ipac"),
            "the alias is the thing to type, not the path: {text}"
        );
        assert!(
            text.contains("session.jsonl"),
            "the suggestion has to be copy-pasteable: {text}"
        );
    }

    /// ...and the flag it suggests actually works.
    #[test]
    fn the_suggested_remap_makes_the_same_recording_play() {
        let remap = Remap::parse("ipac").expect("parses");
        let play = resolve(recording(&[(RECORDED, 5)]), &[remap], &config(LIVE))
            .expect("remapped onto the configured board");
        assert_eq!(play.driving.len(), 1);
        assert_eq!(play.driving[0].id, DeviceId::from(LIVE));
        assert_eq!(play.driving_events(), 5);
        assert!(
            play.notes.iter().any(|n| n.contains("--as")),
            "a remap is never silent: {:?}",
            play.notes
        );
    }

    /// A two-device recording cannot be remapped by a bare target: which one?
    #[test]
    fn a_bare_remap_on_a_multi_device_recording_asks_which_one() {
        let remap = Remap::parse("ipac").expect("parses");
        let err = resolve(
            recording(&[(RECORDED, 2), (DESK, 2)]),
            &[remap],
            &config(LIVE),
        )
        .expect_err("ambiguous");
        assert_eq!(err.code(), "remap-invalid");
        let text = err.to_string();
        assert!(text.contains(RECORDED) && text.contains(DESK), "{text}");
        assert!(
            text.contains("="),
            "show the form that disambiguates: {text}"
        );

        // ...and the explicit form is accepted.
        let remap = Remap::parse(&format!("{RECORDED}=ipac")).expect("parses");
        let play = resolve(
            recording(&[(RECORDED, 2), (DESK, 2)]),
            &[remap],
            &config(LIVE),
        )
        .expect("explicit remap");
        assert_eq!(play.driving[0].id, DeviceId::from(LIVE));
        assert_eq!(play.ignored, vec![(DeviceId::from(DESK), 2)]);
    }

    #[test]
    fn a_remap_whose_left_side_is_not_in_the_recording_says_what_is() {
        let remap = Remap::parse(&format!("{DESK}=ipac")).expect("parses");
        let err = resolve(recording(&[(RECORDED, 2)]), &[remap], &config(LIVE))
            .expect_err("the recording never mentions that device");
        assert_eq!(err.code(), "remap-invalid");
        assert!(err.to_string().contains(RECORDED), "{err}");
    }

    #[test]
    fn an_unknown_remap_target_lists_the_aliases_that_exist() {
        let remap = Remap::parse("panel2").expect("parses");
        let err = resolve(recording(&[(RECORDED, 2)]), &[remap], &config(LIVE))
            .expect_err("no such alias");
        assert_eq!(err.code(), "remap-target-unresolved");
        let text = err.to_string();
        assert!(text.contains("panel2"), "{text}");
        assert!(text.contains("ipac"), "the alias that DOES exist: {text}");
    }

    /// **A `port=` qualifier contains an `=`.** Splitting a `--as` spec on the
    /// last `=`, or treating any `=` as the FROM separator, cuts a selector in
    /// half and blames the user for a spelling that is correct.
    #[test]
    fn a_selector_with_a_port_qualifier_is_not_mistaken_for_a_from_side() {
        let remap = Remap::parse("usb:d209:0430:00:port=7&25EEA38C&0&0000").expect("parses");
        assert_eq!(remap.from, None);
        assert_eq!(remap.target, "usb:d209:0430:00:port=7&25EEA38C&0&0000");

        let remap =
            Remap::parse(&format!("{RECORDED}=usb:d209:0430:00:port=7&AB&0&0000")).expect("parses");
        assert_eq!(remap.from.as_deref(), Some(RECORDED));
        assert_eq!(remap.target, "usb:d209:0430:00:port=7&AB&0&0000");
    }

    #[test]
    fn a_remap_with_nothing_on_one_side_is_refused() {
        assert!(matches!(
            Remap::parse("=ipac"),
            Err(PlayError::BadRemap { .. })
        ));
        assert!(matches!(
            Remap::parse("something="),
            Err(PlayError::BadRemap { .. })
        ));
    }

    /// **Case.** Windows hands back instance paths in whatever case it likes,
    /// and everything downstream compares ids byte-exactly. Without the
    /// alignment pass a recording made yesterday would resolve to "drives
    /// nothing" today, on the same unplugged-nothing machine.
    ///
    /// Breaks against: dropping the `eq_ignore_ascii_case` alignment.
    #[test]
    fn a_recorded_id_that_differs_only_in_case_still_drives_its_slot() {
        let play = resolve(
            recording(&[(&RECORDED.to_lowercase(), 3)]),
            &[],
            &config(RECORDED),
        )
        .expect("same board, different case");
        assert_eq!(play.driving.len(), 1);
        assert_eq!(
            play.driving[0].id,
            DeviceId::from(RECORDED),
            "the plan's exact spelling is what everything downstream compares"
        );
        assert!(
            play.notes.iter().any(|n| n.contains("different case")),
            "{:?}",
            play.notes
        );
    }

    /// A recording that resolves is described before it plays, including the
    /// promise the `--help` makes about live input.
    #[test]
    fn the_report_names_what_drives_what() {
        let play = resolve(
            recording(&[(RECORDED, 4), (DESK, 1)]),
            &[],
            &config(RECORDED),
        )
        .expect("plays");
        let text = render_human(&play, "session.jsonl", Speed::NORMAL, false);
        assert!(text.contains("session.jsonl"), "{text}");
        assert!(text.contains("drives"), "{text}");
        assert!(text.contains("ignored"), "{text}");
        assert!(
            text.contains("never reaches the pads"),
            "the suppression promise is on the report too: {text}"
        );

        let value = play_json(&play, "session.jsonl", Speed::NORMAL, true);
        assert_eq!(value["events"], 5);
        assert_eq!(value["looping"], true);
        assert_eq!(value["driving"][0]["slots"][0], 1);
    }

    /// **The hook stops the session when the recording runs out** — and not
    /// before, and not by pretending somebody pressed Ctrl+C.
    ///
    /// Breaks against: a `ksx play` that lets the capture thread exit instead
    /// (the supervisor reports that as the capture path dying, exit 3), and
    /// against reporting a finished replay as a `CtrlC` stop.
    /// **The hook stops the session when the recording runs out** — and not
    /// before, and not by pretending somebody pressed Ctrl+C.
    ///
    /// Driven by a real `ReplayBackend`, so what "finished" means is the
    /// backend's answer rather than a flag this test set itself.
    ///
    /// Breaks against: a `ksx play` that lets the capture thread exit instead
    /// (the supervisor reports that as the capture path dying, exit 3), and
    /// against reporting a finished replay as a `CtrlC` stop.
    #[test]
    fn the_hook_ends_the_session_exactly_when_the_recording_finishes() {
        use crate::run::supervisor::{NoHook, StopReason};
        use ksx_capture::{CaptureBackend, CaptureCtl, ReplayBackend, VirtualClock};

        let recording = recording(&[(RECORDED, 2)]);
        let backend = ReplayBackend::new(recording).with_clock(Box::new(VirtualClock::new()));
        let progress = backend.progress();
        let mut hook = ReplayHook::new(progress.clone(), Box::new(NoHook));
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(hook.poll(&mut out), None, "nothing has played yet");

        let (tx, rx) = crossbeam_channel::bounded(8);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(backend).run(tx, ctl_rx).expect("thread");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hook.poll(&mut out).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            hook.poll(&mut out),
            Some(HookStop::ReplayFinished),
            "a recording that ran out must end the session"
        );
        ctl_tx.send(CaptureCtl::Shutdown).expect("ctl open");
        let _ = handle.join();
        drop(rx);

        // ...and it ends as a clean stop with its own name, not as a keypress
        // nobody made.
        let stop = StopReason::from(HookStop::ReplayFinished);
        assert_eq!(stop.code(), "replay-finished");
        assert!(
            stop.is_clean(),
            "the recording ending is what was asked for"
        );
        assert_ne!(stop, StopReason::CtrlC);

        hook.finished(&mut out);
        let text = String::from_utf8(out).expect("utf-8");
        assert!(
            text.contains("2 event(s)") && text.contains("1 pass(es)"),
            "the session says what it played: {text}"
        );
    }
}
