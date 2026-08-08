//! **The live fan-out sink: one lossy stream, many consumers.**
//!
//! [`ksx_api::live`] is the *shape*; this is the plumbing. One sink is created
//! per daemon and lives for the daemon's whole life; each session publishes
//! into it while it runs; each surface subscribes to it and pulls at display
//! rate.
//!
//! ```text
//! [engine thread] ── LiveSink::key / ::pad ──┐
//! [output thread] ── LiveSink::feedback ─────┤  bounded, lossy, try_send
//!                                            ▼
//!                        [ArcSwap<Vec<Channel>>]  ← subscribe/unsubscribe (cold)
//!                                            │
//!                     ┌──────────────────────┼───────────────────────┐
//!                     ▼                      ▼                       ▼
//!             cabinet button check     Studio live socket        E8 light bus
//!             (LiveSubscription)          (later)                  (later)
//! ```
//!
//! # The three properties, and how each one is structural rather than promised
//!
//! **1. Nobody subscribed costs nothing measurable.** Every publish opens with
//! one `Relaxed` load of an `AtomicUsize` and returns. No `ArcSwap::load`, no
//! branch on a `Vec`, no allocation — the whole call is a load, a compare and a
//! ret, and it inlines. That is what lets the tap sit in the engine thread's
//! delta funnel at all: a cabinet with no window open runs exactly the pipeline
//! it ran before this module existed.
//!
//! **2. A slow consumer can never backpressure the engine.** Delivery is
//! `try_send` on a BOUNDED channel. Full means the event is dropped and a
//! counter moves; the counter is reported to that consumer in its next frame,
//! so loss is visible rather than silent. Same rule as the M4 delta coalescing
//! and E7's "a slow browser can never backpressure the engine".
//!
//! **3. There is no callback anywhere.** The sink holds `Sender`s, never
//! closures. A wedged UI thread wedges itself; the pipeline notices nothing
//! but a full queue. This is the CONTROL-SURFACE invariant that the tray, the
//! pipe and now this all satisfy the same way — the legacy WPF app died of the
//! opposite arrangement.
//!
//! # Coalescing is the CONSUMER's job, on purpose
//!
//! The producer publishes every transition; [`LiveSubscription::poll`] folds
//! whatever has arrived into one [`LiveFrame`]. Doing it the other way round —
//! sampling the pipeline at 60 Hz — would silently drop a button tap shorter
//! than 16 ms, and "I pressed it and nothing lit" is the exact sentence the
//! button check exists to answer. Instead every intermediate state is folded
//! into [`ksx_api::SlotLive::hit`], so a 4 ms tap is invisible in `down` and
//! unmistakable in `hit`.

// **This used to say "in a build with no UI feature there is nothing that can
// call `subscribe`". That stopped being true when the live pipe landed**:
// `crate::daemon::live_pipe` subscribes per connection and is compiled into
// EVERY Windows build, feature flags or not, because its consumer (Studio, the
// E8 bus) is a separate process rather than a linked window.
//
// What is left is the NON-WINDOWS build, where there is no named pipe to serve
// and no egui window either, so the fold and the frame builder really are
// unreachable. They are still compiled there, and that is deliberate: the
// producer half must be byte-identical in every build — a feature flag that
// changes pipeline code is a feature flag that changes what CI tested — so the
// unreachability is stated here rather than papered over with a `cfg` that
// would fork the file.
#![cfg_attr(not(any(test, windows, feature = "cabinet")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use ksx_api::{KeyHit, LiveFeed, LiveFrame, PadFeedback, SlotLive};
use ksx_core::{
    Axis, Binding, DeviceId, DeviceSelector, DpadDirection, Key, PadState, Trigger, XButton,
    XButtons,
};

/// How many events one subscriber may fall behind by before it starts losing
/// them.
///
/// Sized for a stall, not for throughput: at the ~50 events a second a human
/// panel produces, 1024 is twenty seconds of arrears — far longer than any
/// paint hitch — and it is 16 KB of queue, which a cabinet will not notice.
/// A consumer that is further behind than that has stopped painting, and the
/// honest answer for it is a dropped-frame count rather than a growing queue.
const QUEUE: usize = 1024;

/// Newest keys kept in one frame. A frame is ~16 ms; a human cannot produce
/// 64 keystrokes in one, so this only ever truncates after a stall — and then
/// it keeps the NEWEST, which is what somebody staring at the panel is
/// pressing now.
const KEYS_PER_FRAME: usize = 64;

/// Axis deflection past which an analogue stick counts as "moved" for the
/// button check. About 25% — beyond any resting jitter, well inside any game's
/// deadzone, so a stick that is actually being pushed always registers.
const AXIS_LIVE: i16 = 8_000;

/// One thing that happened in the pipeline.
///
/// Deliberately small and mostly `Copy`: the pad half is 12 bytes moved into a
/// queue, and the key half is a `DeviceId` clone.
///
/// That clone is the only allocation this module adds to any pipeline thread.
/// It happens on the ENGINE thread (never the capture thread, whose
/// allocation-free rule is absolute) and only while a surface is actually
/// subscribed — and it happens **once per keystroke per subscriber, plus one**:
/// [`LiveSink::key`] clones the id into the event, and [`LiveSink::publish`]
/// clones the event again for each queue. With the one subscriber this surface
/// has that is two small allocations per keystroke, at the ~50 keystrokes a
/// second a human panel produces, on a thread whose budget is not the capture
/// thread's. Worth stating precisely rather than rounding to "one": a reader
/// checking the hot path against ARCHITECTURE rule 5 should not have to count
/// them for themselves.
#[derive(Clone, Debug)]
enum LiveEvent {
    /// What the PANEL sent, as the engine received it.
    Key {
        device: DeviceId,
        key: Key,
        down: bool,
    },
    /// What the PAD published — one slot's whole state after a transition.
    Pad { slot: u8, state: PadState },
    /// What a GAME sent back to a pad (E8's stream).
    Feedback(PadFeedback),
}

/// One subscriber's queue.
struct Channel {
    id: u64,
    tx: Sender<LiveEvent>,
    /// Events this consumer lost. Read (and cleared) by the consumer itself.
    dropped: Arc<AtomicU64>,
}

#[derive(Default)]
struct Inner {
    /// The fast gate. Kept apart from `channels` so the common case — nobody
    /// watching — is one relaxed load with no pointer chase at all.
    subscribers: AtomicUsize,
    /// The senders, replaced wholesale on subscribe/unsubscribe. Same
    /// arc-swap-a-snapshot shape `ksx-capture` uses for its hot decision
    /// table, and for the same reason: the reader never takes a lock.
    channels: ArcSwap<Vec<Channel>>,
    /// A session is running right now.
    running: AtomicBool,
    /// **The panel**: the devices bound to a slot in the running session
    /// (`RunPlan::captureable`). Empty between sessions.
    ///
    /// Published by the supervisor in the same breath as `running`, so the two
    /// cannot drift, and read on the engine thread through the same
    /// arc-swapped-snapshot shape as `channels`.
    panel: ArcSwap<Vec<DeviceId>>,
    /// Keys dropped by [`LiveSink::key`] because they came from a device bound
    /// to no slot. Monotonic for the sink's life; each subscription reports its
    /// own delta, which is what lets one counter serve every consumer.
    off_panel: AtomicU64,
    next_id: AtomicU64,
}

/// The publish side. Cloneable, cheap, and held by the daemon for its whole
/// life — sessions come and go through it.
#[derive(Clone, Default)]
pub struct LiveSink {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for LiveSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSink")
            .field("subscribers", &self.subscriber_count())
            .field("running", &self.inner.running.load(Ordering::Relaxed))
            .finish()
    }
}

impl LiveSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// **The gate.** `false` is the common case and the whole reason this
    /// module is free to exist inside the engine thread.
    #[inline]
    pub fn is_live(&self) -> bool {
        self.inner.subscribers.load(Ordering::Relaxed) != 0
    }

    /// How many surfaces are watching. Diagnostics only.
    pub fn subscriber_count(&self) -> usize {
        self.inner.subscribers.load(Ordering::Relaxed)
    }

    /// A session started, driving exactly `panel`. Called by the supervisor,
    /// off the hot path.
    ///
    /// One call rather than two, because the two facts have to move together:
    /// a consumer can tell "nothing is pressed" from "nothing is running" only
    /// if `running` is true, and [`Self::key`] can tell the panel from the desk
    /// only if `panel` is the CURRENT session's bound set. A `set_running(true)`
    /// that forgot to update the panel would filter this session's keys against
    /// the previous session's devices, which is a worse bug than the one this
    /// replaces.
    pub fn session_started(&self, panel: &[DeviceId]) {
        self.inner.panel.store(Arc::new(panel.to_vec()));
        self.inner.running.store(true, Ordering::Relaxed);
    }

    /// The session ended: nothing is running and nothing is the panel.
    pub fn session_ended(&self) {
        self.inner.running.store(false, Ordering::Relaxed);
        self.inner.panel.store(Arc::new(Vec::new()));
    }

    /// What the PANEL sent. Engine thread.
    ///
    /// # Why this FILTERS rather than labels
    ///
    /// Interception forwards every keystroke it does not block, and the engine
    /// sees all of them — so before this filter existed, typing on a desk
    /// keyboard bound to no slot lit the cabinet's button check exactly as
    /// convincingly as pressing the panel did. A screen whose entire purpose is
    /// *"what did THE PANEL send"* answered a different question, and answered
    /// it wrong.
    ///
    /// The alternative was to keep every key and label it with its device.
    /// Rejected, for three reasons:
    ///
    /// 1. **The failure mode looked right.** The whole cost of the bug was that
    ///    a false positive is indistinguishable from a pass. A label makes the
    ///    truth *available*; filtering makes the wrong answer *unreachable*.
    ///    Only the second one survives being read at six feet by somebody
    ///    holding a screwdriver.
    /// 2. **A label is small text on a 10-foot surface.** The button check's
    ///    vocabulary is a big key name and a lit control; a device column would
    ///    be the one thing on the screen nobody standing at the cabinet can
    ///    read.
    /// 3. **An unbound board can crowd out the panel.** Delivery is a bounded
    ///    queue, so somebody typing an email next to the cabinet could push the
    ///    panel's own keys past [`QUEUE`] and into the dropped counter.
    ///    Filtering here — before the clone, before the fan-out — means an
    ///    unbound device cannot consume a subscriber's arrears at all.
    ///
    /// Nothing is hidden by it: `KeyHit` still carries `device` and `alias`, so
    /// two *bound* panels remain two facts, and what was left out is counted
    /// into [`ksx_api::LiveFrame::off_panel`] and said in words on screen.
    #[inline]
    pub fn key(&self, device: &DeviceId, key: Key, down: bool) {
        if !self.is_live() {
            return;
        }
        if !self.inner.panel.load().iter().any(|bound| bound == device) {
            // Presses only, so one keystroke is one count rather than two.
            if down {
                self.inner.off_panel.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        self.publish(LiveEvent::Key {
            device: device.clone(),
            key,
            down,
        });
    }

    /// What the PAD published. Engine thread, in the delta funnel — every
    /// transition, before any coalescing, so a tap cannot be lost here.
    #[inline]
    pub fn pad(&self, slot: u8, state: PadState) {
        if !self.is_live() {
            return;
        }
        self.publish(LiveEvent::Pad { slot, state });
    }

    /// What a GAME sent back to a pad. Output thread (E8).
    #[inline]
    pub fn feedback(&self, feedback: PadFeedback) {
        if !self.is_live() {
            return;
        }
        self.publish(LiveEvent::Feedback(feedback));
    }

    /// The cold half of a publish: fan out to every queue, dropping into any
    /// that is full. Never blocks, never allocates beyond the event itself.
    fn publish(&self, event: LiveEvent) {
        let channels = self.inner.channels.load();
        // The last consumer can unsubscribe between the gate and here. That is
        // not a race worth closing — an event delivered to nobody costs a
        // `Vec` walk of length zero.
        for channel in channels.iter() {
            if let Err(TrySendError::Full(_)) = channel.tx.try_send(event.clone()) {
                channel.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Start watching. The subscription unregisters itself on drop, so a
    /// closed window really does put the pipeline back to paying nothing.
    pub fn subscribe(&self) -> LiveSubscription {
        let (tx, rx) = bounded(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner.channels.rcu(|current| {
            let mut next: Vec<Channel> = current
                .iter()
                .map(|c| Channel {
                    id: c.id,
                    tx: c.tx.clone(),
                    dropped: c.dropped.clone(),
                })
                .collect();
            next.push(Channel {
                id,
                tx: tx.clone(),
                dropped: dropped.clone(),
            });
            next
        });
        // AFTER the channel is in place: a publisher that sees the gate open
        // must find somewhere to publish to.
        self.inner.subscribers.fetch_add(1, Ordering::Release);
        LiveSubscription {
            sink: self.clone(),
            id,
            rx,
            dropped,
            // From HERE, not from zero: whatever an unbound keyboard did before
            // this window opened is not this window's business.
            off_panel_seen: self.inner.off_panel.load(Ordering::Relaxed),
            aliases: Vec::new(),
        }
    }

    fn unsubscribe(&self, id: u64) {
        self.inner.channels.rcu(|current| {
            current
                .iter()
                .filter(|c| c.id != id)
                .map(|c| Channel {
                    id: c.id,
                    tx: c.tx.clone(),
                    dropped: c.dropped.clone(),
                })
                .collect::<Vec<Channel>>()
        });
        self.inner.subscribers.fetch_sub(1, Ordering::Release);
    }
}

/// One surface's view of the stream. Not `Clone`: a subscription is a queue,
/// and two owners of one queue would each see half the events.
pub struct LiveSubscription {
    sink: LiveSink,
    id: u64,
    rx: Receiver<LiveEvent>,
    dropped: Arc<AtomicU64>,
    /// Watermark into [`Inner::off_panel`]. The counter is per-sink and
    /// monotonic; the delta since the last poll is this consumer's share.
    off_panel_seen: u64,
    /// `[[device]] id` → friendly name, supplied by the consumer (which can
    /// read the config; the engine thread cannot and must not).
    ///
    /// Parsed once here rather than per key hit: [`Self::alias_for`] runs for
    /// every key in every frame.
    aliases: Vec<(String, Option<DeviceSelector>, String)>,
}

impl LiveSubscription {
    /// Teach this subscription the cabinet's device names, so a key hit says
    /// "IPAC P1" rather than a 60-character instance path. Re-settable: a
    /// surface refreshes it whenever it re-reads the config.
    pub fn set_aliases(&mut self, aliases: Vec<(String, String)>) {
        self.aliases = aliases
            .into_iter()
            .map(|(id, alias)| (id.clone(), DeviceSelector::parse(&id).ok(), alias))
            .collect();
    }

    /// The name to print for the device an event arrived from.
    ///
    /// **Two comparisons, because config and the live feed no longer speak the
    /// same spelling.** A `[[device]] id` is a selector now, resolved once at
    /// session start; what arrives here is the concrete interface it resolved
    /// to. So `id = 'usb:d209:0430:00'` never equals the id on the wire, and
    /// without the second comparison the Button-Check screen the operator is
    /// standing in front of shows unnamed devices — the whole screen's job,
    /// undone by a config edit that was supposed to be an improvement.
    ///
    /// The selector match is display-grade on purpose: it is fed
    /// [`DeviceFacts::from_instance_path`], which cannot know a serial, so a
    /// `sn=` selector falls through to no name rather than risking one twin's
    /// label on the other twin's keystrokes.
    fn alias_for(&self, device: &str) -> String {
        if let Some((_, _, alias)) = self.aliases.iter().find(|(id, _, _)| id == device) {
            return alias.clone();
        }
        let facts = ksx_core::DeviceFacts::from_instance_path(device);
        self.aliases
            .iter()
            .find(|(_, selector, _)| match (selector, &facts) {
                (Some(selector), Some(facts)) => selector.matches(facts),
                _ => false,
            })
            .map(|(_, _, alias)| alias.clone())
            .unwrap_or_default()
    }
}

impl Drop for LiveSubscription {
    fn drop(&mut self) {
        self.sink.unsubscribe(self.id);
    }
}

impl LiveFeed for LiveSubscription {
    /// Everything queued since the last call, folded into one frame.
    ///
    /// Drains without blocking: whatever is there is the frame, and an empty
    /// queue is an honest "nothing happened since you last looked" rather than
    /// a wait.
    fn poll(&mut self) -> LiveFrame {
        let mut fold = Fold::default();
        while let Ok(event) = self.rx.try_recv() {
            fold.take(event);
        }
        let mut frame = fold.finish(&self.sink);
        frame.dropped = self.dropped.swap(0, Ordering::Relaxed);
        let off_panel = self.sink.inner.off_panel.load(Ordering::Relaxed);
        frame.off_panel = off_panel.saturating_sub(self.off_panel_seen);
        self.off_panel_seen = off_panel;
        for hit in &mut frame.keys {
            hit.alias = self.alias_for(&hit.device);
        }
        frame
    }

    fn unavailable(&self) -> Option<String> {
        if self.sink.inner.running.load(Ordering::Relaxed) {
            None
        } else {
            Some(
                "no session is running — start emulation and the panel's keys will show here"
                    .to_owned(),
            )
        }
    }
}

/// The consumer-side coalescer: many transitions in, one frame out.
#[derive(Default)]
struct Fold {
    slots: Vec<SlotFold>,
    keys: Vec<KeyHit>,
    feedback: Vec<PadFeedback>,
}

/// One slot's fold: the latest state, plus the union of everything seen.
struct SlotFold {
    slot: u8,
    latest: PadState,
    /// Every button bit that was set in ANY state since the last frame.
    hit_buttons: XButtons,
    hit_lt: bool,
    hit_rt: bool,
    /// Per-axis "was deflected at some point", in `lx ly rx ry` order.
    hit_axes: [bool; 4],
}

impl Fold {
    fn take(&mut self, event: LiveEvent) {
        match event {
            LiveEvent::Key { device, key, down } => {
                if self.keys.len() == KEYS_PER_FRAME {
                    // Keep the NEWEST: after a stall, what is being pressed now
                    // is the useful half.
                    self.keys.remove(0);
                }
                self.keys.push(KeyHit {
                    key: key.name().to_owned(),
                    device: device.as_str().to_owned(),
                    alias: String::new(),
                    down,
                });
            }
            LiveEvent::Pad { slot, state } => {
                let entry = match self.slots.iter_mut().find(|s| s.slot == slot) {
                    Some(entry) => entry,
                    None => {
                        self.slots.push(SlotFold {
                            slot,
                            latest: state,
                            hit_buttons: XButtons::empty(),
                            hit_lt: false,
                            hit_rt: false,
                            hit_axes: [false; 4],
                        });
                        self.slots.last_mut().expect("just pushed")
                    }
                };
                entry.latest = state;
                entry.hit_buttons |= state.buttons;
                entry.hit_lt |= state.lt > 0;
                entry.hit_rt |= state.rt > 0;
                for (seen, value) in entry
                    .hit_axes
                    .iter_mut()
                    .zip([state.lx, state.ly, state.rx, state.ry])
                {
                    *seen |= value.saturating_abs() >= AXIS_LIVE;
                }
            }
            LiveEvent::Feedback(feedback) => self.feedback.push(feedback),
        }
    }

    fn finish(mut self, sink: &LiveSink) -> LiveFrame {
        self.slots.sort_by_key(|s| s.slot);
        LiveFrame {
            running: sink.inner.running.load(Ordering::Relaxed),
            slots: self
                .slots
                .iter()
                .map(|fold| SlotLive {
                    slot: fold.slot,
                    down: controls_down(&fold.latest),
                    hit: controls_hit(fold),
                    lt: fold.latest.lt,
                    rt: fold.latest.rt,
                    lx: fold.latest.lx,
                    ly: fold.latest.ly,
                    rx: fold.latest.rx,
                    ry: fold.latest.ry,
                })
                .collect(),
            keys: self.keys,
            feedback: self.feedback,
            dropped: 0,
            off_panel: 0,
        }
    }
}

/// `[[device]]` id → alias, so a key hit says "IPAC P1" and not a
/// 60-character instance path.
///
/// Read on the CONSUMER's own thread — the cabinet window's, or a live-pipe
/// viewer's — because the engine thread that publishes those hits must not
/// read a config file. The fan-out carries the instance path; the consumer
/// names it ([`LiveSubscription::set_aliases`]).
///
/// The id handed over is the **written** spelling, not a resolved one: a
/// window thread has no business enumerating USB, and it does not have to.
/// [`LiveSubscription::alias_for`] matches a `usb:` selector against the
/// concrete id on the wire, which is the whole reason it is selector-aware.
///
/// Lives here rather than in `crate::cabinet` (where it began) because it is
/// not the cabinet's: the live pipe reads it per connection, and that server
/// is compiled into **every** build — a default `ksx daemon` with neither UI
/// feature still serves the feed, because its consumer is another process.
pub fn device_aliases(root: &ksx_config::ConfigRoot) -> Vec<(String, String)> {
    ksx_config::Store::new(root.clone())
        .load_config()
        .map(|loaded| {
            loaded
                .value
                .devices
                .iter()
                .map(|device| (device.id.raw().to_owned(), device.alias.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The four axes, in the order [`SlotFold::hit_axes`] holds them.
const AXES: [Axis; 4] = [Axis::X, Axis::Y, Axis::Rx, Axis::Ry];

/// Canonical control names held in this state.
///
/// Names come from [`ksx_config::function::function_name`] — the SAME function
/// that writes a preset file's `[bindings]` keys — so a live light and a legend
/// row are guaranteed to spell one control one way.
fn controls_down(state: &PadState) -> Vec<String> {
    let mut names = Vec::new();
    push_buttons(&mut names, state.buttons);
    if state.lt > 0 {
        names.push(name(&Binding::Trigger(Trigger::Left)));
    }
    if state.rt > 0 {
        names.push(name(&Binding::Trigger(Trigger::Right)));
    }
    for (axis, value) in AXES
        .iter()
        .zip([state.lx, state.ly, state.rx, state.ry])
        .filter(|(_, value)| value.saturating_abs() >= AXIS_LIVE)
    {
        names.push(axis_name(*axis, value));
    }
    names
}

/// Everything that was down at ANY point since the last frame.
fn controls_hit(fold: &SlotFold) -> Vec<String> {
    let mut names = Vec::new();
    push_buttons(&mut names, fold.hit_buttons);
    if fold.hit_lt {
        names.push(name(&Binding::Trigger(Trigger::Left)));
    }
    if fold.hit_rt {
        names.push(name(&Binding::Trigger(Trigger::Right)));
    }
    for (axis, _) in AXES.iter().zip(fold.hit_axes).filter(|(_, seen)| *seen) {
        names.push(axis_base_name(*axis));
    }
    names
}

fn push_buttons(names: &mut Vec<String>, buttons: XButtons) {
    for (bit, direction) in [
        (XButtons::DPAD_UP, DpadDirection::Up),
        (XButtons::DPAD_DOWN, DpadDirection::Down),
        (XButtons::DPAD_LEFT, DpadDirection::Left),
        (XButtons::DPAD_RIGHT, DpadDirection::Right),
    ] {
        if buttons.contains(bit) {
            names.push(name(&Binding::Dpad(direction)));
        }
    }
    for button in XButton::ALL {
        if buttons.contains(XButtons::from_bits_truncate(*button as u16)) {
            names.push(name(&Binding::Button(*button)));
        }
    }
}

fn name(binding: &Binding) -> String {
    ksx_config::function::function_name(binding)
}

/// `lx.min` / `lx.max` / `lx.-16384` — the deflection AS A PRESET WOULD SPELL
/// IT, so a stick reading and a binding name are the same vocabulary.
fn axis_name(axis: Axis, value: i16) -> String {
    name(&Binding::Axis { axis, value })
}

/// The axis with no value — what "this stick moved at some point" is called.
fn axis_base_name(axis: Axis) -> String {
    let full = axis_name(axis, ksx_core::AXIS_MAX);
    full.split_once('.')
        .map_or(full.clone(), |(base, _)| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(buttons: XButtons) -> PadState {
        PadState {
            buttons,
            ..PadState::default()
        }
    }

    /// The property the whole design turns on: with nobody watching, a publish
    /// is a load and a return — nothing is queued, nothing is cloned, and a
    /// later subscriber sees none of it.
    #[test]
    fn nobody_subscribed_means_nothing_is_published_at_all() {
        let sink = LiveSink::new();
        assert!(!sink.is_live());
        for _ in 0..10_000 {
            sink.pad(1, pad(XButtons::A));
            sink.key(&DeviceId::new("dev"), Key::G, true);
        }
        // Subscribing now must not hand the new consumer ten thousand stale
        // events: they were never taken.
        let mut feed = sink.subscribe();
        assert!(sink.is_live());
        let frame = feed.poll();
        assert!(frame.slots.is_empty() && frame.keys.is_empty());
        assert_eq!(frame.dropped, 0);
    }

    /// ...and dropping the last subscription closes the gate again, so a closed
    /// window really does put the pipeline back where it started.
    #[test]
    fn dropping_the_last_subscription_closes_the_gate() {
        let sink = LiveSink::new();
        let first = sink.subscribe();
        let second = sink.subscribe();
        assert_eq!(sink.subscriber_count(), 2);
        drop(first);
        assert!(sink.is_live(), "one is still watching");
        drop(second);
        assert!(!sink.is_live());
        assert_eq!(sink.subscriber_count(), 0);
    }

    /// The two-column truth, in one frame: what the panel sent and what the pad
    /// published.
    #[test]
    fn one_frame_carries_both_columns_with_canonical_names() {
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        feed.set_aliases(vec![(r"HID\VID_D209".to_owned(), "IPAC P1".to_owned())]);
        sink.session_started(&[DeviceId::new(r"HID\VID_D209")]);

        sink.key(&DeviceId::new(r"HID\VID_D209"), Key::G, true);
        sink.pad(1, pad(XButtons::A | XButtons::DPAD_UP));

        let frame = feed.poll();
        assert!(frame.running);
        assert_eq!(frame.keys.len(), 1);
        assert_eq!(frame.keys[0].key, "G");
        assert_eq!(frame.keys[0].alias, "IPAC P1", "the consumer names devices");
        assert!(frame.keys[0].down);

        let slot = frame.slot(1).expect("slot 1 published");
        assert!(slot.is_down("A"), "{:?}", slot.down);
        assert!(slot.is_down("dpad.up"), "{:?}", slot.down);
        assert!(!slot.is_down("B"));
    }

    /// **The screen the operator is standing in front of.**
    ///
    /// Once `[[device]] id` became a selector, the id in config and the id on
    /// the live feed stopped being the same string: config says
    /// `usb:d209:0430:00`, the wire carries the concrete interface that
    /// resolved to. Match only on equality and Button-Check shows unnamed
    /// devices — the one job that screen has, undone by a config edit that was
    /// supposed to be an improvement.
    #[test]
    fn a_usb_selector_in_config_still_names_the_device_on_the_wire() {
        const LIVE: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
        const OTHER: &str = r"USB\VID_046D&PID_C077&MI_00\7&AAAA&0&0000";
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        feed.set_aliases(vec![("usb:d209:0430:00".to_owned(), "IPAC P1".to_owned())]);
        // The bound set the supervisor publishes is the RESOLVED one — which is
        // exactly why the alias table has to reach across the two spellings.
        sink.session_started(&[DeviceId::new(LIVE), DeviceId::new(OTHER)]);

        sink.key(&DeviceId::new(LIVE), Key::G, true);
        let frame = feed.poll();
        assert_eq!(frame.keys[0].alias, "IPAC P1");

        // A different board is still nameless rather than mislabelled.
        sink.key(&DeviceId::new(OTHER), Key::H, true);
        assert_eq!(feed.poll().keys[0].alias, "");
    }

    /// A serial cannot be read off a path, so a `sn=` entry falls through to no
    /// name. That is the right answer, not a gap: a `sn=` selector is only
    /// written when twins are connected, and matching it on VID/PID/MI alone
    /// would put one board's label on the other board's keystrokes.
    #[test]
    fn a_serial_rung_alias_declines_to_guess_which_twin_is_typing() {
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        const LIVE: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
        feed.set_aliases(vec![("usb:d209:0430:00:sn=4".to_owned(), "P1".to_owned())]);
        sink.session_started(&[DeviceId::new(LIVE)]);
        sink.key(&DeviceId::new(LIVE), Key::G, true);
        assert_eq!(feed.poll().keys[0].alias, "");
    }

    /// A press and release inside ONE display frame: invisible in `down`,
    /// unmistakable in `hit`. This is why the producer publishes transitions
    /// and the consumer coalesces, and not the other way round.
    #[test]
    fn a_press_and_release_between_two_polls_is_still_reported_as_a_hit() {
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        sink.pad(1, pad(XButtons::A));
        sink.pad(1, pad(XButtons::empty()));

        let slot = feed.poll().slot(1).cloned().expect("slot 1");
        assert!(slot.down.is_empty(), "it is already back up");
        assert!(slot.was_hit("A"), "but it definitely happened");

        // ...and the hit does not linger into the next frame.
        sink.pad(1, pad(XButtons::empty()));
        let slot = feed.poll().slot(1).cloned().expect("slot 1");
        assert!(!slot.was_hit("A"));
    }

    /// A consumer that stops polling loses events and is TOLD how many — it
    /// never backpressures the publisher, and the loss is never silent.
    #[test]
    fn a_stalled_consumer_loses_events_and_is_told_the_count() {
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        for _ in 0..(QUEUE + 25) {
            sink.pad(1, pad(XButtons::A));
        }
        let frame = feed.poll();
        assert_eq!(frame.dropped, 25, "everything past the queue was dropped");
        // The counter is per-frame, not cumulative.
        assert_eq!(feed.poll().dropped, 0);
    }

    /// Two surfaces watching the same pipeline each get the whole stream.
    #[test]
    fn every_subscriber_sees_every_event() {
        let sink = LiveSink::new();
        let mut cabinet = sink.subscribe();
        let mut other = sink.subscribe();
        sink.session_started(&[DeviceId::new("dev")]);
        sink.key(&DeviceId::new("dev"), Key::G, true);
        assert_eq!(cabinet.poll().keys.len(), 1);
        assert_eq!(other.poll().keys.len(), 1);
    }

    /// The E8 half of the bus: pad feedback a game sent, on the same stream.
    #[test]
    fn game_feedback_rides_the_same_stream() {
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        sink.feedback(PadFeedback {
            slot: 2,
            large_motor: 200,
            small_motor: 10,
            led_number: 2,
        });
        let frame = feed.poll();
        assert_eq!(frame.feedback.len(), 1);
        assert_eq!(frame.feedback[0].slot, 2);
        assert_eq!(frame.feedback[0].large_motor, 200);
    }

    /// With no session running the feed says so in words, rather than handing
    /// a surface an empty frame that looks like a dead panel.
    #[test]
    fn a_feed_with_no_session_says_why_it_is_empty() {
        let sink = LiveSink::new();
        let feed = sink.subscribe();
        assert!(feed.unavailable().is_some_and(|why| why.contains("start")));
        sink.session_started(&[DeviceId::new("dev")]);
        assert!(feed.unavailable().is_none());
        sink.session_ended();
        assert!(feed.unavailable().is_some(), "and back again when it ends");
    }

    /// **The desk keyboard.** A board bound to no slot lights nothing on a
    /// screen whose whole purpose is "what did THE PANEL send" — and what it
    /// sent is COUNTED rather than silently discarded, so "the panel is dead"
    /// and "you are pressing the wrong keyboard" are different sentences.
    #[test]
    fn keys_from_a_device_bound_to_no_slot_never_reach_the_button_check() {
        let panel = DeviceId::new(r"HID\VID_D209&PID_0430&MI_00");
        let desk = DeviceId::new(r"HID\VID_046D&PID_C52B");

        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        sink.session_started(std::slice::from_ref(&panel));

        sink.key(&desk, Key::E, true);
        sink.key(&desk, Key::E, false);
        sink.key(&panel, Key::G, true);

        let frame = feed.poll();
        assert_eq!(
            frame.keys.len(),
            1,
            "only the panel's key: {:?}",
            frame.keys
        );
        assert_eq!(frame.keys[0].key, "G");
        assert_eq!(frame.keys[0].device, panel.as_str());
        assert_eq!(
            frame.off_panel, 1,
            "one PRESS from an unbound board, reported not hidden"
        );
        // The count is per-frame, like `dropped`.
        assert_eq!(feed.poll().off_panel, 0);
    }

    /// ...and the panel set belongs to the SESSION. Between sessions nothing is
    /// the panel, so a window left open after a game exits cannot go on
    /// lighting up from the last session's board.
    #[test]
    fn the_panel_set_is_cleared_when_the_session_ends() {
        let panel = DeviceId::new("ipac");
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();

        // Before any session: the engine is not running, and nothing is bound.
        sink.key(&panel, Key::G, true);
        assert!(feed.poll().keys.is_empty());

        sink.session_started(std::slice::from_ref(&panel));
        sink.key(&panel, Key::G, true);
        assert_eq!(feed.poll().keys.len(), 1);

        sink.session_ended();
        sink.key(&panel, Key::G, true);
        let frame = feed.poll();
        assert!(frame.keys.is_empty(), "{:?}", frame.keys);
        assert!(!frame.running);
    }

    /// A second session with a different panel filters against ITS OWN bound
    /// set — the reason `session_started` carries both facts in one call.
    #[test]
    fn a_new_session_replaces_the_previous_panel_rather_than_adding_to_it() {
        let first = DeviceId::new("board-a");
        let second = DeviceId::new("board-b");
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();

        sink.session_started(std::slice::from_ref(&first));
        sink.session_ended();
        sink.session_started(std::slice::from_ref(&second));

        sink.key(&first, Key::G, true);
        sink.key(&second, Key::H, true);
        let frame = feed.poll();
        assert_eq!(frame.keys.len(), 1);
        assert_eq!(frame.keys[0].key, "H");
    }

    /// An unbound board cannot consume a subscriber's arrears: the filter is in
    /// front of the queue, not behind it. Somebody typing an email beside the
    /// cabinet must not push the panel's own keys into the dropped counter.
    #[test]
    fn an_unbound_board_cannot_crowd_the_panel_out_of_the_queue() {
        let panel = DeviceId::new("ipac");
        let desk = DeviceId::new("desk");
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        sink.session_started(std::slice::from_ref(&panel));

        for _ in 0..(QUEUE * 4) {
            sink.key(&desk, Key::E, true);
        }
        sink.key(&panel, Key::G, true);

        let frame = feed.poll();
        assert_eq!(frame.dropped, 0, "nothing of the panel's was lost");
        assert_eq!(frame.keys.len(), 1);
        assert_eq!(frame.keys[0].key, "G");
        assert_eq!(frame.off_panel, (QUEUE * 4) as u64);
    }

    /// Analogue readouts use the preset vocabulary, so a stick reading and a
    /// binding name are the same words.
    #[test]
    fn analogue_controls_are_named_the_way_a_preset_spells_them() {
        let sink = LiveSink::new();
        let mut feed = sink.subscribe();
        sink.pad(
            1,
            PadState {
                lt: 255,
                lx: ksx_core::AXIS_MAX,
                ..PadState::default()
            },
        );
        let slot = feed.poll().slot(1).cloned().expect("slot 1");
        assert!(slot.is_down("lt"), "{:?}", slot.down);
        assert!(slot.is_down("lx.max"), "{:?}", slot.down);
        // The HIT column names the axis without its deflection: "this stick
        // moved" is the fact, and which way it went is `down`'s job.
        assert!(slot.was_hit("lx"), "{:?}", slot.hit);
    }
}
