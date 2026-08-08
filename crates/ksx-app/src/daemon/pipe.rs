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
/// this. It is defined with the rest of the protocol in `ksx-api`: the name a
/// client dials is as much a part of the contract as the verbs it carries.
pub const PIPE_NAME: &str = ksx_api::PIPE_NAME;

/// `(title, detail)` rows from games.toml, read on demand so `status` reflects
/// what is on disk now — the same freshness rule as `Reload`.
pub type ProfilesFn = Box<dyn Fn() -> Vec<(String, String)> + Send>;

/// The `map` verb's writer — [`crate::mapping::apply`] over the daemon's
/// config root, injected so protocol tests need no disk.
pub type MapFn = Box<
    dyn Fn(&crate::mapping::MapSpec) -> Result<crate::mapping::AppliedMap, crate::mapping::MapError>
        + Send,
>;

/// The `map-macro` verb's writer — [`crate::mapping::save_macro`], same
/// injection rule as [`MapFn`].
pub type MacroFn = Box<
    dyn Fn(
            &crate::mapping::MacroSpec,
        ) -> Result<crate::mapping::AppliedMacro, crate::mapping::MapError>
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

/// The `slot-assign` verb's writer — [`crate::slots::assign`], same injection
/// rule as [`MapFn`].
pub type SlotAssignFn = Box<
    dyn Fn(&crate::slots::SlotSpec) -> Result<crate::slots::AppliedSlot, crate::slots::SlotError>
        + Send,
>;

/// Everything a pipe request can reach. One struct so the transport, the
/// tests and future verbs share a single wiring point.
pub struct PipeDeps {
    pub tx: Sender<DaemonCommand>,
    pub state: SharedState,
    pub profiles: ProfilesFn,
    pub map: MapFn,
    /// The whole-macro writer (`map-macro`).
    pub save_macro: MacroFn,
    pub restore: RestoreFn,
    pub clear_all: ClearAllFn,
    pub backups: BackupsFn,
    /// The one verb here that is not a preset write: which preset a slot uses
    /// (`slot-assign`, docs/CONTROL-SURFACE.md honest gaps 1 and 5).
    pub slot_assign: SlotAssignFn,
    pub learn: super::learn::LearnService,
}

/// The real [`MapFn`] and [`MacroFn`]: [`crate::mapping::apply`] and
/// [`crate::mapping::save_macro`] against `root`'s store, both behind the
/// session-backup hook — before this daemon lifetime's FIRST write to a
/// preset, the current file is snapshotted to `<file>.session-bak`, which is
/// exactly what `map-restore session-backup` ("undo this session") restores.
/// Once per (daemon lifetime × preset).
///
/// The two are built TOGETHER, and that is the point: they share ONE
/// session-backup set.
///
/// Shared rather than one set each because "the snapshot taken before the
/// FIRST write of this daemon lifetime" has to mean the first write by EITHER
/// of them: a set per writer would let the second one overwrite the undo point
/// with state the user had already changed, and `map-restore session-backup`
/// would then restore a file that was never the starting point.
///
/// The macro writer is otherwise the same shape as the binding writer — a
/// macro body IS a preset write — so neither can drift from the other.
pub fn preset_writers(root: ksx_config::ConfigRoot) -> (MapFn, MacroFn) {
    let backed_up = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::BTreeSet::<String>::new(),
    ));
    // Best-effort ordering: the backup is taken before the write so a
    // successful write can always be undone; if the copy itself fails the
    // write proceeds (a missing undo must not block mapping) and restore will
    // say "no session backup".
    let once = |backed_up: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>| {
        move |store: &ksx_config::Store, preset: &str| {
            let mut backed = backed_up.lock().expect("session-backup set poisoned");
            if !backed.contains(preset)
                && crate::mapping::take_session_backup(store, preset).is_ok()
            {
                backed.insert(preset.to_owned());
            }
        }
    };
    let (map_root, macro_root) = (root.clone(), root);
    let (map_once, macro_once) = (once(backed_up.clone()), once(backed_up));
    (
        Box::new(move |spec| {
            let store = ksx_config::Store::new(map_root.clone());
            map_once(&store, &spec.preset);
            crate::mapping::apply(&store, spec)
        }),
        Box::new(move |spec| {
            let store = ksx_config::Store::new(macro_root.clone());
            macro_once(&store, &spec.preset);
            crate::mapping::save_macro(&store, spec)
        }),
    )
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

/// The real [`SlotAssignFn`]: [`crate::slots::assign`] against `root`'s store.
///
/// No session-backup hook, unlike the preset writers: this writes `config.toml`
/// or `games.toml`, and the store's own `backup()` already copies the file to
/// `<file>.bak-YYYYMMDD-HHMMSS` before every write. The once-per-lifetime
/// `.session-bak` belongs to presets, where a mapping session makes many small
/// edits and "undo everything since the daemon started" is a thing people want.
/// A slot assignment is one deliberate act.
pub fn slot_assign_fn(root: ksx_config::ConfigRoot) -> SlotAssignFn {
    Box::new(move |spec| crate::slots::assign(&ksx_config::Store::new(root.clone()), spec))
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
            r#"request has no "verb" (status | start | stop | reload | map | map-macro | map-restore | map-clear-all | map-backups | slot-assign | learn-key | learn-poll | learn-cancel)"#,
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
        "map-macro" => handle_map_macro(&request, deps, settle),
        "map-restore" => handle_map_restore(&request, deps, settle),
        "map-clear-all" => handle_map_clear_all(&request, deps, settle),
        "map-backups" => handle_map_backups(&request, deps),
        "slot-assign" => handle_slot_assign(&request, deps, settle),
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
            "unknown verb '{other}' (status | start | stop | reload | map | map-macro | \
             map-restore | map-clear-all | map-backups | slot-assign | learn-key | \
             learn-poll | learn-cancel)"
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

/// The pipe `map` verb: same fields as `ksx map` — including `"keys":
/// ["S","Enter"]`, the whole key list for one control (`"key"` is its one-key
/// spelling; exactly one of the two) — plus `"reload": true` to
/// bounce a RUNNING session onto the new binding (a clean `Reload` — stop,
/// re-read from disk, start — never a hot-patch; the CONTROL-SURFACE
/// invariant). With nothing running there is nothing to bounce: the next
/// start reads the file.
fn handle_map(request: &serde_json::Value, deps: &PipeDeps, settle: Duration) -> serde_json::Value {
    // ONE reader, shared with every client (`ksx_api::MapRequest`): which
    // combinations of "key" / "keys" / "clear" are legal, and what each field
    // is called, is answered in the crate both sides link — so a caller can be
    // refused before a round trip, in these exact words, and a field added to
    // the verb cannot reach only one side of it.
    let spec = match ksx_api::MapRequest::from_json(request) {
        Ok(map) => crate::mapping::MapSpec {
            preset: map.preset,
            function: map.function,
            key: map.key,
            keys: map.keys,
            // "force" is now ONLY about a cross-slot duplicate (another slot's
            // preset in a profile that uses this one). It removes nothing: a key
            // already used by another control of THIS preset is a multi-bind and
            // needs no flag at all (docs/INPUT-TRANSFORMS.md §1a).
            force: map.force,
            // "move_from": "B" — the explicit move, the one way this verb unbinds
            // a function the caller did not name in "function".
            move_from: map.move_from,
            when: map.when,
            unless: map.unless,
            // AUTO-FIRE: absent means "not asked about" and leaves the rate alone;
            // 0 clears it (docs/INPUT-TRANSFORMS.md §3).
            turbo_hz: map.turbo_hz,
        },
        Err(refusal) => return err_msg(refusal.message),
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
                // "key" is the FIRST key (null for a clear): unchanged for
                // every one-key write. "keys" is the control's WHOLE list as
                // the file now holds it — what a key-list write reports back.
                "key": applied.key,
                "keys": applied.keys,
                "when": applied.when,
                "unless": applied.unless,
                // AUTO-FIRE (§3): the rate the control now holds and the rate
                // it will actually deliver. Studio shows the second one on the
                // legend row, because it is the one the game will see.
                "turbo_hz": applied.turbo_hz,
                "turbo_effective_hz": applied.turbo_effective_hz,
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

/// The pipe `map-macro` verb: one preset's WHOLE `[macros.<name>]` table.
///
/// ```json
/// {"verb":"map-macro","preset":"IPAC P1","name":"hadouken",
///  "steps":[{"hold":["dpad.down"],"ms":50},
///           {"hold":["dpad.down","dpad.right"],"ms":50},
///           {"hold":["dpad.right"],"ms":50},
///           {"hold":["A"],"frames":3}],
///  "on_release":"finish","retrigger":"ignore","interrupt":"none",
///  "repeat":"turbo","turbo_hz":10,
///  "reload":true}
/// ```
///
/// The body's field names ARE `ksx_config::MacroFile`'s: this verb hands the
/// object straight to the same serde types the preset file uses, so the wire
/// shape and the file shape cannot drift and `frames` survives as `frames`.
/// `{"delete": true}` removes the table (and the `macro.<name>` trigger rows
/// that would otherwise dangle) — an explicit word, never an empty step list.
///
/// `"enabled"` is the one field that means two things, and which one is decided
/// by whether a BODY came with it:
///
/// - `{"steps":[…], "enabled":false}` — an ordinary whole-table write that
///   happens to land disabled. `enabled` is a `MacroFile` field like any other.
/// - `{"name":"hadouken","enabled":false}` with NO `steps` — a TOGGLE. The
///   table on disk keeps every step and every policy and only the flag moves,
///   which is the whole promise of disabling instead of deleting: what comes
///   back is exactly what went away. (`ksx macro --disable` sends this.)
///
/// `"reload": true` applies it to a RUNNING session, and a macro body is a
/// BINDING change: it changes no slot, persona, device or capture backend, so
/// [`crate::run::supervisor::SessionShape::bounce_reason`] finds nothing and
/// the control loop hot-swaps it with the pads left plugged — the same
/// [`super::DaemonCommand::ApplyBindings`] path `map` takes, through the same
/// [`apply_after_write`].
fn handle_map_macro(
    request: &serde_json::Value,
    deps: &PipeDeps,
    settle: Duration,
) -> serde_json::Value {
    // ONE reader again (`ksx_api::MapMacroRequest`), and here it is not just
    // tidiness: the body half IS `ksx_config::MacroFile`, so every field of a
    // macro table travels by construction. The reader this replaced carried an
    // ALLOWLIST of body fields, `repeat` was missing from it, and a card that
    // set `while-held` saved `once` under a "saved" toast. A list that has to
    // be remembered is a bug with a delay on it; there is no list now.
    let spec = match ksx_api::MapMacroRequest::from_json(request) {
        Ok(macro_request) => crate::mapping::MacroSpec {
            preset: macro_request.preset.clone(),
            name: macro_request.name.clone(),
            body: macro_request.body(),
            delete: macro_request.is_delete(),
            set_enabled: macro_request.set_enabled(),
        },
        Err(refusal) => return err_msg(refusal.message),
    };
    match (deps.save_macro)(&spec) {
        Ok(applied) => {
            let mut message = applied.message();
            let outcome = apply_after_write(request, deps, settle, &mut message);
            serde_json::json!({
                "ok": true,
                "message": message,
                "path": applied.path.display().to_string(),
                "preset": applied.preset,
                "name": applied.name,
                "steps": applied.steps,
                "total_ms": applied.total_ms,
                "deleted": applied.deleted,
                // Does the table RUN, and was this write nothing BUT that flag?
                "enabled": applied.enabled,
                "toggled": applied.toggled,
                // The keys that START it — unchanged by this verb (`map` with
                // "macro.<name>" is what writes those), except on a delete,
                // where these are the rows that had to go with the table.
                "triggers": applied.triggers,
                // Advisories, never swallowed: a step below the sampling floor
                // is raised, or run as written and possibly missed.
                "warnings": applied.warnings,
                "backup": applied.backup.as_ref().map(|b| serde_json::json!({
                    "stamp": b.stamp,
                    "label": b.label(),
                    "path": b.path.display().to_string(),
                })),
                "reloaded": outcome.reloaded,
                "hot_swap": outcome.hot,
            })
        }
        Err(err) => {
            let problems = match &err {
                crate::mapping::MapError::BadMacro { problems, .. } => problems.clone(),
                _ => Vec::new(),
            };
            serde_json::json!({
                "ok": false,
                "code": crate::map::error_code(&err),
                "error": err.to_string(),
                // The refusals one by one, so a UI can put each on its own row
                // instead of parsing the sentence apart.
                "problems": problems,
            })
        }
    }
}

/// The pipe `slot-assign` verb: `{"verb":"slot-assign","slot":3,"preset":"IPAC
/// P3","profile":"Steam","reload":true}` — which preset a slot uses
/// (docs/CONTROL-SURFACE.md honest gaps 1 and 5).
///
/// **This is the one write verb that never claims a hot swap.** Every other one
/// enqueues [`DaemonCommand::ApplyBindings`] and lets the control loop pick the
/// cheapest correct answer; a slot assignment enqueues the blunt
/// [`DaemonCommand::Reload`] and reports `restarted`. The reasoning is on
/// [`ksx_api::SlotOutcome::restarted`] and it is deliberate: this verb writes
/// the slot ENTRY, and one predictable answer beats a cheaper one that is only
/// sometimes true.
fn handle_slot_assign(
    request: &serde_json::Value,
    deps: &PipeDeps,
    settle: Duration,
) -> serde_json::Value {
    // ONE reader, shared with every client — the same rule `map` follows: a
    // caller is refused in these exact words before a round trip.
    let assign = match ksx_api::SlotAssignRequest::from_json(request) {
        Ok(assign) => assign,
        Err(refusal) => {
            return serde_json::json!({
                "ok": false, "code": refusal.code, "error": refusal.message,
            })
        }
    };
    // The persona NAME becomes a persona HERE, in the daemon, through
    // `ksx_core`'s one lenient `FromStr` — the parser `ksx pads --persona` and
    // every config file already go through, aliases and all. Not in
    // `SlotAssignRequest::from_json`: ksx-api would then need a persona
    // vocabulary of its own, which is the second copy of the alias table the
    // wire field's doc comment refuses. An unknown name is refused in
    // `UnknownPersona`'s own words, which list every valid one.
    let persona = match assign
        .persona
        .as_deref()
        .map(str::parse::<ksx_core::Persona>)
    {
        None => None,
        Some(Ok(persona)) => Some(persona),
        Some(Err(unknown)) => {
            return serde_json::json!({
                "ok": false,
                "code": "unknown-persona",
                "error": unknown.to_string(),
            })
        }
    };
    let applied = match (deps.slot_assign)(&crate::slots::SlotSpec {
        slot: assign.slot,
        preset: assign.preset.clone(),
        profile: assign.profile.clone(),
        persona,
    }) {
        Ok(applied) => applied,
        Err(err) => {
            return serde_json::json!({
                "ok": false, "code": err.code(), "error": err.to_string(),
            })
        }
    };

    let mut message = applied.message();
    let bounce = bounce_after_slot_write(&assign, &applied, deps, settle, &mut message);
    serde_json::json!({
        "ok": true,
        "message": message,
        "path": applied.path.display().to_string(),
        "slot": applied.slot,
        "preset": applied.preset,
        "previous_preset": applied.previous,
        // Canonical spelling, from `Persona::as_str` — so a surface that
        // echoes this straight back into the next request cannot introduce a
        // second spelling of one persona.
        "persona": applied.persona.as_str(),
        "previous_persona": applied.previous_persona.map(|p| p.as_str()),
        "profile": applied.profile,
        "created": applied.created,
        "unchanged": applied.unchanged,
        "backup": applied.backup.as_ref().map(|path| serde_json::json!({
            "stamp": backup_stamp(path),
            "label": backup_stamp(path),
            "path": path.display().to_string(),
        })),
        "restarted": bounce.restarted,
        // What the daemon DID, not what the caller asked for. `SlotOutcome`'s
        // field is documented as "`reload` was asked for and the daemon acted
        // on it", and echoing the request made that documentation false in the
        // one case it mattered: a running session that was asked to restart and
        // did not come back reported `reloaded: true, restarted: false`, which
        // reads as "nothing was running".
        "reloaded": bounce.reconciled,
    })
}

/// What [`bounce_after_slot_write`] did.
struct Bounce {
    /// The session was torn down and came back on the new wiring.
    restarted: bool,
    /// The running session (if any) now matches what is on disk — either
    /// because it restarted, or because there was nothing running to restart.
    /// `false` means a session is running on the OLD wiring, or was stopped
    /// and could not be started again; the appended message says which.
    reconciled: bool,
}

/// `<file>.bak-YYYYMMDD-HHMMSS` → `YYYYMMDD-HHMMSS`, or the whole file name
/// when it does not carry one. The store names these; this only reads them.
fn backup_stamp(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once(".bak-").map(|(_, stamp)| stamp.to_owned()))
        .unwrap_or_else(|| path.display().to_string())
}

/// `slot-assign`'s tail: bounce a RUNNING session onto the new wiring, and say
/// what happened either way.
///
/// Deliberately NOT [`apply_after_write`]: that one asks the control loop for
/// the cheapest correct answer, and for a preset-only re-point the answer would
/// be a hot swap with the pads left plugged. A caller that was told "the pads
/// replug" and then saw them not replug has been lied to in the harmless
/// direction, which is still a surface nobody can predict.
fn bounce_after_slot_write(
    assign: &ksx_api::SlotAssignRequest,
    applied: &crate::slots::AppliedSlot,
    deps: &PipeDeps,
    settle: Duration,
    message: &mut String,
) -> Bounce {
    let nothing_to_do = Bounce {
        restarted: false,
        reconciled: true,
    };
    let left_stale = Bounce {
        restarted: false,
        reconciled: false,
    };
    if applied.unchanged {
        return nothing_to_do;
    }
    let baseline = snapshot(&deps.state).run;
    let running = matches!(baseline, RunState::Running { .. } | RunState::Starting);
    if !running {
        message.push_str(" — nothing is running, so the next start reads it");
        return nothing_to_do;
    }
    if !assign.reload {
        message.push_str(" — a session is running on the old wiring; `reload` to restart it");
        return left_stale;
    }
    if deps.tx.send(DaemonCommand::Reload).is_err() {
        message.push_str(" — written, but the daemon is shutting down (not restarted)");
        return left_stale;
    }
    let answer = await_start(&deps.state, &baseline, settle);
    let restarted = answer
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if let Some(line) = answer.get("message").or_else(|| answer.get("error")) {
        if let Some(line) = line.as_str() {
            message.push_str(" — ");
            message.push_str(line);
        }
    }
    Bounce {
        restarted,
        reconciled: restarted,
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

/// The api's restore destination as the WRITER's own enum. Two enums, one set
/// of words: `ksx-api` names what a caller may ask for, `mapping::RestoreKind`
/// names what the writer does, and this is the one place they meet — so a
/// destination that a typed caller can express and this daemon cannot is a
/// compile error rather than a refusal in the field.
fn restore_kind(mode: ksx_api::RestoreMode) -> crate::mapping::RestoreKind {
    match mode {
        ksx_api::RestoreMode::Defaults => crate::mapping::RestoreKind::Defaults,
        ksx_api::RestoreMode::SessionBackup => crate::mapping::RestoreKind::SessionBackup,
        ksx_api::RestoreMode::LatestBackup => crate::mapping::RestoreKind::LatestBackup,
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
        .and_then(ksx_api::RestoreMode::parse)
        .map(restore_kind)
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
// Client — moved to `ksx-api` (docs/M9-DECISION.md §6).
//
// The transport was never the daemon's: `ksx session`, Studio and any future
// shell all dial the same pipe, and the crate that owns the request types owns
// the line they travel on. What stays here is the NAME, so every existing
// `pipe::client::request(pipe::PIPE_NAME, …)` call site still reads the way it
// always did — and so a caller that wants the TYPED client asks for
// `ksx_api::Client::new(ksx_api::PipeTransport::new())` instead of hand-rolling
// a second one.
// ---------------------------------------------------------------------------

pub mod client {
    pub use ksx_api::pipe::{request_json as request, TransportError as ClientError};
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

    /// THE REGRESSION, pinned where it happened: a `map-macro` request that
    /// SAYS `repeat`/`turbo_hz` produces a spec that HAS them.
    ///
    /// What this used to test was an ALLOWLIST of body fields kept in this
    /// file. `repeat` was missing from it, so a macro card that set
    /// `while-held` saved `once` — with a "saved" toast, because a dropped
    /// field looks exactly like a field the user never set. The list is gone:
    /// the body half of the request IS `ksx_config::MacroFile`
    /// (`ksx_api::MapMacroRequest`), so a field added to the table is on the
    /// wire and in this spec the moment it compiles, and the only list left is
    /// the ENVELOPE's — a closed set, whose failure mode is a loud refusal
    /// rather than a silent drop.
    #[test]
    fn a_macro_request_carries_every_field_of_the_table_into_the_spec() {
        let request = serde_json::json!({
            "verb": "map-macro",
            "preset": "IPAC P1",
            "name": "hadouken",
            "steps": [{"hold": ["A"], "ms": 50, "allow_short": true},
                      {"hold": ["dpad.down"], "frames": 2}],
            "on_release": "abort",
            "retrigger": "restart",
            "interrupt": "opposing",
            "repeat": "turbo",
            "turbo_hz": 10,
            "enabled": false,
            "reload": true,
        });
        let parsed = ksx_api::MapMacroRequest::from_json(&request).expect("a whole-table write");
        let body = parsed.body();
        assert_eq!(body.repeat, ksx_core::Repeat::Turbo);
        assert_eq!(body.turbo_hz, Some(10));
        assert_eq!(body.gap_ms, None, "the other unit is not invented");
        assert_eq!(body.on_release, ksx_core::OnRelease::Abort);
        assert_eq!(body.retrigger, ksx_core::Retrigger::Restart);
        assert_eq!(body.interrupt, ksx_core::Interrupt::Opposing);
        assert!(!body.enabled, "a write may land disabled");
        assert!(body.steps[0].allow_short);
        // A duration authored in frames survives the wire as frames.
        assert_eq!(body.steps[1].frames, Some(2));
        assert_eq!(body.steps[1].ms, None);
        // ...and a body write is not a toggle, whatever `enabled` says.
        assert_eq!(parsed.set_enabled(), None);
        assert!(!parsed.is_delete());
        assert!(parsed.reload);
    }

    /// Every field `MacroFile` will EVER serialize reaches the spec, because
    /// nothing in this daemon enumerates them. Pinned against the type's own
    /// serde shape, with every field set to a non-default so nothing is
    /// skipped on write.
    #[test]
    fn no_field_of_a_macro_table_can_be_dropped_on_the_way_in() {
        let full: ksx_config::MacroFile = toml::from_str(
            r#"
on_release = "abort"
retrigger = "restart"
interrupt = "opposing"
repeat = "turbo"
turbo_hz = 10
enabled = false
steps = [{ hold = ["A"], ms = 50, allow_short = true }]
"#,
        )
        .unwrap();
        let mut request = serde_json::to_value(&full).expect("a macro table is an object");
        request["verb"] = serde_json::json!("map-macro");
        request["preset"] = serde_json::json!("IPAC P1");
        request["name"] = serde_json::json!("hadouken");
        let parsed = ksx_api::MapMacroRequest::from_json(&request).expect("a whole-table write");
        assert_eq!(
            parsed.body(),
            full,
            "a field of the macro table did not survive the request reader"
        );
    }

    /// The toggle and the delete are still told apart by what is ABSENT, and
    /// still refuse rather than guess.
    #[test]
    fn a_macro_request_without_steps_is_a_toggle_a_delete_or_a_refusal() {
        let toggle = ksx_api::MapMacroRequest::from_json(&serde_json::json!({
            "verb": "map-macro", "preset": "P", "name": "m", "enabled": false
        }))
        .expect("a toggle");
        assert_eq!(toggle.set_enabled(), Some(false));

        let deleted = ksx_api::MapMacroRequest::from_json(&serde_json::json!({
            "verb": "map-macro", "preset": "P", "name": "m", "delete": true
        }))
        .expect("a delete");
        assert!(deleted.is_delete());

        let refused = ksx_api::MapMacroRequest::from_json(&serde_json::json!({
            "verb": "map-macro", "preset": "P", "name": "m"
        }))
        .unwrap_err();
        assert!(refused.message.contains("map-macro needs"), "{refused}");
    }

    /// Every destination a typed caller can ask for is one this daemon writes.
    #[test]
    fn every_api_restore_destination_maps_onto_a_writer_destination() {
        for mode in ksx_api::RestoreMode::ALL {
            assert_eq!(restore_kind(mode).as_str(), mode.as_str());
        }
    }

    // -- the drift pin --------------------------------------------------------

    /// Every field the daemon SAYS, `ksx-api` reads. Recursive, and a missing
    /// key is the failure: a client that cannot see a field is a client that
    /// silently loses it, which is the exact shape of the `repeat` bug in the
    /// other direction.
    ///
    /// A `null` the daemon emits and the type omits is not information lost —
    /// absent and null are the same fact here — so that one case passes.
    fn assert_nothing_dropped(
        verb: &str,
        path: &str,
        said: &serde_json::Value,
        read_back: &serde_json::Value,
    ) {
        match said {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    match read_back.get(key) {
                        Some(mirror) => {
                            assert_nothing_dropped(verb, &format!("{path}/{key}"), value, mirror);
                        }
                        None if value.is_null() => {}
                        None => panic!(
                            "the daemon answers `{verb}` with `{path}/{key}` and ksx-api's \
                             response type does not model it — every client reading that answer \
                             loses the field silently. Add it to the response type in \
                             ksx-api/src/wire.rs."
                        ),
                    }
                }
            }
            serde_json::Value::Array(items) => {
                let mirror = read_back.as_array().unwrap_or_else(|| {
                    panic!("`{verb}` answers `{path}` with an array; the type reads {read_back}")
                });
                assert_eq!(items.len(), mirror.len(), "`{verb}` {path}: row count");
                for (i, (said, mirror)) in items.iter().zip(mirror).enumerate() {
                    assert_nothing_dropped(verb, &format!("{path}/{i}"), said, mirror);
                }
            }
            scalar => assert_eq!(scalar, read_back, "`{verb}` {path}"),
        }
    }

    /// **THE DRIFT PIN.** Every verb, both directions, against the REAL
    /// dispatch — no pipe, no daemon, no mocks of the thing under test.
    ///
    /// For each verb: build the TYPED request (`ksx_api::Request`), serialize
    /// it exactly as a client would, hand the line to `handle_request`, then
    /// read the daemon's answer back through the TYPED response and check that
    /// nothing the daemon said was dropped on the way in.
    ///
    /// This is the test the `repeat` regression needed. That bug was a client
    /// and a daemon holding two descriptions of one message, 3,000 lines
    /// apart, and nothing that failed when they disagreed. Now there is one
    /// description — and this asserts the daemon is still answering it.
    #[test]
    fn every_typed_request_is_answered_by_a_response_ksx_api_models_completely() {
        use ksx_api::{Request, Response};

        let dir = std::env::temp_dir().join(format!(
            "ksx-pipe-drift-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = ksx_config::ConfigRoot::at(&dir);
        let store = ksx_config::Store::new(root.clone());
        let file: ksx_config::PresetFile = toml::from_str(
            r#"
name = "IPAC P1"
[bindings]
A = "S"
B = "D"
macro.hadouken = "P"

[macros.hadouken]
steps = [{ hold = ["A"], ms = 50 }]
"#,
        )
        .unwrap();
        store.save_preset(&file).unwrap();

        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, fixed_profiles());
        let (map, save_macro) = preset_writers(root.clone());
        d.map = map;
        d.save_macro = save_macro;
        d.restore = restore_fn(root.clone());
        d.clear_all = clear_all_fn(root.clone());
        d.backups = backups_fn(root.clone());
        d.slot_assign = slot_assign_fn(root.clone());
        // `slot-assign` writes config.toml, so the file has to exist.
        store
            .save_config(&ksx_config::ConfigFile::default())
            .unwrap();
        let _ = root;

        // Every verb, in an order that leaves real data behind for the ones
        // that read it (the restores take timestamped backups, so `map-backups`
        // answers with rows rather than an empty list).
        let requests = vec![
            Request::Status,
            // Both ACTION shapes: one that enqueues and settles into an honest
            // "requested", one the daemon refuses outright.
            Request::Start { profile: None },
            Request::Stop,
            Request::Map(ksx_api::MapRequest {
                preset: "IPAC P1".into(),
                function: "A".into(),
                keys: vec!["S".into(), "Enter".into()],
                ..ksx_api::MapRequest::default()
            }),
            // A chord, so `when` / `flash` are exercised too.
            Request::Map(ksx_api::MapRequest {
                preset: "IPAC P1".into(),
                function: "rt".into(),
                key: Some("D".into()),
                when: vec!["S".into()],
                ..ksx_api::MapRequest::default()
            }),
            Request::MapMacro(ksx_api::MapMacroRequest {
                preset: "IPAC P1".into(),
                name: "hadouken".into(),
                write: ksx_api::MacroWriteKind::Body(Box::new(
                    toml::from_str(
                        r#"
repeat = "turbo"
turbo_hz = 10
steps = [{ hold = ["dpad.down"], ms = 50 }, { hold = ["A"], frames = 2 }]
"#,
                    )
                    .unwrap(),
                )),
                reload: false,
            }),
            // The toggle and the delete are the same verb with a different
            // meaning, and both answer with the same shape.
            Request::MapMacro(ksx_api::MapMacroRequest {
                preset: "IPAC P1".into(),
                name: "hadouken".into(),
                write: ksx_api::MacroWriteKind::Toggle(false),
                reload: false,
            }),
            Request::MapRestore(ksx_api::RestoreRequest {
                preset: "IPAC P1".into(),
                mode: ksx_api::RestoreMode::Defaults,
                reload: false,
            }),
            Request::MapClearAll(ksx_api::ClearAllRequest {
                preset: "IPAC P1".into(),
                reload: false,
            }),
            Request::MapBackups(ksx_api::BackupsRequest {
                preset: "IPAC P1".into(),
            }),
            Request::SlotAssign(ksx_api::SlotAssignRequest {
                slot: 1,
                preset: Some("IPAC P1".into()),
                profile: None,
                persona: None,
                reload: false,
            }),
            Request::LearnKey,
            Request::LearnPoll,
            Request::LearnCancel,
        ];

        for request in requests {
            let verb = request.verb();
            // The line a client actually sends — serialized from the shared
            // type, not hand-written here.
            let line = request.to_line();
            let said = handle_request(&line, &d, FAST);
            assert!(
                said.get("ok").is_some(),
                "`{verb}` answered without an `ok`: {said}"
            );
            let typed = Response::parse(&request, said.clone())
                .unwrap_or_else(|err| panic!("`{verb}` → {said}\n  unreadable: {err}"));
            assert_nothing_dropped(verb, "", &said, &typed.to_json());

            // ...and the answer is the SHAPE this verb promises, not merely a
            // parseable object.
            let right_shape = matches!(
                (&request, &typed),
                (Request::Status, Response::Status(_))
                    | (
                        Request::Start { .. } | Request::Stop | Request::Reload,
                        Response::Action(_)
                    )
                    | (Request::Map(_), Response::Map(_))
                    | (Request::MapMacro(_), Response::Macro(_))
                    | (
                        Request::MapRestore(_) | Request::MapClearAll(_),
                        Response::Restore(_)
                    )
                    | (Request::MapBackups(_), Response::Backups(_))
                    | (Request::SlotAssign(_), Response::SlotAssign(_))
                    | (
                        Request::LearnKey | Request::LearnPoll | Request::LearnCancel,
                        Response::Learn(_)
                    )
            );
            assert!(right_shape, "`{verb}` was read as the wrong response kind");

            // The verbs whose answers a surface RENDERS get their content
            // checked, so "modelled" cannot mean "modelled as all defaults".
            match (&request, &typed) {
                (_, Response::Status(status)) => {
                    assert_eq!(status.run, "stopped");
                    assert_eq!(status.profiles.len(), 2, "games.toml rows");
                    assert!(status.tooltip.is_some());
                }
                (Request::Map(map), Response::Map(answer)) => {
                    assert!(answer.ok, "{said}");
                    assert_eq!(answer.keys, map.key_list());
                    assert_eq!(answer.preset.as_deref(), Some("IPAC P1"));
                    assert!(answer.path.is_some());
                }
                (_, Response::Macro(answer)) => {
                    assert!(answer.ok, "{said}");
                    assert_eq!(answer.name.as_deref(), Some("hadouken"));
                    assert!(answer.backup.is_some(), "every macro write leaves an undo");
                }
                (_, Response::Restore(answer)) => {
                    assert!(answer.ok, "{said}");
                    assert!(answer.mode.is_some() && answer.wrote.is_some());
                }
                (_, Response::Backups(answer)) => {
                    assert!(answer.ok, "{said}");
                    assert!(
                        !answer.backups.is_empty(),
                        "the restores above each left one: {said}"
                    );
                    assert!(!answer.backups[0].label.is_empty());
                }
                (_, Response::SlotAssign(answer)) => {
                    assert!(answer.ok, "{said}");
                    assert_eq!(answer.slot, Some(1));
                    assert_eq!(answer.preset.as_deref(), Some("IPAC P1"));
                    assert!(answer.created, "slot 1 did not exist in this fixture");
                    assert!(answer.path.is_some());
                    // The pad bounce is in the sentence, always — with nothing
                    // running that reads "the next start reads it".
                    let message = answer.message.clone().unwrap_or_default();
                    assert!(message.contains("pads replugged"), "{message}");
                    assert!(!answer.restarted, "nothing was running to restart");
                }
                (_, Response::Learn(answer)) => {
                    assert!(answer.ok, "{said}");
                    assert!(!answer.state.is_empty());
                }
                // Every other pairing was refused by `right_shape` above.
                _ => {}
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// A `map-macro` that refuses everything — protocol tests that never write
    /// a macro body.
    fn no_macro() -> MacroFn {
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

    /// A `slot-assign` that refuses everything — protocol tests that never
    /// re-wire. The refusal is the real one, so a test that DOES exercise the
    /// verb sees the shape a cabinet would.
    fn no_slot_assign() -> SlotAssignFn {
        Box::new(|spec| {
            Err(crate::slots::SlotError::UnknownPreset {
                preset: spec.preset.clone().unwrap_or_default(),
                available: Vec::new(),
            })
        })
    }

    fn deps(tx: Sender<DaemonCommand>, state: SharedState, profiles: ProfilesFn) -> PipeDeps {
        PipeDeps {
            tx,
            state,
            profiles,
            map: no_map(),
            save_macro: no_macro(),
            restore: no_restore(),
            clear_all: Box::new(|preset| {
                Err(crate::mapping::MapError::UnknownPreset {
                    name: preset.to_owned(),
                    known: Vec::new(),
                })
            }),
            backups: no_backups(),
            slot_assign: no_slot_assign(),
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
            key: spec.key.clone().or_else(|| spec.keys.first().cloned()),
            keys: spec
                .key
                .clone()
                .into_iter()
                .chain(spec.keys.iter().cloned())
                .collect(),
            when: spec.when.clone(),
            unless: spec.unless.clone(),
            also_drives: Vec::new(),
            moved_from: None,
            overridden: Vec::new(),
            flash: Vec::new(),
            shared_macros: Vec::new(),
            turbo_hz: spec.turbo_hz.filter(|hz| *hz > 0),
            turbo_effective_hz: spec.turbo_hz.filter(|hz| *hz > 0).map(|hz| {
                ksx_core::TurboBinding::new(ksx_core::Binding::Button(ksx_core::XButton::A), hz)
                    .effective_hz()
            }),
        })
    }

    /// A `map-macro` writer that records what it was handed and answers with a
    /// plausible write.
    fn scripted_macro(seen: Arc<Mutex<Vec<crate::mapping::MacroSpec>>>) -> MacroFn {
        Box::new(move |spec| {
            seen.lock().unwrap().push(spec.clone());
            Ok(crate::mapping::AppliedMacro {
                path: std::path::PathBuf::from(r"C:\cfg\ksx\presets\IPAC P1.toml"),
                preset: spec.preset.clone(),
                name: spec.name.clone(),
                steps: spec.body.steps.len(),
                total_ms: 200,
                deleted: spec.delete,
                enabled: spec.set_enabled.unwrap_or(spec.body.enabled),
                toggled: spec.set_enabled.is_some(),
                triggers: vec!["P".to_owned()],
                backup: Some(crate::mapping::PresetBackup {
                    path: std::path::PathBuf::from(r"C:\cfg\ksx\presets\IPAC P1.toml.bak-x"),
                    stamp: "20260805-143207".to_owned(),
                }),
                warnings: Vec::new(),
            })
        })
    }

    const HADOUKEN_BODY: &str = r#""steps":[{"hold":["dpad.down"],"ms":50},
        {"hold":["dpad.down","dpad.right"],"ms":50},
        {"hold":["dpad.right"],"ms":50},
        {"hold":["A"],"frames":3}]"#;

    /// The body's field names ARE the preset file's, so the wire and the file
    /// cannot drift — `frames` arrives as `frames`, not as milliseconds.
    #[test]
    fn map_macro_hands_the_file_shaped_body_to_the_writer() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = scripted_macro(seen.clone());
        let v = handle_request(
            &format!(
                r#"{{"verb":"map-macro","preset":"IPAC P1","name":"hadouken",{HADOUKEN_BODY},
                   "on_release":"abort","retrigger":"restart","interrupt":"opposing"}}"#
            ),
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["name"], "hadouken");
        assert_eq!(v["steps"], 4);
        assert_eq!(v["backup"]["stamp"], "20260805-143207", "{v}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let spec = &seen[0];
        assert_eq!(spec.preset, "IPAC P1");
        assert!(!spec.delete);
        assert_eq!(spec.body.steps.len(), 4);
        assert_eq!(spec.body.steps[1].hold, ["dpad.down", "dpad.right"]);
        assert_eq!(spec.body.steps[3].frames, Some(3), "frames stay frames");
        assert_eq!(spec.body.steps[3].ms, None);
        assert_eq!(spec.body.on_release, ksx_core::OnRelease::Abort);
        assert_eq!(spec.body.retrigger, ksx_core::Retrigger::Restart);
        assert_eq!(spec.body.interrupt, ksx_core::Interrupt::Opposing);
    }

    #[test]
    fn map_macro_validates_its_fields_before_touching_the_writer() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = scripted_macro(seen.clone());
        for junk in [
            r#"{"verb":"map-macro"}"#.to_owned(),
            r#"{"verb":"map-macro","preset":"IPAC P1"}"#.to_owned(),
            // No "steps" and no "delete": a misspelled field, not a deletion.
            r#"{"verb":"map-macro","preset":"IPAC P1","name":"hadouken"}"#.to_owned(),
            // Bodies the preset file itself would refuse.
            r#"{"verb":"map-macro","preset":"P","name":"m","steps":[{"hold":["A"],"ms":"soon"}]}"#
                .to_owned(),
            r#"{"verb":"map-macro","preset":"P","name":"m","steps":[{"hold":["A"],"ms":50}],"on_release":"maybe"}"#
                .to_owned(),
            r#"{"verb":"map-macro","preset":"P","name":"m","steps":[{"hold":["A"],"ms":50,"nope":1}]}"#
                .to_owned(),
        ] {
            let v = handle_request(&junk, &d, FAST);
            assert_eq!(v["ok"], false, "{junk} → {v}");
        }
        assert!(
            seen.lock().unwrap().is_empty(),
            "no write may have happened"
        );
    }

    /// Deletion is an explicit word, and it reaches the writer as one.
    #[test]
    fn map_macro_delete_needs_no_steps_and_says_so_in_the_answer() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = scripted_macro(seen.clone());
        let v = handle_request(
            r#"{"verb":"map-macro","preset":"IPAC P1","name":"hadouken","delete":true}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["deleted"], true, "{v}");
        assert_eq!(v["triggers"][0], "P", "{v}");
        assert!(seen.lock().unwrap()[0].delete);
    }

    /// `"enabled"` with NO `steps` is the TOGGLE: it reaches the writer as
    /// `set_enabled` and carries no body, so the table on disk keeps
    /// everything. With `steps` it is an ordinary field of the table instead.
    #[test]
    fn map_macro_enabled_is_a_toggle_without_a_body_and_a_field_with_one() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = scripted_macro(seen.clone());

        // No steps: a toggle. `steps` is absent, so the writer is told to move
        // the flag and nothing else.
        let v = handle_request(
            r#"{"verb":"map-macro","preset":"IPAC P1","name":"hadouken","enabled":false}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["toggled"], true, "{v}");
        assert_eq!(v["enabled"], false, "{v}");
        assert_eq!(seen.lock().unwrap()[0].set_enabled, Some(false));
        assert!(
            seen.lock().unwrap()[0].body.steps.is_empty(),
            "a toggle carries no body"
        );

        // With steps: an ordinary whole-table write that lands disabled.
        let v = handle_request(
            r#"{"verb":"map-macro","preset":"IPAC P1","name":"hadouken",
                "steps":[{"hold":["A"],"ms":50}],"enabled":false}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["toggled"], false, "{v}");
        assert_eq!(v["enabled"], false, "{v}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen[1].set_enabled, None);
        assert!(!seen[1].body.enabled, "the field reached the body");
        assert_eq!(seen[1].body.steps.len(), 1);
    }

    /// The absent-steps refusal has to name the toggle now that one exists —
    /// otherwise the only documented way out of it is `delete`.
    #[test]
    fn map_macro_without_steps_names_both_ways_out() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = scripted_macro(seen.clone());
        let v = handle_request(
            r#"{"verb":"map-macro","preset":"IPAC P1","name":"hadouken"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], false, "{v}");
        let text = v["error"].as_str().unwrap_or_default();
        for part in ["steps", "delete", "enabled"] {
            assert!(text.contains(part), "{text}");
        }
        assert!(seen.lock().unwrap().is_empty(), "nothing may be written");
    }

    /// A macro BODY is a binding change: `reload: true` enqueues
    /// `ApplyBindings` (never a blunt Reload), and when the control loop
    /// reports the in-place swap the response says the pads were left alone.
    /// Same wiring, same fields, same guarantee as `map`.
    #[test]
    fn map_macro_with_reload_hot_swaps_a_running_session() {
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
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = scripted_macro(Arc::new(Mutex::new(Vec::new())));
        let v = handle_request(
            &format!(
                r#"{{"verb":"map-macro","preset":"IPAC P1","name":"hadouken",{HADOUKEN_BODY},"reload":true}}"#
            ),
            &d,
            Duration::from_secs(2),
        );
        assert_eq!(
            loop_thread.join().unwrap(),
            DaemonCommand::ApplyBindings,
            "a macro body must take the hot-swap path, not a pad bounce"
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["reloaded"], true, "{v}");
        assert_eq!(v["hot_swap"], true, "{v}");
    }

    /// A refusal names its problems one by one AND carries the stable code.
    #[test]
    fn map_macro_reports_a_refusal_with_its_code_and_problem_list() {
        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.save_macro = Box::new(|spec| {
            Err(crate::mapping::MapError::BadMacro {
                preset: spec.preset.clone(),
                name: spec.name.clone(),
                problems: vec!["step 0 holds 'warp'".to_owned()],
            })
        });
        let v = handle_request(
            r#"{"verb":"map-macro","preset":"P","name":"m","steps":[{"hold":["warp"],"ms":50}]}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["code"], "macro-invalid", "{v}");
        assert_eq!(v["problems"][0], "step 0 holds 'warp'", "{v}");
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
            // "key" and "keys" are two spellings of one field: both together
            // would mean ignoring one, so the verb refuses instead.
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G","keys":["S"]}"#,
            r#"{"verb":"map","preset":"IPAC P1","function":"A","keys":["S"],"clear":true}"#,
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
                    key: "G".into(),
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
        d.map = preset_writers(root).0;

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

    /// MANY KEYS → ONE CONTROL over the wire, through the REAL writer: one
    /// `map` call with `"keys"` writes the whole list, and the response says
    /// what the control now holds. This is Studio's "add another key" — one
    /// atomic write, not a read-modify-write.
    #[test]
    fn map_writes_a_whole_key_list_and_reports_it_back() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-pipe-keylist-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = ksx_config::ConfigRoot::at(&dir);
        let store = ksx_config::Store::new(root.clone());
        let file: ksx_config::PresetFile =
            toml::from_str("name = \"IPAC P1\"\n[bindings]\nA = \"S\"\nB = \"D\"\n").unwrap();
        store.save_preset(&file).unwrap();

        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.map = preset_writers(root).0;

        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"A","keys":["S","Enter","s"]}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        // Order kept, the duplicate `s` gone, and "key" still the FIRST key
        // for every reader that predates the list.
        assert_eq!(v["keys"], serde_json::json!(["S", "Enter"]), "{v}");
        assert_eq!(v["key"], "S", "{v}");
        assert!(
            v["message"].as_str().unwrap().contains("A = S, Enter"),
            "{v}"
        );
        let on_disk = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(on_disk.contains("A = [\"S\", \"Enter\"]"), "{on_disk}");
        assert!(on_disk.contains("B = \"D\""), "{on_disk}");

        // The per-key ✕ sends the remaining list — one write again.
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"A","keys":["Enter"]}"#,
            &d,
            FAST,
        );
        assert_eq!(v["keys"], serde_json::json!(["Enter"]), "{v}");
        let on_disk = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(on_disk.contains("A = \"Enter\""), "{on_disk}");

        // A single-key write still reports a one-entry list, so a caller can
        // read `keys` unconditionally.
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"B","key":"G"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["keys"], serde_json::json!(["G"]), "{v}");
        // …and a clear reports the empty one.
        let v = handle_request(
            r#"{"verb":"map","preset":"IPAC P1","function":"B","clear":true}"#,
            &d,
            FAST,
        );
        assert_eq!(v["keys"], serde_json::json!([]), "{v}");
        assert_eq!(v["key"], serde_json::Value::Null, "{v}");
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
        d.map = preset_writers(root.clone()).0;
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

    /// The reason `map` and `map-macro` are built together: they write the
    /// same files, so "undo everything since the daemon started" has to mean
    /// the snapshot taken before the first write by EITHER of them. A set per
    /// writer would let the macro write re-snapshot a file the bind had
    /// already changed, and the undo would restore a state that never existed.
    #[test]
    fn the_two_preset_writers_share_one_session_snapshot() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-pipe-shared-bak-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = ksx_config::ConfigRoot::at(&dir);
        let store = ksx_config::Store::new(root.clone());
        let file: ksx_config::PresetFile = toml::from_str(
            "name = \"IPAC P1\"\n[bindings]\nA = \"S\"\n\
             [macros.m]\nsteps = [{ hold = [\"A\"], ms = 50 }]\n",
        )
        .unwrap();
        store.save_preset(&file).unwrap();

        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        let (map, save_macro) = preset_writers(root.clone());
        d.map = map;
        d.save_macro = save_macro;
        d.restore = restore_fn(root);

        // A bind first (which takes the snapshot), then a macro body.
        for req in [
            r#"{"verb":"map","preset":"IPAC P1","function":"A","key":"G"}"#,
            r#"{"verb":"map-macro","preset":"IPAC P1","name":"m","steps":[{"hold":["B"],"ms":90}]}"#,
        ] {
            let v = handle_request(req, &d, FAST);
            assert_eq!(v["ok"], true, "{req} → {v}");
        }

        let v = handle_request(
            r#"{"verb":"map-restore","preset":"IPAC P1","mode":"session-backup"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        let restored = std::fs::read_to_string(store.preset_path("IPAC P1").unwrap()).unwrap();
        assert!(
            restored.contains("A = \"S\""),
            "the snapshot is the PRE-first-write state: {restored}"
        );
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
                        &super::super::super::NoUi,
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

    /// **A slot assignment whose restart FAILED must not report as an idle
    /// daemon.** `reloaded` is documented as "`reload` was asked for and the
    /// daemon acted on it"; echoing the REQUEST made it true in a case where
    /// the daemon had torn a session down and could not bring it back, and
    /// `SlotOutcome::headline` then printed "nothing was running, so nothing
    /// had to restart" at somebody whose four pads had just vanished.
    #[test]
    fn a_slot_assign_whose_restart_fails_says_so_and_never_claims_nothing_was_running() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-slot-bounce-{}-{:?}",
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
        store
            .save_config(&ksx_config::ConfigFile::default())
            .unwrap();

        let state = shared(RunState::Running { slots: 4 });
        let (tx, rx) = unbounded();
        let mut d = deps(tx, state.clone(), no_profiles());
        d.slot_assign = slot_assign_fn(root);

        // Stand in for the control loop's Reload: the session comes down and
        // the restart fails on the new wiring.
        let loop_thread = std::thread::spawn({
            let state = state.clone();
            move || {
                let command = rx.recv_timeout(Duration::from_secs(2)).expect("Reload");
                if let Ok(mut s) = state.lock() {
                    s.run = RunState::Starting;
                }
                std::thread::sleep(Duration::from_millis(10));
                if let Ok(mut s) = state.lock() {
                    s.run = RunState::Failed {
                        message: "cannot start: no usable slot".to_owned(),
                    };
                }
                command
            }
        });

        let v = handle_request(
            r#"{"verb":"slot-assign","slot":1,"preset":"IPAC P1","reload":true}"#,
            &d,
            Duration::from_secs(2),
        );
        assert_eq!(loop_thread.join().unwrap(), DaemonCommand::Reload);

        assert_eq!(v["ok"], true, "the FILE was written: {v}");
        assert_eq!(v["restarted"], false, "the restart failed: {v}");
        assert_eq!(
            v["reloaded"], false,
            "the running session was NOT reconciled: {v}"
        );
        let message = v["message"].as_str().unwrap();
        assert!(message.contains("no usable slot"), "{message}");

        // ...and that is what a 10-foot surface prints, verbatim.
        let outcome: ksx_api::SlotOutcome =
            serde_json::from_value::<ksx_api::SlotAssignResponse>(v.clone())
                .expect("a slot-assign response")
                .into();
        let headline = outcome.headline();
        assert!(headline.contains("no usable slot"), "{headline}");
        assert!(
            !headline.contains("nothing was running"),
            "the lie this test exists for: {headline}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The persona crosses the pipe and lands in the file** — the end of the
    /// wire that task #8 opened, exercised through `handle_request` rather than
    /// through the writer, because the parse happens HERE and nowhere else.
    ///
    /// Three things at once, each of which was a way to get this wrong:
    ///
    /// 1. an ALIAS (`ds4`) is accepted and canonicalized. The alias table lives
    ///    in one `Persona::FromStr`; a daemon that only took canonical names
    ///    would make every surface carry a copy of it to be useful;
    /// 2. an unknown name is refused in `UnknownPersona`'s own words, which
    ///    list every valid persona — so the answer to a typo is the menu;
    /// 3. a request with NO persona leaves the slot's persona alone, and says
    ///    so by reporting `previous_persona: null`.
    ///
    /// Breaks against: a `from_json` that parses the persona in ksx-api (which
    /// would put the alias table on the client side of the boundary), a
    /// handler that drops the field, and any writer that treats absent as
    /// `xbox360`.
    #[test]
    fn a_persona_crosses_the_pipe_by_alias_and_an_unknown_one_gets_the_menu() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-slot-persona-{}-{:?}",
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
        store
            .save_config(&ksx_config::ConfigFile::default())
            .unwrap();

        let state = shared(RunState::Stopped);
        let (tx, _rx) = unbounded();
        let mut d = deps(tx, state, no_profiles());
        d.slot_assign = slot_assign_fn(root);

        // 1 — an alias, on a slot this call creates.
        let v = handle_request(
            r#"{"verb":"slot-assign","slot":5,"preset":"IPAC P1","persona":"ds4"}"#,
            &d,
            FAST,
        );
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["persona"], "playstation", "canonical, not the alias: {v}");
        assert_eq!(v["created"], true);
        assert_eq!(
            v["previous_persona"],
            serde_json::Value::Null,
            "a new slot presented itself as nothing before: {v}"
        );
        let text = std::fs::read_to_string(store.root().config_path()).unwrap();
        assert!(text.contains("persona = \"playstation\""), "{text}");

        // 2 — a name nothing knows. The refusal IS the menu.
        let bad = handle_request(
            r#"{"verb":"slot-assign","slot":5,"preset":"IPAC P1","persona":"gamecube"}"#,
            &d,
            FAST,
        );
        assert_eq!(bad["ok"], false, "{bad}");
        assert_eq!(bad["code"], "unknown-persona");
        let error = bad["error"].as_str().unwrap();
        for persona in ksx_core::Persona::ALL {
            assert!(error.contains(persona.as_str()), "{error} omits {persona}");
        }

        // 3 — no persona at all: the slot keeps the PlayStation it just got,
        // and nothing claims a change.
        let kept = handle_request(
            r#"{"verb":"slot-assign","slot":5,"preset":"IPAC P1"}"#,
            &d,
            FAST,
        );
        assert_eq!(kept["ok"], true, "{kept}");
        assert_eq!(kept["persona"], "playstation", "NOT re-personaed: {kept}");
        assert_eq!(kept["previous_persona"], serde_json::Value::Null);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
