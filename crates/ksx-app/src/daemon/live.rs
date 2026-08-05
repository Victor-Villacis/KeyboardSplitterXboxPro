//! The real [`SessionFactory`]: config on disk → one `supervise()` call.
//!
//! Everything driver-shaped lives behind `#[cfg(windows)]`, so the control loop
//! and its tests stay platform-independent.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{HealthSlot, SessionFactory, SessionRunner, SessionSummary};

/// Re-reads configuration on every `make()`. That is what "Reload config"
/// means: a clean stop and a clean start from whatever is on disk now, never a
/// hot-patch of a live pipeline.
///
/// The **claim** is the one thing that is not re-made per session: `panel` is
/// the daemon's, made once at startup and handed to every session that follows
/// (see [`super::panel`]). Re-reading the config can therefore change slots,
/// presets and games freely, but not which board is claimed — for that the
/// daemon has to be restarted, and `crate::capture::build_session` says so
/// rather than claiming something new mid-flight.
pub struct LiveFactory {
    pub root: ksx_config::ConfigRoot,
    pub game: Option<String>,
    pub no_launch: bool,
    pub panel: Option<Arc<super::panel::Panel>>,
}

impl LiveFactory {
    /// The [`super::PanelKeyboard`] the control loop must drive, derived from
    /// **this factory's** claim.
    ///
    /// It is a method rather than something `daemon::run` assembles separately
    /// so the two cannot disagree: for the whole of M6 the daemon passed
    /// `panel_for(None)` to the control loop while its sessions each made their
    /// own claim, which type-checked perfectly and meant a claimed panel was
    /// dead between sessions. One source of truth, one claim.
    pub fn panel_keyboard(&self) -> Box<dyn super::PanelKeyboard> {
        super::panel_for(self.panel.clone())
    }
}

impl SessionFactory for LiveFactory {
    fn make(&mut self) -> anyhow::Result<Box<dyn SessionRunner>> {
        let plan = self.resolve_plan()?;
        let launch = match (&self.game, self.no_launch) {
            (Some(title), false) => {
                let games = ksx_config::Store::new(self.root.clone()).load_games()?;
                match games.value.games.iter().find(|g| &g.title == title) {
                    Some(entry) => {
                        let spec = ksx_games::LaunchSpec::from_entry(entry);
                        ksx_games::preflight(&spec)?;
                        Some(spec)
                    }
                    None => None,
                }
            }
            _ => None,
        };
        Ok(Box::new(LiveRunner {
            slots: plan.slots.len(),
            plan,
            launch,
            games_toml: self.root.games_path(),
            panel: self.panel.clone(),
            health: HealthSlot::default(),
            swap: crate::run::supervisor::HotSwapSlot::default(),
        }))
    }

    /// The same resolution [`Self::make`] does, without building a runner —
    /// the input to the hot-swap eligibility check, and therefore necessarily
    /// the SAME call, or the daemon would be comparing a plan it would never
    /// actually run.
    ///
    /// `resolve_as`, not `resolve`, for the reason `daemon::run` uses it: this
    /// is the path a tray "Reload config" takes, so a "nothing to run" refusal
    /// must suggest `ksx daemon --game "…"`. Suggesting `ksx run` from inside a
    /// running daemon hands the user a foreground session.
    fn resolve_plan(&self) -> anyhow::Result<crate::run::plan::RunPlan> {
        crate::run::plan::resolve_as(&self.root, self.game.as_deref(), "ksx daemon")
            .map_err(|err| anyhow::anyhow!("{err}"))
    }

    fn config_dir(&self) -> PathBuf {
        self.root
            .config_path()
            .parent()
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
    }

    fn game(&self) -> Option<String> {
        self.game.clone()
    }

    fn set_game(&mut self, game: Option<String>) {
        self.game = game;
    }
}

struct LiveRunner {
    plan: crate::run::plan::RunPlan,
    launch: Option<ksx_games::LaunchSpec>,
    games_toml: PathBuf,
    slots: usize,
    /// The daemon's claim, borrowed for this session. `None` means there is
    /// nothing claimed and the session builds its own backends, exactly as
    /// `ksx run` does.
    panel: Option<Arc<super::panel::Panel>>,
    /// Handed to the control loop before this runner moves onto its own thread,
    /// and filled in below the moment the capture backend exists.
    health: HealthSlot,
    /// Same handshake for the binding hot-swap: taken by the control loop
    /// before the move, filled in by `supervise` once the engine thread is up.
    swap: crate::run::supervisor::HotSwapSlot,
}

impl SessionRunner for LiveRunner {
    fn slots(&self) -> usize {
        self.slots
    }

    fn health_slot(&self) -> HealthSlot {
        self.health.clone()
    }

    fn hot_swap_slot(&self) -> crate::run::supervisor::HotSwapSlot {
        self.swap.clone()
    }

    #[cfg(windows)]
    fn run(
        &mut self,
        stop: Arc<AtomicBool>,
        out: &mut dyn Write,
    ) -> anyhow::Result<SessionSummary> {
        use crate::run::supervisor::{self, RunOptions, SessionHook, Wiring};
        use ksx_output::VigemBackend;

        let pads = VigemBackend::connect()?;
        // Same per-device backend selection as `ksx run`, from the same place:
        // a daemon that chose backends differently would be a second, untested
        // capture path (see `crate::capture`). The one difference is the claim:
        // this session *borrows* the daemon's, and returns it untouched when it
        // ends, so the panel is a keyboard again the moment the game is gone.
        let capture = crate::capture::build_session(&self.plan, self.panel.as_ref())?;

        // Publish this session's health before `supervise` takes the backend,
        // so the tray can report a mid-session REBOOT REQUIRED or watchdog trip
        // while it is happening. A *view*, not the handle: the daemon's WinUSB
        // claim keeps one handle alive across every session, and only a
        // baseline taken here means "this session" (see `ksx_capture::
        // HealthView`). Nothing is added to the capture thread by this — the
        // view reads the same lock-free atomics it always published into.
        self.health
            .publish(ksx_capture::HealthView::new(capture.health()));

        let hook: Box<dyn SessionHook> = match self.launch.clone() {
            Some(spec) => Box::new(crate::run::game::GameHook::new(
                spec,
                ksx_games::RealHost::new(),
                self.games_toml.clone(),
            )),
            None => Box::new(supervisor::NoHook),
        };
        let mut options = RunOptions {
            beep: true,
            // The daemon's own latch (tray Stop, Quit, Reload) is the only stop
            // source. `ctrl_c::requested()` deliberately is NOT consulted: it is
            // a process-lifetime latch ("stays true once tripped" — see
            // ctrl_c.rs) and a daemon runs many sessions, so honouring it would
            // make the first Ctrl+C end every future session ~50 ms after start,
            // forever. `daemon::run` never installs the handler either, so
            // Ctrl+C takes the default action and ends the process.
            stop: Box::new(move || stop.load(Ordering::SeqCst)),
            hook,
            ..RunOptions::default()
        };
        let outcome = supervisor::supervise(
            &self.plan,
            Wiring {
                capture,
                pads: Box::new(pads),
            },
            &mut options,
            out,
        )?;
        Ok(SessionSummary {
            stop_code: outcome.stop.code().to_owned(),
            message: outcome.stop.message(),
            exit_code: outcome.exit_code(),
            slots: outcome.pads.len(),
            reboot_required: outcome.health.reboot_required,
            watchdog_tripped: outcome.health.watchdog_tripped,
            dropped_events: outcome.health.dropped_events,
        })
    }

    #[cfg(not(windows))]
    fn run(
        &mut self,
        _stop: Arc<AtomicBool>,
        _out: &mut dyn Write,
    ) -> anyhow::Result<SessionSummary> {
        anyhow::bail!("`ksx daemon` drives Windows kernel drivers and is Windows-only")
    }
}
