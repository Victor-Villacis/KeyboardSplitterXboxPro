//! **The local machine, as a surface reads it** — the `ksx-api` read side,
//! implemented once for every front end that runs on this box.
//!
//! Two consumers today and they want the identical facts: ksx Studio's status
//! page and mapper, and the cabinet's "am I working" and slot picker. Before
//! this module the collectors lived inside `studio.rs`, which meant a build
//! with `--features cabinet` and no Studio could not see them — a contract
//! cannot be owned by whichever surface was written first
//! (docs/CONTROL-SURFACE.md says exactly that about `ksx-api` itself).
//!
//! Nothing here needs a daemon. That is the whole point of
//! [`ksx_api::StatusSource`] being a separate trait from
//! [`ksx_api::ControlSource`] (docs/M9-DECISION.md §6): the config store and
//! the platform collectors answer with the pipe dead, which is what keeps a
//! read-only mapper alive behind the "No daemon" banner and what lets the
//! cabinet's status screen say something useful about a machine whose daemon
//! just died.
//!
//! Every call re-runs the collectors. Point-in-time by design, never cached:
//! a status row that is quietly ten minutes old is worse than one that took
//! 40 ms to fetch.

use ksx_api::{
    MacroSnapshot, MacroStepView, MacroView, MapperSlot, MapperSnapshot, PadRow, PresetRow,
    PresetsView, ProfileRow, Refusal, StatusSnapshot, StatusSource, TemplateRow,
};
use ksx_platform::autostart;
use ksx_platform::{BusDriverReport, InterceptionReport, ServiceState};

pub fn configured_profile() -> Option<String> {
    let root = ksx_config::ConfigRoot::discover().ok()?;
    let store = ksx_config::Store::new(root);
    if store
        .load_config()
        .is_ok_and(|loaded| !loaded.value.slots.is_empty())
    {
        return None;
    }
    store
        .load_games()
        .ok()?
        .value
        .games
        .first()
        .map(|game| game.title.clone())
}

/// The real snapshot provider: nothing cached, nothing owned — each call
/// re-runs the same read-only collectors `ksx doctor` and `ksx autostart
/// --status` use.
pub struct CollectorSource;

impl StatusSource for CollectorSource {
    fn snapshot(&self) -> StatusSnapshot {
        collect_snapshot()
    }

    fn mapper(&self) -> MapperSnapshot {
        collect_mapper()
    }

    fn macros(&self, preset: &str) -> MacroSnapshot {
        let root = match ksx_config::ConfigRoot::discover() {
            Ok(root) => root,
            Err(err) => {
                return MacroSnapshot::unavailable(&format!("config root not found: {err}"))
            }
        };
        collect_macros(&ksx_config::Store::new(root), preset)
    }
}

/// One preset's `[macros]` tables, re-read from disk per call like everything
/// else this provider serves.
///
/// Deliberately the FILE's shape, not a resolved one: `ms` and `frames` stay
/// apart and `allow_short` is passed through as written, because the editor's
/// job is to show what the file says (and to emit a block that can go back
/// into it). Read from `PresetFile` rather than through `to_core()` for the
/// same reason it is worth saying twice — a preset with a typo somewhere ELSE
/// still has readable macros, and a page that showed an empty grid for it
/// would be claiming "this preset defines none", which is a different fact.
fn collect_macros(store: &ksx_config::Store, preset_name: &str) -> MacroSnapshot {
    let loaded = match store.load_presets() {
        Ok(loaded) => loaded,
        Err(err) => return MacroSnapshot::unavailable(&format!("presets unreadable: {err}")),
    };
    let known: Vec<String> = loaded.value.iter().map(|p| p.name.clone()).collect();
    let Some(file) = loaded.value.into_iter().find(|p| p.name == preset_name) else {
        return MacroSnapshot::unavailable(&format!(
            "no preset called \"{preset_name}\" is on disk (presets found: {})",
            if known.is_empty() {
                "none".to_owned()
            } else {
                known.join(", ")
            }
        ));
    };
    let macros = file
        .macros
        .iter()
        .map(|(name, def)| MacroView {
            name: name.clone(),
            steps: def
                .steps
                .iter()
                .map(|step| MacroStepView {
                    hold: step.hold.clone(),
                    ms: step.ms,
                    frames: step.frames,
                    allow_short: step.allow_short,
                })
                .collect(),
            on_release: def.on_release.as_str().to_owned(),
            retrigger: def.retrigger.as_str().to_owned(),
            interrupt: def.interrupt.as_str().to_owned(),
            repeat: def.repeat.as_str().to_owned(),
            turbo_hz: def.turbo_hz,
            gap_ms: def.gap_ms,
            // Said as the negative on the wire so the default is the ordinary
            // case; the card renders it as the loud state it is.
            disabled: !def.enabled,
            // The `macro.<name>` rows of `[bindings]` — many keys → one macro
            // is native, exactly like many keys → one button. Read through the
            // mapping writer's own helper, so the keys the card shows are the
            // keys a delete would take with it.
            triggers: crate::mapping::macro_trigger_keys(&file, name),
        })
        .collect();
    MacroSnapshot::read(&file.name, macros)
}

/// The mapper's slot list, re-read from disk per call (fresh writes = fresh
/// zone tags): `config.toml` `[[slot]]` entries when present, otherwise the
/// first games.toml profile's slots — this cabinet keeps its slots in the
/// game profiles. Preset bindings come through the same store the `map` verb
/// writes with.
fn collect_mapper() -> MapperSnapshot {
    let root = match ksx_config::ConfigRoot::discover() {
        Ok(root) => root,
        Err(err) => return MapperSnapshot::unavailable(&format!("config root not found: {err}")),
    };
    let config_root = root.dir().display().to_string();
    let store = ksx_config::Store::new(root);

    // (number, keyboard, preset, persona, macro master switch), plus the
    // source line and the machine-readable half of it: WHICH FILE these came
    // from, because a surface that offers to change one of these slots has to
    // write back to the file it read them from
    // (`ksx_api::MapperSnapshot::profile`).
    let (rows, source, from_profile) = match store.load_config() {
        Ok(loaded) if !loaded.value.slots.is_empty() => {
            let rows: Vec<(u8, String, String, ksx_core::Persona, ksx_core::MacroSwitch)> = loaded
                .value
                .slots
                .iter()
                .map(|s| {
                    (
                        s.number,
                        s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
                        s.preset.clone(),
                        s.persona,
                        s.macros,
                    )
                })
                .collect();
            (rows, "config.toml [[slot]] entries".to_owned(), None)
        }
        _ => match store.load_games() {
            Ok(loaded) => match loaded.value.games.first() {
                Some(game) => {
                    let rows = game
                        .slots
                        .iter()
                        .map(|s| {
                            (
                                s.number,
                                s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
                                s.preset.clone(),
                                s.persona,
                                s.macros,
                            )
                        })
                        .collect();
                    (
                        rows,
                        format!("slots of profile \"{}\" (games.toml)", game.title),
                        Some(game.title.clone()),
                    )
                }
                None => {
                    return MapperSnapshot {
                        generated_at: now_utc(),
                        source: "no [[slot]] entries in config.toml and no games.toml profiles"
                            .to_owned(),
                        profile: None,
                        config_root,
                        slots: Vec::new(),
                    }
                }
            },
            Err(err) => {
                return MapperSnapshot::unavailable(&format!("games.toml unreadable: {err}"))
            }
        },
    };

    let slots = rows
        .into_iter()
        .map(|(number, keyboard, preset_name, persona, macros)| {
            let bindings = preset_bindings(&store, &preset_name);
            let turbo = preset_turbo(&store, &preset_name);
            // The newest restore point, read from disk rather than from the
            // daemon: the label is still true (and still worth showing) when
            // nothing answers the pipe.
            let backup = crate::mapping::list_backups(&store, &preset_name)
                .ok()
                .and_then(|backups| backups.first().map(|b| b.label()));
            MapperSlot {
                number,
                persona: persona.as_str().to_owned(),
                persona_label: persona.label().to_owned(),
                preset: preset_name,
                keyboard,
                bindings,
                backup,
                turbo,
                // The tournament switch, straight off the slot entry: "my
                // macros do nothing" has two causes, and this is the one you
                // cannot see by reading the preset.
                macros_off: !macros.is_on(),
            }
        })
        .collect();

    MapperSnapshot {
        generated_at: now_utc(),
        source,
        profile: from_profile,
        config_root,
        slots,
    }
}

/// Canonical function name → bound key names. Inert `"None"` placeholders
/// become EMPTY lists (the page's honest "unbound" tag); an unreadable or
/// missing preset yields no entries at all — its zones all render unbound,
/// and a `map` attempt will name the real problem.
fn preset_bindings(
    store: &ksx_config::Store,
    preset_name: &str,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut bindings = std::collections::BTreeMap::new();
    let Ok(Some(loaded)) = store.load_preset(preset_name) else {
        return bindings;
    };
    let Ok(core) = loaded.value.to_core() else {
        return bindings;
    };
    for (key, binding) in &core.entries {
        let function = ksx_config::function_name(binding);
        let keys: &mut Vec<String> = bindings.entry(function).or_default();
        if *key != ksx_core::Key::None {
            keys.push(key.name().to_owned());
        }
    }
    bindings
}

/// Canonical function name → its AUTO-FIRE rate, as authored
/// (docs/INPUT-TRANSFORMS.md §3). Read from the same file and the same core
/// model the bindings come from, so the legend's rate and the legend's keys
/// can never disagree.
fn preset_turbo(
    store: &ksx_config::Store,
    preset_name: &str,
) -> std::collections::BTreeMap<String, u32> {
    let mut rates = std::collections::BTreeMap::new();
    let Ok(Some(loaded)) = store.load_preset(preset_name) else {
        return rates;
    };
    let Ok(core) = loaded.value.to_core() else {
        return rates;
    };
    for t in &core.turbo {
        rates.insert(ksx_config::function_name(&t.binding), t.hz);
    }
    rates
}

fn collect_snapshot() -> StatusSnapshot {
    let report = ksx_platform::collect();
    let (daemon_running, daemon_detail) = daemon_check();
    let (profiles, config_root) = load_profiles();

    StatusSnapshot {
        generated_at: now_utc(),
        vigem: bus_line(&report.vigembus),
        interception: interception_line(&report.interception),
        daemon_running,
        daemon_detail,
        autostart: autostart_line(),
        pads: report
            .virtual_pads
            .pads
            .iter()
            .map(|p| PadRow {
                persona: p.persona_guess.label().to_owned(),
                instance: p.instance_id.clone(),
            })
            .collect(),
        profiles,
        config_root,
    }
}

fn now_utc() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

fn service_state_label(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "service running",
        ServiceState::Stopped => "service stopped",
        ServiceState::StartPending => "service start pending",
        ServiceState::StopPending => "service stop pending",
        ServiceState::Paused => "service paused",
        ServiceState::PausePending => "service pause pending",
        ServiceState::ContinuePending => "service continue pending",
        ServiceState::NotRegistered => "service not registered with the SCM",
        ServiceState::Unknown => "service state unknown",
    }
}

fn bus_line(bus: &BusDriverReport) -> String {
    if !bus.installed {
        return "not installed".to_owned();
    }
    let state = bus
        .service
        .as_ref()
        .map_or("service state unknown", |s| service_state_label(s.state));
    match bus
        .driver_file
        .as_ref()
        .and_then(|f| f.file_version.as_deref())
    {
        Some(version) => format!("installed — {state} — driver v{version}"),
        None => format!("installed — {state} — driver version unknown"),
    }
}

fn interception_line(icpt: &InterceptionReport) -> String {
    if !icpt.installed {
        return "not installed (the M6 target state once WinUSB capture lands)".to_owned();
    }
    let filter = if icpt.keyboard.filter_active {
        "keyboard filter active"
    } else {
        "keyboard filter NOT in the class stack"
    };
    match icpt
        .keyboard
        .driver_file
        .as_ref()
        .and_then(|f| f.file_version.as_deref())
    {
        Some(version) => format!("installed — {filter} — driver v{version}"),
        None => format!("installed — {filter}"),
    }
}

/// Tasklist-style liveness check: any OTHER `ksx.exe` process. Honest about
/// its own limits — a process list cannot tell a tray daemon from a `ksx
/// run` session; the session panel's control pipe is the authoritative
/// daemon view, and this row exists to catch a ksx that is alive but NOT
/// answering the pipe (a foreground session, a pre-pipe daemon).
fn daemon_check() -> (bool, String) {
    let self_pid = std::process::id();
    let ksx: Vec<_> = ksx_platform::process::snapshot()
        .into_iter()
        .filter(|p| p.pid != self_pid && p.name_matches("ksx.exe"))
        .collect();
    if ksx.is_empty() {
        (
            false,
            "no other ksx.exe process (process-list check; the Session panel's \
             control pipe is the authoritative daemon view)"
                .to_owned(),
        )
    } else {
        let pids: Vec<String> = ksx.iter().map(|p| p.pid.to_string()).collect();
        (
            true,
            format!(
                "ksx.exe alive (pid {}) — daemon or session; if the Session \
                 panel shows no control channel, this one predates it or is a \
                 foreground `ksx run`",
                pids.join(", ")
            ),
        )
    }
}

fn autostart_line() -> String {
    match autostart::query(autostart::DEFAULT_TASK_NAME) {
        Ok(autostart::Status::NotRegistered) => "not registered".to_owned(),
        Ok(autostart::Status::Registered(task)) => {
            let mode = task.mode().map_or("unrecognized command", |m| m.describe());
            match task.game() {
                Some(game) => format!("registered — {mode} — profile \"{game}\""),
                None => format!("registered — {mode}"),
            }
        }
        Err(err) => format!("query failed: {err}"),
    }
}

/// One `[[game]]` entry, preflighted into something a surface can branch on.
///
/// The verdicts are the honest three, and the middle one is the one worth
/// keeping separate: `ksx_games::preflight` cannot check a `steam://` URL —
/// only the shell resolves it — so a protocol profile is `launcher`, not `ok`.
/// Reporting it green would be ksx claiming a check it did not make, and the
/// user finding out at the same moment they used to find out about the missing
/// .exe.
fn profile_detail(entry: &ksx_config::GameEntry) -> ksx_api::ProfileDetail {
    use ksx_games::{preflight, LaunchSpec, LaunchTarget, PreflightError};

    let spec = LaunchSpec::from_entry(entry);
    let protocol = matches!(spec.target, LaunchTarget::Protocol { .. });
    let (state, verdict, broken_path) = match preflight(&spec) {
        Ok(()) if protocol => (
            "launcher",
            match &spec.target {
                LaunchTarget::Protocol { launcher, .. } => {
                    format!("handed to {launcher}; ksx cannot verify it ahead of time")
                }
                // Unreachable: `protocol` IS this match arm.
                LaunchTarget::Executable { .. } => "handed to the shell".to_owned(),
            },
            None,
        ),
        Ok(()) => ("ok", "the program is there".to_owned(), None),
        // The refusal text is `ksx run`'s own — the same sentence, at the
        // moment it is useful instead of the moment it is too late.
        Err(err @ PreflightError::ExeMissing { .. }) => (
            "broken",
            err.to_string(),
            Some(entry.path.trim().to_owned()),
        ),
        Err(err @ PreflightError::NotAFile { .. }) => (
            "broken",
            err.to_string(),
            Some(entry.path.trim().to_owned()),
        ),
        Err(err @ PreflightError::NoPath { .. }) => ("broken", err.to_string(), None),
    };

    let mut presets: Vec<String> = Vec::new();
    for slot in &entry.slots {
        if !presets.contains(&slot.preset) {
            presets.push(slot.preset.clone());
        }
    }

    ksx_api::ProfileDetail {
        title: entry.title.clone(),
        path: entry.path.clone(),
        arguments: entry.arguments.clone(),
        slots: entry.slots.len(),
        presets,
        state: state.to_owned(),
        verdict,
        broken_path,
    }
}

/// A store error as the refusal a surface flashes.
fn refuse_config(what: &str, err: ksx_config::ConfigError) -> Refusal {
    Refusal::with_remedy(
        ksx_api::codes::REFUSED,
        format!("{what}: {err}"),
        "run `ksx doctor`",
    )
}

/// A profile-write refusal, keeping the planner's stable code and its advice.
///
/// The code is the planner's, not a fresh one invented here — that is the
/// whole point of `ProfileError::code()` existing, and it is what lets a JSON
/// caller and a web form agree on why they were refused.
fn profile_refusal(err: crate::profile_edit::ProfileError) -> Refusal {
    let advice = err.advice();
    let refusal = Refusal::new(ksx_api::codes::REFUSED, err.to_string());
    if advice.is_empty() {
        refusal
    } else {
        refusal.remedy(advice)
    }
}

fn preset_refusal(err: crate::preset_edit::PresetError) -> Refusal {
    let refusal = Refusal::new(ksx_api::codes::REFUSED, err.to_string());
    match err.advice() {
        Some(advice) => refusal.remedy(advice),
        None => refusal,
    }
}

fn load_profiles() -> (Vec<ProfileRow>, String) {
    let root = match ksx_config::ConfigRoot::discover() {
        Ok(root) => root,
        Err(err) => return (Vec::new(), format!("(config root not found: {err})")),
    };
    let root_display = root.dir().display().to_string();
    let profiles = match ksx_config::Store::new(root).load_games() {
        Ok(loaded) => loaded
            .value
            .games
            .iter()
            .map(|g| ProfileRow {
                title: g.title.clone(),
                detail: match g.slots.len() {
                    1 => format!("{} — 1 slot", g.path),
                    n => format!("{} — {n} slots", g.path),
                },
            })
            .collect(),
        Err(err) => vec![ProfileRow {
            title: "(games.toml unreadable)".to_owned(),
            detail: err.to_string(),
        }],
    };
    (profiles, root_display)
}

/// Is emulation live right now?
///
/// Through [`ksx_api::Client`] rather than a hand-built pipe request, because
/// that is the one typed way every surface asks and a second JSON shape here
/// is exactly what `ksx-api` exists to prevent.
///
/// An unreachable daemon answers `false`, deliberately — the same call
/// `crate::pads::prune` makes and for the same reason: with no daemon there is
/// certainly no session, and a refusal that fires because a diagnostic could
/// not reach a process that is not there would block the fix exactly when the
/// machine most needs it.
fn session_is_running() -> bool {
    use ksx_api::ControlSource as _;
    ksx_api::Client::new(ksx_api::PipeTransport::new())
        .session()
        .running
}

/// A plan's refusal as a [`Refusal`], keeping the remedy when it has one.
///
/// `Refusal::with_remedy` and `Refusal::new` are two calls, and every plan
/// that has no next step would otherwise carry an empty one — which reads on a
/// page as an instruction with the words missing.
fn refusal_of(code: &'static str, message: String, remedy: Option<String>) -> Refusal {
    match remedy {
        Some(remedy) => Refusal::with_remedy(code, message, remedy),
        None => Refusal::new(code, message),
    }
}

// Studio does not consume this yet — its pages read presets through the mapper
// snapshot. The cabinet does, for its slot picker. Kept here rather than in
// `crate::cabinet` because it is a MACHINE read like every other one in this
// file, and the next surface that wants a preset list should find it here.
#[cfg_attr(not(feature = "cabinet"), allow(dead_code))]
/// The MACHINE verbs, for the ones this machine can answer without a daemon
/// and without consent.
///
/// Only [`ksx_api::MachineSource::presets`] is implemented, and that is not an
/// oversight — it is the trait's own design (`ksx-api::machine`): every other
/// default REFUSES in words and names the CLI verb that carries the consent
/// step. A pad test competes for the four XInput slots; a WinUSB claim can
/// leave a panel that no longer types. Neither of those belongs behind a
/// joystick and two buttons, so the cabinet asks for the one read it needs and
/// is told "not here, run this" for the rest — on screen, per press.
pub struct LocalMachine;

impl ksx_api::MachineSource for LocalMachine {
    /// The one thing the cabinet could not previously ask for.
    ///
    /// Until now this fell through to the trait's default, which REFUSES with
    /// "not here, run `ksx devices`" — so a screen that wanted to list devices
    /// got a refusal, and the device picker had nothing to render. It is
    /// read-only and safe mid-session on both halves (see `crate::devices`),
    /// which is why it needs none of the consent ceremony `pads` and
    /// `winusb_claim` keep.
    /// Gated to `cabinet` because that is the only surface that constructs a
    /// `LocalMachine` today. Studio reads the machine through its own
    /// providers, so building this there would carry `to_view` as dead code —
    /// which `--features studio` refuses at `-D warnings`, and which the CI
    /// matrix caught the moment this was written.
    #[cfg(all(windows, feature = "cabinet"))]
    fn devices(&self) -> Result<ksx_api::DevicesView, Refusal> {
        Ok(crate::devices::to_view(&crate::devices::collect()))
    }

    /// The picker read: boards, not devnodes.
    ///
    /// Not feature-gated the way [`Self::devices`] is, because both UI
    /// surfaces want it — Studio's `/devices` page and the cabinet's device
    /// screen render the identical [`ksx_api::DeviceScanView`] — and because
    /// `crate::device_scan::view` is what keeps `crate::devices::to_view` from
    /// being dead code in a `--features studio` build.
    ///
    /// Read-only end to end: it enumerates, it resolves the configured ids
    /// against that enumeration, and it composes commands as STRINGS. Nothing
    /// is opened, claimed or written, so it is safe mid-session.
    #[cfg(windows)]
    fn device_scan(&self) -> Result<ksx_api::DeviceScanView, Refusal> {
        let devices = crate::devices::to_view(&crate::devices::collect());
        let store = crate::device_edit::store().map_err(config_refusal)?;
        let config = store.load_config().map_err(config_refusal)?.value;
        let games = store.load_games().map_err(config_refusal)?.value;
        Ok(crate::device_scan::view(
            &devices,
            &crate::device_edit::connected_facts(),
            &config,
            &games,
        ))
    }

    /// Write one `[[device]]` entry — the plan/apply pair, never the CLI verb.
    ///
    /// `crate::device_edit::pick` looks like the obvious call and would kill
    /// this process: its refusal path is `refuse()`, which is `-> !` and ends
    /// in `std::process::exit`. A daemon-free config write inside a web server
    /// must be able to say no and keep serving, so this drives the same
    /// `plan_pick` + `apply_pick` the CLI does and turns the error into a
    /// [`Refusal`] the surface renders.
    ///
    /// Picking is NOT claiming, and the view says so ([`DevicePickView::
    /// claimed`], [`DevicePickView::next_step`]). Nothing here rebinds a
    /// driver — that needs elevation and stays in the CLI (`docs/SURFACES.md`
    /// §3).
    #[cfg(windows)]
    fn device_pick(
        &self,
        spec: &ksx_api::DevicePickSpec,
    ) -> Result<ksx_api::DevicePickView, Refusal> {
        let survey = ksx_platform::winusb::survey();
        let connected = crate::device_edit::connected_facts();
        let store = crate::device_edit::store().map_err(config_refusal)?;
        let config = store.load_config().map_err(config_refusal)?.value;
        // For the rename check only: re-picking under a new name orphans every
        // slot that named the old one, in config.toml AND in every profile.
        let games = store.load_games().map_err(config_refusal)?.value;

        let wanted = crate::device_edit::PickSpec {
            query: spec.query.trim().to_owned(),
            // A blank box means "derive one", exactly like the absent `--alias`
            // flag — the web form always submits the field, so "" must not
            // become an empty alias the writer then refuses.
            alias: spec
                .alias
                .as_deref()
                .map(str::trim)
                .filter(|alias| !alias.is_empty())
                .map(str::to_owned),
            // `None`, and there is no control to make it anything else. The
            // backend is a statement of fact about the binding
            // (`docs/DEVICE-IDENTITY.md` §7 rule (b)), and the one thing a
            // surface could ask for — `winusb` — needs a claim, which
            // `docs/SURFACES.md` §3 marks "never" for the browser. A picker
            // that offered the choice would be offering an outcome this
            // surface cannot produce.
            backend: None,
        };
        let plan = crate::device_edit::plan_pick(&survey, &connected, &config, &games, &wanted)
            .map_err(pick_refusal)?;
        let outcome = crate::device_edit::apply_pick(&store, &plan).map_err(pick_refusal)?;
        Ok(pick_view(&outcome))
    }

    /// Delete one `[[device]]` entry — and say what that did NOT do.
    ///
    /// Three removals exist in ksx and they are not interchangeable:
    /// `ksx pads --prune` drops stale virtual pads off the ViGEm bus,
    /// `ksx winusb release` puts a claimed board back on the keyboard stack,
    /// and this forgets a config entry. Deleting the entry releases nothing,
    /// which is why [`ksx_api::DeviceRemoveView::still_claimed`] is filled and
    /// carries the release command with it.
    #[cfg(windows)]
    fn device_remove(
        &self,
        spec: &ksx_api::DeviceRemoveSpec,
    ) -> Result<ksx_api::DeviceRemoveView, Refusal> {
        let survey = ksx_platform::winusb::survey();
        let connected = crate::device_edit::connected_facts();
        let store = crate::device_edit::store().map_err(config_refusal)?;
        let config = store.load_config().map_err(config_refusal)?.value;
        let games = store.load_games().map_err(config_refusal)?.value;

        let wanted = crate::device_edit::RemoveSpec {
            alias: spec.alias.trim().to_owned(),
            force: spec.force,
        };
        let plan = crate::device_edit::plan_remove(&survey, &connected, &config, &games, &wanted)
            .map_err(remove_refusal)?;
        let outcome = crate::device_edit::apply_remove(&store, &plan).map_err(remove_refusal)?;
        Ok(remove_view(&outcome))
    }

    /// The first run, and the two verbs a person actually performs on a
    /// configuration.
    ///
    /// All three land in [`crate::onboard`], which is where the CLI's own
    /// `config export|import` machinery is reached from — there is no second
    /// reader, no second writer and no second validator. What differs is only
    /// the answer's shape: a value, rather than a console and an exit code.
    /// Ungated (unlike [`Self::devices`]) because none of it touches hardware:
    /// it is the config store and nothing else, on every platform.
    fn setup_state(&self) -> Result<ksx_api::SetupView, Refusal> {
        crate::onboard::state()
    }

    fn config_export(
        &self,
        request: &ksx_api::ExportRequest,
    ) -> Result<ksx_api::ConfigExport, Refusal> {
        crate::onboard::export(request)
    }

    /// **Dry run unless `request.apply`** — the CLI's consent shape, unchanged.
    fn config_import(
        &self,
        request: &ksx_api::ImportRequest,
    ) -> Result<ksx_api::ImportReport, Refusal> {
        crate::onboard::import(request)
    }

    /// **Can a pad be plugged on this machine right now?**
    ///
    /// One read (`collect_vigembus`) and one judgement
    /// (`ksx_platform::advice::vigembus_advice`) — deliberately the SAME
    /// judgement `ksx doctor` prints, reached through the same function, so a
    /// first-run page and the driver report can never disagree about whether
    /// this machine has a bus. Re-deriving "installed and running" here would
    /// have been three lines and a second opinion.
    ///
    /// Read-only: two registry reads, one service query, one file-version
    /// read. Nothing is installed, and nothing here could install anything —
    /// `docs/SURFACES.md` §3 marks driver installation `never` for the browser
    /// surface, and this is what lets a browser page obey that while still
    /// saying, before the button, that the button cannot work.
    fn pad_bus(&self) -> Result<ksx_api::PadBusView, Refusal> {
        let bus = ksx_platform::collect_vigembus();
        let version = bus
            .driver_file
            .as_ref()
            .and_then(|file| file.file_version.clone());
        // At most one piece of advice comes back per bus (each arm returns),
        // and none at all means healthy. `first()` rather than an index so a
        // future second entry degrades to "the worst thing doctor said" rather
        // than a panic.
        let code = ksx_platform::advice::vigembus_advice(&bus)
            .first()
            .map_or(ksx_api::pad_bus_codes::HEALTHY, |advice| advice.code);
        Ok(ksx_api::PadBusView::from_doctor(code, version))
    }

    /// Everything `ksx pads` and `ksx pads --prune` know, in one read.
    ///
    /// The collectors are the same ones both CLI paths use; what this adds is
    /// the two DECISIONS pre-computed — the prune plan, and what a spawn may
    /// legally offer — so no surface re-derives "is a bus restart allowed" or
    /// "how many of these will a game actually see" (docs/SURFACES.md §1).
    fn pads_view(&self, session_running: bool) -> Result<ksx_api::PadsView, Refusal> {
        use crate::pads::surface;

        // The bus's children ONLY — not `collect()`, which also reads two
        // service keys, walks the Interception class filters, probes for
        // HIDMaestro's DLL and snapshots the process table. Studio polls this
        // every 2 s and throws five sixths of that away.
        let report = ksx_platform::collect_virtual_pads();
        // From XInput, not from the bus: the bus can only ever show ksx its
        // OWN virtual pads, so a real wired Xbox pad is invisible to it and
        // every "N more will be readable" built on that count is wrong by
        // however many real pads are plugged in. `None` stays `None` all the
        // way to the page — see `PadsView::xinput_in_use`.
        let xinput_in_use = ksx_platform::xinput::slots_in_use();
        let owners: Vec<String> = report
            .owners
            .iter()
            .map(|o| format!("{} (pid {})", o.name, o.pid))
            .collect();
        let elevated = ksx_platform::process::is_elevated();
        let plan = crate::pads::plan_prune(
            report.bus_instance_id.as_deref(),
            report.count,
            session_running,
        );
        let prune = surface::prune_plan_view(&plan);
        Ok(ksx_api::PadsView {
            generated_at: now_utc(),
            summary: surface::summary_line(report.count),
            bus_line: surface::bus_line(report.bus_instance_id.as_deref()),
            bus_instance_id: report.bus_instance_id.clone(),
            pads: report
                .pads
                .iter()
                .map(|p| ksx_api::VirtualPadRow {
                    instance_id: p.instance_id.clone(),
                    hardware_id: p.hardware_id.clone(),
                    persona: p.persona_guess.label().to_owned(),
                    xinput: p.persona_guess == ksx_platform::PersonaGuess::Xbox360,
                })
                .collect(),
            owners_line: surface::owners_line(&owners),
            owners,
            session_running,
            xinput_ceiling: ksx_core::MAX_XINPUT_SLOTS,
            xinput_in_use,
            xinput_line: surface::xinput_line(xinput_in_use),
            elevated,
            elevation_line: surface::elevation_line(elevated),
            confirm_line: surface::confirm_line(prune.count),
            // Empty on purpose: this read ANSWERED. The heading belongs to
            // `PadsView::unreadable` alone.
            unreadable_heading: String::new(),
            prune,
            spawn: surface::spawn_offer(session_running, xinput_in_use, report.count),
        })
    }

    /// `ksx pads --count N --persona P`, minus the console.
    ///
    /// The plan decides and this only carries it out — including the refusal
    /// the CLI never needed to make, because whoever typed the command already
    /// knew emulation was stopped and a page click knows nothing.
    fn pads(&self, spec: &ksx_api::PadsSpawnSpec) -> Result<String, Refusal> {
        use crate::pads::surface::{self, SpawnPlan};

        let persona: ksx_core::Persona = spec.persona.parse().map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("'{}' is not a controller ksx knows: {err}", spec.persona),
                "pick a persona from the list",
            )
        })?;
        // Re-read at ACTION time, deliberately: the view a page is rendering
        // may be two seconds old, and two seconds is long enough for a
        // session to start or for another submit's pads to land on the bus.
        let report = ksx_platform::collect_virtual_pads();
        let plan = surface::plan_spawn(
            spec.count,
            persona,
            spec.hold_secs,
            session_is_running(),
            ksx_platform::xinput::slots_in_use(),
            report.count,
        );
        let SpawnPlan::Plug {
            count,
            persona,
            hold_secs,
            ..
        } = plan
        else {
            return Err(refusal_of(
                plan.code().unwrap_or(ksx_api::codes::REFUSED),
                plan.message(),
                plan.remedy(),
            ));
        };
        surface::plug_and_hold(count, persona, hold_secs).map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                err.to_string(),
                "run `ksx doctor` — and `ksx install-drivers` if ViGEmBus is missing",
            )
        })?;
        Ok(plan.message())
    }

    /// `ksx pads --prune`, keeping the CLI's consent shape exactly.
    ///
    /// `confirm == false` is `--yes` absent: a dry run that changes nothing
    /// and says what it would have done. `confirm == true` still refuses
    /// without an administrator token, because ksx never self-elevates — and
    /// the refusal hands over the command that works, which is the whole
    /// reason `PrunePlan::command()` exists.
    fn pads_prune(&self, confirm: bool) -> Result<String, Refusal> {
        use crate::pads::PrunePlan;

        let report = ksx_platform::collect_virtual_pads();
        let plan = crate::pads::plan_prune(
            report.bus_instance_id.as_deref(),
            report.count,
            session_is_running(),
        );
        let PrunePlan::Restart {
            bus_instance_id,
            count,
        } = &plan
        else {
            return match &plan {
                PrunePlan::Nothing => {
                    Ok("no virtual pads on the bus — nothing to prune.".to_owned())
                }
                PrunePlan::NoBus => Err(Refusal::with_remedy(
                    ksx_api::codes::REFUSED,
                    "ViGEmBus exposes no devnode to restart".to_owned(),
                    "if joy.cpl still lists pads, reboot — there is nothing here to act on",
                )),
                PrunePlan::SessionRunning { count } => Err(Refusal::with_remedy(
                    ksx_api::codes::REFUSED,
                    format!(
                        "a session is running, and those {count} pad(s) are the ones it is \
                         driving — pruning would unplug them mid-game"
                    ),
                    "stop emulation first, then prune",
                )),
                // Unreachable: `Restart` is the pattern this `else` excludes.
                PrunePlan::Restart { .. } => unreachable!(),
            };
        };
        let command = plan
            .command()
            .unwrap_or_else(|| format!("pnputil /restart-device \"{bus_instance_id}\""));
        if !confirm {
            return Ok(format!(
                "dry run — {count} virtual pad(s) would be cleared by restarting the bus. \
                 Nothing was changed."
            ));
        }
        // Answered BEFORE anything is narrated as done, the same ordering the
        // CLI keeps: "clearing 15 pads" printed above a refusal reads as though
        // ksx acted and then changed its mind.
        if ksx_platform::process::is_elevated() == Some(false) {
            return Err(Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                "restarting a bus device needs an administrator token, and ksx never \
                 self-elevates"
                    .to_owned(),
                command,
            ));
        }
        let planned = ksx_platform::winusb::PlannedCommand::pnputil(
            &["/restart-device", bus_instance_id],
            "restart the bus, which drops every child pad with it",
        );
        match ksx_platform::winusb::run_command(&planned) {
            Ok(_) => Ok(format!(
                "cleared {count} virtual pad(s) — the bus was restarted."
            )),
            Err(err) => Err(Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("the bus restart failed: {err}"),
                command,
            )),
        }
    }

    fn presets(&self) -> Result<PresetsView, Refusal> {
        let root = ksx_config::ConfigRoot::discover().map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("the config root could not be found: {err}"),
                "run `ksx doctor`",
            )
        })?;
        let store = ksx_config::Store::new(root.clone());
        let loaded = store.load_presets().map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("the presets folder could not be read: {err}"),
                "run `ksx preset list`",
            )
        })?;
        let presets = loaded
            .value
            .iter()
            .map(|file| {
                let core = file.to_core().ok();
                PresetRow {
                    name: file.name.clone(),
                    bound: core.as_ref().map_or(0, |preset| preset.entries.len()),
                    macros: file.macros.len(),
                    // The two in-box seeds. Named rather than derived, because
                    // "may I overwrite this" is a question about identity, not
                    // about what the file happens to contain today.
                    protected: matches!(
                        file.name.to_ascii_lowercase().as_str(),
                        "default" | "empty"
                    ),
                    source: store
                        .preset_path(&file.name)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|_| file.name.clone()),
                }
            })
            .collect();
        // The in-box layouts, from the one registry that holds them
        // (`ksx_core::templates::TEMPLATES` — the same slice
        // `ksx preset list --templates` prints). This field was
        // `Vec::new()` until 2026-08-07: the typed surface promised "the
        // templates a new one can be seeded from" and answered with nothing,
        // so a surface offering "start from a template" had an empty menu and
        // no way to tell that apart from a machine with no templates. Two
        // lines, and the whole of task #14's `keyboard-2p` becomes reachable
        // from something other than a shell.
        // ...composed by `TemplateRow::roster`, which the staged setup serves
        // from too: one mapping from `Template` to a row, so "seed a new
        // preset file" and "dress a staged controller" cannot describe the
        // same panel differently on two screens.
        let templates = TemplateRow::roster();
        Ok(PresetsView {
            config_root: root.presets_dir().display().to_string(),
            presets,
            templates,
        })
    }

    /// games.toml, **preflighted**.
    ///
    /// The one thing this does that no existing read did: it runs
    /// [`ksx_games::preflight`] per row, which is the identical check
    /// `ksx run --game` makes at launch. Same function, same refusal type,
    /// moved to the moment a person is looking at the list rather than the
    /// moment they wanted to play — which is the entire difference between
    /// "MAME 4P is broken, that path is gone" and a cabinet that does nothing
    /// when the button is pressed.
    ///
    /// A protocol URL (`steam://…`) reports `launcher`, never `ok`: preflight
    /// passes it by construction because only the shell can resolve it, and
    /// calling that "ok" would claim a check ksx did not make.
    fn profiles(&self) -> Result<ksx_api::ProfilesView, Refusal> {
        let root = ksx_config::ConfigRoot::discover().map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("the config root could not be found: {err}"),
                "run `ksx doctor`",
            )
        })?;
        let store = ksx_config::Store::new(root.clone());
        let mut notes = Vec::new();
        let games = match store.load_games() {
            Ok(loaded) => {
                notes.extend(loaded.warnings.iter().map(ToString::to_string));
                loaded.value
            }
            Err(err) => {
                return Err(Refusal::with_remedy(
                    ksx_api::codes::REFUSED,
                    format!("games.toml could not be read: {err}"),
                    "run `ksx config export --what games`",
                ))
            }
        };
        let profiles = games.games.iter().map(profile_detail).collect();
        Ok(ksx_api::ProfilesView {
            generated_at: now_utc(),
            config_root: root.dir().display().to_string(),
            games_path: root.games_path().display().to_string(),
            profiles,
            notes,
        })
    }

    /// Append a `[[game]]` — plan, then apply, then report.
    ///
    /// The preset list is read here rather than trusted from the caller: a
    /// form posts a string, and "the preset exists" is a fact about this disk
    /// at this instant, not about the page that was drawn two minutes ago.
    fn profile_new(&self, spec: &ksx_api::NewProfile) -> Result<String, Refusal> {
        let root = ksx_config::ConfigRoot::discover().map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("the config root could not be found: {err}"),
                "run `ksx doctor`",
            )
        })?;
        let store = ksx_config::Store::new(root);
        let games = store
            .load_games()
            .map_err(|err| refuse_config("games.toml could not be read", err))?
            .value;
        let presets: Vec<String> = store
            .load_presets()
            .map_err(|err| refuse_config("the presets folder could not be read", err))?
            .value
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let plan = crate::profile_edit::plan_new(
            &games,
            &presets,
            &crate::profile_edit::NewProfileSpec {
                title: spec.title.clone(),
                path: spec.path.clone(),
                arguments: spec.arguments.clone(),
                slots: spec.slots,
                preset: spec.preset.clone(),
            },
        )
        .map_err(profile_refusal)?;
        let outcome = crate::profile_edit::apply_new(&store, &plan).map_err(profile_refusal)?;
        Ok(outcome.message())
    }

    /// Instantiate an in-box template into a preset file — the same two calls
    /// `ksx preset new` makes, through the same writer.
    fn preset_new(&self, spec: &ksx_api::NewPreset) -> Result<String, Refusal> {
        let root = ksx_config::ConfigRoot::discover().map_err(|err| {
            Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                format!("the config root could not be found: {err}"),
                "run `ksx doctor`",
            )
        })?;
        let store = ksx_config::Store::new(root);
        let path = store
            .canonical_preset_path(&spec.name)
            .map_err(|err| refuse_config("that preset name cannot be a file name", err))?;
        let existing = store
            .load_preset(&spec.name)
            .map_err(|err| refuse_config("the presets folder could not be read", err))?
            .is_some();
        let plan = crate::preset_edit::plan_new(
            existing.then_some(path).as_deref(),
            &crate::preset_edit::NewPresetSpec {
                name: spec.name.clone(),
                template: spec.template.clone(),
                player: spec.player,
                force: spec.force,
            },
        )
        .map_err(preset_refusal)?;
        let outcome = crate::preset_edit::apply_new(&store, &plan).map_err(preset_refusal)?;
        Ok(outcome.message())
    }

    /// Reuses the tray's own launcher, deliberately.
    ///
    /// `crate::studio_launch::open` already carries the property that makes
    /// this honest — probe, start if needed, WAIT for the port, and only then
    /// open anything — and a second launch path would be a second place for
    /// that ordering to be got wrong. Since M9 it opens a chrome-less
    /// application window rather than a browser tab, and the cabinet gets that
    /// for free precisely because it did not grow a launcher of its own. It
    /// returns immediately and does the waiting on its own thread, which is
    /// also what the cabinet needs: this is called from the worker thread, but
    /// eight seconds of port-polling is not something to hold even a worker on.
    #[cfg(feature = "studio")]
    fn open_studio(&self) -> Result<String, Refusal> {
        // `open` narrates to a writer for the tray's console. This caller has
        // no console — the flash carries the same sentence — so the narration
        // goes nowhere and the URL is the return value.
        crate::studio_launch::open(&mut std::io::sink());
        Ok(crate::studio_launch::url())
    }

    /// A cabinet built without `--features studio` has no Studio to open, and
    /// `studio_launch` is not even compiled in.
    ///
    /// The trait's default refusal says "run `ksx studio`", which is a command
    /// this binary does not have — advice that sends someone to a dead end is
    /// worse than the refusal it decorates. So this build says what is actually
    /// true about itself.
    #[cfg(not(feature = "studio"))]
    fn open_studio(&self) -> Result<String, Refusal> {
        Err(Refusal::with_remedy(
            ksx_api::codes::REFUSED,
            "this ksx was built without Studio".to_owned(),
            "rebuild with `--features studio` (the release build ships it)",
        ))
    }
}

// ---------------------------------------------------------------------------
// device pick / remove: outcomes and refusals, as the api spells them
// ---------------------------------------------------------------------------
//
// Field copying and nothing else, deliberately. Every decision above these
// lines — which interface is the keyboard, which rung is unique, whether an
// alias is taken, which slots break — was taken by `crate::device_edit`'s pure
// planners. `docs/SURFACES.md` §1: a surface calls a plan and renders the
// result; if one of these functions ever grew an `if` about hardware, the plan
// is the thing that is missing a field.

/// A config-store failure, worded for a surface.
///
/// The remedy is `ksx doctor` rather than the failing verb: a store that will
/// not open is not a device problem, and sending someone back to press the same
/// button is how a refusal becomes a loop.
#[cfg(windows)]
fn config_refusal(err: ksx_config::ConfigError) -> Refusal {
    Refusal::with_remedy(
        ksx_api::codes::REFUSED,
        format!("the config could not be read: {err}"),
        "run `ksx doctor`",
    )
}

/// `PickError` → `Refusal`, keeping the CLI's own refusal CODE.
///
/// One vocabulary across surfaces: `alias-taken` is `alias-taken` whether it
/// came back from a terminal or from a form post, and the advice — which names
/// the way forward — rides along as the remedy instead of being re-invented per
/// page. The message is one line by construction, which is what the flash needs
/// (`server.rs` caps it at 300 characters).
#[cfg(windows)]
fn pick_refusal(err: crate::device_edit::PickError) -> Refusal {
    let advice = err.advice();
    let refusal = Refusal::new(err.code(), err.to_string());
    if advice.is_empty() {
        refusal
    } else {
        refusal.remedy(advice)
    }
}

#[cfg(windows)]
fn remove_refusal(err: crate::device_edit::RemoveError) -> Refusal {
    let advice = err.advice();
    let refusal = Refusal::new(err.code(), err.to_string());
    if advice.is_empty() {
        refusal
    } else {
        refusal.remedy(advice)
    }
}

#[cfg(windows)]
fn pick_view(outcome: &crate::device_edit::PickOutcome) -> ksx_api::DevicePickView {
    let plan = &outcome.plan;
    let verb = match plan.replaces {
        Some(_) => "updated",
        None => "wrote",
    };
    ksx_api::DevicePickView {
        alias: plan.alias.clone(),
        id: plan.selector.to_string(),
        backend: match plan.backend {
            ksx_config::Backend::Winusb => "winusb".to_owned(),
            ksx_config::Backend::Interception => "interception".to_owned(),
        },
        board: plan.name.clone(),
        instance_id: plan.instance_id.clone(),
        replaced: plan.replaces.clone(),
        claimed: plan.claimed,
        port_pinned: !plan.selector.survives_replug(),
        // Printed, never run. Claiming needs elevation, so the only honest
        // thing a browser can do with it is show it (`docs/SURFACES.md` §3).
        next_step: (!plan.claimed).then(|| format!("ksx winusb claim {}", plan.instance_id)),
        backup: outcome.backup.as_ref().map(|p| p.display().to_string()),
        // ONE sentence: `PickOutcome::message` is a whole report and would be
        // truncated to nothing useful by the flash's 300-character cap. The
        // report's facts are on the page, in the row this just wrote.
        summary: format!(
            "{verb} [[device]] \"{}\" — id = {} — nothing was claimed",
            plan.alias, plan.selector
        ),
    }
}

#[cfg(windows)]
fn remove_view(outcome: &crate::device_edit::RemoveOutcome) -> ksx_api::DeviceRemoveView {
    let plan = &outcome.plan;
    let mut summary = format!("removed [[device]] \"{}\" (id = {})", plan.alias, plan.id);
    if plan.still_claimed.is_some() {
        // The one thing a person must not miss on their way back to the list:
        // the board is still off the Windows keyboard stack and there is no
        // longer an entry anywhere explaining why the panel does not type.
        summary.push_str(" — the board is STILL CLAIMED; releasing it is a separate step");
    }
    ksx_api::DeviceRemoveView {
        alias: plan.alias.clone(),
        id: plan.id.clone(),
        still_claimed: plan.still_claimed.clone(),
        release_command: plan
            .still_claimed
            .as_ref()
            .map(|id| format!("ksx winusb release {id} --yes")),
        breaks: plan.breaks.iter().map(ToString::to_string).collect(),
        backup: outcome.backup.as_ref().map(|p| p.display().to_string()),
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_api::MacroWrite;

    /// A pointer, not a test: the request SHAPES this file used to pin
    /// (`map` with `"key"` / `"keys"` / `"clear"`, `map-macro` with the whole
    /// table) are `ksx-api`'s now, and pinned there — once, against the type
    /// both this daemon and every client use. What is still pinned HERE is the
    /// part that is genuinely this machine's: what the collectors read off
    /// disk, and that an edit survives the WHOLE path back to it.
    #[test]
    fn the_wire_shapes_are_pinned_in_the_crate_that_owns_them() {
        // The one assertion worth keeping at this end: the client every
        // out-of-process surface builds is the shared one, dialling the
        // well-known name.
        let client = ksx_api::Client::new(ksx_api::PipeTransport::new())
            .with_offline_profile(configured_profile);
        assert_eq!(client.sink().path(), crate::daemon::pipe::PIPE_NAME);
    }

    /// The macro editor's WHOLE read side, against a real store: the file's
    /// own shape comes through untranslated (`ms` and `frames` stay apart,
    /// `allow_short` as written), the policies come through as the words the
    /// page prints, and `triggers` carries the keys the `macro.<name>` rows
    /// bind — many keys → one macro, like any other binding.
    #[test]
    fn macros_are_read_in_the_files_own_shape_with_their_trigger_keys() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-studio-macros-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ksx_config::Store::new(ksx_config::ConfigRoot::at(&dir));
        let file: ksx_config::PresetFile = toml::from_str(
            r#"
name = "IPAC P1"
[bindings]
A = "S"
macro.hadouken = ["P", "O"]
macro.taunt = "None"

[macros.hadouken]
on_release = "abort"
retrigger = "restart"
interrupt = "opposing"
steps = [
  { hold = ["dpad.down"], ms = 50 },
  { hold = ["dpad.down","dpad.right"], frames = 3 },
  { hold = [], ms = 5, allow_short = true },
]

[macros.taunt]
steps = [{ hold = ["back"], ms = 200 }]
"#,
        )
        .unwrap();
        store.save_preset(&file).unwrap();

        let snapshot = collect_macros(&store, "IPAC P1");
        assert!(snapshot.available, "{}", snapshot.reason);
        assert_eq!(snapshot.preset, "IPAC P1");
        assert_eq!(snapshot.macros.len(), 2);

        let hadouken = &snapshot.macros[0];
        assert_eq!(hadouken.name, "hadouken");
        assert_eq!(hadouken.on_release, "abort");
        assert_eq!(hadouken.retrigger, "restart");
        assert_eq!(hadouken.interrupt, "opposing");
        assert_eq!(hadouken.triggers, ["P", "O"]);
        assert_eq!(hadouken.steps.len(), 3);
        assert_eq!(hadouken.steps[0].ms, Some(50));
        assert_eq!(hadouken.steps[0].frames, None);
        // A duration authored in frames must survive the read AS frames.
        assert_eq!(hadouken.steps[1].frames, Some(3));
        assert_eq!(hadouken.steps[1].ms, None);
        assert_eq!(hadouken.steps[1].hold, ["dpad.down", "dpad.right"]);
        assert!(hadouken.steps[2].hold.is_empty(), "a neutral gap is legal");
        assert!(hadouken.steps[2].allow_short);

        // Defaults are omitted from the file and come back as the words the
        // page prints, never as an empty string.
        let taunt = &snapshot.macros[1];
        assert_eq!(taunt.name, "taunt");
        assert_eq!(taunt.on_release, "finish");
        assert_eq!(taunt.retrigger, "ignore");
        assert_eq!(taunt.interrupt, "none");
        // The inert "None" placeholder is not a trigger key.
        assert!(taunt.triggers.is_empty());

        // A preset that is not there is UNAVAILABLE with a reason, which is a
        // different fact from "this preset defines no macros" — and a preset
        // that IS there with none says so as an available, empty read.
        let missing = collect_macros(&store, "IPAC P2");
        assert!(!missing.available);
        assert!(missing.reason.contains("IPAC P2"), "{}", missing.reason);
        assert!(missing.reason.contains("IPAC P1"), "{}", missing.reason);

        let plain: ksx_config::PresetFile =
            toml::from_str("name = \"Plain\"\n[bindings]\nA = \"S\"\n").unwrap();
        store.save_preset(&plain).unwrap();
        let none = collect_macros(&store, "Plain");
        assert!(none.available && none.macros.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// EVERY macro field survives a FULL round trip: disk → `collect_macros`
    /// → the editor's draft → `MacroWrite` → `macro_wire` → the daemon's
    /// `map-macro` body reader → `mapping::save_macro` → disk → read again.
    ///
    /// This is the test the `repeat` bug needed. `repeat = "while-held"` was
    /// set in the card, saved, toasted "saved" — and came back `once`, because
    /// the pipe's body allowlist forwarded only `steps` and the three
    /// interruption policies, so `repeat`, `turbo_hz` and `gap_ms` were
    /// dropped between the wire and `MacroFile` and serde filled the default.
    /// A dropped field, a struct-literal conversion that forgets a member, and
    /// a `default()` that overwrites all present IDENTICALLY to the user: the
    /// value they typed is not the value they get back. So the assertion is
    /// per FIELD and table-driven — adding a field to the card without adding
    /// a row here is the only way this can regress silently again.
    #[test]
    fn every_macro_field_survives_the_whole_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-studio-roundtrip-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ksx_config::Store::new(ksx_config::ConfigRoot::at(&dir));

        /// One field's round trip: how the editor EDITS it, and what the read
        /// side must say afterwards.
        struct Case {
            what: &'static str,
            edit: fn(&mut MacroView),
            check: fn(&MacroView),
        }

        let cases = [
            Case {
                what: "repeat = while-held (the reported bug)",
                edit: |m| m.repeat = "while-held".into(),
                check: |m| assert_eq!(m.repeat, "while-held"),
            },
            Case {
                what: "repeat = turbo with an authored turbo_hz",
                edit: |m| {
                    m.repeat = "turbo".into();
                    m.turbo_hz = Some(12);
                    m.gap_ms = None;
                },
                check: |m| {
                    assert_eq!(m.repeat, "turbo");
                    assert_eq!(m.turbo_hz, Some(12));
                    // The rate as AUTHORED: a turbo written in hertz must not
                    // come back converted into the gap spelling.
                    assert_eq!(m.gap_ms, None);
                },
            },
            Case {
                what: "repeat = turbo with an authored gap_ms",
                edit: |m| {
                    m.repeat = "turbo".into();
                    m.turbo_hz = None;
                    m.gap_ms = Some(50);
                },
                check: |m| {
                    assert_eq!(m.repeat, "turbo");
                    assert_eq!(m.gap_ms, Some(50));
                    assert_eq!(m.turbo_hz, None, "the other unit is not invented");
                },
            },
            Case {
                what: "on_release = abort",
                edit: |m| m.on_release = "abort".into(),
                check: |m| assert_eq!(m.on_release, "abort"),
            },
            Case {
                what: "retrigger = restart",
                edit: |m| m.retrigger = "restart".into(),
                check: |m| assert_eq!(m.retrigger, "restart"),
            },
            Case {
                what: "interrupt = any-input",
                edit: |m| m.interrupt = "any-input".into(),
                check: |m| assert_eq!(m.interrupt, "any-input"),
            },
            Case {
                what: "interrupt = opposing",
                edit: |m| m.interrupt = "opposing".into(),
                check: |m| assert_eq!(m.interrupt, "opposing"),
            },
            Case {
                what: "a step's hold SET (many functions at once)",
                edit: |m| m.steps[0].hold = vec!["dpad.down".into(), "dpad.right".into()],
                check: |m| assert_eq!(m.steps[0].hold, ["dpad.down", "dpad.right"]),
            },
            Case {
                what: "an EMPTY hold — a deliberate neutral gap, not a nothing",
                edit: |m| m.steps[0].hold = Vec::new(),
                check: |m| assert!(m.steps[0].hold.is_empty()),
            },
            Case {
                what: "a duration authored in ms stays ms",
                edit: |m| {
                    m.steps[0].ms = Some(120);
                    m.steps[0].frames = None;
                },
                check: |m| {
                    assert_eq!(m.steps[0].ms, Some(120));
                    assert_eq!(m.steps[0].frames, None, "ms must not become frames");
                },
            },
            Case {
                what: "a duration authored in FRAMES stays frames",
                edit: |m| {
                    m.steps[0].ms = None;
                    m.steps[0].frames = Some(3);
                },
                check: |m| {
                    // The whole point of keeping the two units apart: a macro
                    // written by a frame-counter must read back in frames, not
                    // as the 50 ms it happens to equal.
                    assert_eq!(m.steps[0].frames, Some(3));
                    assert_eq!(m.steps[0].ms, None, "frames must not become ms");
                },
            },
            Case {
                what: "allow_short on a deliberately sub-floor step",
                edit: |m| {
                    m.steps[0].ms = Some(5);
                    m.steps[0].frames = None;
                    m.steps[0].allow_short = true;
                },
                check: |m| {
                    assert!(m.steps[0].allow_short);
                    // ...and the short duration is kept AS WRITTEN, not raised
                    // on disk — allow_short is the author saying they meant it.
                    assert_eq!(m.steps[0].ms, Some(5));
                },
            },
            Case {
                what: "step COUNT and order",
                edit: |m| {
                    m.steps = vec![
                        MacroStepView {
                            hold: vec!["dpad.down".into()],
                            ms: Some(50),
                            frames: None,
                            allow_short: false,
                        },
                        MacroStepView {
                            hold: vec!["A".into()],
                            ms: None,
                            frames: Some(2),
                            allow_short: false,
                        },
                    ];
                },
                check: |m| {
                    assert_eq!(m.steps.len(), 2);
                    assert_eq!(m.steps[0].hold, ["dpad.down"]);
                    assert_eq!(m.steps[1].hold, ["A"]);
                    assert_eq!(m.steps[1].frames, Some(2));
                },
            },
        ];

        for case in cases {
            // ── disk: the macro as it starts, plus the trigger rows that
            // start it. Rewritten per case so each one is independent.
            let file: ksx_config::PresetFile = toml::from_str(
                r#"
name = "IPAC P1"
[bindings]
A = "S"
macro.hadouken = ["P", "O"]

[macros.hadouken]
steps = [{ hold = ["A"], ms = 50 }]
"#,
            )
            .unwrap();
            store.save_preset(&file).unwrap();

            // ── read: what the card is seeded with.
            let before = collect_macros(&store, "IPAC P1");
            assert!(before.available, "{}: {}", case.what, before.reason);
            let mut draft = before.macros[0].clone();

            // ── edit: the one field this case is about.
            (case.edit)(&mut draft);

            // ── save: the editor's own path, hop for hop — the surface's
            // draft, the typed request, the LINE it serializes to, the
            // daemon's reader, the one writer.
            let request = MacroWrite {
                preset: "IPAC P1".into(),
                name: draft.name.clone(),
                steps: draft.steps.clone(),
                on_release: draft.on_release.clone(),
                retrigger: draft.retrigger.clone(),
                interrupt: draft.interrupt.clone(),
                repeat: draft.repeat.clone(),
                turbo_hz: draft.turbo_hz,
                gap_ms: draft.gap_ms,
                delete: false,
                enabled: Some(!draft.disabled),
                reload: true,
            }
            .to_request()
            .unwrap_or_else(|err| panic!("{}: the draft was refused: {err}", case.what));
            let wire = serde_json::to_value(ksx_api::Request::MapMacro(request))
                .unwrap_or_else(|err| panic!("{}: unserializable: {err}", case.what));
            let read = ksx_api::MapMacroRequest::from_json(&wire)
                .unwrap_or_else(|err| panic!("{}: the wire body was refused: {err}", case.what));
            crate::mapping::save_macro(
                &store,
                &crate::mapping::MacroSpec {
                    preset: "IPAC P1".into(),
                    name: draft.name.clone(),
                    body: read.body(),
                    delete: read.is_delete(),
                    set_enabled: read.set_enabled(),
                },
            )
            .unwrap_or_else(|err| panic!("{}: the write was refused: {err}", case.what));

            // ── read again, from disk: does the value survive?
            let after = collect_macros(&store, "IPAC P1");
            assert!(after.available, "{}: {}", case.what, after.reason);
            let round = after
                .macros
                .iter()
                .find(|m| m.name == "hadouken")
                .unwrap_or_else(|| panic!("{}: the macro is gone", case.what));
            (case.check)(round);

            // A body write is not a trigger write: the keys that START the
            // macro must still be there afterwards, or saving a policy would
            // quietly unbind the macro.
            assert_eq!(round.triggers, ["P", "O"], "{}", case.what);
            // And the fields this case did NOT touch keep their defaults
            // rather than drifting.
            assert_eq!(round.name, "hadouken", "{}", case.what);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The macro editor's SAVE is one `map-macro` request carrying the whole
    /// table, in the preset FILE's own field names — and a delete carries no
    /// body at all, so the verb's missing-steps refusal still protects a write.
    ///
    /// Kept at this end (as well as in `ksx-api`) because the thing under test
    /// is the SURFACE's draft: blank selects, a rate in one of two units, and
    /// the `enabled` flag that means two different things.
    #[test]
    fn save_macro_sends_the_whole_table_in_one_map_macro_request() {
        let write = MacroWrite {
            preset: "IPAC P1".into(),
            name: "hadouken".into(),
            steps: vec![
                MacroStepView {
                    hold: vec!["dpad.down".into()],
                    ms: Some(50),
                    frames: None,
                    allow_short: false,
                },
                MacroStepView {
                    hold: vec!["A".into()],
                    ms: None,
                    frames: Some(3),
                    allow_short: false,
                },
            ],
            on_release: "abort".into(),
            // Blank is the file's own "field omitted" case: the default.
            retrigger: String::new(),
            interrupt: "  ".into(),
            repeat: "while-held".into(),
            turbo_hz: None,
            gap_ms: None,
            delete: false,
            enabled: None,
            reload: true,
        };
        let wire = serde_json::to_value(ksx_api::Request::MapMacro(
            write.to_request().expect("a valid draft"),
        ))
        .expect("serializable");
        assert_eq!(wire["verb"], "map-macro");
        assert_eq!(wire["preset"], "IPAC P1");
        assert_eq!(wire["name"], "hadouken");
        assert_eq!(wire["reload"], true);
        assert_eq!(wire["on_release"], "abort");
        // A blank select is the file's own omitted field — the default, which
        // the file does not spell either.
        assert!(wire.get("retrigger").is_none(), "{wire}");
        assert!(wire.get("interrupt").is_none(), "{wire}");
        assert_eq!(wire["repeat"], "while-held");
        assert_eq!(wire["steps"][0]["hold"][0], "dpad.down");
        assert_eq!(wire["steps"][0]["ms"], 50);
        assert_eq!(wire["steps"][1]["frames"], 3, "frames stay frames: {wire}");
        assert!(wire["steps"][1].get("ms").is_none(), "{wire}");

        let deleted = serde_json::to_value(ksx_api::Request::MapMacro(
            MacroWrite {
                delete: true,
                ..write
            }
            .to_request()
            .expect("a delete needs no body"),
        ))
        .expect("serializable");
        assert_eq!(deleted["delete"], true);
        assert!(deleted.get("steps").is_none(), "{deleted}");
    }

    #[test]
    fn bus_line_includes_the_driver_version() {
        use ksx_platform::{DriverFileReport, ServiceInfo, StartType};
        let bus = BusDriverReport {
            installed: true,
            service: Some(ServiceInfo {
                start_type: StartType::Demand,
                image_path: None,
                display_name: None,
                state: ServiceState::Running,
            }),
            driver_file: Some(DriverFileReport {
                path: "C:\\Windows\\System32\\drivers\\ViGEmBus.sys".into(),
                file_version: Some("1.21.442.0".into()),
                file_version_string: None,
                company: None,
                description: None,
                signature: None,
            }),
        };
        assert_eq!(
            bus_line(&bus),
            "installed — service running — driver v1.21.442.0"
        );
        assert_eq!(
            bus_line(&BusDriverReport {
                installed: false,
                service: None,
                driver_file: None
            }),
            "not installed"
        );
    }

    #[test]
    fn daemon_check_is_honest_about_its_mechanism() {
        // Cannot assert liveness (depends on the machine), but the wording
        // must always disclose the mechanism's limit and point at the pipe.
        let (_, detail) = daemon_check();
        assert!(detail.contains("Session panel"), "{detail}");
    }
}
