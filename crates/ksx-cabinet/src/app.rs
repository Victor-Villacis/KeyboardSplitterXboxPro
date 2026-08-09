//! The window: state, the worker thread, and the chrome around the screens.
//!
//! # The threading contract, inherited verbatim
//!
//! `daemon/tray.rs` states the property this window has to keep: *it owns no
//! channel to the capture thread, the engine thread or the output thread; its
//! only outbound edge is a `Sender`; it reads state through one mutex it holds
//! for the length of a clone.* This window is the same shape, one layer out:
//!
//! ```text
//! [ui thread]      egui. Draws. Reads Arc<Mutex<Slow>> for a clone, drains a
//!                  lossy LiveFeed, and sends Asks. NEVER performs a verb.
//!        │ Sender<Ask>              ▲ Arc<Mutex<Slow>>
//!        ▼                          │
//! [cabinet worker] owns the ksx-api sources. Every verb happens here, where
//!                  blocking is free — a `start` settles for up to five
//!                  seconds, and a UI thread that waited for it would be a
//!                  frozen window on a cabinet.
//! ```
//!
//! That split is not stylistic. The pipe's action verbs poll for their outcome
//! (docs/CONTROL-SURFACE.md: "Action verbs poll the snapshot up to 5 s"), and
//! `StatusSource::snapshot` re-runs the driver collectors. Both belong off the
//! paint thread, and neither of them can reach a pipeline thread from there
//! either — `ksx-api`'s whole contract is that it has exactly the tray's reach.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ksx_api::{
    ControlSource, LiveFeed, LiveFrame, MachineSource, MapperSnapshot, SessionView,
    SlotAssignRequest, StatusSnapshot, StatusSource,
};

use crate::nav::{Action, Focus, Nav, Screen};
use crate::{screens, theme};

/// How often the worker re-reads the machine when nothing is being asked of
/// it. Two seconds: fast enough that a pad plugged in the next room shows up
/// before anybody wonders, slow enough that the driver collectors are free.
const REFRESH: Duration = Duration::from_millis(2000);

/// How long a flash line stays up. Long enough to read at six feet standing.
const FLASH_FOR: Duration = Duration::from_secs(9);

/// How long a control stays lit after it was last seen down. Two stages: FULL
/// is "this is happening", FADE is "this happened just now" — which is what
/// makes a button check readable when somebody is tapping rather than holding.
const LIT_FULL: Duration = Duration::from_millis(220);
const LIT_FADE: Duration = Duration::from_millis(1600);

/// Panel keys kept on screen. Enough to see a rhythm, few enough that every
/// one of them is big.
const KEY_LOG: usize = 6;

/// Everything the cabinet is handed. All four are `ksx-api` traits: this crate
/// has no pipe client, no config store and no `DaemonCommand` of its own, and
/// that is what makes "the cabinet OPERATES" a structural fact rather than a
/// promise.
pub struct Cabinet {
    /// Read side — satisfiable with no daemon running.
    pub status: Arc<dyn StatusSource>,
    /// Write side — exactly the tray's reach.
    pub control: Arc<dyn ControlSource>,
    /// The machine verbs. Only [`MachineSource::presets`] is used here; the
    /// rest refuse in words, which is exactly what the slot picker prints when
    /// a host does not supply one.
    pub machine: Arc<dyn MachineSource>,
    /// The lossy live fan-out (docs/CONTROL-SURFACE.md "a lossy fan-out
    /// sink"). Polled on the UI thread because polling it cannot block.
    pub feed: Box<dyn LiveFeed>,
}

/// What the UI asks the worker to do. One variant per backend verb, and there
/// is no variant that is not one — the standing rule, in type form.
#[derive(Clone, Debug)]
pub enum Ask {
    /// Re-read everything now (after an action, or on demand).
    Refresh,
    Start(Option<String>),
    Stop,
    Reload,
    Assign {
        slot: u8,
        preset: String,
        profile: Option<String>,
    },
    /// Get Studio on screen — start it if nothing is listening, then open a
    /// browser. The flash carries the URL either way, because on a cabinet the
    /// useful answer is often "type this on your phone" rather than a window.
    OpenStudio,
}

/// A one-line answer, with a tone.
#[derive(Clone, Debug)]
pub struct Flash {
    pub text: String,
    pub tone: Tone,
    pub at: Instant,
    /// The command that works anyway, when the refusal named one.
    pub remedy: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
}

/// Everything the worker publishes. Cloned wholesale by the UI thread, which
/// holds the lock for exactly that long.
#[derive(Clone, Default)]
pub struct Slow {
    pub session: SessionView,
    pub snapshot: StatusSnapshot,
    pub mapper: MapperSnapshot,
    /// Preset names on disk, for the slot picker.
    pub presets: Vec<String>,
    /// Why there are none, when there are none — never an empty list that
    /// looks like a cabinet with no presets.
    pub presets_error: Option<String>,
    pub flash: Option<Flash>,
    /// A verb is in flight. The UI renders the control as busy rather than
    /// letting a second press queue a second start.
    pub busy: bool,
}

/// The worker. Owns the sources; blocks freely; never draws.
fn worker(
    status: Arc<dyn StatusSource>,
    control: Arc<dyn ControlSource>,
    machine: Arc<dyn MachineSource>,
    slow: Arc<Mutex<Slow>>,
    asks: Receiver<Ask>,
    repaint: impl Fn() + Send + 'static,
) {
    let refresh = |slow: &Arc<Mutex<Slow>>| {
        let session = control.session();
        let snapshot = status.snapshot();
        let mapper = status.mapper();
        let (presets, presets_error) = match machine.presets() {
            Ok(view) => (view.presets.into_iter().map(|row| row.name).collect(), None),
            Err(refusal) => (Vec::new(), Some(refusal.message)),
        };
        if let Ok(mut slow) = slow.lock() {
            slow.session = session;
            slow.snapshot = snapshot;
            slow.mapper = mapper;
            slow.presets = presets;
            slow.presets_error = presets_error;
        }
    };

    refresh(&slow);
    repaint();
    loop {
        let ask = match asks.recv_timeout(REFRESH) {
            Ok(ask) => Some(ask),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            // The window is gone. Nothing to serve.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if let Some(ask) = ask {
            if let Ok(mut slow) = slow.lock() {
                slow.busy = true;
            }
            repaint();
            let flash = perform(&control, &machine, &ask);
            if let Ok(mut slow) = slow.lock() {
                slow.busy = false;
                if let Some(flash) = flash {
                    slow.flash = Some(flash);
                }
            }
        }
        refresh(&slow);
        repaint();
    }
}

/// One verb. Every arm is one `ksx-api` call and nothing else.
fn perform(
    control: &Arc<dyn ControlSource>,
    machine: &Arc<dyn MachineSource>,
    ask: &Ask,
) -> Option<Flash> {
    let now = Instant::now();
    let said = |text: String, tone: Tone, remedy: Option<String>| {
        Some(Flash {
            text,
            tone,
            at: now,
            remedy,
        })
    };
    match ask {
        Ask::Refresh => None,
        Ask::Start(profile) => match control.start(profile.as_deref()) {
            Ok(message) => said(message, Tone::Ok, None),
            Err(refusal) => said(refusal.message, Tone::Bad, refusal.remedy),
        },
        Ask::Stop => match control.stop() {
            Ok(message) => said(message, Tone::Ok, None),
            Err(refusal) => said(refusal.message, Tone::Bad, refusal.remedy),
        },
        Ask::Reload => match control.reload() {
            Ok(message) => said(message, Tone::Ok, None),
            Err(refusal) => said(refusal.message, Tone::Bad, refusal.remedy),
        },
        Ask::Assign {
            slot,
            preset,
            profile,
        } => {
            let outcome = control.assign_slot(&SlotAssignRequest {
                slot: *slot,
                preset: Some(preset.clone()),
                profile: profile.clone(),
                // **Absent, and it stays absent.** docs/SURFACES.md §10 puts
                // the persona MENU on Studio and leaves the egui a view: this
                // screen picks a preset, and a `None` here is the wire's way
                // of saying "I was not asked about the persona", which leaves
                // whatever the slot already presents itself as untouched. An
                // egui that filled this in would re-persona a slot from the
                // one surface that cannot show the five options or their
                // consequences.
                persona: None,
                // The bounce, asked for explicitly, because the modal that got
                // us here said the pads would replug.
                reload: true,
            });
            let tone = if outcome.ok { Tone::Warn } else { Tone::Bad };
            let remedy = outcome.refusal().and_then(|r| r.remedy);
            said(outcome.headline(), tone, remedy)
        }
        // The URL is the payload, not a decoration: this screen has no pointer
        // and the phone in the player's hand is a better Studio client than the
        // cabinet ever will be. So it is shown whether or not a browser opened,
        // and it goes in `remedy` — the field the footer renders in monospace
        // for exactly this "here is the thing to type" case.
        Ask::OpenStudio => match machine.open_studio() {
            Ok(url) => said(format!("opening Studio — {url}"), Tone::Ok, Some(url)),
            Err(refusal) => said(refusal.message, Tone::Bad, refusal.remedy),
        },
    }
}

/// How long a control has been lit, and how brightly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lit {
    /// Down right now, or within [`LIT_FULL`] of it.
    Full,
    /// Seen within [`LIT_FADE`] — "that just happened".
    Fading,
}

/// The whole window.
pub struct App {
    pub focus: Focus,
    slow: Arc<Mutex<Slow>>,
    asks: Sender<Ask>,
    feed: Box<dyn LiveFeed>,
    #[cfg(windows)]
    pads: crate::pad::PadNav,
    /// The newest live frame, kept because a frame covers ~16 ms and a person
    /// looking at a panel needs longer than that.
    pub frame: LiveFrame,
    /// Panel keys, newest FIRST, capped at [`KEY_LOG`].
    pub keys: Vec<(ksx_api::KeyHit, Instant)>,
    /// `(slot, control) -> last seen down`. The decay that makes a tap
    /// visible.
    lit: Vec<(u8, String, Instant)>,
    /// Total events the sink dropped while this window was open. Reported, not
    /// hidden.
    pub dropped: u64,
    /// Keys the sink left out because they came from a device bound to no slot
    /// — a desk keyboard beside the cabinet. Reported for the same reason
    /// [`Self::dropped`] is: an absence nobody can account for reads as a
    /// broken panel.
    pub off_panel: u64,
    /// The slot whose preset is being picked, if any.
    pub picking: Option<u8>,
    /// What a confirmed modal will do.
    pending: Option<Ask>,
    started: Instant,
    /// Window-lifecycle logging state (`crate::lib`'s logging contract).
    ///
    /// Three edges are *transitions*, not states, so each needs its previous
    /// value: a line per frame would drown the log a cabinet keeps for
    /// fourteen days, and a line per change is exactly the ghost-hunting
    /// record that was missing.
    painted: bool,
    was_focused: bool,
    viewports_seen: usize,
    /// Focus regained, counted over [`FOCUS_WINDOW`]. See
    /// [`App::log_lifecycle`]: a window that keeps losing and regaining focus
    /// is being walked over by something, and saying so is the difference
    /// between "the cabinet UI flickers" and a named cause.
    focus_regains: u32,
    focus_since: Instant,
    focus_warned: bool,
}

/// How long the focus-theft detector counts over, and how many regains inside
/// that window are too many.
///
/// Six in a minute is far past anything a person does — alt-tabbing to look
/// something up is one or two — and comfortably under the *twenty-five* the
/// observed ghost produced (a console window conjured every ~2.35 s, holding
/// focus for ~200 ms each time).
const FOCUS_WINDOW: Duration = Duration::from_secs(60);
const FOCUS_THEFT: u32 = 6;

impl App {
    pub fn new(ctx: &egui::Context, cabinet: Cabinet) -> Self {
        theme::install(ctx);
        let slow = Arc::new(Mutex::new(Slow::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let slow = slow.clone();
            let ctx = ctx.clone();
            let Cabinet {
                status,
                control,
                machine,
                ..
            } = &cabinet;
            let (status, control, machine) = (status.clone(), control.clone(), machine.clone());
            // A named thread, like every other thread in this project, so a
            // stack trace says which one it was.
            let spawned = std::thread::Builder::new()
                .name("ksx-cabinet-worker".into())
                .spawn(move || {
                    worker(status, control, machine, slow, rx, move || {
                        ctx.request_repaint();
                    });
                });
            // An error path that used to be `let _ =`. Without the worker the
            // window paints an empty machine for ever and every press is a
            // no-op — a failure that looks exactly like a daemon that has
            // stopped answering, and said nothing anywhere.
            match &spawned {
                Ok(_) => tracing::debug!("cabinet window: worker thread started"),
                Err(err) => tracing::error!(
                    %err,
                    "cabinet window: the worker thread could not start — this window will \
                     show an empty machine and perform no verb"
                ),
            }
        }
        Self {
            focus: Focus::default(),
            slow,
            asks: tx,
            feed: cabinet.feed,
            #[cfg(windows)]
            pads: crate::pad::PadNav::new(),
            frame: LiveFrame::idle(),
            keys: Vec::new(),
            lit: Vec::new(),
            dropped: 0,
            off_panel: 0,
            picking: None,
            pending: None,
            started: Instant::now(),
            painted: false,
            was_focused: false,
            viewports_seen: 0,
            focus_regains: 0,
            focus_since: Instant::now(),
            focus_warned: false,
        }
    }

    /// The published state, cloned. The lock is held for exactly this long —
    /// the tray's rule, and for the same reason.
    pub fn slow(&self) -> Slow {
        self.slow
            .lock()
            .map(|slow| slow.clone())
            .unwrap_or_default()
    }

    pub fn ask(&self, ask: Ask) {
        let _ = self.asks.send(ask);
    }

    /// Why the live feed has nothing, when it has nothing.
    pub fn feed_unavailable(&self) -> Option<String> {
        self.feed.unavailable()
    }

    /// How lit a control is, if at all.
    pub fn lit(&self, slot: u8, control: &str, now: Instant) -> Option<Lit> {
        let seen = self
            .lit
            .iter()
            .find(|(s, c, _)| *s == slot && c == control)
            .map(|(_, _, at)| *at)?;
        let age = now.saturating_duration_since(seen);
        if age <= LIT_FULL {
            Some(Lit::Full)
        } else if age <= LIT_FADE {
            Some(Lit::Fading)
        } else {
            None
        }
    }

    /// Every control of `slot` that is lit right now, sorted, with how.
    ///
    /// Read from the window's own decay map rather than from the newest frame:
    /// a frame covers ~16 ms, and a button check that only showed what happened
    /// in the last frame would flash each press for one paint and then go
    /// blank — the exact opposite of what somebody re-wiring a panel needs.
    pub fn lit_controls(&self, slot: u8, now: Instant) -> Vec<(String, Lit)> {
        let mut lit: Vec<(String, Lit)> = self
            .lit
            .iter()
            .filter(|(s, _, _)| *s == slot)
            .filter_map(|(_, control, _)| {
                self.lit(slot, control, now)
                    .map(|how| (control.clone(), how))
            })
            .collect();
        lit.sort_by(|a, b| a.0.cmp(&b.0));
        lit
    }

    /// Wipe the key log and the lights — the button check's one action.
    pub fn clear_log(&mut self) {
        self.keys.clear();
        self.lit.clear();
        self.dropped = 0;
        self.off_panel = 0;
    }

    /// Drain the live feed into the window's short memory.
    fn take_frame(&mut self, now: Instant) {
        let frame = self.feed.poll();
        self.dropped += frame.dropped;
        self.off_panel += frame.off_panel;
        for hit in &frame.keys {
            // Only presses go in the log. A release is the same key a moment
            // later and would halve how much history fits on screen.
            if hit.down {
                self.keys.insert(0, (hit.clone(), now));
                self.keys.truncate(KEY_LOG);
            }
        }
        for slot in &frame.slots {
            // `hit` first: it contains everything `down` does, plus whatever
            // was tapped and released between two paints.
            for control in slot.hit.iter().chain(slot.down.iter()) {
                match self
                    .lit
                    .iter_mut()
                    .find(|(s, c, _)| *s == slot.slot && c == control)
                {
                    Some((_, _, at)) => *at = now,
                    None => self.lit.push((slot.slot, control.clone(), now)),
                }
            }
        }
        self.lit
            .retain(|(_, _, at)| now.saturating_duration_since(*at) <= LIT_FADE);
        self.frame = frame;
    }

    /// Collect this frame's navigation, from whichever path is live.
    ///
    /// Both are read every frame and merged. There is no mode switch and no
    /// setting: with emulation stopped the keyboard path produces events and
    /// the pad path produces none; with emulation running it is the other way
    /// round (see `crate::pad`). Reading both is what makes that transition
    /// invisible to the person at the cabinet.
    fn navigation(&mut self, ctx: &egui::Context, now: Instant) -> Vec<Nav> {
        let mut moves = Vec::new();
        let focused = ctx.input(|i| i.focused);
        if focused {
            ctx.input_mut(|i| {
                for (key, nav) in [
                    (egui::Key::ArrowUp, Nav::Up),
                    (egui::Key::ArrowDown, Nav::Down),
                    (egui::Key::ArrowLeft, Nav::Left),
                    (egui::Key::ArrowRight, Nav::Right),
                    (egui::Key::Enter, Nav::Confirm),
                    (egui::Key::Space, Nav::Confirm),
                    (egui::Key::Escape, Nav::Back),
                    (egui::Key::Backspace, Nav::Back),
                ] {
                    // Consumed, so a stray press cannot also reach a widget —
                    // and counted, so holding a key repeats at the OS's rate
                    // rather than once per paint.
                    for _ in 0..i.count_and_consume_key(egui::Modifiers::NONE, key) {
                        moves.push(nav);
                    }
                }
            });
        }
        #[cfg(windows)]
        if focused {
            // Only while this window has focus. While a session runs, these
            // are the same presses the GAME is receiving, and a background
            // window must not eat a quarter-circle.
            moves.extend(self.pads.poll(now));
        }
        let _ = now;
        moves
    }

    /// How many pads are answering XInput right now, and whether any ever did.
    /// Rendered on the Status screen so "the panel does nothing here" is a
    /// sentence rather than a mystery.
    pub fn pad_nav_state(&self) -> (usize, bool) {
        #[cfg(windows)]
        {
            (self.pads.connected_count(), self.pads.any_pad_seen())
        }
        #[cfg(not(windows))]
        {
            (0, false)
        }
    }

    /// Seconds this window has been open — the "nothing has happened yet"
    /// grace the button check uses before it starts nagging.
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Apply one navigation, letting the current screen decide what it meant.
    fn act(&mut self, nav: Nav, slow: &Slow) {
        match self.focus.apply(nav) {
            Action::Moved => {}
            Action::AtTop => {
                // Back from the top of the preset picker leaves the picker.
                // Back from the top of anything else does nothing at all — it
                // is never "quit" (see `nav::Focus::apply`).
                self.picking = None;
            }
            Action::Activate(row) => screens::activate(self, slow, row),
            Action::Confirmed { yes } => {
                let pending = self.pending.take();
                if yes {
                    if let Some(ask) = pending {
                        self.ask(ask);
                    }
                    self.picking = None;
                }
            }
        }
    }

    /// Queue an action behind the one modal this surface has.
    pub fn confirm(
        &mut self,
        question: impl Into<String>,
        consequence: impl Into<String>,
        ask: Ask,
    ) {
        self.pending = Some(ask);
        self.focus.ask(question, consequence);
    }

    /// Say something without asking anything — the "this surface cannot do
    /// that, here is what can" path.
    pub fn say(&self, text: impl Into<String>, tone: Tone) {
        if let Ok(mut slow) = self.slow.lock() {
            slow.flash = Some(Flash {
                text: text.into(),
                tone,
                at: Instant::now(),
                remedy: None,
            });
        }
    }
}

impl App {
    /// The window's lifecycle edges, one line each, at the top of every paint.
    ///
    /// # Why the viewport COUNT is in here
    ///
    /// "A ghost window that flashes but never fully loads, over and behind the
    /// cabinet UI" has three plausible explanations from inside this crate and
    /// they are indistinguishable without evidence: eframe creating and
    /// destroying a window on the `run_app_on_demand` path, a second egui
    /// viewport being opened, or something outside egui entirely. Logging the
    /// viewport map's size on the first frame and on every change separates
    /// them in one line — if this crate ever owns two viewports, it says so,
    /// and if it never does the ghost is somebody else's (it was: a console
    /// window conjured by a `schtasks` spawn — see
    /// `ksx_platform::process::no_window`).
    fn log_lifecycle(&mut self, ctx: &egui::Context) {
        let (focused, viewports, close_requested) = ctx.input(|i| {
            (
                i.focused,
                i.raw.viewports.len(),
                i.viewport().close_requested(),
            )
        });

        if !self.painted {
            self.painted = true;
            self.viewports_seen = viewports;
            tracing::info!(
                first_paint_ms = self.started.elapsed().as_millis(),
                viewports,
                viewport_id = ?ctx.viewport_id(),
                focused,
                "cabinet window: first frame painted"
            );
        } else if viewports != self.viewports_seen {
            // This crate opens exactly one viewport. A change here is the
            // ghost, named.
            tracing::warn!(
                was = self.viewports_seen,
                now = viewports,
                "cabinet window: the viewport count changed — this surface owns ONE window"
            );
            self.viewports_seen = viewports;
        }

        if focused != self.was_focused {
            self.was_focused = focused;
            tracing::debug!(focused, "cabinet window: focus changed");
            if focused {
                self.note_focus_regained();
            }
        }

        if close_requested {
            // The single most important line in this file. Inside the daemon
            // this must be followed by "event loop returned cleanly" and
            // NOTHING else — no session stop, no claim release, no quit.
            tracing::info!(
                open_for_ms = self.started.elapsed().as_millis(),
                "cabinet window: close requested — the window is going away; \
                 emulation, the claim and the tray are untouched"
            );
        }
    }

    /// **The focus-theft detector**, and the reason it is `warn!` rather than
    /// another `debug!` line.
    ///
    /// The ghost that cost an evening was a *console window*, conjured every
    /// two seconds behind the status refresh and destroyed again ~200 ms later
    /// (`ksx_platform::process::no_window`). From the cabinet's side that is
    /// invisible except as this: focus lost and regained, over and over, at a
    /// steady period. Nothing in the window's own state can see the culprit —
    /// but the *symptom* is unmistakable, and a cabinet's log should describe a
    /// panel that is being walked over rather than leaving somebody to notice
    /// the flicker and wonder.
    ///
    /// Warned once per [`FOCUS_WINDOW`]: enough to be seen in a day's log, not
    /// enough to fill one.
    fn note_focus_regained(&mut self) {
        if self.focus_since.elapsed() >= FOCUS_WINDOW {
            self.focus_since = Instant::now();
            self.focus_regains = 0;
            self.focus_warned = false;
        }
        self.focus_regains += 1;
        if self.focus_regains >= FOCUS_THEFT && !self.focus_warned {
            self.focus_warned = true;
            tracing::warn!(
                regained = self.focus_regains,
                within_s = FOCUS_WINDOW.as_secs(),
                "cabinet window: something keeps taking focus away from this window and giving \
                 it back. A window flashing over the panel is usually a console conjured for a \
                 child process by a daemon that has released its own console"
            );
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.log_lifecycle(ctx);
        self.take_frame(now);
        let slow = self.slow();
        for nav in self.navigation(ctx, now) {
            self.act(nav, &slow);
        }

        // Three docked regions, not one scroller. The footer is the control
        // panel of a surface with no mouse, and the flash is the answer to the
        // press somebody just made: neither may ever be pushed off the bottom
        // of a cabinet screen by a long list, so neither is inside the scroll
        // area.
        let gutter = egui::Margin::symmetric(theme::sp::S6 as i8, theme::sp::S3 as i8);
        let chrome = egui::Frame::NONE
            .fill(theme::role::SURFACE)
            .inner_margin(gutter);
        egui::TopBottomPanel::top("ksx-cabinet-head")
            .frame(chrome)
            .show_separator_line(false)
            .show(ctx, |ui| screens::head(ui, self, &slow));
        egui::TopBottomPanel::bottom("ksx-cabinet-foot")
            .frame(chrome)
            .show_separator_line(false)
            .show(ctx, |ui| screens::foot(ui, self, &slow, now));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::role::SURFACE).inner_margin(
                egui::Margin::symmetric(theme::sp::S6 as i8, theme::sp::S1 as i8),
            ))
            .show(ctx, |ui| screens::body(ui, self, &slow, now));

        if let Some(confirm) = self.focus.confirming.clone() {
            screens::modal(ctx, &confirm);
        }

        // A live surface repaints; a still one sleeps. With the button check
        // open this is ~60 Hz and everything else is idle, which is the
        // display-rate coalescing rule from the consumer's side.
        let live = self.focus.screen == Screen::ButtonCheck || !self.lit.is_empty();
        ctx.request_repaint_after(if live {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(250)
        });
    }

    /// eframe's last call before the window goes. The counterpart to
    /// `App::new`'s "viewport created" line, so a log always has both ends.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        tracing::info!(
            open_for_ms = self.started.elapsed().as_millis(),
            keys_seen = self.keys.len(),
            dropped = self.dropped,
            off_panel = self.off_panel,
            "cabinet window: closed"
        );
    }
}

/// The subscription and the worker's `Sender` go here, and with them the
/// pipeline's cost (`ksx-backend`'s `crate::feed`). Logged because "did the closed
/// window actually stop paying" is otherwise only answerable by inspection.
impl Drop for App {
    fn drop(&mut self) {
        tracing::debug!(
            "cabinet window: app dropped — live subscription released, worker asked to stop"
        );
    }
}

/// Is this flash still worth showing?
pub fn flash_alive(flash: &Flash, now: Instant) -> bool {
    now.saturating_duration_since(flash.at) < FLASH_FOR
}
