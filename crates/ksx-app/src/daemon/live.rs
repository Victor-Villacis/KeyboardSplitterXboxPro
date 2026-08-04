//! The real [`SessionFactory`]: config on disk → one `supervise()` call.
//!
//! Everything driver-shaped lives behind `#[cfg(windows)]`, so the control loop
//! and its tests stay platform-independent.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{SessionFactory, SessionRunner, SessionSummary};

/// Re-reads configuration on every `make()`. That is what "Reload config"
/// means: a clean stop and a clean start from whatever is on disk now, never a
/// hot-patch of a live pipeline.
pub struct LiveFactory {
    pub root: ksx_config::ConfigRoot,
    pub game: Option<String>,
    pub no_launch: bool,
}

impl SessionFactory for LiveFactory {
    fn make(&mut self) -> anyhow::Result<Box<dyn SessionRunner>> {
        let plan = crate::run::plan::resolve(&self.root, self.game.as_deref())
            .map_err(|err| anyhow::anyhow!("{err}"))?;
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
        }))
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
}

struct LiveRunner {
    plan: crate::run::plan::RunPlan,
    launch: Option<ksx_games::LaunchSpec>,
    games_toml: PathBuf,
    slots: usize,
}

impl SessionRunner for LiveRunner {
    fn slots(&self) -> usize {
        self.slots
    }

    #[cfg(windows)]
    fn run(
        &mut self,
        stop: Arc<AtomicBool>,
        out: &mut dyn Write,
    ) -> anyhow::Result<SessionSummary> {
        use crate::run::supervisor::{self, RunOptions, SessionHook, Wiring};
        use ksx_capture::InterceptionBackend;
        use ksx_output::VigemBackend;

        let pads = VigemBackend::connect()?;
        let capture = InterceptionBackend::new()?;

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
            // Two independent stop sources: the daemon's own latch (tray Stop,
            // Quit, Reload) and the console handler, so Ctrl+C in a headless
            // daemon still ends the session cleanly.
            stop: Box::new(move || stop.load(Ordering::SeqCst) || crate::ctrl_c::requested()),
            hook,
            ..RunOptions::default()
        };
        let outcome = supervisor::supervise(
            &self.plan,
            Wiring {
                capture: Box::new(capture),
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
