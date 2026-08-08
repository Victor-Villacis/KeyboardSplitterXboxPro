//! The live feed's transport: `\\.\pipe\ksx-live`, one direction, many frames
//! per connection.
//!
//! [`crate::live`] is the *shape*; this is the line it travels on for a surface
//! that is not inside the daemon's process. It is the exact counterpart of
//! [`crate::pipe`] — same `std`-only file I/O, same "a non-Windows open simply
//! fails NotFound, which is the truthful *no daemon here* answer", same
//! `NotRunning` verdict — and the differences are all consequences of one fact:
//! **a stream is not a conversation.**
//!
//! | | control pipe | live pipe |
//! |---|---|---|
//! | shape | one line out, one line in, per connection | many lines in, none out, per connection |
//! | direction | duplex | `PIPE_ACCESS_OUTBOUND` — the client *cannot* write |
//! | daemon side | one thread, connections served in turn | one thread per connection |
//! | deadline | [`crate::pipe::TransportError::TimedOut`] after a budget | none; waiting is the operation |
//! | lifetime | the length of one verb | the length of one open tab |
//!
//! [`crate::LiveSource`]'s docs carry the three reasons this could not be a
//! verb on the control pipe. The short version is the last row: a subscription
//! that dies between calls loses every event between them, and loses the count
//! as well.

use std::io::{BufRead as _, BufReader, Read as _};

use crate::live::{LiveEnvelope, LiveSource, LiveStream, LIVE_PIPE_NAME};
use crate::pipe::TransportError;
use crate::refusal::{codes, Refusal};

/// A line longer than this is not a frame. Sized well above the largest honest
/// one: 64 key hits and `MAX_SLOTS` slots of control names is a few kilobytes,
/// so a megabyte is three orders of magnitude of headroom and still a bound —
/// a `read_line` with none would let a wedged writer grow this process's
/// memory without limit.
const MAX_FRAME: u64 = 1024 * 1024;

/// The live feed over the named pipe. One `open()` is one subscription.
pub struct PipeLiveSource {
    path: String,
}

impl PipeLiveSource {
    /// Talk to the well-known live pipe.
    pub fn new() -> Self {
        Self::at(LIVE_PIPE_NAME)
    }

    /// Talk to a named pipe — tests use throwaway names.
    pub fn at(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    /// The pipe this source dials.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Default for PipeLiveSource {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveSource for PipeLiveSource {
    fn open(&self) -> Result<Box<dyn LiveStream>, Refusal> {
        // READ-ONLY, and not as a courtesy: the daemon creates this pipe
        // `PIPE_ACCESS_OUTBOUND`, so asking for write access fails at the
        // object manager. One-directional in the kernel beats one-directional
        // by convention.
        let pipe = crate::pipe::open_with(&self.path, true, false).map_err(live_refusal)?;
        Ok(Box::new(PipeLiveStream {
            lines: BufReader::new(pipe),
        }))
    }
}

/// A transport failure on the LIVE channel, worded for a live consumer.
///
/// Deliberately not `Refusal::from(TransportError)`: that one says "no daemon
/// control channel", which sends a reader to the wrong pipe. The *code* is the
/// same `no-channel` — surfaces branch on it to mean "everything here is
/// inert, and there is a way to fix it" — and so is the remedy.
fn live_refusal(err: TransportError) -> Refusal {
    match err {
        TransportError::NotRunning => Refusal::with_remedy(
            codes::NO_CHANNEL,
            "no ksx daemon is running, so there is no live input feed — \
             start the daemon (tray, or `ksx daemon`)",
            "ksx daemon",
        ),
        other => Refusal::new(codes::PIPE_ERROR, format!("the live feed failed: {other}")),
    }
}

struct PipeLiveStream {
    lines: BufReader<std::fs::File>,
}

impl LiveStream for PipeLiveStream {
    fn next_frame(&mut self) -> Result<LiveEnvelope, Refusal> {
        let mut line = String::new();
        // `take` bounds ONE line, so the limit is re-armed per read.
        let read = (&mut self.lines)
            .take(MAX_FRAME)
            .read_line(&mut line)
            .map_err(|err| {
                Refusal::new(
                    codes::PIPE_ERROR,
                    format!("the live feed read failed: {err}"),
                )
            })?;
        // EOF. The daemon closed the pipe — it exited, or it is shutting down.
        // This is the COMMON end of a stream and it arrives at once, which is
        // what lets `next_frame` have no deadline of its own.
        if read == 0 {
            return Err(Refusal::with_remedy(
                codes::NO_CHANNEL,
                "the live feed ended — the daemon closed it (it stopped, or is shutting down)",
                "ksx daemon",
            ));
        }
        // The cap fired, so this line has no end and the next read would begin
        // mid-frame. A stream that cannot prove it is on a line boundary ends
        // rather than resyncing into plausible nonsense.
        if read as u64 >= MAX_FRAME {
            return Err(Refusal::new(
                codes::PIPE_ERROR,
                format!(
                    "the live feed sent a line longer than {MAX_FRAME} bytes; \
                         the stream is out of sync"
                ),
            ));
        }
        serde_json::from_str(line.trim()).map_err(|err| {
            Refusal::new(
                codes::PIPE_ERROR,
                format!("the live feed sent a frame this build cannot read: {err}"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{KeyHit, LiveFrame, SlotLive};

    /// A pipe nobody is serving is "no daemon", with a remedy — and it says
    /// LIVE FEED, not "control channel". The code is shared on purpose (a
    /// surface branches on `no-channel` to disable everything at once); the
    /// sentence is not, because a reader sent to the control pipe to debug a
    /// dead live feed looks in the wrong place.
    #[test]
    fn an_unserved_live_pipe_is_no_channel_and_names_the_live_feed() {
        let refusal = live_refusal(TransportError::NotRunning);
        assert!(refusal.is_no_channel());
        assert!(refusal.message.contains("live input feed"), "{refusal}");
        assert_eq!(refusal.remedy.as_deref(), Some("ksx daemon"));
        assert!(
            !refusal.message.contains("control channel"),
            "the live feed is not the control pipe: {refusal}"
        );
    }

    /// ...and a conversation that broke is NOT "no daemon". The distinction is
    /// the one a surface acts on: `no-channel` means "start the daemon",
    /// anything else means "the daemon is there and something else is wrong".
    #[test]
    fn a_broken_live_conversation_is_a_pipe_error_not_a_missing_daemon() {
        let refusal = live_refusal(TransportError::Protocol("torn line".into()));
        assert_eq!(refusal.code, codes::PIPE_ERROR);
        assert!(!refusal.is_no_channel());
    }

    /// **The wire contract, end to end.** The envelope the daemon writes is the
    /// envelope a consumer reads — including the `unavailable` sentence, which
    /// is composed once in the daemon and never re-derived from `running:
    /// false` on the far side.
    ///
    /// Catches the version that serialized a bare `LiveFrame`: `running: false`
    /// arrived intact and the REASON did not, so every surface had to invent
    /// its own sentence and the browser and the cabinet said different things
    /// about the same daemon.
    #[test]
    fn an_envelope_round_trips_with_its_reason_intact() {
        let envelope = LiveEnvelope {
            frame: LiveFrame {
                running: true,
                slots: vec![SlotLive {
                    slot: 3,
                    down: vec!["A".to_owned()],
                    hit: vec!["A".to_owned(), "dpad.up".to_owned()],
                    ..SlotLive::default()
                }],
                keys: vec![KeyHit {
                    key: "G".to_owned(),
                    device: r"HID\VID_D209".to_owned(),
                    alias: "IPAC P1".to_owned(),
                    down: true,
                }],
                dropped: 4,
                off_panel: 9,
                ..LiveFrame::default()
            },
            unavailable: None,
        };
        let line = serde_json::to_string(&envelope).unwrap();
        assert!(!line.contains('\n'), "one frame is one line: {line}");
        assert_eq!(
            serde_json::from_str::<LiveEnvelope>(&line).unwrap(),
            envelope
        );

        let quiet = LiveEnvelope::unreachable("no session is running");
        let line = serde_json::to_string(&quiet).unwrap();
        assert_eq!(serde_json::from_str::<LiveEnvelope>(&line).unwrap(), quiet);
        assert_eq!(quiet.unavailable.as_deref(), Some("no session is running"));
    }

    /// A frame from a daemon NEWER than this build must not kill the stream on
    /// a field this build has never heard of. Same rule as
    /// `Refusal::from_wire`: carry what you understand, do not refuse the
    /// conversation.
    #[test]
    fn an_unknown_field_on_the_wire_does_not_break_the_stream() {
        let line = r#"{"frame":{"running":true,"slots":[],"keys":[],"feedback":[],
                       "dropped":0,"off_panel":0,"future_field":42},"soon":"yes"}"#;
        let envelope: LiveEnvelope =
            serde_json::from_str(line).expect("an unknown field is not a protocol break");
        assert!(envelope.frame.running);
    }
}
