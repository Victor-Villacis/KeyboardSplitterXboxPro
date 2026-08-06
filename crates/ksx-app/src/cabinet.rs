//! `ksx cabinet` — hosting the 10-foot OPERATE surface (feature `cabinet`).
//!
//! `ksx-cabinet` draws; this module says what it is drawing. Three hosts, one
//! window:
//!
//! | host | control source | when |
//! |---|---|---|
//! | **inside `ksx daemon`** ([`spawn_in_daemon`]) | the daemon's OWN dispatch, called directly — no pipe, no socket, no second process | the tray's "Open cabinet UI" |
//! | **its own process** ([`run`]) | [`ksx_api::PipeTransport`] over `\\.\pipe\ksx-daemon` | `ksx cabinet`, the recovery path |
//! | **`--demo`** ([`run_demo`]) | fixtures that refuse every write in words | design review and screenshots |
//!
//! # The in-daemon source, and the thing it CANNOT do
//!
//! [`daemon_sink`] builds a second [`crate::daemon::pipe::PipeDeps`] and calls
//! [`crate::daemon::pipe::handle_request`] directly. That gives the window
//! exactly the tray's reach — the same `DaemonCommand` on the same channel,
//! the same `DaemonState` snapshot — with no IPC in the middle, which is the
//! property E7 wanted from a native UI (docs/M9-DECISION.md §6, the
//! `InProcess` row).
//!
//! Its preset writers are **refusals**, deliberately, and that is the
//! load-bearing detail: the cabinet OPERATES and Studio AUTHORS, so a cabinet
//! that ever tried to send `map`, `map-macro`, `map-restore`, `map-clear-all`
//! or `learn-key` is refused **by its own dispatch**, in words, naming the
//! surface that can. The rule is not a review checklist item; it is what this
//! table is wired to, and `tests::the_in_daemon_dispatch_refuses_every_
//! authoring_verb_in_words` asserts every entry of it.
//!
//! The one honest cost: `handle_request` takes a request LINE, so a verb is
//! serialized to a JSON string and parsed back inside one process. That is one
//! small allocation per human click, against a rate of a few clicks a minute —
//! the same measurement that decided M9 in the first place.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ksx_api::{Refusal, Request, Response, VerbSink};

/// How long an in-process action verb polls the daemon snapshot for its
/// outcome. The pipe's own number: long enough for pads to plug, short enough
/// that the cabinet's worker is never parked behind a wedged start.
const SETTLE: Duration = Duration::from_secs(5);

/// `ksx cabinet` — the window as its OWN process, over the control pipe.
///
/// The recovery path, and the same shape `ksx studio` has: if the in-daemon
/// window cannot be created (a locked-down desktop, a GPU with no usable
/// context), this one still opens and still drives the daemon.
pub fn run() -> anyhow::Result<()> {
    let cabinet = ksx_cabinet::Cabinet {
        status: Arc::new(crate::sources::CollectorSource),
        control: Arc::new(
            ksx_api::Client::new(ksx_api::PipeTransport::new())
                .with_offline_profile(crate::sources::configured_profile),
        ),
        machine: Arc::new(crate::sources::LocalMachine),
        // No daemon-side sink from out here: the fan-out lives inside the
        // daemon's process, and there is no wire protocol for it yet. Said in
        // words rather than rendered as a panel where nothing ever lights.
        feed: Box::new(ksx_api::NoFeed::new(
            "the button check needs the window that runs INSIDE the daemon — open it from the \
             tray (\"Open cabinet UI\"). This copy can start, stop and re-wire, but it cannot \
             see the live pipeline.",
        )),
    };
    ksx_cabinet::run(cabinet).map_err(|err| anyhow::anyhow!("{err}"))
}

/// `ksx cabinet --demo` — the same window against a scripted machine.
///
/// Performs no verb and touches no config. This is how the 10-foot design is
/// reviewed and screenshotted without starting a session on a real cabinet.
pub fn run_demo() -> anyhow::Result<()> {
    ksx_cabinet::run(ksx_cabinet::demo::cabinet()).map_err(|err| anyhow::anyhow!("{err}"))
}

/// The in-process verb sink: the daemon's own dispatch, called directly.
struct DaemonSink {
    /// One at a time. The dispatch is `&PipeDeps`, but the learn service and
    /// the writers behind it are not `Sync`; the pipe server serializes
    /// connections for the same reason and it costs a cabinet nothing — there
    /// is one window and one human in front of it.
    deps: Mutex<crate::daemon::pipe::PipeDeps>,
}

impl VerbSink for DaemonSink {
    fn call(&self, request: &Request) -> Result<Response, Refusal> {
        let line = request.to_line();
        let answered = {
            let deps = self.deps.lock().map_err(|_| {
                Refusal::new(
                    ksx_api::codes::PIPE_ERROR,
                    "the cabinet's dispatch panicked",
                )
            })?;
            crate::daemon::pipe::handle_request(&line, &deps, SETTLE)
        };
        Response::parse(request, answered)
    }
}

/// The daemon-hosted control source: the tray's reach, no IPC, and **no
/// authoring**.
fn daemon_sink(
    tx: crossbeam_channel::Sender<crate::daemon::DaemonCommand>,
    state: crate::daemon::SharedState,
    root: ksx_config::ConfigRoot,
) -> DaemonSink {
    let profiles_root = root.clone();
    DaemonSink {
        deps: Mutex::new(crate::daemon::pipe::PipeDeps {
            tx,
            state,
            profiles: Box::new(move || crate::daemon::pipe::profile_rows(&profiles_root)),
            // THE CABINET CANNOT AUTHOR. Every preset writer below is a
            // refusal that names the surface which can, so the rule is
            // enforced by this table rather than by whoever writes the next
            // screen. `ksx-cabinet` has no UI for any of them either — this is
            // the second lock on the same door.
            map: cannot_author("edit a binding", "ksx map"),
            save_macro: cannot_author_macro(),
            restore: cannot_author_restore(),
            clear_all: cannot_author_clear(),
            backups: Box::new(|preset| {
                Err(crate::mapping::MapError::UnknownPreset {
                    name: preset.to_owned(),
                    known: Vec::new(),
                })
            }),
            // The one write the cabinet DOES have: which preset a slot uses.
            slot_assign: crate::daemon::pipe::slot_assign_fn(root),
            // Learning a key is AUTHORING — it exists to fill in a binding —
            // so it is refused here like the other four. The first pass wired
            // the real `with_rawinput()` learner with a comment saying learn
            // is authoring, which left the one door in this table that the
            // comment above claims is locked standing open. Nothing in
            // `ksx-cabinet` asks for it today; "no caller today" is not a
            // lock, and this table IS the lock.
            learn: crate::daemon::learn::LearnService::refusing(
                "the cabinet cannot learn a key. That is authoring — it exists to fill in a \
                 binding. Author with ksx Studio, or `ksx map --learn`",
            ),
        }),
    }
}

/// A preset writer that refuses, naming the surface that does it.
fn cannot_author(what: &'static str, command: &'static str) -> crate::daemon::pipe::MapFn {
    Box::new(move |spec| {
        Err(crate::mapping::MapError::UnknownPreset {
            name: format!(
                "{} — the cabinet cannot {what}. It OPERATES: it chooses among things that \
                 already exist. Author with ksx Studio, or `{command}`",
                spec.preset
            ),
            known: Vec::new(),
        })
    })
}

fn cannot_author_macro() -> crate::daemon::pipe::MacroFn {
    Box::new(|spec| {
        Err(crate::mapping::MapError::UnknownPreset {
            name: format!(
                "{} — the cabinet cannot edit macros. Author with ksx Studio, or `ksx macro`",
                spec.preset
            ),
            known: Vec::new(),
        })
    })
}

fn cannot_author_restore() -> crate::daemon::pipe::RestoreFn {
    Box::new(|preset, _kind| {
        Err(crate::mapping::MapError::UnknownPreset {
            name: format!(
                "{preset} — the cabinet cannot restore a preset. That is authoring: \
                 ksx Studio, or `ksx map --restore`"
            ),
            known: Vec::new(),
        })
    })
}

fn cannot_author_clear() -> crate::daemon::pipe::ClearAllFn {
    Box::new(|preset| {
        Err(crate::mapping::MapError::UnknownPreset {
            name: format!(
                "{preset} — the cabinet cannot clear a preset. That is authoring: \
                 ksx Studio, or `ksx map --clear-all`"
            ),
            known: Vec::new(),
        })
    })
}

/// The window host: **one thread for the daemon's whole life**, not one per
/// open.
///
/// That distinction is load-bearing and was got wrong the first time. winit
/// allows exactly one event loop per **process** — `EventLoopBuilder::build`
/// swaps a process-global `EVENT_LOOP_CREATED` atomic that is never reset
/// (winit 0.30 `event_loop.rs`) — and eframe works around it by caching the
/// loop in a **thread-local** and re-entering it with `run_app_on_demand`
/// ("we reuse the event-loop so we can support closing and opening an eframe
/// window multiple times", `eframe::native::run::with_event_loop`).
///
/// A fresh thread per open misses that cache, hits winit's global latch, and
/// `run_native` returns `RecreationAttempt`. So the FIRST "Open cabinet UI" of
/// a daemon's life worked and every one after it failed — silently, because
/// the only report was a `tracing::error!` into a log file and the tray's
/// stdout is the null device once the console is released. A tray item that
/// stops working after the first use, with nothing on screen saying why, is
/// exactly the "a surface that cannot act must SAY so" invariant inverted.
///
/// Hence: one host thread, parked on a channel, re-entering the window per
/// request.
struct Host {
    /// Ping it to show the window. Bounded at one: a request made while the
    /// window is closing is still honoured, and a third click cannot queue a
    /// third window.
    open: crossbeam_channel::Sender<()>,
    /// A window is on screen right now.
    showing: Arc<std::sync::atomic::AtomicBool>,
}

/// Open the cabinet window as a **fourth thread inside `ksx daemon`**.
///
/// Returns immediately; the window pumps its own event loop on the host
/// thread. **Closing it does not quit the daemon** — nobody joins that thread,
/// and `Quit` stays the tray's. A cabinet whose emulation stopped because
/// somebody shut a status panel would be a cabinet nobody trusts.
///
/// A second call while a window is already open is a no-op. A second call
/// AFTER one was closed opens it again, which is the case [`Host`] exists for.
pub fn spawn_in_daemon(
    tx: crossbeam_channel::Sender<crate::daemon::DaemonCommand>,
    state: crate::daemon::SharedState,
    root: ksx_config::ConfigRoot,
    feed: crate::feed::LiveSink,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    static HOST: OnceLock<Host> = OnceLock::new();

    // The four arguments are captured ONCE, by the first call. That is
    // correct rather than lucky: `daemon::run` builds one `Surfaces` for the
    // life of the process and hands the same channel, the same snapshot, the
    // same config root and the same fan-out to every click. A later call's
    // clones are dropped here, which is why they are cheap ones.
    let host = HOST.get_or_init(move || {
        // Bounded(1): the ping is a level, not a queue.
        let (open_tx, open_rx) = crossbeam_channel::bounded::<()>(1);
        let showing = Arc::new(AtomicBool::new(false));
        let on_screen = showing.clone();
        let spawned = std::thread::Builder::new()
            .name("ksx-cabinet".into())
            .spawn(move || {
                while open_rx.recv().is_ok() {
                    on_screen.store(true, Ordering::SeqCst);
                    // A FRESH subscription and a fresh alias table per window:
                    // the previous one went away with the previous `App`, and
                    // the config may have been edited in between.
                    let mut subscription = feed.subscribe();
                    subscription.set_aliases(device_aliases(&root));
                    let cabinet = ksx_cabinet::Cabinet {
                        status: Arc::new(crate::sources::CollectorSource),
                        control: Arc::new(ksx_api::Client::new(daemon_sink(
                            tx.clone(),
                            state.clone(),
                            root.clone(),
                        ))),
                        machine: Arc::new(crate::sources::LocalMachine),
                        feed: Box::new(subscription),
                    };
                    // The window owns this thread until it is closed.
                    // `run_on_any_thread` is what makes that legal: a Win32
                    // message pump belongs to the thread that created its
                    // window, which is the same fact `daemon/tray.rs` is built
                    // on. Returning drops the `App` — and with it the live
                    // subscription and the worker's `Sender`, so a closed
                    // window really does put the pipeline back to paying
                    // nothing (`crate::feed`).
                    if let Err(err) = ksx_cabinet::run_on_any_thread(cabinet) {
                        // The log file is the only surface left at this point:
                        // the tray has no way to show text and this thread has
                        // no console. `console::detach_notice` names the file
                        // on the daemon's last console line, which is the best
                        // this path can honestly do.
                        tracing::error!(%err, "the cabinet window could not be opened");
                    }
                    on_screen.store(false, Ordering::SeqCst);
                }
            });
        if spawned.is_err() {
            // `open_rx` went with the closure, so every later `try_send`
            // reports Disconnected and says so rather than looking queued.
            tracing::error!("could not spawn the cabinet window thread");
        }
        Host {
            open: open_tx,
            showing,
        }
    });

    if host.showing.load(Ordering::SeqCst) {
        tracing::info!("the cabinet window is already open");
        return;
    }
    match host.open.try_send(()) {
        Ok(()) => {}
        // A request is already queued — the window is on its way up.
        Err(crossbeam_channel::TrySendError::Full(())) => {
            tracing::info!("the cabinet window is already opening");
        }
        // The host thread never started, or died. Nothing will show.
        Err(crossbeam_channel::TrySendError::Disconnected(())) => {
            tracing::error!("the cabinet window host is gone; no window can be opened");
        }
    }
}

/// `[[device]]` id → alias, so a key hit says "IPAC P1" and not a 60-character
/// instance path.
///
/// Read HERE, on the window's own thread, because the engine thread that
/// publishes those hits must not read a config file — the fan-out carries the
/// instance path and the consumer names it (`crate::feed`).
fn device_aliases(root: &ksx_config::ConfigRoot) -> Vec<(String, String)> {
    ksx_config::Store::new(root.clone())
        .load_config()
        .map(|loaded| {
            loaded
                .value
                .devices
                .iter()
                .map(|device| (device.id.clone(), device.alias.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use ksx_api::{ControlSource, MacroWrite, RestoreMode};

    /// **The cabinet OPERATES; Studio AUTHORS — and this table is the lock, not
    /// the absence of a screen.**
    ///
    /// `ksx-cabinet` has no UI for any authoring verb, which is the first lock.
    /// This is the second: the in-daemon dispatch the window is handed refuses
    /// every one of them, in words, naming the surface that can. Asserted
    /// rather than reviewed, because the failure mode is somebody adding a
    /// screen later and finding the door already open.
    #[test]
    fn the_in_daemon_dispatch_refuses_every_authoring_verb_in_words() {
        let dir = std::env::temp_dir().join(format!(
            "ksx-cabinet-lock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = ksx_config::ConfigRoot::at(&dir);
        let (tx, _rx) = crossbeam_channel::unbounded();
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::daemon::DaemonState::default()));
        let control = ksx_api::Client::new(super::daemon_sink(tx, state, root));

        let mut refusals: Vec<String> = Vec::new();

        let bound = control.bind_keys("IPAC P1", "A", &["S".to_owned()], false, false, None);
        assert!(!bound.ok, "a binding write must never land here");
        refusals.push(bound.refusal().expect("a refusal").message);

        let saved = control.save_macro(&MacroWrite {
            preset: "IPAC P1".into(),
            name: "hadouken".into(),
            steps: vec![ksx_api::MacroStepView {
                hold: vec!["A".into()],
                ms: Some(50),
                frames: None,
                allow_short: false,
            }],
            on_release: String::new(),
            retrigger: String::new(),
            interrupt: String::new(),
            repeat: String::new(),
            turbo_hz: None,
            gap_ms: None,
            delete: false,
            enabled: None,
            reload: false,
        });
        assert!(!saved.ok, "a macro write must never land here");
        refusals.push(saved.refusal().expect("a refusal").message);

        refusals.push(
            control
                .restore("IPAC P1", RestoreMode::Defaults)
                .expect_err("a restore must never land here")
                .message,
        );
        refusals.push(
            control
                .clear_all("IPAC P1")
                .expect_err("a clear must never land here")
                .message,
        );

        // The learner too: it exists to fill in a binding, which is authoring.
        let learn = control.learn_start();
        assert!(!learn.ok, "the cabinet must not be able to learn a key");
        refusals.push(learn.error.clone().expect("a worded refusal"));

        for refusal in &refusals {
            assert!(
                refusal.contains("Studio") || refusal.contains("ksx "),
                "a refusal owes the surface that CAN do it: {refusal}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
