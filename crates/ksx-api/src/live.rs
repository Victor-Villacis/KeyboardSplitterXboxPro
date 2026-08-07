//! **The live fan-out: what a surface sees of a running pipeline.**
//!
//! This module is the *shape* of the stream. The sink that fills it lives in
//! `ksx-app` (`crate::feed`), because it owns the pipeline; the shape lives
//! here so that every consumer names one set of types — the cabinet's button
//! check today, Studio's live socket and E8's feedback bus next
//! (docs/ENHANCEMENTS.md E8: "the feedback consumer is a BUS, not a lamp
//! driver"; docs/MAPPER-UX.md Build C: "same socket feeds the E8 light bus and
//! the 3D viewer later — one stream, three consumers").
//!
//! # The three rules this stream is built on
//!
//! 1. **Lossy, always.** A consumer that cannot keep up loses frames and is
//!    told how many. It can never backpressure the engine — the same rule as
//!    the M4 delta coalescing and E7's "a slow browser can never backpressure
//!    the engine".
//! 2. **Display-rate coalesced (~60 Hz), and coalesced by the CONSUMER.** The
//!    producer publishes transitions; [`LiveFrame`] is what a consumer folds
//!    them into when it wakes to paint. That ordering matters: a producer that
//!    sampled at 60 Hz would *miss* a 8 ms button tap, and the one thing a
//!    button check may never do is fail to notice a press.
//! 3. **Never a callback into a pipeline thread.** There is no closure
//!    anywhere in this stream. The pipeline pushes into a bounded queue and
//!    walks away; every consumer runs on its own thread and pulls. A wedged
//!    UI costs a window, never a keyboard (docs/CONTROL-SURFACE.md
//!    "Invariants a GUI must not break").
//!
//! # What it is for
//!
//! The two-column truth of the button check: **what the panel sent** beside
//! **what the pad published**, per slot. Those are different facts and the gap
//! between them is the whole diagnosis — a key that arrives but lights no
//! control is an unbound key; a control that lights with no key behind it is a
//! macro or a chord; neither column moving is a wiring fault upstream of ksx.

use serde::{Deserialize, Serialize};

/// One coalesced picture of the running pipeline, as of the instant a consumer
/// asked for it.
///
/// Serializable so that Studio's future live socket carries exactly this and
/// not a fourth description of the same facts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveFrame {
    /// A session is running and this frame describes it. `false` means the
    /// engine is not there — which a surface must SAY ("start emulation to
    /// check buttons") rather than render as a panel where nothing happens.
    pub running: bool,
    /// Per slot, newest state first published. One entry per plugged pad.
    pub slots: Vec<SlotLive>,
    /// Keys the PANEL sent since the last frame, newest last, capped.
    pub keys: Vec<KeyHit>,
    /// Feedback the GAMES sent back to the pads since the last frame (rumble,
    /// LED slot). Empty on a PlayStation persona, always: ViGEmBus has no DS4
    /// notification IOCTL and never will (docs/ENHANCEMENTS.md E8 §1).
    pub feedback: Vec<PadFeedback>,
    /// Frames the SINK dropped because this consumer was too slow, since the
    /// last poll. Reported rather than hidden: a button check that silently
    /// skipped the press you just made is worse than one that says it did.
    pub dropped: u64,
    /// Keys from a device **bound to no slot** that were left out of
    /// [`Self::keys`] since the last poll.
    ///
    /// Not a fault and not an error — a desk keyboard beside the cabinet is
    /// allowed to type. It is here so that "the panel is dead" and "you are
    /// pressing the wrong keyboard" are different sentences on screen instead
    /// of the same empty column.
    pub off_panel: u64,
}

impl LiveFrame {
    /// The frame that means "there is no pipeline to watch". `reason` is what
    /// a surface prints where the columns would be.
    pub fn idle() -> Self {
        Self::default()
    }

    /// The slot's live state, if this frame has one.
    pub fn slot(&self, number: u8) -> Option<&SlotLive> {
        self.slots.iter().find(|slot| slot.slot == number)
    }
}

/// One slot's published pad state, plus what it has done recently.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotLive {
    /// Slot NUMBER (1..=8), never an XInput user index.
    pub slot: u8,
    /// Canonical control names currently held — `"A"`, `"dpad.up"`, `"lt"`.
    /// The vocabulary presets are written in, so a legend and a live light
    /// name the same thing.
    pub down: Vec<String>,
    /// Controls that went down at ANY point since the last frame, even if they
    /// are already back up.
    ///
    /// This is the field that makes 60 Hz honest. A tap shorter than a frame
    /// is invisible in `down` and unmistakable here, so "I pressed it and
    /// nothing lit" can only ever mean the key really did not arrive.
    pub hit: Vec<String>,
    /// Analogue readouts, as they went on the wire.
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

impl SlotLive {
    /// Is this control down right now?
    pub fn is_down(&self, control: &str) -> bool {
        self.down.iter().any(|c| c == control)
    }

    /// Did it go down since the last frame (even if it is up again)?
    pub fn was_hit(&self, control: &str) -> bool {
        self.hit.iter().any(|c| c == control)
    }
}

/// One key the panel sent, as the engine received it.
///
/// "As the engine received it" is the honest scope and worth stating: these are
/// the keys the capture backend FORWARDED. A key blocked upstream, or one from
/// a device bound to no slot, is not here — and that absence is itself the
/// finding when a panel is rewired. (For most of M9 the second half of that
/// sentence was aspirational: Interception forwards every keyboard it does not
/// block, so an unbound desk keyboard lit the button check as convincingly as
/// the panel did. The sink filters to the session's bound devices now, and
/// counts what it left out into [`LiveFrame::off_panel`].)
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyHit {
    /// The key name a preset would spell it with (`"G"`, `"LeftCtrl"`).
    pub key: String,
    /// Device instance path — which board it came from, so two panels sending
    /// the same key are still two facts.
    pub device: String,
    /// A friendly name for the device when the config gives one, else empty.
    pub alias: String,
    /// `true` = press.
    pub down: bool,
}

/// One feedback packet a GAME sent to one pad (E8's stream).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadFeedback {
    pub slot: u8,
    pub large_motor: u8,
    pub small_motor: u8,
    /// The XInput LED slot the bus announced — the player number a game thinks
    /// this pad is.
    pub led_number: u8,
}

/// A subscription to the live fan-out.
///
/// One method, and it is a PULL: the consumer decides when it can afford a
/// frame, which is what keeps the producer free of any knowledge of consumers.
/// Implementations coalesce everything queued since the last call, so calling
/// it at display rate is the intended use and calling it slowly is merely
/// lossy, never wrong.
pub trait LiveFeed: Send {
    /// Everything that happened since the previous call, folded into one
    /// frame.
    fn poll(&mut self) -> LiveFrame;

    /// Why this feed has nothing to show, when it has nothing to show —
    /// rendered in place of the columns. `None` means "the frame is the whole
    /// answer".
    fn unavailable(&self) -> Option<String> {
        None
    }
}

/// The feed for a surface with no pipeline behind it at all: every poll is an
/// idle frame, and it says why in words.
///
/// Not a silent stub — the same rule as [`crate::ControlSource`]'s defaults. A
/// surface handed this renders "no live data, and here is why", never an empty
/// grid that looks like a panel with nothing pressed.
pub struct NoFeed {
    reason: String,
}

impl NoFeed {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl LiveFeed for NoFeed {
    fn poll(&mut self) -> LiveFrame {
        LiveFrame::idle()
    }

    fn unavailable(&self) -> Option<String> {
        Some(self.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tap shorter than one display frame is invisible in `down` and
    /// unmistakable in `hit` — the property the whole button check rests on.
    #[test]
    fn a_control_that_went_down_and_up_inside_one_frame_still_reports_a_hit() {
        let slot = SlotLive {
            slot: 1,
            down: Vec::new(),
            hit: vec!["A".to_owned()],
            ..SlotLive::default()
        };
        assert!(!slot.is_down("A"), "it is already back up");
        assert!(slot.was_hit("A"), "but it definitely happened");
        assert!(!slot.was_hit("B"));
    }

    #[test]
    fn a_feed_with_no_pipeline_says_why_instead_of_rendering_an_empty_panel() {
        let mut feed = NoFeed::new("no daemon is running");
        assert_eq!(feed.poll(), LiveFrame::idle());
        assert!(!feed.poll().running);
        assert_eq!(feed.unavailable().as_deref(), Some("no daemon is running"));
    }

    #[test]
    fn a_frame_round_trips_through_json_so_one_shape_serves_every_transport() {
        let frame = LiveFrame {
            running: true,
            slots: vec![SlotLive {
                slot: 2,
                down: vec!["dpad.up".to_owned()],
                hit: vec!["dpad.up".to_owned(), "A".to_owned()],
                lt: 255,
                ..SlotLive::default()
            }],
            keys: vec![KeyHit {
                key: "G".to_owned(),
                device: r"HID\VID_D209&PID_0430".to_owned(),
                alias: "IPAC P1".to_owned(),
                down: true,
            }],
            feedback: vec![PadFeedback {
                slot: 1,
                large_motor: 200,
                small_motor: 0,
                led_number: 1,
            }],
            dropped: 3,
            off_panel: 7,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(serde_json::from_str::<LiveFrame>(&json).unwrap(), frame);
        assert_eq!(frame.slot(2).map(|s| s.lt), Some(255));
        assert!(frame.slot(4).is_none());
    }
}
