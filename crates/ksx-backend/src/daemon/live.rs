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
    /// The daemon-lifetime live fan-out (`crate::feed`). Handed to every
    /// session so a surface can watch the pipeline; costs a session nothing
    /// while nobody is subscribed, which is every session on a cabinet with no
    /// window open.
    pub feed: crate::feed::LiveSink,
    /// **Play without saving** (`docs/FIRST-RUN.md` §2): when this is set,
    /// every session is built from the STAGED setup instead of from the config
    /// on disk, and no file is read or written to do it.
    ///
    /// `Option` rather than a second factory because the two must be the same
    /// session path: same claim, same supervisor, same panel handling, same
    /// reap. Only the plan's origin differs, which is exactly one branch in
    /// [`SessionFactory::resolve_plan`].
    pub staged: Option<ksx_core::CommitSpec>,
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
            feed: self.feed.clone(),
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
    ///
    /// **This is also where `[[device]]` selectors become concrete devnodes**
    /// — inside `resolve_as`, so start and reload get the same values. Hot-swap
    /// eligibility (`SessionShape::bounce_reason`) compares the `DeviceId`s in
    /// the plans these two calls return; resolve anywhere downstream of that
    /// comparison and every preset edit reports "slot N's input device changed"
    /// and bounces a live session mid-game (`docs/DEVICE-IDENTITY.md` §8).
    fn resolve_plan(&self) -> anyhow::Result<crate::run::plan::RunPlan> {
        // The staged setup wins while one is set, and it is the ONLY branch
        // that differs: `crate::stage::resolve` runs the same
        // `plan::resolve_devices` pass `resolve_as` runs, so "which board is
        // this" is answered once for both.
        if let Some(spec) = &self.staged {
            return crate::stage::resolve(spec).map_err(|err| anyhow::anyhow!("{err}"));
        }
        crate::run::plan::resolve_as(&self.root, self.game.as_deref(), "ksx daemon")
            .map_err(|err| anyhow::anyhow!("{err}"))
    }

    /// This factory CAN run a staged setup, so it takes the override and says
    /// so.
    fn set_staged(&mut self, spec: Option<ksx_core::CommitSpec>) -> bool {
        self.staged = spec;
        true
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
    /// The daemon-lifetime live fan-out this session publishes into. The sink
    /// outlives the session on purpose: a surface watching it does not lose
    /// its subscription when a game exits, it sees `running` go false.
    feed: crate::feed::LiveSink,
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
        use crate::run::supervisor::{self, SessionHook, Wiring};
        use ksx_output::{RoutedBackend, VigemBackend};

        // Same persona → backend routing as `ksx run`, from the same
        // constructor: a daemon that selected stacks differently would be a
        // second, untested output path.
        let pads = RoutedBackend::standard(Box::new(VigemBackend::connect()?));
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
        let mut options = Self::run_options(&self.swap, &self.feed, stop, hook);
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

impl LiveRunner {
    /// Everything `supervise` needs that is neither the plan nor the wiring.
    ///
    /// A free-standing constructor rather than a struct literal inside `run`,
    /// and `swap` a **required parameter** rather than a field read, because
    /// that field is what went wrong: `run` built its options with
    /// `..RunOptions::default()` and never named `hot_swap`, so the default —
    /// documented as "a slot nobody reads", which is right for `ksx run` and
    /// wrong for a daemon — quietly won. The session then published its engine
    /// handle into a slot the control loop was not holding, and every binding
    /// edit that should have swapped tables under a live engine fell back to
    /// the three-second stall and full pad bounce instead.
    ///
    /// Nothing failed. No test noticed, because the tests hand a slot to the
    /// supervisor directly and never go through this path. Passing it as an
    /// argument is what makes forgetting it a compile error next time.
    #[cfg(windows)]
    fn run_options(
        swap: &crate::run::supervisor::HotSwapSlot,
        feed: &crate::feed::LiveSink,
        stop: Arc<AtomicBool>,
        hook: Box<dyn crate::run::supervisor::SessionHook>,
    ) -> crate::run::supervisor::RunOptions {
        use crate::run::supervisor::RunOptions;
        RunOptions {
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
            // The live fan-out. Free while nobody is subscribed, which is
            // every session on a cabinet with no window open (`crate::feed`).
            feed: feed.clone(),
            // THE ONE THAT WAS MISSING. `hot_swap_slot()` hands this same slot
            // to the control loop before the runner moves onto its own thread;
            // `supervise` publishes the engine's handle into whatever is here.
            // They have to be the same slot or the handshake has two ends and
            // no middle.
            hot_swap: swap.clone(),
            ..RunOptions::default()
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::run::supervisor::{HotSwapSlot, NoHook};

    /// The handshake has two ends; this asserts they are the same slot.
    ///
    /// The bug this pins was invisible to every existing test, because the
    /// tests hand a `HotSwapSlot` to `supervise` directly and never build a
    /// daemon session's options. So the assertion has to be about *identity* —
    /// `Arc::ptr_eq`, not "a slot exists" — since a defaulted slot is a
    /// perfectly good `HotSwapSlot` that simply nobody is listening to.
    #[test]
    fn the_daemons_session_publishes_into_the_slot_the_control_loop_holds() {
        let swap = HotSwapSlot::default();
        let options = LiveRunner::run_options(
            &swap,
            &crate::feed::LiveSink::default(),
            Arc::new(AtomicBool::new(false)),
            Box::new(NoHook),
        );
        assert!(
            options.hot_swap.is_same_slot(&swap),
            "the session must publish its engine handle into the control loop's own slot; \
             a fresh one type-checks and silently disables binding hot-swap"
        );
    }

    /// Guard the shape, not just this instance: `RunOptions` is built with
    /// `..RunOptions::default()`, so any field that is both defaultable and
    /// load-bearing can be dropped by omission and nothing will complain. The
    /// default slot is deliberately inert, which is correct for `ksx run` and
    /// exactly what made the daemon's version look fine.
    #[test]
    fn a_defaulted_slot_is_not_the_control_loops_slot() {
        let swap = HotSwapSlot::default();
        assert!(
            !HotSwapSlot::default().is_same_slot(&swap),
            "two default slots must not compare equal, or the test above proves nothing"
        );
    }
}
