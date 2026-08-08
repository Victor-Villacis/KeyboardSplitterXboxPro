//! `ReplayBackend` — a recorded session as a capture source.
//!
//! The third [`CaptureBackend`], beside Interception and WinUSB: instead of
//! reading a driver it reads a file `ksx monitor --record` wrote, and emits the
//! events it holds on the schedule they were recorded at. Everything downstream
//! is untouched — same engine, same presets, same personas, same pads — because
//! the only thing that changed is where the [`KeyEvent`]s came from. That is
//! the whole point: a recording drives the identical pipeline a human does, so
//! replaying one is both an attract-mode demo and a full-stack regression test
//! that needs no hardware.
//!
//! # One format, and it already exists
//!
//! The corpus is the JSONL `ksx monitor --record` writes (documented in
//! `ksx-app/src/monitor.rs`), one object per line:
//!
//! ```json
//! {"t_ms": 1042, "device": "HID\\VID_D209&PID_0430&REV_0056&MI_00", "key": "A", "down": true}
//! ```
//!
//! A **macro** is a trigger-fired sequence bound to a key
//! (`docs/INPUT-TRANSFORMS.md` §1c); a **session** is a whole timeline with no
//! trigger at all. They are different things, so a session is not stored as a
//! macro — and a second file format for the same four fields would be a second
//! thing to keep true. Parsing is hand-rolled here because this crate is
//! deliberately serde-free and because the format is a *stable contract*, not a
//! derived type.
//!
//! # Escapes are deliberately NOT evaluated on a replayed stroke
//!
//! [`CaptureBackend::escapes`] is undefaulted precisely so a new backend has to
//! say what it does about the emergency hatch. This one **never observes its own
//! events**: on an I-PAC, P1 Button 1 *is* LeftCtrl, so a recorded four-player
//! session is full of `LeftCtrl ×5` gestures that were never a gesture — they
//! were somebody playing. Feeding them to [`crate::escape::EscapeWatch`] would
//! make an attract loop free the keyboards and stop itself, at a moment nobody
//! asked for anything. The hatch is a thing a *person* does, so it stays with
//! the backend that watches the real board ([`Silenced`], which is exactly why
//! playback keeps one).
//!
//! # The clock is injected
//!
//! [`ReplayClock`] has one method — wait until this event's stamp — so the
//! schedule can be driven by [`VirtualClock`] in a test and asserted directly,
//! with no sleeping and no wall-clock flake.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use ksx_core::{DeviceId, Key, KeyEvent};

use crate::backend::{CaptureBackend, CaptureCtl, DeviceInfo, DeviceKind, ExitReason, Handles};
use crate::escape::EscapeHandle;
use crate::health::HealthHandle;
use crate::presence::PresenceHandle;

/// How often a real-clock wait wakes to re-check the control channel. Bounds
/// shutdown latency in the middle of a long gap between recorded events — an
/// attract loop must stop when a player walks up, not when the file says so.
const POLL: Duration = Duration::from_millis(50);

/// Event-channel capacity for [`Silenced`]'s discarded stream. Only ever holds
/// what the drain thread has not taken yet.
const SILENCED_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// The recording
// ---------------------------------------------------------------------------

/// One recorded event: exactly the four fields of the corpus line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedEvent {
    /// Milliseconds since the recording started.
    pub t_ms: u64,
    /// The device id **as it was at record time** — which after a replug, or on
    /// another machine, may name nothing at all. Resolving that is the caller's
    /// job (`ksx-app`'s `play::resolve`), not this type's.
    pub device: DeviceId,
    pub key: Key,
    pub down: bool,
}

/// A parsed `--record` file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recording {
    events: Vec<RecordedEvent>,
}

/// Why a file is not a recording. Every variant names the line, because the
/// only useful thing to say about a bad line is which one it is.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RecordingError {
    #[error(
        "line {line} has no \"{field}\" field. Every line of a recording is one JSON object \
         {{t_ms, device, key, down}}, as `ksx monitor --record` writes it — a `ksx monitor \
         --json` capture of *stdout* is not one, because that stream also carries warning \
         and summary lines"
    )]
    MissingField { line: usize, field: &'static str },
    #[error("line {line}: \"{field}\" is {found:?}, which is not {expected}")]
    BadField {
        line: usize,
        field: &'static str,
        found: String,
        expected: &'static str,
    },
    #[error(
        "line {line}: \"{name}\" is not a key name ksx knows. Recordings use the legacy \
         InterceptionKey spelling, exactly as `ksx monitor` prints it (`A`, `LeftControl`, \
         `Numpad5`) — it is case-sensitive"
    )]
    UnknownKey { line: usize, name: String },
    #[error(
        "line {line}: t_ms goes backwards ({previous} then {found}). A recording is a \
         timeline, so its stamps only ever increase; this file was edited or concatenated \
         from two runs"
    )]
    TimeWentBackwards {
        line: usize,
        previous: u64,
        found: u64,
    },
    #[error(
        "this recording holds no events. Playing it would plug the pads and then do nothing \
         — record one with `ksx monitor --record <FILE>` first"
    )]
    Empty,
}

impl Recording {
    /// Parse the whole file. Blank lines are skipped; everything else must be a
    /// corpus line.
    pub fn parse(text: &str) -> Result<Self, RecordingError> {
        let mut events: Vec<RecordedEvent> = Vec::new();
        let mut previous: Option<u64> = None;
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let t_ms = raw_field(line, "t_ms")
                .ok_or(RecordingError::MissingField {
                    line: number,
                    field: "t_ms",
                })?
                .parse::<u64>()
                .map_err(|_| RecordingError::BadField {
                    line: number,
                    field: "t_ms",
                    found: raw_field(line, "t_ms").unwrap_or_default().to_owned(),
                    expected: "a whole number of milliseconds",
                })?;
            if let Some(previous) = previous {
                if t_ms < previous {
                    return Err(RecordingError::TimeWentBackwards {
                        line: number,
                        previous,
                        found: t_ms,
                    });
                }
            }
            previous = Some(t_ms);

            let device = unescape(raw_field(line, "device").ok_or(
                RecordingError::MissingField {
                    line: number,
                    field: "device",
                },
            )?);
            let name = unescape(raw_field(line, "key").ok_or(RecordingError::MissingField {
                line: number,
                field: "key",
            })?);
            let key = Key::from_name(&name).ok_or(RecordingError::UnknownKey {
                line: number,
                name: name.clone(),
            })?;
            let down = match raw_field(line, "down").ok_or(RecordingError::MissingField {
                line: number,
                field: "down",
            })? {
                "true" => true,
                "false" => false,
                other => {
                    return Err(RecordingError::BadField {
                        line: number,
                        field: "down",
                        found: other.to_owned(),
                        expected: "true or false",
                    })
                }
            };

            events.push(RecordedEvent {
                t_ms,
                device: DeviceId::new(device),
                key,
                down,
            });
        }
        if events.is_empty() {
            return Err(RecordingError::Empty);
        }
        Ok(Self { events })
    }

    /// Build one directly — for tests and for callers that already hold events.
    pub fn from_events(events: Vec<RecordedEvent>) -> Self {
        Self { events }
    }

    pub fn events(&self) -> &[RecordedEvent] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Every distinct device the recording mentions, in first-seen order.
    pub fn devices(&self) -> Vec<DeviceId> {
        let mut out: Vec<DeviceId> = Vec::new();
        for event in &self.events {
            if !out.contains(&event.device) {
                out.push(event.device.clone());
            }
        }
        out
    }

    /// How many events name `device`.
    pub fn count_for(&self, device: &DeviceId) -> usize {
        self.events.iter().filter(|e| &e.device == device).count()
    }

    /// The last stamp — how long one pass takes at speed 1.0.
    pub fn duration_ms(&self) -> u64 {
        self.events.last().map_or(0, |e| e.t_ms)
    }

    /// Point every event recorded against `from` at `to`. Returns how many
    /// moved, so a caller can refuse a remap that matched nothing rather than
    /// starting a session that would drive no pad.
    pub fn remap(&mut self, from: &DeviceId, to: &DeviceId) -> usize {
        let mut moved = 0;
        for event in &mut self.events {
            if &event.device == from {
                event.device = to.clone();
                moved += 1;
            }
        }
        moved
    }
}

/// The value of `"name":` on one JSONL line, still escaped, quotes stripped.
fn raw_field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\":");
    let rest = line[line.find(&needle)? + needle.len()..].trim_start();
    let Some(body) = rest.strip_prefix('"') else {
        return Some(rest.split([',', '}']).next()?.trim());
    };
    // A quoted value ends at the first unescaped quote. Device ids are full of
    // backslashes (`HID\\VID_D209&...`), so this cannot stop at the first `"`
    // it sees without checking what precedes it.
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(&body[..i]),
            _ => i += 1,
        }
    }
    None
}

/// JSON string escapes, undone.
fn unescape(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            // `\\`, `\"`, `\/` and anything else: the character itself. A
            // device id's backslashes are the only escape that occurs in
            // practice, and dropping the marker is what un-escaping them means.
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Playback rate
// ---------------------------------------------------------------------------

/// A playback rate: `1.0` is real time, `2.0` is twice as fast.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed(f64);

/// Why a `--speed` value was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "speed must be a number between {min} and {max} — it multiplies the recorded pace, so \
     0 or a negative value does not describe a playback at all",
    min = Speed::MIN,
    max = Speed::MAX
)]
pub struct SpeedError;

impl Speed {
    /// Real time.
    pub const NORMAL: Speed = Speed(1.0);
    /// A hundred times slower than recorded — a whole pass of the cabinet
    /// corpus still finishes inside an evening.
    pub const MIN: f64 = 0.01;
    /// A hundred times faster. Beyond this every event is due at once and the
    /// "schedule" stops meaning anything.
    pub const MAX: f64 = 100.0;

    pub fn new(value: f64) -> Result<Self, SpeedError> {
        if !value.is_finite() || !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(SpeedError);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> f64 {
        self.0
    }

    /// A recorded stamp, in playback milliseconds.
    pub fn scale(self, t_ms: u64) -> u64 {
        (t_ms as f64 / self.0).round() as u64
    }
}

impl Default for Speed {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl std::fmt::Display for Speed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x", self.0)
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// How a replay waits for the next event.
///
/// One method, because waiting for a stamp is the only thing playback does with
/// time. `interrupted` is polled *while* waiting — it is where the capture
/// thread drains its control channel — and returning `true` from it aborts the
/// wait, which is what makes `Shutdown` land inside a long recorded gap.
pub trait ReplayClock: Send {
    /// Block until `t_ms` have elapsed since playback began. Returns `false` if
    /// `interrupted` asked to stop first.
    fn wait_until(&mut self, t_ms: u64, interrupted: &mut dyn FnMut() -> bool) -> bool;
}

/// Wall-clock playback: the recorded pace, as it happened.
#[derive(Debug)]
pub struct RealClock {
    start: Instant,
    poll: Duration,
}

impl RealClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            poll: POLL,
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayClock for RealClock {
    fn wait_until(&mut self, t_ms: u64, interrupted: &mut dyn FnMut() -> bool) -> bool {
        let due = Duration::from_millis(t_ms);
        loop {
            if interrupted() {
                return false;
            }
            let elapsed = self.start.elapsed();
            if elapsed >= due {
                return true;
            }
            std::thread::sleep((due - elapsed).min(self.poll));
        }
    }
}

/// A clock that never sleeps: it records the deadline and returns.
///
/// The schedule then becomes something a test can *read* — [`Self::waits`] is
/// the exact sequence of stamps playback asked for — instead of something it
/// has to measure with a stopwatch and a tolerance.
#[derive(Clone, Debug, Default)]
pub struct VirtualClock(Arc<Mutex<Vec<u64>>>);

impl VirtualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every deadline playback waited for, in order.
    pub fn waits(&self) -> Vec<u64> {
        self.0.lock().expect("virtual clock poisoned").clone()
    }
}

impl ReplayClock for VirtualClock {
    fn wait_until(&mut self, t_ms: u64, interrupted: &mut dyn FnMut() -> bool) -> bool {
        self.0.lock().expect("virtual clock poisoned").push(t_ms);
        !interrupted()
    }
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ProgressInner {
    events: AtomicU64,
    passes: AtomicU64,
    finished: AtomicBool,
}

/// A live, cloneable view of how far playback has got.
///
/// Grab it before [`CaptureBackend::run`], like the health handle. `finished`
/// is how the session above learns that the recording ran out — the capture
/// thread deliberately stays alive after that (a thread that simply exited
/// would be reported as a capture failure, which it is not).
#[derive(Clone, Debug, Default)]
pub struct ReplayProgress(Arc<ProgressInner>);

impl ReplayProgress {
    pub fn events(&self) -> u64 {
        self.0.events.load(Ordering::Relaxed)
    }

    /// Completed passes over the recording. Always 1 when a non-looping replay
    /// has finished.
    pub fn passes(&self) -> u64 {
        self.0.passes.load(Ordering::Relaxed)
    }

    /// The recording ran out and playback is not looping.
    pub fn finished(&self) -> bool {
        self.0.finished.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------------

/// A recorded session, played back as if it were a keyboard.
pub struct ReplayBackend {
    recording: Recording,
    speed: Speed,
    looping: bool,
    clock: Box<dyn ReplayClock>,
    progress: ReplayProgress,
    health: HealthHandle,
    escapes: EscapeHandle,
    presence: PresenceHandle,
}

impl ReplayBackend {
    /// Play `recording` at real time, once.
    pub fn new(recording: Recording) -> Self {
        let presence = PresenceHandle::new();
        presence.publish(recording.devices());
        Self {
            recording,
            speed: Speed::NORMAL,
            looping: false,
            clock: Box::new(RealClock::new()),
            progress: ReplayProgress::default(),
            health: HealthHandle::new(),
            escapes: EscapeHandle::new(),
            presence,
        }
    }

    pub fn with_speed(mut self, speed: Speed) -> Self {
        self.speed = speed;
        self
    }

    /// Restart at the end instead of stopping — the cabinet's attract mode.
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    /// Drive the schedule with a different clock. [`VirtualClock`] makes the
    /// whole timeline assertable without sleeping.
    pub fn with_clock(mut self, clock: Box<dyn ReplayClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Publish into pre-existing handles instead of fresh ones — how the replay
    /// and the [`Silenced`] real board become one session under
    /// [`crate::CompositeBackend`] (one health state, one escape latch).
    pub fn with_handles(mut self, handles: Handles) -> Self {
        self.health = handles.health;
        self.escapes = handles.escapes;
        self
    }

    /// Publish progress into an existing handle.
    ///
    /// For the caller that has to hold the handle *before* it can build the
    /// backend — `ksx play` wires its session hook from this, and the backend
    /// itself is only constructed later, inside the closure that runs once the
    /// pads are up (`crate::run::live_session`).
    pub fn with_progress(mut self, progress: ReplayProgress) -> Self {
        self.progress = progress;
        self
    }

    /// Live progress. Grab it before `run`.
    pub fn progress(&self) -> ReplayProgress {
        self.progress.clone()
    }

    /// How long one pass takes at the configured speed.
    pub fn pass_ms(&self) -> u64 {
        self.speed.scale(self.recording.duration_ms())
    }
}

impl CaptureBackend for ReplayBackend {
    /// The recording's devices, as the session's input devices.
    ///
    /// This is what makes a plan's slots look connected: the board that drives
    /// them is the file, and the file is right here. The real board's presence
    /// is *not* reported (see [`Silenced::presence`]) — unplugging it mid-replay
    /// changes nothing about what the pads are being fed, so invalidating those
    /// slots would be a lie.
    fn devices(&mut self) -> Vec<DeviceInfo> {
        self.recording
            .devices()
            .into_iter()
            .map(|id| DeviceInfo {
                id,
                interception_slot: None,
                friendly: Some("recorded session".to_owned()),
                kind: DeviceKind::Keyboard,
            })
            .collect()
    }

    fn health(&self) -> HealthHandle {
        self.health.clone()
    }

    /// A handle this backend never writes to — see the module docs. A replayed
    /// `LeftCtrl ×5` is somebody's P1 Button 1 from ten minutes ago, not a
    /// request to release the keyboards now.
    fn escapes(&self) -> EscapeHandle {
        self.escapes.clone()
    }

    fn presence(&self) -> PresenceHandle {
        self.presence.clone()
    }

    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        let ReplayBackend {
            recording,
            speed,
            looping,
            mut clock,
            progress,
            health,
            escapes: _,
            presence: _,
        } = *self;

        std::thread::Builder::new()
            .name("ksx-capture-replay".into())
            .spawn(move || {
                // One pass is as long as the recording, so pass N's event i is
                // due at `N * period + scaled(t_ms)`. No artificial gap: the
                // wrap lands exactly where the recording ended, which is the
                // one instant a recorded timeline has an opinion about.
                let period = speed.scale(recording.duration_ms());
                let mut held: Vec<(DeviceId, Key)> = Vec::new();
                let mut pass: u64 = 0;

                'playback: loop {
                    for event in recording.events() {
                        let due = pass.saturating_mul(period) + speed.scale(event.t_ms);
                        if !wait(clock.as_mut(), due, &ctl) {
                            return ExitReason::Shutdown;
                        }
                        track(&mut held, event.device.clone(), event.key, event.down);
                        match emit(&tx, &event.device, event.key, event.down) {
                            Sent::Ok => {
                                progress.0.events.fetch_add(1, Ordering::Relaxed);
                            }
                            Sent::Dropped => health.add_dropped(1),
                            Sent::Gone => return ExitReason::ChannelClosed,
                        }
                    }
                    progress.0.passes.fetch_add(1, Ordering::Relaxed);
                    if !looping {
                        break 'playback;
                    }

                    // Anything still held at the wrap is released before the
                    // recording starts over. A balanced recording (the cabinet
                    // corpus is one) leaves this empty; an unbalanced one would
                    // otherwise loop with a button stuck down forever, on a
                    // machine nobody is standing at.
                    pass += 1;
                    let wrap = pass.saturating_mul(period);
                    for (device, key) in std::mem::take(&mut held) {
                        if !wait(clock.as_mut(), wrap, &ctl) {
                            return ExitReason::Shutdown;
                        }
                        match emit(&tx, &device, key, false) {
                            Sent::Ok => {
                                progress.0.events.fetch_add(1, Ordering::Relaxed);
                            }
                            Sent::Dropped => health.add_dropped(1),
                            Sent::Gone => return ExitReason::ChannelClosed,
                        }
                    }
                }

                // The recording ran out. Stay alive and controllable: a capture
                // thread that exits here is reported by the supervisor as the
                // capture path dying, and this is the opposite of that — it is
                // the session doing exactly what was asked. `progress.finished`
                // is how the caller above turns it into a clean stop.
                progress.0.finished.store(true, Ordering::Release);
                loop {
                    match ctl.recv() {
                        Ok(CaptureCtl::Shutdown) => return ExitReason::Shutdown,
                        Ok(_) => {}
                        Err(_) => return ExitReason::ScriptExhausted,
                    }
                }
            })
    }
}

/// Wait for `due`, draining control messages meanwhile. `false` means the
/// session asked to stop.
fn wait(clock: &mut dyn ReplayClock, due: u64, ctl: &Receiver<CaptureCtl>) -> bool {
    let mut stop = false;
    let mut interrupted = || {
        loop {
            match ctl.try_recv() {
                // Capture control is for a backend that can suppress a real
                // keystroke. Nothing this one emits ever reached Windows, so
                // there is nothing to swallow and nothing to hand back — the
                // messages are accepted and ignored, except the one that ends
                // the session.
                Ok(CaptureCtl::Shutdown) => stop = true,
                Ok(_) => {}
                // Nobody can stop this thread any more; a replay with no
                // supervisor is not something to keep driving pads with.
                Err(crossbeam_channel::TryRecvError::Disconnected) => stop = true,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
            }
        }
        stop
    };
    clock.wait_until(due, &mut interrupted)
}

enum Sent {
    Ok,
    Dropped,
    Gone,
}

fn emit(tx: &Sender<KeyEvent>, device: &DeviceId, key: Key, down: bool) -> Sent {
    let event = KeyEvent {
        device: device.clone(),
        key,
        down,
        t: stamp_now(),
    };
    match tx.try_send(event) {
        Ok(()) => Sent::Ok,
        // Same policy as every other backend: count it, never block. The
        // watchdog is not armed here on purpose — it exists to hand a real
        // keyboard back to Windows when the consumer stalls, and a recording
        // has no keyboard to hand back.
        Err(TrySendError::Full(_)) => Sent::Dropped,
        Err(TrySendError::Disconnected(_)) => Sent::Gone,
    }
}

/// Remember what is down, so a loop wrap can release it.
fn track(held: &mut Vec<(DeviceId, Key)>, device: DeviceId, key: Key, down: bool) {
    let at = held.iter().position(|(d, k)| d == &device && *k == key);
    match (down, at) {
        (true, None) => held.push((device, key)),
        (false, Some(at)) => {
            held.remove(at);
        }
        _ => {}
    }
}

/// The `KeyEvent.t` unit, identical to what the Interception and WinUSB
/// backends stamp with, so `ksx doctor --latency` compares like with like.
///
/// It is a *pipeline* stamp, not a schedule one: the histogram must go on
/// measuring capture→submit, which is the same question whether the event came
/// from a driver or from a file.
fn stamp_now() -> u64 {
    #[cfg(windows)]
    {
        let mut t: i64 = 0;
        // SAFETY: out-pointer to a stack i64; QPC cannot fail on XP+.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
        t as u64
    }
    #[cfg(not(windows))]
    {
        use std::sync::OnceLock;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
}

// ---------------------------------------------------------------------------
// Silenced
// ---------------------------------------------------------------------------

/// A capture backend whose events go nowhere.
///
/// Playback needs the real board **captured and ignored** at the same time.
/// Captured, because a panel that still types while a recording plays means the
/// player is fighting it — in the game, where it is the game that sees both.
/// Ignored, because the recording is the session's only source of truth about
/// what was pressed; merging a live stroke into it would produce a timeline
/// that never happened.
///
/// So the child runs exactly as it would in `ksx run` — it gets the real
/// control channel, so `SetCaptured` still swallows strokes, and it publishes
/// into the same [`Handles`], so `LeftCtrl ×5` on the real panel still frees
/// every keyboard in the session — and its event stream is drained into
/// nothing.
pub struct Silenced {
    inner: Box<dyn CaptureBackend>,
}

impl Silenced {
    pub fn new(inner: Box<dyn CaptureBackend>) -> Self {
        Self { inner }
    }
}

impl CaptureBackend for Silenced {
    /// **Nothing.** The devices are already reported by the replay beside it,
    /// and one board listed twice is what the supervisor's duplicate-hardware-id
    /// check exists to refuse — it would read two entries for one id as two
    /// identical boards and refuse the session before anything was plugged.
    fn devices(&mut self) -> Vec<DeviceInfo> {
        Vec::new()
    }

    fn health(&self) -> HealthHandle {
        self.inner.health()
    }

    fn escapes(&self) -> EscapeHandle {
        self.inner.escapes()
    }

    /// Unsupported, and not merely for convenience: during playback the pads are
    /// fed by the file. If the real board is unplugged mid-recording nothing
    /// about what the slots are receiving changes, so reporting it gone would
    /// invalidate slots that are working perfectly.
    fn presence(&self) -> PresenceHandle {
        PresenceHandle::unsupported()
    }

    fn run(
        self: Box<Self>,
        _tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        let (inner_tx, inner_rx) = crossbeam_channel::bounded::<KeyEvent>(SILENCED_CAPACITY);
        let handle = self.inner.run(inner_tx, ctl)?;
        // Drained rather than simply dropped: a disconnected receiver ends the
        // child with `ChannelClosed`, which the composite treats as the session
        // dying. The drain thread exits on its own when the child drops its
        // sender.
        std::thread::Builder::new()
            .name("ksx-capture-silenced".into())
            .spawn(move || while inner_rx.recv().is_ok() {})?;
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPAC: &str = r"HID\VID_D209&PID_0430&REV_0056&MI_00";
    const DESK: &str = r"HID\VID_046D&PID_C31C&REV_6402&MI_00";

    fn line(t_ms: u64, device: &str, key: &str, down: bool) -> String {
        format!(
            "{{\"t_ms\":{t_ms},\"device\":\"{}\",\"key\":\"{key}\",\"down\":{down}}}",
            device.replace('\\', "\\\\")
        )
    }

    fn two_line_recording() -> String {
        format!(
            "{}\n{}\n",
            line(0, IPAC, "A", true),
            line(120, IPAC, "A", false)
        )
    }

    /// The corpus format, round-tripped through the writer's own escaping.
    ///
    /// Breaks against: a parser that stops the device string at the first `"`
    /// without honouring `\\`, and against one that leaves `\\` doubled — both
    /// produce a device id that matches no configured board, i.e. a replay that
    /// refuses to start or drives nothing.
    #[test]
    fn a_recorded_line_parses_back_to_the_id_that_was_recorded() {
        let recording = Recording::parse(&two_line_recording()).expect("parses");
        assert_eq!(recording.len(), 2);
        assert_eq!(recording.events()[0].device, DeviceId::from(IPAC));
        assert_eq!(recording.events()[0].key, Key::A);
        assert!(recording.events()[0].down);
        assert_eq!(recording.events()[1].t_ms, 120);
        assert!(!recording.events()[1].down);
        assert_eq!(recording.duration_ms(), 120);
        assert_eq!(recording.devices(), vec![DeviceId::from(IPAC)]);
    }

    #[test]
    fn field_order_and_spacing_do_not_matter() {
        let text = format!(
            "{{ \"down\": true , \"key\": \"Left\", \"device\": \"{}\", \"t_ms\": 42 }}",
            IPAC.replace('\\', "\\\\")
        );
        let recording = Recording::parse(&text).expect("parses");
        assert_eq!(recording.events()[0].key, Key::Left);
        assert_eq!(recording.events()[0].t_ms, 42);
        assert_eq!(recording.events()[0].device, DeviceId::from(IPAC));
    }

    #[test]
    fn blank_lines_are_skipped_and_an_empty_file_is_refused() {
        let text = format!("\n{}\n\n", line(0, IPAC, "A", true));
        assert_eq!(Recording::parse(&text).expect("parses").len(), 1);
        assert_eq!(Recording::parse("\n\n").unwrap_err(), RecordingError::Empty);
    }

    /// A `ksx monitor --json` capture of stdout is the mistake this names: it
    /// holds the same event lines plus `{"warning":…}` and `{"summary":…}`, so
    /// a lenient parser would silently play a truncated session.
    #[test]
    fn a_line_that_is_not_an_event_is_refused_by_number() {
        let text = format!(
            "{}\n{{\"summary\":{{\"events\":1}}}}\n",
            line(0, IPAC, "A", true)
        );
        let err = Recording::parse(&text).unwrap_err();
        assert_eq!(
            err,
            RecordingError::MissingField {
                line: 2,
                field: "t_ms"
            }
        );
        let text = err.to_string();
        assert!(text.contains("line 2"), "{text}");
        assert!(text.contains("monitor --record"), "{text}");
    }

    #[test]
    fn an_unknown_key_name_names_the_line_and_the_spelling() {
        let text = line(0, IPAC, "Banana", true);
        let err = Recording::parse(&text).unwrap_err();
        assert!(matches!(err, RecordingError::UnknownKey { line: 1, .. }));
        assert!(err.to_string().contains("Banana"), "{err}");
        assert!(err.to_string().contains("case-sensitive"), "{err}");
    }

    /// Two recordings concatenated: the second one's stamps restart at 0, which
    /// would make playback wait out the whole first recording again before
    /// emitting anything. Refuse it where it is diagnosable.
    #[test]
    fn a_timeline_that_goes_backwards_is_refused() {
        let text = format!(
            "{}\n{}\n",
            line(500, IPAC, "A", true),
            line(10, IPAC, "A", false)
        );
        let err = Recording::parse(&text).unwrap_err();
        assert_eq!(
            err,
            RecordingError::TimeWentBackwards {
                line: 2,
                previous: 500,
                found: 10,
            }
        );
    }

    #[test]
    fn remap_moves_only_the_device_it_names_and_reports_how_many() {
        let text = format!(
            "{}\n{}\n{}\n",
            line(0, IPAC, "A", true),
            line(5, DESK, "B", true),
            line(9, IPAC, "A", false)
        );
        let mut recording = Recording::parse(&text).expect("parses");
        let target = DeviceId::from(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000");
        assert_eq!(recording.remap(&DeviceId::from(IPAC), &target), 2);
        assert_eq!(recording.count_for(&target), 2);
        assert_eq!(recording.count_for(&DeviceId::from(DESK)), 1);
        // A remap that names nothing must be visible as such, not silent.
        assert_eq!(recording.remap(&DeviceId::from(IPAC), &target), 0);
    }

    #[test]
    fn speed_refuses_the_values_that_are_not_a_playback() {
        assert!(Speed::new(0.0).is_err());
        assert!(Speed::new(-1.0).is_err());
        assert!(Speed::new(f64::NAN).is_err());
        assert!(Speed::new(f64::INFINITY).is_err());
        assert!(Speed::new(1000.0).is_err());
        assert_eq!(Speed::new(1.0).expect("valid"), Speed::NORMAL);
        // Faster means sooner, slower means later — the direction a `--speed`
        // flag has to have.
        assert_eq!(Speed::new(2.0).expect("valid").scale(1000), 500);
        assert_eq!(Speed::new(0.5).expect("valid").scale(1000), 2000);
        assert_eq!(Speed::NORMAL.scale(1042), 1042);
    }

    /// Run a non-looping replay to the end and collect everything it emitted.
    fn drive(backend: ReplayBackend) -> (Vec<KeyEvent>, ExitReason) {
        let progress = backend.progress();
        let (tx, rx) = crossbeam_channel::bounded(1024);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(backend).run(tx, ctl_rx).expect("thread");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !progress.finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(progress.finished(), "the recording should have run out");
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        ctl_tx.send(CaptureCtl::Shutdown).expect("ctl open");
        let reason = handle.join().expect("no panic");
        (events, reason)
    }

    /// **The schedule, asserted rather than measured.** Every event is waited
    /// for at its recorded stamp, in order.
    ///
    /// Breaks against a backend that emits everything at once (no wait at all),
    /// one that waits for *deltas* instead of absolute stamps (the second wait
    /// would be 120, not 1042), and one that applies speed to the wrong side of
    /// the division.
    #[test]
    fn the_schedule_is_the_recorded_stamps_on_a_virtual_clock() {
        let text = format!(
            "{}\n{}\n{}\n",
            line(0, IPAC, "A", true),
            line(120, IPAC, "A", false),
            line(1042, IPAC, "B", true)
        );
        let clock = VirtualClock::new();
        let backend = ReplayBackend::new(Recording::parse(&text).expect("parses"))
            .with_clock(Box::new(clock.clone()));
        let (events, reason) = drive(backend);

        assert_eq!(reason, ExitReason::Shutdown);
        assert_eq!(events.len(), 3);
        assert_eq!(clock.waits(), vec![0, 120, 1042]);
        assert_eq!(events[2].key, Key::B);
    }

    /// `--speed 2` halves every stamp; `--speed 0.5` doubles it. The DELTAS are
    /// what a viewer perceives, so they are what this pins.
    #[test]
    fn speed_scales_the_whole_timeline_not_just_the_first_gap() {
        let text = format!(
            "{}\n{}\n{}\n",
            line(0, IPAC, "A", true),
            line(120, IPAC, "A", false),
            line(1042, IPAC, "B", true)
        );
        let recording = Recording::parse(&text).expect("parses");

        let fast = VirtualClock::new();
        drive(
            ReplayBackend::new(recording.clone())
                .with_speed(Speed::new(2.0).expect("valid"))
                .with_clock(Box::new(fast.clone())),
        );
        assert_eq!(fast.waits(), vec![0, 60, 521]);

        let slow = VirtualClock::new();
        drive(
            ReplayBackend::new(recording)
                .with_speed(Speed::new(0.5).expect("valid"))
                .with_clock(Box::new(slow.clone())),
        );
        assert_eq!(slow.waits(), vec![0, 240, 2084]);
    }

    /// A loop's second pass is offset by exactly one recording, so the pace a
    /// viewer sees across the wrap is the pace inside it.
    ///
    /// Breaks against a loop that restarts its clock at zero — every pass after
    /// the first would then be due immediately and the attract mode would turn
    /// into a burst.
    #[test]
    fn looping_offsets_each_pass_by_one_whole_recording() {
        let clock = VirtualClock::new();
        let recording = Recording::parse(&two_line_recording()).expect("parses");
        let backend = ReplayBackend::new(recording)
            .looping(true)
            .with_clock(Box::new(clock.clone()));
        let progress = backend.progress();

        let (tx, rx) = crossbeam_channel::bounded(1024);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(backend).run(tx, ctl_rx).expect("thread");
        // Three passes' worth, then stop it: a loop never ends on its own.
        let mut events = Vec::new();
        while events.len() < 6 {
            events.push(rx.recv_timeout(Duration::from_secs(5)).expect("event"));
        }
        ctl_tx.send(CaptureCtl::Shutdown).expect("ctl open");
        assert_eq!(handle.join().expect("no panic"), ExitReason::Shutdown);

        let waits = clock.waits();
        assert_eq!(&waits[..6], &[0, 120, 120, 240, 240, 360]);
        assert!(progress.passes() >= 2, "passes are counted");
        assert!(
            !progress.finished(),
            "a looping replay never reports itself finished"
        );
    }

    /// A recording that ends with a key still down must not loop with that
    /// button stuck: the wrap releases it first.
    ///
    /// Breaks against a loop that simply starts over — the pad would hold the
    /// button for the rest of the night, on a machine nobody is standing at.
    #[test]
    fn a_key_left_down_is_released_before_the_loop_wraps() {
        let text = format!(
            "{}\n{}\n",
            line(0, IPAC, "A", true),
            line(50, IPAC, "B", true)
        );
        let clock = VirtualClock::new();
        let backend = ReplayBackend::new(Recording::parse(&text).expect("parses"))
            .looping(true)
            .with_clock(Box::new(clock.clone()));

        let (tx, rx) = crossbeam_channel::bounded(1024);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(backend).run(tx, ctl_rx).expect("thread");
        let mut events = Vec::new();
        while events.len() < 4 {
            events.push(rx.recv_timeout(Duration::from_secs(5)).expect("event"));
        }
        ctl_tx.send(CaptureCtl::Shutdown).expect("ctl open");
        assert_eq!(handle.join().expect("no panic"), ExitReason::Shutdown);

        assert!(events[0].down && events[1].down, "both presses");
        assert!(
            !events[2].down && !events[3].down,
            "both keys are released at the wrap before anything is pressed again: {:?}",
            events.iter().map(|e| (e.key, e.down)).collect::<Vec<_>>()
        );
        let released: Vec<Key> = events[2..4].iter().map(|e| e.key).collect();
        assert!(released.contains(&Key::A) && released.contains(&Key::B));
    }

    /// **The escape hatch is a thing a person does.** On an I-PAC, P1 Button 1
    /// IS LeftCtrl, so any recorded four-player session contains `LeftCtrl ×5`
    /// many times over. A replay that evaluated escapes would free the keyboards
    /// and stop the session, mid-attract-loop, because of something a player did
    /// ten minutes ago.
    ///
    /// Breaks against a `ReplayBackend` that runs an `EscapeWatch` over its own
    /// event stream, the way every real backend does.
    #[test]
    fn a_recorded_left_ctrl_gesture_does_not_fire_the_escape_hatch() {
        let mut text = String::new();
        for i in 0..5 {
            text.push_str(&line(i * 10, IPAC, "LeftControl", true));
            text.push('\n');
            text.push_str(&line(i * 10 + 5, IPAC, "LeftControl", false));
            text.push('\n');
        }
        let backend = ReplayBackend::new(Recording::parse(&text).expect("parses"))
            .with_clock(Box::new(VirtualClock::new()));
        let escapes = backend.escapes();
        let (events, _) = drive(backend);

        assert_eq!(events.len(), 10, "every stroke still reaches the engine");
        let snapshot = escapes.snapshot();
        assert_eq!(
            snapshot.toggles, 0,
            "a replayed gesture must not toggle capture"
        );
        assert!(
            !snapshot.passthrough,
            "...and must not release the keyboards"
        );
    }

    #[test]
    fn a_finished_replay_stays_alive_and_says_so() {
        let backend = ReplayBackend::new(Recording::parse(&two_line_recording()).expect("parses"))
            .with_clock(Box::new(VirtualClock::new()));
        let progress = backend.progress();
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let handle = Box::new(backend).run(tx, ctl_rx).expect("thread");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !progress.finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(progress.finished(), "the recording ran out");
        assert_eq!(progress.events(), 2);
        assert_eq!(progress.passes(), 1);
        assert!(
            !handle.is_finished(),
            "the capture thread must NOT exit on its own — the supervisor reads that \
             as the capture path dying and reports a runtime failure"
        );

        ctl_tx.send(CaptureCtl::Shutdown).expect("ctl open");
        assert_eq!(handle.join().expect("no panic"), ExitReason::Shutdown);
        drop(rx);
    }

    #[test]
    fn a_dropped_receiver_ends_the_thread() {
        let backend = ReplayBackend::new(Recording::parse(&two_line_recording()).expect("parses"))
            .with_clock(Box::new(VirtualClock::new()));
        let (tx, rx) = crossbeam_channel::bounded(4);
        let (_ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<CaptureCtl>();
        drop(rx);
        let handle = Box::new(backend).run(tx, ctl_rx).expect("thread");
        assert_eq!(handle.join().expect("no panic"), ExitReason::ChannelClosed);
    }

    /// The whole point of [`Silenced`]: the child is captured exactly as a live
    /// session would capture it — so its strokes never reach Windows — and none
    /// of its events reach the engine.
    ///
    /// Breaks against wiring the real backend in unwrapped, which is the version
    /// where the player at the panel fights the recording.
    #[test]
    fn a_silenced_backend_still_swallows_strokes_but_reports_nothing() {
        use crate::keymap::{KEY_DOWN, KEY_UP};
        use crate::mock::{MockCaptureBackend, MockStroke};

        let devices = vec![DeviceInfo {
            id: DeviceId::from(IPAC),
            interception_slot: Some(1),
            friendly: None,
            kind: DeviceKind::Keyboard,
        }];
        let script = vec![
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_UP,
            },
        ];
        let child = MockCaptureBackend::new(devices, script).with_pace(Duration::from_millis(5));
        let resent = child.resent_log();
        let mut silenced = Silenced::new(Box::new(child));

        assert!(
            silenced.devices().is_empty(),
            "the replay beside it reports the devices; two entries for one id is what \
             the duplicate-hardware-id refusal is for"
        );
        assert!(!silenced.presence().is_supported());

        let (tx, rx) = crossbeam_channel::bounded::<KeyEvent>(16);
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        ctl_tx
            .send(CaptureCtl::SetCaptured(vec![DeviceId::from(IPAC)]))
            .expect("ctl open");
        let handle = Box::new(silenced).run(tx, ctl_rx).expect("thread");

        // Give the child time to run its whole script.
        let deadline = Instant::now() + Duration::from_secs(5);
        while resent.lock().expect("log").is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        ctl_tx.send(CaptureCtl::Shutdown).expect("ctl open");
        assert_eq!(handle.join().expect("no panic"), ExitReason::Shutdown);

        assert!(
            resent.lock().expect("log").is_empty(),
            "a captured board must not type into Windows while a recording plays"
        );
        assert!(
            rx.try_recv().is_err(),
            "and not one of its events may reach the engine"
        );
    }
}
