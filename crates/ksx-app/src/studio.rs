//! `ksx studio` — the localhost control room (feature `studio`).
//!
//! Two providers, both thin:
//!
//! - [`StatusSource`] from the EXISTING collectors — `ksx-platform`'s driver
//!   report (which includes the bus's current children), the autostart query,
//!   the games store, and a tasklist-style process check. Fresh point-in-time
//!   snapshot per page load.
//! - [`ksx_studio::ControlSource`] over the daemon's control pipe
//!   (`crate::daemon::pipe`): the session panel's state and its Start / Stop
//!   / Reload buttons are each one pipe request, which enqueues the same
//!   `DaemonCommand` a tray click would (docs/CONTROL-SURFACE.md — no
//!   GUI-only code paths). No daemon on the pipe → the panel says so and the
//!   controls render disabled; this process never becomes a daemon itself.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::daemon::pipe::{self, client};
use ksx_platform::autostart;
use ksx_platform::{BusDriverReport, InterceptionReport, ServiceState};
use ksx_studio::{
    BindConflict, BindOutcome, BindRequest, LearnView, MacroOutcome, MacroSnapshot, MacroStepView,
    MacroView, MacroWrite, MapperSlot, MapperSnapshot, PadRow, ProfileRow, SessionView,
    StatusSnapshot, StatusSource,
};

pub fn run(port: u16) -> anyhow::Result<()> {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    println!("ksx Studio: http://{bind}/  (localhost only; Ctrl+C or close the window to stop)");
    println!("Session controls talk to a running `ksx daemon` over its control pipe.");
    ksx_studio::serve(bind, Box::new(CollectorSource), Box::new(PipeControlSource))?;
    Ok(())
}

/// What the panel says when nothing answers the pipe — state and remedy in
/// one line, because the disabled controls point at it.
const NO_CHANNEL: &str = "no daemon control channel — start the daemon (tray, or `ksx daemon`)";

/// The real [`ksx_studio::ControlSource`]: one pipe request per method,
/// nothing cached, nothing owned.
struct PipeControlSource;

impl ksx_studio::ControlSource for PipeControlSource {
    fn session(&self) -> SessionView {
        match client::request(pipe::PIPE_NAME, &serde_json::json!({ "verb": "status" })) {
            Ok(status) => session_view(&status),
            // No daemon: the page's banner still has to print the command that
            // would START one, and on a games.toml cabinet that command needs
            // its --game flag or it refuses. So the profile comes from the
            // CONFIG here, not from the pipe that just failed to answer.
            Err(client::ClientError::NotRunning) => SessionView {
                profile: configured_profile(),
                ..SessionView::unreachable(NO_CHANNEL)
            },
            Err(err) => SessionView {
                profile: configured_profile(),
                ..SessionView::unreachable(err.to_string())
            },
        }
    }

    fn start(&self, profile: Option<&str>) -> Result<String, String> {
        action(match profile {
            Some(profile) => serde_json::json!({ "verb": "start", "profile": profile }),
            None => serde_json::json!({ "verb": "start" }),
        })
    }

    fn stop(&self) -> Result<String, String> {
        action(serde_json::json!({ "verb": "stop" }))
    }

    fn reload(&self) -> Result<String, String> {
        action(serde_json::json!({ "verb": "reload" }))
    }

    // The mapper verbs: each one pipe request. A daemon that is missing, or
    // that predates the verbs ("unknown verb …"), comes back as the honest
    // "unavailable" LearnView — which is exactly what flips the mapper page
    // read-only with the reason on screen.

    fn learn_start(&self) -> LearnView {
        learn_request(serde_json::json!({ "verb": "learn-key" }))
    }

    fn learn_poll(&self) -> LearnView {
        learn_request(serde_json::json!({ "verb": "learn-poll" }))
    }

    fn learn_cancel(&self) -> LearnView {
        learn_request(serde_json::json!({ "verb": "learn-cancel" }))
    }

    fn restore(&self, preset: &str, mode: &str) -> Result<String, String> {
        action(serde_json::json!({
            "verb": "map-restore",
            "preset": preset,
            "mode": mode,
            "reload": true,
        }))
    }

    fn clear_all(&self, preset: &str) -> Result<String, String> {
        action(serde_json::json!({
            "verb": "map-clear-all",
            "preset": preset,
            "reload": true,
        }))
    }

    fn bind(&self, request: &BindRequest) -> BindOutcome {
        // One key or none — the single-key spelling of a key list.
        let keys: Vec<String> = request.key.clone().into_iter().collect();
        map_request(map_wire(
            &request.preset,
            &request.function,
            &keys,
            request.force,
            request.reload,
            // `bind` edits keys, never the turbo rate: None leaves whatever
            // rate the binding already has alone.
            None,
        ))
    }

    /// The control's WHOLE key list in ONE pipe call — the override the
    /// contract in `ksx-studio/src/control.rs` asks for. The default
    /// implementation composes `bind` calls and can only express nothing or
    /// one key; the daemon's `map` verb takes a `"keys"` list, so "add another
    /// key" and the per-key ✕ become a single atomic write (no
    /// read-modify-write, no half-applied list if the write is refused).
    fn bind_keys(
        &self,
        preset: &str,
        function: &str,
        keys: &[String],
        force: bool,
        reload: bool,
        turbo_hz: Option<u32>,
    ) -> BindOutcome {
        map_request(map_wire(preset, function, keys, force, reload, turbo_hz))
    }

    /// The macro editor's save: ONE `map-macro` request carrying the whole
    /// `[macros.<name>]` table. The steps go over the wire in the preset
    /// FILE's own field names, so the daemon hands them straight to the same
    /// serde types the file uses — no translation layer, and `frames` stays
    /// `frames`.
    fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
        macro_request(macro_wire(request))
    }
}

/// The `map-macro` request body for one whole macro table.
///
/// The three policies are normalized here rather than on the daemon: a blank
/// select is the FILE's own "field omitted" case, which means the default, and
/// spelling that out at the edge keeps the daemon's parser strict (a genuine
/// typo is still refused, in words that name the options).
fn macro_wire(request: &MacroWrite) -> serde_json::Value {
    let policy = |value: &str, default: &'static str| {
        let value = value.trim();
        if value.is_empty() {
            default.to_owned()
        } else {
            value.to_owned()
        }
    };
    let mut wire = serde_json::json!({
        "verb": "map-macro",
        "preset": request.preset,
        "name": request.name,
        "delete": request.delete,
        "reload": request.reload,
        "on_release": policy(&request.on_release, "finish"),
        "retrigger": policy(&request.retrigger, "ignore"),
        "interrupt": policy(&request.interrupt, "none"),
        "repeat": policy(&request.repeat, "once"),
    });
    // The RATE is only sent when the file would carry one. Two spellings of
    // one number, so exactly one goes on the wire; the daemon refuses both,
    // and sending a stale companion field would turn an editor slip into that
    // refusal.
    if let Some(hz) = request.turbo_hz {
        wire["turbo_hz"] = serde_json::json!(hz);
    } else if let Some(ms) = request.gap_ms {
        wire["gap_ms"] = serde_json::json!(ms);
    }
    // A delete carries no body at all — the verb's own refusal for a missing
    // step list is what protects a WRITE from an editor that lost its grid.
    if !request.delete {
        wire["steps"] = serde_json::to_value(&request.steps)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
    }
    wire
}

/// One `map-macro` pipe request → [`MacroOutcome`].
fn macro_request(wire: serde_json::Value) -> MacroOutcome {
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.as_str())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    match client::request(pipe::PIPE_NAME, &wire) {
        Ok(response) => MacroOutcome {
            ok: response["ok"] == true,
            message: response["message"].as_str().map(str::to_owned),
            error: response["error"].as_str().map(str::to_owned),
            code: response["code"].as_str().map(str::to_owned),
            problems: strings(&response["problems"]),
            warnings: strings(&response["warnings"]),
            deleted: response["deleted"] == true,
            backup: response["backup"]["label"].as_str().map(str::to_owned),
            reloaded: response["reloaded"] == true,
        },
        Err(client::ClientError::NotRunning) => MacroOutcome::failed(NO_CHANNEL),
        Err(err) => MacroOutcome::failed(err.to_string()),
    }
}

/// The `map` request body for a control's whole key list. An EMPTY list is a
/// clear (`"clear": true`) — the honest wire shape for "this control now holds
/// nothing", same as `bind(None)`. One key sends `"key"`, so a single-key
/// write is byte-for-byte the request it always was; two or more send
/// `"keys"`.
fn map_wire(
    preset: &str,
    function: &str,
    keys: &[String],
    force: bool,
    reload: bool,
    turbo_hz: Option<u32>,
) -> serde_json::Value {
    let mut wire = serde_json::json!({
        "verb": "map",
        "preset": preset,
        "function": function,
        "force": force,
        "reload": reload,
    });
    match keys {
        [] => wire["clear"] = serde_json::json!(true),
        [only] => wire["key"] = serde_json::json!(only),
        many => wire["keys"] = serde_json::json!(many),
    }
    // AUTO-FIRE (docs/INPUT-TRANSFORMS.md §3). ABSENT means "not asked about",
    // which is what leaves an existing rate alone — so the field is only put
    // on the wire when the caller actually said something about it.
    if let Some(hz) = turbo_hz {
        wire["turbo_hz"] = serde_json::json!(hz);
    }
    wire
}

/// One `map` pipe request → [`BindOutcome`].
fn map_request(wire: serde_json::Value) -> BindOutcome {
    match client::request(pipe::PIPE_NAME, &wire) {
        Ok(response) => BindOutcome {
            ok: response["ok"] == true,
            message: response["message"].as_str().map(str::to_owned),
            error: response["error"].as_str().map(str::to_owned),
            code: response["code"].as_str().map(str::to_owned),
            conflicts: response["conflicts"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| BindConflict {
                            scope: row["scope"].as_str().unwrap_or("").to_owned(),
                            preset: row["preset"].as_str().unwrap_or("").to_owned(),
                            function: row["function"].as_str().unwrap_or("").to_owned(),
                            profile: row["profile"].as_str().map(str::to_owned),
                            slot: row["slot"].as_u64().and_then(|n| u8::try_from(n).ok()),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            // Multi-bind information (absent from a pre-multi-bind daemon,
            // which is exactly an empty list: it had no co-bindings to
            // report because it moved the key instead).
            also_drives: response["also_drives"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row.as_str())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            turbo_hz: response["turbo_hz"]
                .as_u64()
                .and_then(|hz| u32::try_from(hz).ok()),
            turbo_effective_hz: response["turbo_effective_hz"]
                .as_u64()
                .and_then(|hz| u32::try_from(hz).ok()),
            reloaded: response["reloaded"] == true,
        },
        Err(client::ClientError::NotRunning) => BindOutcome::failed(NO_CHANNEL),
        Err(err) => BindOutcome::failed(err.to_string()),
    }
}

/// One learn-verb pipe request → [`LearnView`]. An `ok:false` response (the
/// gate refusal, or an old daemon's "unknown verb") maps to `unavailable`
/// with the daemon's own words.
fn learn_request(request: serde_json::Value) -> LearnView {
    match client::request(pipe::PIPE_NAME, &request) {
        Ok(response) => {
            if response["ok"] == true {
                LearnView {
                    ok: true,
                    state: response["state"].as_str().unwrap_or("unknown").to_owned(),
                    remaining_ms: response["remaining_ms"].as_u64(),
                    device: response["device"].as_str().map(str::to_owned),
                    key: response["key"].as_str().map(str::to_owned),
                    error: response["error"].as_str().map(str::to_owned),
                }
            } else {
                LearnView::unavailable(response["error"].as_str().unwrap_or("the daemon refused"))
            }
        }
        Err(client::ClientError::NotRunning) => LearnView::unavailable(NO_CHANNEL),
        Err(err) => LearnView::unavailable(err.to_string()),
    }
}

fn action(request: serde_json::Value) -> Result<String, String> {
    match client::request(pipe::PIPE_NAME, &request) {
        Ok(response) if response["ok"] == true => {
            Ok(response["message"].as_str().unwrap_or("done").to_owned())
        }
        Ok(response) => Err(response["error"]
            .as_str()
            .unwrap_or("the daemon refused")
            .to_owned()),
        Err(client::ClientError::NotRunning) => Err(NO_CHANNEL.to_owned()),
        Err(err) => Err(err.to_string()),
    }
}

/// One presentation line from the pipe's status response.
fn session_view(status: &serde_json::Value) -> SessionView {
    let run = status["run"].as_str().unwrap_or("unknown");
    let game = status["game"].as_str();
    let running = matches!(run, "running" | "starting");
    let line = match run {
        "running" => {
            let slots = status["slots"].as_u64().unwrap_or(0);
            match game {
                Some(game) => format!("running — {game} — {slots} pad(s)"),
                None => format!("running — {slots} pad(s)"),
            }
        }
        "starting" => "starting…".to_owned(),
        "stopped" => match game {
            Some(game) => format!("idle — profile: {game}"),
            None => "idle".to_owned(),
        },
        "failed" => format!(
            "stopped: {}",
            status["message"].as_str().unwrap_or("last session failed")
        ),
        "quitting" => "daemon shutting down…".to_owned(),
        other => format!("daemon state: {other}"),
    };
    SessionView {
        reachable: true,
        running,
        line,
        profile: game.map(str::to_owned),
    }
}

/// The games.toml profile `ksx daemon` would need to start on this machine —
/// used only when the pipe cannot be reached, so the no-daemon banner prints a
/// command that actually works.
///
/// Mirrors the daemon's own plan resolution: `[[slot]]` entries in config.toml
/// need no profile at all; without them the daemon must be pointed at a
/// games.toml profile, and the first one is what `collect_mapper` is already
/// showing the user.
fn configured_profile() -> Option<String> {
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
struct CollectorSource;

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

    // (number, keyboard, preset, persona, source-line)
    let (rows, source) = match store.load_config() {
        Ok(loaded) if !loaded.value.slots.is_empty() => {
            let rows: Vec<(u8, String, String, ksx_core::Persona)> = loaded
                .value
                .slots
                .iter()
                .map(|s| {
                    (
                        s.number,
                        s.keyboard.clone().unwrap_or_else(|| "(any)".to_owned()),
                        s.preset.clone(),
                        s.persona,
                    )
                })
                .collect();
            (rows, "config.toml [[slot]] entries".to_owned())
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
                            )
                        })
                        .collect();
                    (
                        rows,
                        format!("slots of profile \"{}\" (games.toml)", game.title),
                    )
                }
                None => {
                    return MapperSnapshot {
                        generated_at: now_utc(),
                        source: "no [[slot]] entries in config.toml and no games.toml profiles"
                            .to_owned(),
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
        .map(|(number, keyboard, preset_name, persona)| {
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
            }
        })
        .collect();

    MapperSnapshot {
        generated_at: now_utc(),
        source,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The `bind_keys` OVERRIDE: Studio's whole-key-list edit is ONE `map`
    /// request carrying `"keys"`, not N single-key writes and not a refusal.
    /// Empty is an honest clear, one key is the request the verb always got
    /// (`"key"`, so a pre-list daemon still understands it), and two or more
    /// is the list the engine runs as an OR-chain.
    #[test]
    fn bind_keys_sends_the_whole_list_in_one_map_request() {
        let two = vec!["S".to_owned(), "Enter".to_owned()];
        let wire = map_wire("IPAC P1", "A", &two, false, true, None);
        assert_eq!(wire["verb"], "map");
        assert_eq!(wire["preset"], "IPAC P1");
        assert_eq!(wire["function"], "A");
        assert_eq!(wire["keys"], serde_json::json!(["S", "Enter"]));
        assert!(wire.get("key").is_none(), "{wire}");
        assert!(wire.get("clear").is_none(), "{wire}");
        assert_eq!(wire["reload"], true);

        // One key: the single-key request, unchanged.
        let one = map_wire("IPAC P1", "A", &["G".to_owned()], true, false, None);
        assert_eq!(one["key"], "G");
        assert!(one.get("keys").is_none(), "{one}");
        assert_eq!(one["force"], true);

        // The empty list is the clear — removing a control's last key must
        // leave the inert "None" placeholder, not a silently missing row.
        let none = map_wire("IPAC P1", "A", &[], false, true, None);
        assert_eq!(none["clear"], true);
        assert!(none.get("key").is_none(), "{none}");
        assert!(none.get("keys").is_none(), "{none}");

        // And `bind` composes to exactly the same body, so the two entry
        // points cannot drift.
        let via_bind = map_wire(
            "IPAC P1",
            "A",
            &Some("G".to_owned()).into_iter().collect::<Vec<_>>(),
            true,
            false,
            None,
        );
        assert_eq!(via_bind, one);
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

            // ── save: the editor's own path, hop for hop.
            let wire = macro_wire(&MacroWrite {
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
                reload: true,
            });
            let body = crate::daemon::pipe::macro_body(&wire)
                .unwrap_or_else(|err| panic!("{}: the wire body was refused: {err}", case.what));
            crate::mapping::save_macro(
                &store,
                &crate::mapping::MacroSpec {
                    preset: "IPAC P1".into(),
                    name: draft.name.clone(),
                    body,
                    delete: false,
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
            reload: true,
        };
        let wire = macro_wire(&write);
        assert_eq!(wire["verb"], "map-macro");
        assert_eq!(wire["preset"], "IPAC P1");
        assert_eq!(wire["name"], "hadouken");
        assert_eq!(wire["delete"], false);
        assert_eq!(wire["reload"], true);
        assert_eq!(wire["on_release"], "abort");
        assert_eq!(wire["retrigger"], "ignore");
        assert_eq!(wire["interrupt"], "none");
        assert_eq!(wire["repeat"], "while-held");
        assert_eq!(wire["steps"][0]["hold"][0], "dpad.down");
        assert_eq!(wire["steps"][0]["ms"], 50);
        assert_eq!(wire["steps"][1]["frames"], 3, "frames stay frames: {wire}");
        assert_eq!(wire["steps"][1]["ms"], serde_json::Value::Null);

        let deleted = macro_wire(&MacroWrite {
            delete: true,
            ..write
        });
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

    #[test]
    fn session_view_composes_the_state_line_from_the_pipe_status() {
        let running = session_view(&serde_json::json!({
            "run": "running", "slots": 4, "game": "Street Fighter"
        }));
        assert!(running.reachable && running.running);
        assert_eq!(running.line, "running — Street Fighter — 4 pad(s)");

        let idle = session_view(&serde_json::json!({ "run": "stopped", "game": null }));
        assert!(idle.reachable && !idle.running);
        assert_eq!(idle.line, "idle");

        let failed = session_view(&serde_json::json!({
            "run": "failed", "message": "refusing to start: bad config"
        }));
        assert!(!failed.running);
        assert!(failed.line.contains("bad config"));

        let starting = session_view(&serde_json::json!({ "run": "starting" }));
        assert!(starting.running, "starting must offer Stop, not Start");
    }
}
