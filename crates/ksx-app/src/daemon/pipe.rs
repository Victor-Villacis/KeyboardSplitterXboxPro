//! The daemon's external control channel: `\\.\pipe\ksx-daemon`.
//!
//! One JSON request line in, one JSON response line out, per connection —
//! that is the whole protocol. Verbs: `status`, `start` (optional `profile`),
//! `stop`, `reload`, plus the M7 mapper slice: `map` (edit one preset binding
//! through the same [`crate::mapping::apply`] the CLI verb uses — no
//! pipe-private editor), `learn-key` / `learn-poll` / `learn-cancel` (the
//! asynchronous "press the panel key" recorder, [`super::learn`]). `ksx
//! session` and Studio are thin clients of this; docs/CONTROL-SURFACE.md
//! carries the request/response examples.
//!
//! # Reach
//!
//! The pipe thread has exactly the tray's reach and no more: it enqueues the
//! same [`DaemonCommand`] values the tray menu produces, reads the same
//! [`DaemonState`] snapshot the tray polls, and reads games.toml from disk.
//! It owns no path to the factory, the panel, or any pipeline thread — a
//! wedged pipe client costs other clients their turn on the pipe, never a
//! keyboard.
//!
//! # Trust model
//!
//! The pipe is created with the DEFAULT security descriptor, which grants
//! access to the creating user (plus SYSTEM and administrators) and nobody
//! else. Same-user processes can already `taskkill` the daemon, so the pipe
//! adds no authority they lack; other users get ERROR_ACCESS_DENIED from the
//! object manager before ksx sees the connection. No token, no auth layer.
//!
//! # Concurrency
//!
//! One server thread serves connections **sequentially**. The next pipe
//! instance is created *before* the current connection is served, so a second
//! client (two Studio processes, a `ksx session` racing a page load) connects
//! and simply waits its turn instead of seeing ERROR_FILE_NOT_FOUND; the
//! client side additionally retries briefly on FILE_NOT_FOUND and
//! ERROR_PIPE_BUSY as belt and braces.

use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use super::{DaemonCommand, DaemonState, RunState, SharedState};

/// The one well-known name. Tests use throwaway names; everything else uses
/// this.
pub const PIPE_NAME: &str = r"\\.\pipe\ksx-daemon";

/// `(title, detail)` rows from games.toml, read on demand so `status` reflects
/// what is on disk now — the same freshness rule as `Reload`.
pub type ProfilesFn = Box<dyn Fn() -> Vec<(String, String)> + Send>;

/// The `map` verb's writer — [`crate::mapping::apply`] over the daemon's
/// config root, injected so protocol tests need no disk.
pub type MapFn = Box<
    dyn Fn(&crate::mapping::MapSpec) -> Result<crate::mapping::AppliedMap, crate::mapping::MapError>
        + Send,
>;

/// The `map-restore` verb's writer — [`crate::mapping::restore`], same
/// injection rule as [`MapFn`].
pub type RestoreFn = Box<
    dyn Fn(
            &str,
            crate::mapping::RestoreKind,
        ) -> Result<crate::mapping::AppliedRestore, crate::mapping::MapError>
        + Send,
>;

/// The `map-clear-all` verb's writer — [`crate::mapping::clear_all`], same
/// injection rule as [`MapFn`].
pub type ClearAllFn =
    Box<dyn Fn(&str) -> Result<crate::mapping::AppliedRestore, crate::mapping::MapError> + Send>;

/// The `map-backups` verb's reader — [`crate::mapping::list_backups`], same
/// injection rule as [`MapFn`].
pub type BackupsFn =
    Box<dyn Fn(&str) -> Result<Vec<crate::mapping::PresetBackup>, crate::mapping::MapError> + Send>;

/// Everything a pipe request can reach. One struct so the transport, the
/// tests and future verbs share a single wiring point.
pub struct PipeDeps {
    pub tx: Sender<DaemonCommand>,
    pub state: SharedState,
    pub profiles: ProfilesFn,
    pub map: MapFn,
    pub restore: RestoreFn,
    pub clear_all: ClearAllFn,
    pub backups: BackupsFn,
    pub learn: super::learn::LearnService,
}

/// The real [`MapFn`]: [`crate::mapping::apply`] against `root`'s store —
/// with the session-backup hook: before this daemon lifetime's FIRST write to
/// a preset, the current file is snapshotted to `<file>.session-bak`, which
/// is exactly what `map-restore session-backup` ("undo this session")
/// restores. Once per (daemon lifetime × preset); the set lives in the
/// closure.
pub fn map_fn(root: ksx_config::ConfigRoot) -> MapFn {
    let backed_up = std::sync::Mutex::new(std::collections::BTreeSet::<String>::new());
    Box::new(move |spec| {
        let store = ksx_config::Store::new(root.clone());
        {
            let mut backed = backed_up.lock().expect("session-backup set poisoned");
            if !backed.contains(&spec.preset) {
                // Best-effort ordering: the backup is taken before apply so a
                // successful write can always be undone; if the copy itself
                // fails the bind proceeds (a missing undo must not block
                // mapping) and restore will say "no session backup".
                if crate::mapping::take_session_backup(&store, &spec.preset).is_ok() {
                    backed.insert(spec.preset.clone());
                }
            }
        }
        crate::mapping::apply(&store, spec)
    })
}

/// The real [`RestoreFn`]: [`crate::mapping::restore`] against `root`'s store.
pub fn restore_fn(root: ksx_config::ConfigRoot) -> RestoreFn {
    Box::new(move |preset, kind| {
        crate::mapping::restore(&ksx_config::Store::new(root.clone()), preset, kind)
    })
}

/// The real [`ClearAllFn`]: [`crate::mapping::clear_all`] against `root`'s
/// store.
pub fn clear_all_fn(root: ksx_config::ConfigRoot) -> ClearAllFn {
    Box::new(move |preset| crate::mapping::clear_all(&ksx_config::Store::new(root.clone()), preset))
}

/// The real [`BackupsFn`]: [`crate::mapping::list_backups`] against `root`'s
/// store. Read-only — the one mapper verb that never writes.
pub fn backups_fn(root: ksx_config::ConfigRoot) -> BackupsFn {
    Box::new(move |preset| {
        crate::mapping::list_backups(&ksx_config::Store::new(root.clone()), preset)
    })
}

/// games.toml rows for the status response. Unreadable configuration reports
/// itself as a row rather than vanishing — same honesty rule as Studio.
pub fn profile_rows(root: &ksx_config::ConfigRoot) -> Vec<(String, String)> {
    match ksx_config::Store::new(root.clone()).load_games() {
        Ok(loaded) => loaded
            .value
            .games
            .iter()
            .map(|g| {
                let detail = match g.slots.len() {
                    1 => format!("{} — 1 slot", g.path),
                    n => format!("{} — {n} slots", g.path),
                };
                (g.title.clone(), detail)
            })
            .collect(),
        Err(err) => vec![("(games.toml unreadable)".to_owned(), err.to_string())],
    }
}

/// How long an action verb polls the snapshot for the command's outcome
/// before answering "requested". Long enough for pads to plug; short enough
/// that a client is never parked behind a wedged start.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
const SETTLE_POLL: Duration = Duration::from_millis(25);
/// A request line longer than this is an attack or a bug, not a verb.
const MAX_REQUEST: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Verb handling — pure with respect to the transport, so the protocol is
// testable without a pipe (and without Windows).
// ---------------------------------------------------------------------------

fn snapshot(state: &SharedState) -> DaemonState {
    state.lock().map(|s| s.clone()).unwrap_or_default()
}

fn run_word(run: &RunState) -> &'static str {
    match run {
        RunState::Stopped => "stopped",
        RunState::Starting => "starting",
        RunState::Running { .. } => "running",
        RunState::Failed { .. } => "failed",
        RunState::Quitting => "quitting",
    }
}

fn status_json(state: &SharedState, profiles: &ProfilesFn) -> serde_json::Value {
    let snap = snapshot(state);
    let (slots, message) = match &snap.run {
        RunState::Running { slots } => (Some(*slots), None),
        RunState::Failed { message } => (None, Some(message.clone())),
        _ => (None, None),
    };
    let rows: Vec<serde_json::Value> = profiles()
        .into_iter()
        .map(|(title, detail)| serde_json::json!({ "title": title, "detail": detail }))
        .collect();
    serde_json::json!({
        "ok": true,
        "run": run_word(&snap.run),
        "slots": slots,
        "message": message,
        "game": snap.game,
        "tooltip": snap.tooltip(),
        "profiles": rows,
        "last": snap.last.as_ref().map(|l| serde_json::json!({
            "stop_code": l.stop_code,
            "message": l.message,
            "exit_code": l.exit_code,
            "reboot_required": l.reboot_required,
            "watchdog_tripped": l.watchdog_tripped,
            "dropped_events": l.dropped_events,
        })),
        "live": snap.live.as_ref().map(|h| serde_json::json!({
            "reboot_required": h.reboot_required,
            "watchdog_tripped": h.watchdog_tripped,
            "dropped_events": h.dropped_events,
        })),
    })
}

fn ok_msg(message: String) -> serde_json::Value {
    serde_json::json!({ "ok": true, "message": message })
}

fn err_msg(error: impl Into<String>) -> serde_json::Value {
    serde_json::json!({ "ok": false, "error": error.into() })
}

/// Poll the snapshot until a start (or reload) settles. `baseline` is the run
/// state from before the command was enqueued: a `Failed` identical to the
/// baseline is old news unless `Starting` was seen in between.
fn await_start(state: &SharedState, baseline: &RunState, settle: Duration) -> serde_json::Value {
    let deadline = Instant::now() + settle;
    let mut saw_starting = false;
    loop {
        let snap = snapshot(state);
        match &snap.run {
            // A reload's baseline is already Running: the OLD session must
            // not be reported as the new one, so Running only settles once
            // the state has visibly moved off the baseline.
            RunState::Running { slots } if saw_starting || snap.run != *baseline => {
                return ok_msg(format!("running ({slots} slot(s))"));
            }
            RunState::Starting => saw_starting = true,
            RunState::Failed { message } if saw_starting || snap.run != *baseline => {
                return err_msg(message.clone());
            }
            RunState::Stopped if saw_starting => {
                return err_msg("the session ended as soon as it started");
            }
            RunState::Quitting => return err_msg("the daemon is shutting down"),
            _ => {}
        }
        if Instant::now() >= deadline {
            return ok_msg(
                "requested; the daemon has not reported a new state yet — \
                 check `ksx session status`"
                    .to_owned(),
            );
        }
        std::thread::sleep(SETTLE_POLL);
    }
}

fn await_stop(state: &SharedState, settle: Duration) -> serde_json::Value {
    let deadline = Instant::now() + settle;
    loop {
        let snap = snapshot(state);
        match &snap.run {
            RunState::Stopped => return ok_msg("stopped".to_owned()),
            // The session is over either way; a nonzero summary is its
            // verdict, not a failure of the stop.
            RunState::Failed { message } => return ok_msg(format!("stopped ({message})")),
            RunState::Quitting => return err_msg("the daemon is shutting down"),
            _ => {}
        }
        if Instant::now() >= deadline {
            return ok_msg(
                "stop requested; the session has not reported ending yet — \
                 check `ksx session status`"
                    .to_owned(),
            );
        }
        std::thread::sleep(SETTLE_POLL);
    }
}

/// One request line → one response value. Everything the pipe can do, with
/// the transport factored out.
pub fn handle_request(line: &str, deps: &PipeDeps, settle: Duration) -> serde_json::Value {
    let tx = &deps.tx;
    let state = &deps.state;
    let request: serde_json::Value = match serde_json::from_str(line.trim()) {
        Ok(value) => value,
        Err(err) => return err_msg(format!("request is not a JSON object: {err}")),
    };
    let Some(verb) = request.get("verb").and_then(|v| v.as_str()) else {
        return err_msg(
            r#"request has no "verb" (status | start | stop | reload | map | map-restore | map-clear-all | map-backups | learn-key | learn-poll | learn-cancel)"#,
        );
    };
    match verb {
        "status" => status_json(state, &deps.profiles),
        "start" => {
            let profile = request
                .get("profile")
                .and_then(|p| p.as_str())
                .filter(|p| !p.trim().is_empty())
                .map(str::to_owned);
            let baseline = snapshot(state).run;
            if matches!(baseline, RunState::Running { .. } | RunState::Starting) {
                return err_msg("already running");
            }
            if tx.send(DaemonCommand::Start { game: profile }).is_err() {
                return err_msg("the daemon is shutting down");
            }
            await_start(state, &baseline, settle)
        }
        "stop" => {
            let baseline = snapshot(state).run;
            if !matches!(baseline, RunState::Running { .. } | RunState::Starting) {
                return err_msg("not running");
            }
            if tx.send(DaemonCommand::Stop).is_err() {
                return err_msg("the daemon is shutting down");
            }
            await_stop(state, settle)
        }
        "reload" => {
            let baseline = snapshot(state).run;
            if tx.send(DaemonCommand::Reload).is_err() {
                return err_msg("the daemon is shutting down");
            }
            await_start(state, &baseline, settle)
        }
        "map" => handle_map(&request, deps, settle),
        "map-restore" => handle_map_restore(&request, deps, settle),
        "map-clear-all" => handle_map_clear_all(&request, deps, settle),
        "map-backups" => handle_map_backups(&request, deps),
        // Learn needs an IDLE daemon, and this refusal is deliberate — it was
        // re-examined in full on 2026-08-05 and kept.
        //
        // The mechanical reason: a running session's bound keyboards are
        // captured below win32k, so a Raw Input observer never sees them and
        // the learn would sit there for 10 s hearing nothing. Refusing is the
        // honest answer.
        //
        // The reason we did NOT "fix" it by tapping our own capture stream —
        // which we demonstrably could, since the pipeline is holding those very
        // keystrokes — is worth writing down, because it looks like an obvious
        // win from the outside:
        //
        //   1. the capture thread is the one thread on this machine where a bug
        //      freezes every keyboard until reboot. It is time-critical,
        //      allocation-free and lock-free ON PURPOSE. A convenience feature
        //      does not get a code path in it;
        //   2. a key pressed to be LEARNED would also fire its current binding,
        //      on every slot it fans out to — mapping would inject real
        //      gameplay input into whatever is running;
        //   3. rebinding a key while it is physically held could leave a
        //      virtual button pressed under the old binding and released under
        //      the new one: exactly the stuck-key class the engine's
        //      all-keys-up rule and `swap_tables`' release-on-swap exist to
        //      prevent;
        //   4. mapping is a between-games activity in every tool in the field
        //      study (MAME's TAB menu pauses the machine, RetroArch binds from
        //      its menu). Nobody remaps mid-fight.
        //
        // So the refusal stays — and Studio turns it into one click ("Pause
        // emulation & map", then "Resume emulation") instead of a dead end.
        // docs/CONTROL-SURFACE.md "learn-key semantics".
        "learn-key" => {
            if matches!(
                snapshot(state).run,
                RunState::Running { .. } | RunState::Starting
            ) {
                return err_msg(
                    "learn-key is unavailable while a session is running — captured \
                     keys never reach the observer; stop the session first \
                     (`ksx session stop`, or Studio's \"Pause emulation & map\"), \
                     or bind directly with `ksx map`",
                );
            }
            deps.learn.start()
        }
        "learn-poll" => deps.learn.poll(),
        "learn-cancel" => deps.learn.cancel(),
        other => err_msg(format!(
            "unknown verb '{other}' (status | start | stop | reload | map | \
             map-restore | map-clear-all | map-backups | learn-key | learn-poll |              learn-cancel)"
        )),
    }
}

/// The pipe `map-clear-all` verb: `{"verb":"map-clear-all","preset":…}` plus
/// the same optional `"reload"` as `map`. Unbinds every function of one preset
/// (leaving each one listed and inert), after taking a timestamped backup —
/// so the most destructive mapper button is still one click from undone.
fn handle_map_clear_all(
    request: &serde_json::Value,
    deps: &PipeDeps,
    settle: Duration,
) -> serde_json::Value {
    let Some(preset) = request
        .get("preset")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
    else {
        return err_msg(r#"map-clear-all needs a "preset""#);
    };
    match (deps.clear_all)(preset) {
        Ok(applied) => {
            let mut message = applied.message();
            let outcome = apply_after_write(request, deps, settle, &mut message);
            serde_json::json!({
                "ok": true,
                "message": message,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
                "mode": applied.kind.as_str(),
                "wrote": applied.kind.destination(),
                "backup": applied.backup.as_ref().map(|b| serde_json::json!({
                    "stamp": b.stamp,
                    "label": b.label(),
                    "path": b.path.display().to_string(),
                })),
                "reloaded": outcome.reloaded,
                "hot_swap": outcome.hot,
            })
        }
        Err(err) => err_msg(err.to_string()),
    }
}

/// The pipe `map-backups` verb: `{"verb":"map-backups","preset":…}` → the
/// timestamped restore points on disk, newest first. Read-only, so it never
/// touches the session and never reports a reload.
fn handle_map_backups(request: &serde_json::Value, deps: &PipeDeps) -> serde_json::Value {
    let Some(preset) = request
        .get("preset")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
    else {
        return err_msg(r#"map-backups needs a "preset""#);
    };
    match (deps.backups)(preset) {
        Ok(backups) => serde_json::json!({
            "ok": true,
            "preset": preset,
            "backups": backups.iter().map(|b| serde_json::json!({
                "stamp": b.stamp,
                "label": b.label(),
                "path": b.path.display().to_string(),
            })).collect::<Vec<_>>(),
        }),
        Err(err) => err_msg(err.to_string()),
    }
}

/// The pipe `map` verb: same fields as `ksx map`, plus `"reload": true` to
/// bounce a RUNNING session onto the new binding (a clean `Reload` — stop,
/// re-read from disk, start — never a hot-patch; the CONTROL-SURFACE
/// invariant). With nothing running there is nothing to bounce: the next
/// start reads the file.
fn handle_map(request: &serde_json::Value, deps: &PipeDeps, settle: Duration) -> serde_json::Value {
    let field = |name: &str| {
        request
            .get(name)
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
    };
    let Some(preset) = field("preset") else {
        return err_msg(r#"map needs a "preset""#);
    };
    let Some(function) = field("function") else {
        return err_msg(r#"map needs a "function""#);
    };
    let clear = request
        .get("clear")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let key = field("key");
    if key.is_none() && !clear {
        return err_msg(r#"map needs a "key" (or "clear": true)"#);
    }
    if key.is_some() && clear {
        return err_msg(r#"map takes either "key" or "clear", not both"#);
    }
    // CHORD guards, optional and absent from every pre-chord caller:
    // "when": ["B"] / "unless": ["LeftShift"] (docs/INPUT-TRANSFORMS.md §1b).
    let list = |name: &str| -> Vec<String> {
        request
            .get(name)
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let spec = crate::mapping::MapSpec {
        preset,
        function,
        key,
        // "force" is now ONLY about a cross-slot duplicate (another slot's
        // preset in a profile that uses this one). It removes nothing: a key
        // already used by another control of THIS preset is a multi-bind and
        // needs no flag at all (docs/INPUT-TRANSFORMS.md §1a).
        force: request
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        // "move_from": "B" — the explicit move, the one way this verb unbinds
        // a function the caller did not name in "function".
        move_from: field("move_from"),
        when: list("when"),
        unless: list("unless"),
    };
    match (deps.map)(&spec) {
        Ok(applied) => {
            let mut message = applied.message();
            let outcome = apply_after_write(request, deps, settle, &mut message);
            serde_json::json!({
                "ok": true,
                "message": message,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
                "function": applied.function,
                "key": applied.key,
                "when": applied.when,
                "unless": applied.unless,
                // MULTI-BIND: the other controls of this preset this key also
                // drives. Studio renders it as the legend's "also A · B"
                // badges (ksx-studio/src/render_map.rs `shared_labels`), which
                // it re-derives from disk — this field is the same truth in
                // the write's own answer.
                "also_drives": applied.also_drives,
                // What "move_from" unbound, or null.
                "moved_from": crate::mapping::moved_from_json(applied.moved_from.as_ref()),
                "conflicts": crate::mapping::conflicts_json(&applied.overridden),
                "flash": crate::mapping::flash_json(&applied.flash),
                "reloaded": outcome.reloaded,
                // true = the live session took it with the pads left plugged.
                "hot_swap": outcome.hot,
            })
        }
        Err(crate::mapping::MapError::Conflicts {
            ref key,
            ref conflicts,
        }) => {
            let err = crate::mapping::MapError::Conflicts {
                key: key.clone(),
                conflicts: conflicts.clone(),
            };
            serde_json::json!({
                "ok": false,
                "code": "conflict",
                "error": err.to_string(),
                "conflicts": crate::mapping::conflicts_json(conflicts),
            })
        }
        Err(err) => err_msg(err.to_string()),
    }
}

/// What [`apply_after_write`] did to the running session — the pipe's half of
/// FIX 3's split.
struct Applied {
    /// The running session now has the change (either way it got there).
    reloaded: bool,
    /// It got there WITHOUT the pads being unplugged.
    hot: bool,
}

/// The shared tail of every write verb (`map`, `map-restore`): honour
/// `"reload": true` against a RUNNING session, and append the honest status
/// note either way.
///
/// The verb enqueued is [`DaemonCommand::ApplyBindings`], not `Reload`. The
/// control loop then decides: a binding-only edit is hot-swapped into the live
/// engine (pads stay plugged — Victor's "why does it need to disconnect to
/// reconnect?"), anything structural bounces the session exactly as before.
/// The pipe keeps the tray's reach and no more: it enqueues a command and
/// reads the [`DaemonState`] snapshot the control loop wrote the verdict into,
/// identified by generation so a concurrent client's answer is never mistaken
/// for this one.
fn apply_after_write(
    request: &serde_json::Value,
    deps: &PipeDeps,
    settle: Duration,
    message: &mut String,
) -> Applied {
    let running = matches!(
        snapshot(&deps.state).run,
        RunState::Running { .. } | RunState::Starting
    );
    let want_reload = request
        .get("reload")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !running {
        message.push_str(" — the next session start reads it");
        return Applied {
            reloaded: false,
            hot: false,
        };
    }
    if !want_reload {
        message.push_str(" — a session is running; `reload` to apply now");
        return Applied {
            reloaded: false,
            hot: false,
        };
    }

    let baseline = snapshot(&deps.state)
        .apply
        .map_or(0, |report| report.generation);
    if deps.tx.send(DaemonCommand::ApplyBindings).is_err() {
        message.push_str(" — saved, but the daemon is shutting down (not applied)");
        return Applied {
            reloaded: false,
            hot: false,
        };
    }
    match await_apply(&deps.state, baseline, settle) {
        Some(report) => {
            message.push_str(&format!(" — {}", report.message));
            Applied {
                reloaded: report.ok && (report.hot || report.restarted),
                hot: report.hot,
            }
        }
        None => {
            message.push_str(" — saved; the daemon has not reported applying it yet");
            Applied {
                reloaded: false,
                hot: false,
            }
        }
    }
}

/// Poll [`DaemonState::apply`] until its generation moves past `baseline`.
fn await_apply(state: &SharedState, baseline: u64, settle: Duration) -> Option<super::ApplyReport> {
    let deadline = Instant::now() + settle;
    loop {
        if let Some(report) = snapshot(state).apply {
            if report.generation > baseline {
                return Some(report);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(SETTLE_POLL);
    }
}

/// The pipe `map-restore` verb: `{"verb":"map-restore","preset":…,"mode":
/// "defaults"|"session-backup"|"latest-backup"}` plus the same optional
/// `"reload"` as `map`.
///
/// The three destinations, spelled out because "defaults" is the one that
/// surprises people: `defaults` writes the GENERIC KEYBOARD layout (S=A, D=B,
/// A=X, W=Y…), not "this preset as it shipped"; `session-backup` restores the
/// snapshot taken before this daemon lifetime's first `map` write ("undo this
/// session"); `latest-backup` restores the newest
/// `<preset>.toml.bak-YYYYMMDD-HHMMSS`, which is the undo for a previous
/// restore. Every one of them copies the current file to a fresh timestamped
/// backup first, and the response names it.
fn handle_map_restore(
    request: &serde_json::Value,
    deps: &PipeDeps,
    settle: Duration,
) -> serde_json::Value {
    let Some(preset) = request
        .get("preset")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
    else {
        return err_msg(r#"map-restore needs a "preset""#);
    };
    let Some(kind) = request
        .get("mode")
        .and_then(|v| v.as_str())
        .and_then(crate::mapping::RestoreKind::parse)
    else {
        return err_msg(
            r#"map-restore needs a "mode": "defaults" | "session-backup" | "latest-backup""#,
        );
    };
    match (deps.restore)(preset, kind) {
        Ok(applied) => {
            let mut message = applied.message();
            let outcome = apply_after_write(request, deps, settle, &mut message);
            serde_json::json!({
                "ok": true,
                "message": message,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
                "mode": applied.kind.as_str(),
                // What the caller's confirm dialog promised, echoed back.
                "wrote": applied.kind.destination(),
                "backup": applied.backup.as_ref().map(|b| serde_json::json!({
                    "stamp": b.stamp,
                    "label": b.label(),
                    "path": b.path.display().to_string(),
                })),
                "reloaded": outcome.reloaded,
                "hot_swap": outcome.hot,
            })
        }
        Err(err) => err_msg(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Client — plain std file I/O. `\\.\pipe\...` opens through CreateFileW under
// std, so the client needs no FFI and compiles everywhere (a non-Windows open
// simply fails NotFound, which is the truthful "no daemon here" answer).
// ---------------------------------------------------------------------------

pub mod client {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::time::{Duration, Instant};

    /// Why a request produced no response.
    #[derive(Debug)]
    pub enum ClientError {
        /// The pipe does not exist: no daemon is running — or the one that is
        /// predates the control channel. `ksx session` maps this to exit 2.
        NotRunning,
        Io(std::io::Error),
        Protocol(String),
    }

    impl std::fmt::Display for ClientError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::NotRunning => write!(
                    f,
                    "no ksx daemon control channel at the pipe (the daemon is \
                     not running, or it predates `ksx session`) — start one \
                     with `ksx daemon`"
                ),
                Self::Io(err) => write!(f, "control pipe I/O failed: {err}"),
                Self::Protocol(what) => write!(f, "control pipe protocol error: {what}"),
            }
        }
    }

    impl std::error::Error for ClientError {}

    /// WinError 231: every instance is mid-conversation. The daemon is alive.
    const ERROR_PIPE_BUSY: i32 = 231;
    /// Total budget for open retries (busy server, instance-rotation races).
    const CONNECT_BUDGET: Duration = Duration::from_secs(2);
    const RETRY_PAUSE: Duration = Duration::from_millis(50);
    /// FILE_NOT_FOUND is definitive after this many looks — the retries only
    /// paper over the daemon's instance rotation, which is sub-millisecond.
    const NOT_FOUND_TRIES: u32 = 3;

    fn open(pipe_path: &str) -> Result<std::fs::File, ClientError> {
        let deadline = Instant::now() + CONNECT_BUDGET;
        let mut not_found = 0;
        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(pipe_path)
            {
                Ok(file) => return Ok(file),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    not_found += 1;
                    if not_found >= NOT_FOUND_TRIES {
                        return Err(ClientError::NotRunning);
                    }
                }
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if Instant::now() >= deadline {
                        return Err(ClientError::Io(err));
                    }
                }
                Err(err) => return Err(ClientError::Io(err)),
            }
            std::thread::sleep(RETRY_PAUSE);
        }
    }

    /// One request line in, one response line out.
    pub fn request(
        pipe_path: &str,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        let mut pipe = open(pipe_path)?;
        let mut line = request.to_string();
        line.push('\n');
        pipe.write_all(line.as_bytes()).map_err(ClientError::Io)?;
        pipe.flush().map_err(ClientError::Io)?;

        let mut response = String::new();
        BufReader::new(pipe)
            .read_line(&mut response)
            .map_err(ClientError::Io)?;
        if response.trim().is_empty() {
            return Err(ClientError::Protocol(
                "the daemon closed the connection without a response".into(),
            ));
        }
        serde_json::from_str(response.trim())
            .map_err(|err| ClientError::Protocol(format!("unparsable response: {err}")))
    }
}

// ---------------------------------------------------------------------------
// Server — Win32 named pipe, plain threads. No async runtime anywhere: E7
// rule A (`cargo tree -p ksx-app` shows no tokio) holds by construction.
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod server {
    use super::*;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    /// One pipe instance. Closes on drop; [`Instance::finish`] is the
    /// graceful path (flush → disconnect) for a served connection.
    struct Instance(HANDLE);

    // SAFETY: a pipe HANDLE is a kernel object reference, valid on any thread
    // of the owning process; only the raw-pointer typedef blocks the auto
    // impl.
    unsafe impl Send for Instance {}

    impl Drop for Instance {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateNamedPipeW and is closed
            // exactly once (drop consumes self).
            unsafe { CloseHandle(self.0) };
        }
    }

    impl Instance {
        /// `first` asserts sole ownership of the pipe name: a second daemon
        /// must fail here, not silently split the client stream with the
        /// first.
        fn create(wide_name: &[u16], first: bool) -> Result<Self, u32> {
            let mut open_mode = PIPE_ACCESS_DUPLEX;
            if first {
                open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            // SAFETY: `wide_name` is NUL-terminated and outlives the call; a
            // null security-attributes pointer selects the default (same-user)
            // descriptor — the trust model documented on this module.
            let handle = unsafe {
                CreateNamedPipeW(
                    wide_name.as_ptr(),
                    open_mode,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    0,
                    std::ptr::null(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                // SAFETY: immediately after the failed call, same thread.
                return Err(unsafe { GetLastError() });
            }
            Ok(Self(handle))
        }

        /// Block until a client connects. A client that raced ahead of us
        /// (ERROR_PIPE_CONNECTED) is already connected — success.
        fn connect(&self) -> bool {
            // SAFETY: `self.0` is a live pipe handle; a null OVERLAPPED means
            // synchronous, which is this server's whole design.
            let ok = unsafe { ConnectNamedPipe(self.0, std::ptr::null_mut()) };
            // SAFETY: immediately after the call, same thread.
            ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED
        }

        /// Read until `\n`, EOF, or the size cap.
        fn read_line(&self) -> Option<String> {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                let mut read: u32 = 0;
                // SAFETY: `chunk` outlives the call and its length is passed;
                // `read` receives the byte count.
                let ok = unsafe {
                    ReadFile(
                        self.0,
                        chunk.as_mut_ptr(),
                        chunk.len() as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 || read == 0 {
                    return None;
                }
                buf.extend_from_slice(&chunk[..read as usize]);
                if buf.contains(&b'\n') {
                    break;
                }
                if buf.len() > MAX_REQUEST {
                    return None;
                }
            }
            String::from_utf8(buf).ok()
        }

        fn write_all(&self, mut bytes: &[u8]) -> bool {
            while !bytes.is_empty() {
                let mut written: u32 = 0;
                // SAFETY: `bytes` outlives the call and its length is passed;
                // `written` receives the byte count.
                let ok = unsafe {
                    WriteFile(
                        self.0,
                        bytes.as_ptr(),
                        bytes.len() as u32,
                        &mut written,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 || written == 0 {
                    return false;
                }
                bytes = &bytes[written as usize..];
            }
            true
        }

        /// Flush + disconnect, so the client reads the full response before
        /// the handle goes away. Drop then closes it.
        fn finish(self) {
            // SAFETY: live handle; flush-then-disconnect is the documented
            // graceful server-side teardown for byte pipes.
            unsafe {
                FlushFileBuffers(self.0);
                DisconnectNamedPipe(self.0);
            }
        }
    }

    /// Serve `name` until the process exits. Returns immediately; the thread
    /// logs and dies (leaving tray/stdin untouched) if the name cannot be
    /// owned — e.g. a second daemon is already serving it.
    pub fn spawn(name: String, deps: PipeDeps) {
        spawn_with(name, deps, SETTLE_TIMEOUT);
    }

    /// [`spawn`] with the settle timeout exposed, so tests are not 5 s each.
    pub fn spawn_with(name: String, deps: PipeDeps, settle: Duration) {
        let result = std::thread::Builder::new()
            .name("ksx-daemon-pipe".into())
            .spawn(move || serve(&name, &deps, settle));
        if let Err(err) = result {
            tracing::error!("could not spawn the control-pipe thread: {err}");
        }
    }

    fn serve(name: &str, deps: &PipeDeps, settle: Duration) {
        let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut instance = match Instance::create(&wide_name, true) {
            Ok(instance) => instance,
            Err(code) => {
                tracing::error!(
                    "control pipe {name} unavailable (WinError {code}); \
                     is another ksx daemon already running?"
                );
                return;
            }
        };
        tracing::info!("control pipe listening on {name}");
        loop {
            if !instance.connect() {
                // A failed accept on a healthy handle is transient; recreate
                // rather than spin on it.
                drop(instance);
                match Instance::create(&wide_name, false) {
                    Ok(fresh) => instance = fresh,
                    Err(code) => {
                        tracing::error!("control pipe died (WinError {code})");
                        return;
                    }
                }
                continue;
            }
            // The NEXT instance exists before this connection is served:
            // clients arriving mid-request queue on it instead of finding no
            // pipe at all.
            let next = Instance::create(&wide_name, false);
            if let Some(line) = instance.read_line() {
                let mut response = handle_request(&line, deps, settle).to_string();
                response.push('\n');
                instance.write_all(response.as_bytes());
            }
            instance.finish();
            match next {
                Ok(fresh) => instance = fresh,
                Err(code) => {
                    tracing::error!("control pipe died (WinError {code})");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crossbeam_channel::unbounded;

    fn shared(run: RunState) -> SharedState {
        Arc::new(Mutex::new(DaemonState {
            run,
            ..DaemonState::default()
        }))
    }

    fn no_profiles() -> ProfilesFn {
        Box::new(Vec::new)
    }

    fn fixed_profiles() -> ProfilesFn {
        Box::new(|| {
            vec![
                (
                    "Street Fighter".to_owned(),
                    r"C:\sf.exe — 2 slots".to_owned(),
                ),
                ("Metal Slug".to_owned(), r"C:\ms.exe — 1 slot".to_owned()),
            ]
        })
    }

    /// A `map` that refuses everything — protocol tests that never map.
    fn no_map() -> MapFn {
        Box::new(|spec| {
            Err(crate::mapping::MapError::UnknownPreset {
                name: spec.preset.clone(),
                known: Vec::new(),
            })
        })
    }

    /// A learn service whose observer parks until cancelled (protocol tests
    /// drive phases through the service API, not a keyboard).
    fn idle_learn() -> super::super::learn::LearnService {
        super::super::learn::LearnService::new(Arc::new(|timeout, cancel| {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(None)
        }))
    }

    /// A `map-restore` that refuses everything — protocol tests that never
    /// restore.
    fn no_restore() -> RestoreFn {
        Box::new(|preset, _kind| {
            Err(crate::mapping::MapError::UnknownPreset {
                name: preset.to_owned(),
                known: Vec::new(),
            })
        })
    }

    /// A `map-backups` that always answers "none".
    fn no_backups() -> BackupsFn {
        Box::new(|_preset| Ok(Vec::new()))
    }

    fn deps(tx: Sender<DaemonCommand>, state: SharedState, profiles: ProfilesFn) -> PipeDeps {
        PipeDeps {
            tx,
            state,
            profiles,
            map: no_map(),
            restore: no_restore(),
            clear_all: Box::new(|preset| {
                Err(crate::mapping::MapError::UnknownPreset {
                    name: preset.to_owned(),
                    known: Vec::new(),
                })
            }),
            backups: no_backups(),
            learn: idle_learn(),
        }
    }

    /// Play the control loop's `ApplyBindings` half: consume the command and
    /// write the verdict back into the snapshot, exactly as
    /// `daemon::apply_bindings` does.
    fn answer_apply(
        rx: crossbeam_channel::Receiver<DaemonCommand>,
        state: SharedState,
        report: super::super::ApplyReport,
    ) -> std::thread::JoinHandle<DaemonCommand> {
        std::thread::spawn(move || {
            let command = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("a command was enqueued");
            if let Ok(mut s) = state.lock() {
                let generation = s.apply.as_ref().map_or(0, |a| a.generation) + 1;
                s.apply = Some(super::super::ApplyReport {
                    generation,
                    ..report
                });
            }
            command
        })
    }

    const FAST: Duration = Duration::from_millis(50);

    // -- protocol, no transport ---------------------------------------------

    #[test]
    fn status_reports_state_game_and_profiles() {
        let state = shared(RunState::Running { slots: 4 });
        state.lock().unwrap().game = Some("Street Fighter".into());
        let (tx, _rx) = unbounded();
        let v = handle_request(
            r#"{"verb":"status"}"#,
            &deps(tx.clone(), state.clone(), fixed_profiles()),
            FAST,
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["run"], "running");
        assert_eq!(v["slots"], 4);
        assert_eq!(v["game"], "Street Fighter");
        assert_eq!(v["profiles"][1]["title"], "Metal Slug");
        assert!(v["tooltip"].as_str().unwrap().contains("running, 4 pad(s)"));
    }

    #[test]
    fn start_enqueues_the_same_command_the_tray_produces() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        let worker = std::thread::spawn({
            let state = state.clone();
            move || {
                let command = rx.recv_timeout(Duration::from_secs(1)).unwrap();
                state.lock().unwrap().run = RunState::Running { slots: 2 };
                command
            }
        });
        let v = handle_request(
            r#"{"verb":"start","profile":"Metal Slug"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            Duration::from_secs(2),
        );
        assert_eq!(v["ok"], true, "{v}");
        assert!(v["message"].as_str().unwrap().contains("2 slot(s)"), "{v}");
        assert_eq!(
            worker.join().unwrap(),
            DaemonCommand::Start {
                game: Some("Metal Slug".into())
            }
        );
    }

    #[test]
    fn start_with_no_or_blank_profile_keeps_the_daemons_configured_game() {
        for request in [r#"{"verb":"start"}"#, r#"{"verb":"start","profile":" "}"#] {
            let state = shared(RunState::Stopped);
            let (tx, rx) = unbounded();
            let _ = handle_request(
                request,
                &deps(tx.clone(), state.clone(), no_profiles()),
                FAST,
            );
            assert_eq!(
                rx.try_recv().unwrap(),
                DaemonCommand::Start { game: None },
                "{request}"
            );
        }
    }

    #[test]
    fn start_while_running_is_refused_without_enqueuing() {
        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        let v = handle_request(
            r#"{"verb":"start"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            FAST,
        );
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("already running"));
        assert!(rx.try_recv().is_err(), "nothing may be enqueued");
    }

    /// The same validation path as the tray: a bad profile fails in the
    /// factory's plan resolution, lands in `RunState::Failed`, and the pipe
    /// reports that message — no parallel validator in the pipe thread.
    #[test]
    fn a_start_that_fails_in_the_factory_reports_the_failure_message() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        std::thread::spawn({
            let state = state.clone();
            move || {
                let _ = rx.recv_timeout(Duration::from_secs(1)).unwrap();
                state.lock().unwrap().run = RunState::Starting;
                std::thread::sleep(Duration::from_millis(10));
                state.lock().unwrap().run = RunState::Failed {
                    message: "unknown game \"Typo Fighter\"".into(),
                };
            }
        });
        let v = handle_request(
            r#"{"verb":"start","profile":"Typo Fighter"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            Duration::from_secs(2),
        );
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["error"].as_str().unwrap().contains("Typo Fighter"));
    }

    /// A make() that fails faster than the poll interval must still be
    /// reported: the baseline comparison catches a Stopped→Failed jump even
    /// when Starting was never observed.
    #[test]
    fn a_fast_failure_is_not_mistaken_for_stale_state() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        std::thread::spawn({
            let state = state.clone();
            move || {
                let _ = rx.recv_timeout(Duration::from_secs(1)).unwrap();
                state.lock().unwrap().run = RunState::Failed {
                    message: "cannot start".into(),
                };
            }
        });
        let v = handle_request(
            r#"{"verb":"start"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            Duration::from_secs(2),
        );
        assert_eq!(v["ok"], false, "{v}");
    }

    #[test]
    fn an_unprocessed_command_times_out_into_an_honest_requested_answer() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let v = handle_request(
            r#"{"verb":"start"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert!(v["message"].as_str().unwrap().contains("requested"), "{v}");
    }

    #[test]
    fn stop_when_nothing_runs_is_refused() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        let v = handle_request(
            r#"{"verb":"stop"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            FAST,
        );
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("not running"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn stop_waits_for_the_session_to_end() {
        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        std::thread::spawn({
            let state = state.clone();
            move || {
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                    DaemonCommand::Stop
                );
                state.lock().unwrap().run = RunState::Stopped;
            }
        });
        let v = handle_request(
            r#"{"verb":"stop"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            Duration::from_secs(2),
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["message"], "stopped");
    }

    #[test]
    fn reload_enqueues_reload_and_reports_the_new_session() {
        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        std::thread::spawn({
            let state = state.clone();
            move || {
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                    DaemonCommand::Reload
                );
                state.lock().unwrap().run = RunState::Starting;
                std::thread::sleep(Duration::from_millis(10));
                state.lock().unwrap().run = RunState::Running { slots: 6 };
            }
        });
        let v = handle_request(
            r#"{"verb":"reload"}"#,
            &deps(tx.clone(), state.clone(), no_profiles()),
            Duration::from_secs(2),
        );
        assert_eq!(v["ok"], true, "{v}");
        assert!(v["message"].as_str().unwrap().contains("6 slot(s)"));
    }

    // -- the mapper verbs: map / learn-key / learn-poll / learn-cancel ------

    /// A `map` that records what it was asked and reports a scripted result.
    fn scripted_map(
        result: fn(
            &crate::mapping::MapSpec,
        ) -> Result<crate::mapping::AppliedMap, crate::mapping::MapError>,
        seen: Arc<Mutex<Vec<crate::mapping::MapSpec>>>,
    ) -> MapFn {
        Box::new(move |spec| {
            seen.lock().unwrap().push(spec.clone());
            result(spec)
        })
    }

    fn applied_ok(
        spec: &crate::mapping::MapSpec,
    ) -> Result<crate::mapping::AppliedMap, crate::mapping::MapError> {
        Ok(crate::mapping::AppliedMap {
            path: std::path::PathBuf::from(r"C:\cfg\ksx\presets\IPAC P1.toml"),
            preset: spec.preset.clone(),
            function: spec.function.to_ascii_uppercase(),
            key: spec.key.clone(),
            when: spec.when.clone(),
            unless: spec.unless.clone(),
            also_drives: Vec::new(),
            moved_from: None,
            overridden: Vec::new(),
            flash: Vec::new(),
        })
    }

    #[test]
    fn map_validates_its_fields_before_touching_the_writer() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.map = scripted_map(applied_ok, seen.clone());
        for junk in [
            r#"{"verb":"map"}"#,
            r#"{"verb":"map","preset":"IPAC P1"}"#,
            r#"{"verb":"map","preset":"IPAC P1","function":"A"}"#,
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G","clear":true}"#,
        ] {
            let v = handle_request(junk, &d, FAST);
            assert_eq!(v["ok"], false, "{junk} → {v}");
        }
        assert!(
            seen.lock().unwrap().is_empty(),
            "no write may have happened"
        );
    }

    #[test]
    fn map_while_stopped_writes_and_says_the_next_start_reads_it() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.map = scripted_map(applied_ok, seen.clone());
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"a","key":"G","reload":true}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["function"], "A");
        assert_eq!(v["key"], "G");
        assert_eq!(v["reloaded"], false);
        assert!(
            v["message"]
                .as_str()
                .unwrap()
                .contains("next session start"),
            "{v}"
        );
        assert!(rx.try_recv().is_err(), "nothing to reload when stopped");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].preset, "IPAC P1");
        assert!(!seen[0].force);
    }

    /// FIX 3, pipe half — the hot branch. `reload: true` with a running
    /// session enqueues `ApplyBindings` (NOT `Reload`), and when the control
    /// loop reports a hot swap the response says the pads were left alone.
    #[test]
    fn map_with_reload_hot_swaps_a_running_session() {
        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        let loop_thread = answer_apply(
            rx,
            state.clone(),
            super::super::ApplyReport {
                generation: 0,
                ok: true,
                hot: true,
                restarted: false,
                message: "bindings applied live — pads untouched".to_owned(),
            },
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.map = scripted_map(applied_ok, seen);
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G","reload":true}"#,
            &d,
            Duration::from_secs(2),
        );
        assert_eq!(
            loop_thread.join().unwrap(),
            DaemonCommand::ApplyBindings,
            "a binding save must not enqueue a blunt Reload any more"
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["reloaded"], true, "{v}");
        assert_eq!(v["hot_swap"], true, "{v}");
        assert!(
            v["message"]
                .as_str()
                .unwrap()
                .contains("bindings applied live — pads untouched"),
            "{v}"
        );
    }

    /// …and the bounce branch: a structural change reports the restart, in the
    /// same field shape, so the caller can tell the two apart.
    #[test]
    fn map_with_reload_reports_a_restart_when_the_change_is_structural() {
        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        let loop_thread = answer_apply(
            rx,
            state.clone(),
            super::super::ApplyReport {
                generation: 0,
                ok: true,
                hot: false,
                restarted: true,
                message: "session restarted — slot 3 changed persona (Xbox 360 → PlayStation \
                          (DS4)) needs the pads replugged"
                    .to_owned(),
            },
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.map = scripted_map(applied_ok, seen);
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G","reload":true}"#,
            &d,
            Duration::from_secs(2),
        );
        assert_eq!(loop_thread.join().unwrap(), DaemonCommand::ApplyBindings);
        assert_eq!(v["reloaded"], true, "{v}");
        assert_eq!(v["hot_swap"], false, "{v}");
        assert!(
            v["message"].as_str().unwrap().contains("session restarted"),
            "{v}"
        );
    }

    #[test]
    fn map_without_reload_says_a_running_session_needs_one() {
        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.map = scripted_map(applied_ok, seen);
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["reloaded"], false);
        assert!(
            v["message"].as_str().unwrap().contains("`reload` to apply"),
            "{v}"
        );
        assert!(rx.try_recv().is_err(), "no unasked reload");
    }

    #[test]
    fn map_conflicts_come_back_as_structured_rows() {
        fn conflicted(
            _spec: &crate::mapping::MapSpec,
        ) -> Result<crate::mapping::AppliedMap, crate::mapping::MapError> {
            Err(crate::mapping::MapError::Conflicts {
                key: "G".into(),
                conflicts: vec![crate::mapping::MapConflict {
                    preset: "IPAC P2".into(),
                    function: "A".into(),
                    profile: Some("Steam".into()),
                    slot: Some(2),
                }],
            })
        }
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.map = scripted_map(conflicted, Arc::new(Mutex::new(Vec::new())));
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"B","key":"G"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["code"], "conflict");
        assert_eq!(v["conflicts"][0]["preset"], "IPAC P2");
        assert_eq!(v["conflicts"][0]["function"], "A");
        assert_eq!(v["conflicts"][0]["profile"], "Steam");
        assert_eq!(v["conflicts"][0]["slot"], 2);
        assert!(
            v["error"].as_str().unwrap().contains("\"IPAC P2\"'s A"),
            "{v}"
        );
    }

    /// The mapper's "Map all to one key", through the real writer over the
    /// real verb: three ordinary `map` calls with one key, no `force`, and all
    /// three stick. The response carries the co-bindings (`also_drives`) so
    /// Studio can say what the key drives without waiting for its next poll.
    #[test]
    fn map_binds_one_key_to_several_functions_and_reports_the_co_bindings() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-pipe-multibind-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = ksx_config::ConfigRoot::at(&dir);
        let store = ksx_config::Store::new(root.clone());
        let file: ksx_config::PresetFile =
            toml::from_str("name = \"IPAC P1\"\n[bindings]\nA = \"S\"\nB = \"D\"\nrt = \"E\"\n")
                .unwrap();
        store.save_preset(&file).unwrap();

        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.map = map_fn(root);

        let mut last = serde_json::Value::Null;
        for function in ["A", "B", "rt"] {
            let request =
                format!(r#"{{"verb":"map","preset":"IPAC P1","function":"{function}","key":"P"}}"#);
            last = handle_request(&request, &d, FAST);
            assert_eq!(last["ok"], true, "{request} → {last}");
            assert_eq!(last["moved_from"], serde_json::Value::Null, "{last}");
        }
        assert_eq!(last["also_drives"], serde_json::json!(["A", "B"]), "{last}");
        assert!(
            last["message"].as_str().unwrap().contains("P also drives"),
            "{last}"
        );

        let on_disk = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        for row in ["A = \"P\"", "B = \"P\"", "rt = \"P\""] {
            assert!(on_disk.contains(row), "missing {row} in:\n{on_disk}");
        }

        // …and the explicit move is reachable over the same verb, naming what
        // it unbound and leaving the other two alone.
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"X","key":"P","move_from":"rt"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["moved_from"]["function"], "rt", "{v}");
        assert_eq!(v["moved_from"]["unbound"], true, "{v}");
        assert_eq!(v["also_drives"], serde_json::json!(["A", "B"]), "{v}");
        let on_disk = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(on_disk.contains("rt = \"None\""), "{on_disk}");
        assert!(on_disk.contains("A = \"P\""), "{on_disk}");
        assert!(on_disk.contains("X = \"P\""), "{on_disk}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- map-restore --------------------------------------------------------

    #[test]
    fn map_restore_validates_preset_and_mode() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let d = deps(tx, state, no_profiles());
        for junk in [
            r#"{"verb":"map-restore"}"#,
            r#"{"verb":"map-restore","preset":"IPAC P1"}"#,
            r#"{"verb":"map-restore","preset":"IPAC P1","mode":"yolo"}"#,
        ] {
            let v = handle_request(junk, &d, FAST);
            assert_eq!(v["ok"], false, "{junk} → {v}");
        }
        // The refusal must list all three spellings, or a caller cannot guess
        // the one it is missing.
        let v = handle_request(
            r#"{"verb":"map-restore","preset":"P","mode":"x"}"#,
            &d,
            FAST,
        );
        let error = v["error"].as_str().unwrap();
        for mode in ["defaults", "session-backup", "latest-backup"] {
            assert!(error.contains(mode), "{error}");
        }
    }

    fn restored(preset: &str, kind: crate::mapping::RestoreKind) -> crate::mapping::AppliedRestore {
        crate::mapping::AppliedRestore {
            path: std::path::PathBuf::from(r"C:\cfg\ksx\presets\IPAC P1.toml"),
            preset: preset.to_owned(),
            kind,
            backup: Some(crate::mapping::PresetBackup {
                path: std::path::PathBuf::from(
                    r"C:\cfg\ksx\presets\IPAC P1.toml.bak-20260805-143207",
                ),
                stamp: "20260805-143207".to_owned(),
            }),
        }
    }

    #[test]
    fn map_restore_defaults_reports_the_write_the_backup_and_honours_reload() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.restore = Box::new(|preset, kind| {
            assert_eq!(kind, crate::mapping::RestoreKind::Defaults);
            Ok(restored(preset, kind))
        });
        let v = handle_request(
            r#"{"verb":"map-restore","preset":"IPAC P1","mode":"defaults","reload":true}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["preset"], "IPAC P1");
        assert_eq!(v["mode"], "defaults");
        assert_eq!(v["reloaded"], false, "nothing runs, nothing reloads");
        // FIX 2: the response says exactly what was written — never the bare
        // word "defaults" — and where the old file went.
        assert!(
            v["wrote"].as_str().unwrap().contains("generic keyboard"),
            "{v}"
        );
        assert_eq!(v["backup"]["stamp"], "20260805-143207", "{v}");
        assert_eq!(v["backup"]["label"], "2026-08-05 14:32:07 UTC", "{v}");
        let message = v["message"].as_str().unwrap();
        assert!(message.contains("generic keyboard layout"), "{v}");
        assert!(message.contains("backed up as"), "{v}");
        assert!(rx.try_recv().is_err(), "no reload while stopped");
    }

    /// The third destination (FIX 2): undo the previous restore.
    #[test]
    fn map_restore_accepts_latest_backup() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.restore = Box::new(|preset, kind| {
            assert_eq!(kind, crate::mapping::RestoreKind::LatestBackup);
            Ok(restored(preset, kind))
        });
        let v = handle_request(
            r#"{"verb":"map-restore","preset":"IPAC P1","mode":"latest-backup"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["mode"], "latest-backup", "{v}");
    }

    /// `map-backups` is the read-only list the mapper labels its third button
    /// with ("Restore backup from …").
    #[test]
    fn map_backups_lists_the_restore_points_newest_first() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.backups = Box::new(|preset| {
            assert_eq!(preset, "IPAC P1");
            Ok(vec![
                crate::mapping::PresetBackup {
                    path: std::path::PathBuf::from("b.bak-20260805-143207"),
                    stamp: "20260805-143207".to_owned(),
                },
                crate::mapping::PresetBackup {
                    path: std::path::PathBuf::from("a.bak-20260804-090000"),
                    stamp: "20260804-090000".to_owned(),
                },
            ])
        });
        let v = handle_request(r#"{"verb":"map-backups","preset":"IPAC P1"}"#, &d, FAST);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["backups"][0]["label"], "2026-08-05 14:32:07 UTC", "{v}");
        assert_eq!(v["backups"][1]["stamp"], "20260804-090000", "{v}");
        assert!(rx.try_recv().is_err(), "a read-only verb touches nothing");

        let v = handle_request(r#"{"verb":"map-backups"}"#, &d, FAST);
        assert_eq!(v["ok"], false, "a preset is required: {v}");
    }

    #[test]
    fn map_restore_surfaces_the_no_backup_reason() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.restore = Box::new(|preset, _kind| {
            Err(crate::mapping::MapError::NoSessionBackup {
                preset: preset.to_owned(),
            })
        });
        let v = handle_request(
            r#"{"verb":"map-restore","preset":"IPAC P1","mode":"session-backup"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], false, "{v}");
        assert!(
            v["error"].as_str().unwrap().contains("nothing to undo"),
            "{v}"
        );
    }

    /// End-to-end through the REAL writers: the daemon-lifetime map_fn takes
    /// the session backup before its first write, and map-restore
    /// session-backup undoes every later write of that lifetime.
    #[test]
    fn map_fn_snapshots_once_and_session_backup_restores_it() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-pipe-session-bak-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = ksx_config::ConfigRoot::at(&dir);
        let store = ksx_config::Store::new(root.clone());
        let file: ksx_config::PresetFile =
            toml::from_str("name = \"IPAC P1\"\n[bindings]\nA = \"S\"\n").unwrap();
        store.save_preset(&file).unwrap();

        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.map = map_fn(root.clone());
        d.restore = restore_fn(root);

        for req in [
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G"}"#,
            r#"{"verb":"map","preset":"IPAC P1","function":"B","key":"F"}"#,
        ] {
            let v = handle_request(req, &d, FAST);
            assert_eq!(v["ok"], true, "{req} → {v}");
        }
        let edited = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(edited.contains("A = \"G\""), "{edited}");

        let v = handle_request(
            r#"{"verb":"map-restore","preset":"IPAC P1","mode":"session-backup"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        let restored = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(
            restored.contains("A = \"S\""),
            "backup is the PRE-first-write state: {restored}"
        );
        assert!(!restored.contains("B = \"F\""), "{restored}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn learn_key_is_refused_while_a_session_runs() {
        for run in [RunState::Running { slots: 4 }, RunState::Starting] {
            let state = shared(run);
            let (tx, _rx) = unbounded();
            let v = handle_request(
                r#"{"verb":"learn-key"}"#,
                &deps(tx, state, no_profiles()),
                FAST,
            );
            assert_eq!(v["ok"], false, "{v}");
            let error = v["error"].as_str().unwrap();
            assert!(error.contains("while a session is running"), "{error}");
            assert!(error.contains("ksx map"), "the way out is named: {error}");
        }
    }

    #[test]
    fn learn_key_listens_with_a_countdown_then_cancel_stops_it() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let d = deps(tx, state, no_profiles());
        let v = handle_request(r#"{"verb":"learn-key"}"#, &d, FAST);
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["state"], "listening");
        assert!(v["remaining_ms"].as_u64().unwrap() <= 10_000, "{v}");

        let v = handle_request(r#"{"verb":"learn-poll"}"#, &d, FAST);
        assert_eq!(v["state"], "listening");

        let v = handle_request(r#"{"verb":"learn-cancel"}"#, &d, FAST);
        assert_eq!(v["state"], "cancelled");
        let v = handle_request(r#"{"verb":"learn-poll"}"#, &d, FAST);
        assert_eq!(v["state"], "cancelled");
    }

    #[test]
    fn junk_and_unknown_verbs_are_answered_not_dropped() {
        let state = shared(RunState::Stopped);
        let (tx, rx) = unbounded();
        for junk in ["not json", "{}", r#"{"verb":"launch nukes"}"#, ""] {
            let v = handle_request(junk, &deps(tx.clone(), state.clone(), no_profiles()), FAST);
            assert_eq!(v["ok"], false, "{junk:?} → {v}");
            assert!(v["error"].is_string(), "{junk:?} → {v}");
        }
        assert!(rx.try_recv().is_err());
    }

    // -- transport, Windows only --------------------------------------------

    #[cfg(windows)]
    mod transport {
        use super::*;

        fn unique_pipe(tag: &str) -> String {
            format!(r"\\.\pipe\ksx-test-{}-{tag}", std::process::id())
        }

        #[test]
        fn one_request_one_response_per_connection_served_sequentially() {
            let name = unique_pipe("roundtrip");
            let state = shared(RunState::Running { slots: 4 });
            let (tx, _rx) = unbounded();
            server::spawn_with(
                name.clone(),
                deps(tx, state, fixed_profiles()),
                Duration::from_millis(100),
            );

            // Sequential connections: each opens, asks, gets one line.
            for _ in 0..3 {
                let v = client::request(&name, &serde_json::json!({ "verb": "status" }))
                    .expect("round trip");
                assert_eq!(v["ok"], true);
                assert_eq!(v["run"], "running");
                assert_eq!(v["profiles"][0]["title"], "Street Fighter");
            }
        }

        #[test]
        fn concurrent_clients_all_get_served() {
            let name = unique_pipe("concurrent");
            let state = shared(RunState::Stopped);
            let (tx, _rx) = unbounded();
            server::spawn_with(
                name.clone(),
                deps(tx, state, no_profiles()),
                Duration::from_millis(10),
            );
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let name = name.clone();
                    std::thread::spawn(move || {
                        client::request(&name, &serde_json::json!({ "verb": "status" }))
                    })
                })
                .collect();
            for handle in handles {
                let v = handle.join().unwrap().expect("every client is answered");
                assert_eq!(v["ok"], true);
            }
        }

        #[test]
        fn no_daemon_means_not_running_not_a_hang() {
            let err = client::request(
                &unique_pipe("absent"),
                &serde_json::json!({ "verb": "status" }),
            )
            .unwrap_err();
            assert!(matches!(err, client::ClientError::NotRunning), "{err}");
        }

        #[test]
        fn a_second_server_on_the_same_name_is_refused_but_the_first_keeps_serving() {
            let name = unique_pipe("dup");
            let state = shared(RunState::Stopped);
            let (tx, _rx) = unbounded();
            server::spawn_with(
                name.clone(),
                deps(tx.clone(), state.clone(), no_profiles()),
                Duration::from_millis(10),
            );
            // Let the first server own the name before the pretender tries.
            let v = client::request(&name, &serde_json::json!({ "verb": "status" })).unwrap();
            assert_eq!(v["ok"], true);
            server::spawn_with(
                name.clone(),
                deps(tx, state, no_profiles()),
                Duration::from_millis(10),
            );
            std::thread::sleep(Duration::from_millis(100));
            let v = client::request(&name, &serde_json::json!({ "verb": "status" }))
                .expect("the first server still answers");
            assert_eq!(v["ok"], true);
        }

        /// The full loop: pipe → channel → REAL control loop → factory, with
        /// the profile override landing in the factory and the response
        /// reporting the running session.
        #[test]
        fn the_pipe_drives_the_real_control_loop_end_to_end() {
            struct BlockingRunner;
            impl super::super::super::SessionRunner for BlockingRunner {
                fn run(
                    &mut self,
                    stop: Arc<std::sync::atomic::AtomicBool>,
                    _out: &mut dyn std::io::Write,
                ) -> anyhow::Result<super::super::super::SessionSummary> {
                    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(super::super::super::SessionSummary {
                        stop_code: "daemon-stop".into(),
                        message: "stopped from the pipe".into(),
                        ..Default::default()
                    })
                }
                fn slots(&self) -> usize {
                    3
                }
            }
            struct Factory {
                game: Arc<Mutex<Option<String>>>,
            }
            impl super::super::super::SessionFactory for Factory {
                fn make(&mut self) -> anyhow::Result<Box<dyn super::super::super::SessionRunner>> {
                    Ok(Box::new(BlockingRunner))
                }
                fn config_dir(&self) -> std::path::PathBuf {
                    std::path::PathBuf::from(r"C:\cfg\ksx")
                }
                fn game(&self) -> Option<String> {
                    self.game.lock().unwrap().clone()
                }
                fn set_game(&mut self, game: Option<String>) {
                    *self.game.lock().unwrap() = game;
                }
            }

            let name = unique_pipe("loop");
            let (tx, rx) = unbounded();
            let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));
            let game = Arc::new(Mutex::new(None));
            server::spawn_with(
                name.clone(),
                deps(tx.clone(), state.clone(), no_profiles()),
                Duration::from_secs(2),
            );
            let loop_thread = std::thread::spawn({
                let state = state.clone();
                let game = game.clone();
                move || {
                    let mut factory = Factory { game };
                    let mut out: Vec<u8> = Vec::new();
                    super::super::super::control_loop_with(
                        rx,
                        state,
                        &mut factory,
                        &mut super::super::super::NoPanel,
                        &mut out,
                    );
                }
            });

            let v = client::request(
                &name,
                &serde_json::json!({ "verb": "start", "profile": "Metal Slug" }),
            )
            .expect("start round trip");
            assert_eq!(v["ok"], true, "{v}");
            assert!(v["message"].as_str().unwrap().contains("3 slot(s)"), "{v}");
            assert_eq!(game.lock().unwrap().as_deref(), Some("Metal Slug"));

            let v = client::request(&name, &serde_json::json!({ "verb": "status" })).unwrap();
            assert_eq!(v["run"], "running");
            assert_eq!(v["game"], "Metal Slug");

            let v = client::request(&name, &serde_json::json!({ "verb": "stop" })).unwrap();
            assert_eq!(v["ok"], true, "{v}");

            tx.send(DaemonCommand::Quit).unwrap();
            loop_thread.join().unwrap();
        }
    }
}
