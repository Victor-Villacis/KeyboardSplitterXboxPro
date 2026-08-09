//! The claimed-panel keyboard: what makes a WinUSB claim survivable.
//!
//! # The problem M6 creates
//!
//! A WinUSB-claimed interface is not a keyboard. Windows sees nothing from it —
//! that is the point, and it is why blocking becomes free and structural. But
//! LaunchBox and RetroBat navigate their menus with keystrokes, so **the moment
//! emulation stops, the panel goes dead**, and if ksx is not running at all it
//! was never alive.
//!
//! # The answer
//!
//! ksx holds the claim continuously and puts the keystrokes back itself
//! whenever it is not translating them:
//!
//! ```text
//!            emulation stopped                      emulation running
//!            ─────────────────                      ─────────────────
//!  panel ──▶ capture ──▶ Typethrough ──▶ SendInput   panel ──▶ capture ──▶ engine ──▶ pads
//!                        (types)                       (Typethrough muted, forwards)
//! ```
//!
//! The daemon (M5) is what guarantees ksx is always running: `ksx autostart
//! --enable` registers `ksx daemon` at logon, and the daemon owns the claim for
//! its whole lifetime rather than per-session ([`super::panel`]). That ownership
//! inversion is the structural difference between the Interception era and this
//! one — see `docs/ARCHITECTURE.md` §M6.
//!
//! # This thread is the panel's *only* injector, and the fork in the road
//!
//! One thread drains the claim's event stream, and it does two things with each
//! event: hands it to [`Typethrough`] (which types it, or does not, depending on
//! the mode) and — while a session is attached — forwards it to that session's
//! capture channel. Both duties live here on purpose:
//!
//! - **One held-key set.** If the capture backend injected for itself *and* a
//!   typethrough injected too, the panel would have two independent records of
//!   what Windows believes is held down, and every mode switch would be a chance
//!   to strand a key. The daemon therefore builds its claim with
//!   [`ksx_platform::inject::NullInjector`] and lets this thread do all of it.
//! - **No gap and no double.** Muting/unmuting is one state change on one
//!   thread, acknowledged before [`TypethroughCtl::set_emulating`] returns —
//!   there is no window in which both injectors are live, because there is only
//!   ever one.
//!
//! # Emergency escapes still bypass everything
//!
//! `LeftCtrl ×5` is detected in the capture thread and latched in the shared
//! [`EscapeHandle`]. This thread reads that latch **live, per event** (one
//! relaxed atomic load), exactly as `ksx_capture::winusb` does: a gesture puts
//! the panel back to typing on the very next key, with no supervisor, no control
//! message, and nothing that a wedged session could starve.
//!
//! # The failure mode, stated honestly
//!
//! If ksx is not running, a WinUSB-claimed panel does nothing at all. No
//! keystrokes, no menu, no way to type the release command from that panel.
//! That is the price of escaping the 2026 cross-signed driver cliff, and the
//! mitigations are exactly two: autostart, and the one-command rollback
//! (`ksx winusb release <device> --yes`, or the `pnputil` lines in
//! `docs/RECOVERY.md` §2 for when ksx itself will not start). Injected keys also
//! never reach the secure desktop, which is why `ksx winusb claim` refuses to
//! take the machine's last keyboard.

use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ksx_capture::{EscapeHandle, HealthHandle};
use ksx_core::KeyEvent;
use ksx_platform::inject::{KeyInjector, Typethrough, TypethroughStats};

use super::PanelKeyboard;

/// What the daemon (or an attached session) tells the typethrough thread.
#[derive(Debug)]
pub enum Command {
    /// Emulation owns the panel: inject nothing, and release anything held.
    Emulating,
    /// Emulation is stopped: the panel is a keyboard again.
    Typing,
    /// Also forward every event to this session's capture channel.
    Attach(Sender<KeyEvent>),
    /// The session is gone; stop forwarding.
    Detach,
    Shutdown,
}

/// A command, with an optional acknowledgement.
///
/// The ack is not ceremony. A mode change that is merely *queued* races the
/// keystroke stream: the supervisor arms the engine, the player's next press
/// arrives, and `select!` is free to pick the keystroke before the mode
/// message — so the same press would be translated to a pad button *and* typed
/// onto the desktop. Waiting for the ack makes "emulation owns the panel from
/// here on" true at the instant `set_emulating` returns.
#[derive(Debug)]
struct Message {
    command: Command,
    ack: Option<Sender<()>>,
}

/// How long the daemon will wait for the typethrough thread to acknowledge.
///
/// Bounded, because the daemon's control thread must never be hostage to it:
/// a wedged typethrough costs you a panel that keeps typing for a moment too
/// long, which is annoying. A wedged control thread costs you a daemon that
/// cannot stop a session, which is not.
const ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// What the thread is built with beyond the injector.
#[derive(Clone, Debug, Default)]
pub struct TypethroughConfig {
    /// The session-wide escape latch, read live per event. `None` means "no
    /// escape awareness" — correct only for a typethrough with no capture
    /// backend behind it (the unit tests below).
    pub escapes: Option<EscapeHandle>,
    /// Where a full session channel is counted. Same handle the capture backend
    /// publishes into, so `ksx run`'s dropped-event report covers the daemon's
    /// hand-off too.
    pub health: HealthHandle,
}

/// A cheap, cloneable handle for changing the thread's mode.
///
/// Separate from [`TypethroughService`] because the panel is shared: the daemon
/// control loop changes the mode through one clone while a session's capture
/// adapter attaches and detaches through another, and neither owns the thread.
#[derive(Clone, Debug)]
pub struct TypethroughCtl(Sender<Message>);

impl TypethroughCtl {
    /// `true` = emulation owns the panel. Returns once the change is in effect.
    pub fn set_emulating(&self, emulating: bool) {
        let command = if emulating {
            Command::Emulating
        } else {
            Command::Typing
        };
        self.send(command);
    }

    /// Start forwarding events to a session's capture channel.
    pub fn attach(&self, events: Sender<KeyEvent>) {
        self.send(Command::Attach(events));
    }

    /// Stop forwarding: the session is over.
    pub fn detach(&self) {
        self.send(Command::Detach);
    }

    /// Send and wait for the thread to say it has applied it.
    ///
    /// A dead typethrough thread is not fatal to the daemon: the pipeline still
    /// works, the panel just stops typing. Logged, never escalated — losing the
    /// frontend's menu must not also lose the ability to stop a session.
    fn send(&self, command: Command) {
        let (ack, ack_rx) = crossbeam_channel::bounded::<()>(1);
        if self
            .0
            .send(Message {
                command,
                ack: Some(ack),
            })
            .is_err()
        {
            tracing::warn!("the typethrough thread is gone; a claimed panel will not type");
            return;
        }
        if ack_rx.recv_timeout(ACK_TIMEOUT).is_err() {
            tracing::warn!(
                "the typethrough thread did not acknowledge in {ACK_TIMEOUT:?}; continuing anyway"
            );
        }
    }
}

/// A running typethrough thread.
///
/// Dropping it shuts the thread down, and the thread's `Typethrough` releases
/// every key it is holding on the way out — so even an abrupt daemon teardown
/// cannot leave a key latched down on the desktop.
pub struct TypethroughService {
    ctl: Sender<Message>,
    handle: Option<JoinHandle<TypethroughStats>>,
}

impl TypethroughService {
    /// Start typing whatever arrives on `events`.
    ///
    /// Begins in "typing" mode: a daemon that comes up and does nothing must
    /// still leave the user able to drive their frontend. The injector is the
    /// caller's (`SendInputInjector` in production, a recording one in CI) —
    /// there is deliberately no constructor that picks one for you, because the
    /// panel's *only* injector is the one that ends up here.
    pub fn spawn_with_config<I: KeyInjector + 'static>(
        events: Receiver<KeyEvent>,
        injector: I,
        config: TypethroughConfig,
    ) -> std::io::Result<Self> {
        let (ctl, ctl_rx) = crossbeam_channel::unbounded::<Message>();
        let handle = std::thread::Builder::new()
            .name("ksx-typethrough".into())
            .spawn(move || pump(events, ctl_rx, Typethrough::new(injector), config))?;
        Ok(Self {
            ctl,
            handle: Some(handle),
        })
    }

    /// A handle for changing the mode from anywhere.
    pub fn ctl(&self) -> TypethroughCtl {
        TypethroughCtl(self.ctl.clone())
    }

    /// Stop the thread and collect its counters. Idempotent-ish: a second call
    /// returns the default.
    pub fn join(&mut self) -> TypethroughStats {
        let _ = self.ctl.send(Message {
            command: Command::Shutdown,
            ack: None,
        });
        match self.handle.take() {
            Some(handle) => handle.join().unwrap_or_default(),
            None => TypethroughStats::default(),
        }
    }
}

impl PanelKeyboard for TypethroughService {
    fn set_emulating(&mut self, emulating: bool) {
        self.ctl().set_emulating(emulating);
    }
}

impl Drop for TypethroughService {
    fn drop(&mut self) {
        self.join();
    }
}

/// The thread body. Kept separate so it is exercised directly in tests without
/// a daemon around it.
fn pump<I: KeyInjector>(
    events: Receiver<KeyEvent>,
    ctl: Receiver<Message>,
    mut typethrough: Typethrough<I>,
    config: TypethroughConfig,
) -> TypethroughStats {
    let escapes = config.escapes;
    let health = config.health;
    // What the daemon/session asked for. The *effective* mode also depends on
    // the escape latch, which is why this is kept separately from
    // `Typethrough::is_emulating`.
    let mut emulating = false;
    let mut session: Option<Sender<KeyEvent>> = None;

    /// Apply the asked-for mode, overridden by a latched emergency escape.
    fn apply<I: KeyInjector>(
        typethrough: &mut Typethrough<I>,
        emulating: bool,
        escapes: Option<&EscapeHandle>,
    ) {
        let freed = escapes.is_some_and(EscapeHandle::passthrough);
        typethrough.set_emulating(emulating && !freed);
    }

    loop {
        crossbeam_channel::select! {
            recv(ctl) -> msg => match msg {
                Ok(Message { command, ack }) => {
                    match command {
                        Command::Emulating => emulating = true,
                        Command::Typing => emulating = false,
                        Command::Attach(tx) => session = Some(tx),
                        Command::Detach => session = None,
                        Command::Shutdown => break,
                    }
                    apply(&mut typethrough, emulating, escapes.as_ref());
                    // Acknowledge only after the change is actually in effect —
                    // that is the whole guarantee (see `Message`).
                    if let Some(ack) = ack {
                        let _ = ack.send(());
                    }
                }
                Err(_) => break,
            },
            recv(events) -> msg => match msg {
                Ok(event) => {
                    // The session first: a keystroke the player is making right
                    // now is worth more than the bookkeeping below it.
                    if let Some(tx) = &session {
                        match tx.try_send(event.clone()) {
                            Ok(()) => {}
                            // Same policy as every capture backend: count it,
                            // never block. The supervisor's watchdog is what
                            // reacts to a stalled consumer.
                            Err(TrySendError::Full(_)) => health.add_dropped(1),
                            // The session's channel is gone; its adapter will
                            // notice and end the session.
                            Err(TrySendError::Disconnected(_)) => session = None,
                        }
                    }
                    // Live escape-latch read, per event, before the decision —
                    // a `LeftCtrl x5` frees the panel with nothing in the way.
                    apply(&mut typethrough, emulating, escapes.as_ref());
                    typethrough.on_event(&event);
                }
                // The capture side is gone. Nothing left to type.
                Err(_) => break,
            },
        }
    }
    let stats = typethrough.stats();
    // `typethrough` drops here and releases everything it is holding.
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_core::{DeviceId, Key};
    use ksx_platform::inject::RecordingInjector;
    use std::time::{Duration, Instant};

    fn event(key: Key, down: bool) -> KeyEvent {
        KeyEvent {
            device: DeviceId::new(r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000"),
            key,
            down,
            t: 0,
        }
    }

    /// The plain, escape-unaware service every test but one wants.
    fn spawn_with<I: KeyInjector + 'static>(
        events: Receiver<KeyEvent>,
        injector: I,
    ) -> std::io::Result<TypethroughService> {
        TypethroughService::spawn_with_config(events, injector, TypethroughConfig::default())
    }

    /// Wait for a predicate rather than sleeping a fixed amount: the pump is a
    /// thread, and a flaky ordering test is worse than none.
    fn eventually(mut pred: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn a_freshly_spawned_service_types() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let injector = RecordingInjector::new();
        let log = injector.log();
        let mut svc = spawn_with(rx, injector).unwrap();

        tx.send(event(Key::Up, true)).unwrap();
        tx.send(event(Key::Up, false)).unwrap();
        assert!(
            eventually(|| log.trace() == "Up+ Up-"),
            "a stopped daemon must still drive the frontend: {}",
            log.trace()
        );
        svc.join();
    }

    /// `set_emulating` is acknowledged, so every keystroke sent afterwards is
    /// unambiguously in the new mode. Without the ack this test would be a
    /// race and the product would have one too: a press translated to a pad
    /// button *and* typed onto the desktop.
    #[test]
    fn a_service_told_to_emulate_types_nothing_from_the_moment_it_returns() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let injector = RecordingInjector::new();
        let log = injector.log();
        let mut svc = spawn_with(rx, injector).unwrap();

        svc.set_emulating(true);
        for _ in 0..20 {
            tx.send(event(Key::A, true)).unwrap();
            tx.send(event(Key::A, false)).unwrap();
        }
        // Give the pump every chance to get it wrong.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(log.trace(), "", "emulation owns these keys");

        svc.set_emulating(false);
        tx.send(event(Key::B, true)).unwrap();
        assert!(
            eventually(|| log.trace().contains("B+")),
            "typing must resume when emulation stops: {}",
            log.trace()
        );
        svc.join();
    }

    /// The stuck-key guarantee, end to end through the thread: a key held when
    /// the daemon shuts down must be released.
    #[test]
    fn shutdown_releases_a_key_that_was_still_held() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let injector = RecordingInjector::new();
        let log = injector.log();
        let mut svc = spawn_with(rx, injector).unwrap();

        tx.send(event(Key::Left, true)).unwrap();
        assert!(eventually(|| log.trace() == "Left+"));
        let stats = svc.join();
        assert!(stats.injected >= 1);
        assert!(
            log.events().contains(&(Key::Left, false)),
            "Left outlived the daemon held down: {}",
            log.trace()
        );
    }

    /// Losing the capture side ends the thread — and still releases.
    #[test]
    fn a_dropped_event_source_ends_the_thread_and_releases() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let injector = RecordingInjector::new();
        let log = injector.log();
        let mut svc = spawn_with(rx, injector).unwrap();
        tx.send(event(Key::Space, true)).unwrap();
        assert!(eventually(|| log.trace() == "Space+"));
        drop(tx);
        assert!(
            eventually(|| log.events().contains(&(Key::Space, false))),
            "{}",
            log.trace()
        );
        svc.join();
    }

    /// An attached session sees every event, and the panel stops typing them.
    /// This is the hand-off the daemon performs at session start.
    #[test]
    fn an_attached_session_receives_events_the_panel_no_longer_types() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let injector = RecordingInjector::new();
        let log = injector.log();
        let svc = spawn_with(rx, injector).unwrap();
        let ctl = svc.ctl();

        let (session_tx, session_rx) = crossbeam_channel::bounded(8);
        ctl.set_emulating(true);
        ctl.attach(session_tx);

        tx.send(event(Key::A, true)).unwrap();
        let got = session_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the session must receive the claim's events");
        assert_eq!(got.key, Key::A);
        assert_eq!(log.trace(), "", "emulation owns these keys");

        // ...and detaching hands the panel back without a re-claim.
        ctl.detach();
        ctl.set_emulating(false);
        tx.send(event(Key::B, true)).unwrap();
        assert!(eventually(|| log.trace().contains("B+")), "{}", log.trace());
        assert!(
            session_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a detached session must not keep receiving"
        );
    }

    /// The emergency escape is read from the shared latch on the *next event*,
    /// with no control message: `LeftCtrl x5` on a claimed panel must put the
    /// keystrokes back even while a session believes it owns the panel, and
    /// even if that session's supervisor is wedged.
    #[test]
    fn a_latched_escape_makes_the_panel_type_again_with_no_control_message() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let escapes = EscapeHandle::new();
        let injector = RecordingInjector::new();
        let log = injector.log();
        let mut svc = TypethroughService::spawn_with_config(
            rx,
            injector,
            TypethroughConfig {
                escapes: Some(escapes.clone()),
                health: HealthHandle::new(),
            },
        )
        .unwrap();
        svc.set_emulating(true);

        tx.send(event(Key::A, true)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(log.trace(), "", "emulation owns this key");

        // The capture thread completes `LeftCtrl x5` and latches passthrough —
        // the real gesture, through the real detector, on the shared handle.
        // Nothing sends this thread a message about it.
        let mut watch = ksx_capture::EscapeWatch::new(escapes.clone());
        for _ in 0..5 {
            for down in [true, false] {
                watch.observe(&event(Key::LeftControl, down));
            }
        }
        assert!(escapes.passthrough(), "the gesture must have latched");
        tx.send(event(Key::B, true)).unwrap();
        assert!(
            eventually(|| log.trace().contains("B+")),
            "a latched escape must free the panel on the very next key: {}",
            log.trace()
        );
        svc.join();
    }
}
