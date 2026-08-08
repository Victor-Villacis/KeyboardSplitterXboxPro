//! The pipe transport: one JSON line out, one JSON line in, per connection.
//!
//! Plain `std` file I/O. `\\.\pipe\...` opens through `CreateFileW` under std,
//! so the client needs no FFI and compiles everywhere (a non-Windows open
//! simply fails NotFound, which is the truthful "no daemon here" answer).
//!
//! This is one implementation of [`crate::VerbSink`], and the *only* thing it
//! adds over the in-process one is the line: same request type, same response
//! type, same parser. That is what makes the choice of transport a deployment
//! decision rather than an architectural one — and why the "serialization tax"
//! argument for a native UI is a measurement, not a premise (docs/M9-DECISION.md).

use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::refusal::{codes, Refusal};
use crate::wire::{Request, Response};

/// What a surface says when nothing answers the pipe — state and remedy in one
/// line, because the disabled controls point at it.
pub const NO_CHANNEL: &str = "no daemon control channel — start the daemon (tray, or `ksx daemon`)";

/// Why a request produced no response.
#[derive(Debug)]
pub enum TransportError {
    /// The pipe does not exist: no daemon is running — or the one that is
    /// predates the control channel. `ksx session` maps this to exit 2.
    NotRunning,
    /// The daemon accepted the connection and then never answered, for a whole
    /// [`RESPONSE_BUDGET`]. The one place this happens in practice is teardown:
    /// the pipe object outlives the thread that served it by a moment, so a
    /// client connects into a conversation nobody is having.
    TimedOut,
    Io(std::io::Error),
    Protocol(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => write!(
                f,
                "no ksx daemon control channel at the pipe (the daemon is \
                 not running, or it predates `ksx session`) — start one \
                 with `ksx daemon`"
            ),
            Self::TimedOut => write!(
                f,
                "the daemon accepted the connection but did not answer within \
                 {}s — it is probably shutting down; if it is running, check \
                 its log",
                RESPONSE_BUDGET.as_secs()
            ),
            Self::Io(err) => write!(f, "control pipe I/O failed: {err}"),
            Self::Protocol(what) => write!(f, "control pipe protocol error: {what}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<TransportError> for Refusal {
    /// The refusal a surface flashes. "No daemon" is its own code because it
    /// is the one failure that means EVERY control is inert rather than this
    /// one call being wrong — and it is the one that always has a remedy.
    fn from(err: TransportError) -> Self {
        match err {
            TransportError::NotRunning => {
                Refusal::with_remedy(codes::NO_CHANNEL, NO_CHANNEL, "ksx daemon")
            }
            other => Refusal::new(codes::PIPE_ERROR, other.to_string()),
        }
    }
}

/// WinError 231: every instance is mid-conversation. The daemon is alive.
const ERROR_PIPE_BUSY: i32 = 231;
/// Total budget for open retries (busy server, instance-rotation races).
const CONNECT_BUDGET: Duration = Duration::from_secs(2);
const RETRY_PAUSE: Duration = Duration::from_millis(50);
/// FILE_NOT_FOUND is definitive after this many looks — the retries only paper
/// over the daemon's instance rotation, which is sub-millisecond.
const NOT_FOUND_TRIES: u32 = 3;

fn open(pipe_path: &str) -> Result<std::fs::File, TransportError> {
    open_with(pipe_path, true, true)
}

/// [`open`] with the access mode named.
///
/// The live feed's channel ([`crate::LIVE_PIPE_NAME`]) is created
/// **outbound-only** by the daemon, so a duplex open of it fails with
/// ACCESS_DENIED — a read-only open is not an optimisation there, it is the
/// only one the kernel will grant. Sharing the retry policy rather than
/// copying it keeps one answer to "is a daemon there?": the same
/// `NOT_FOUND_TRIES` looks, the same busy budget, the same
/// [`TransportError::NotRunning`] verdict on both channels.
pub(crate) fn open_with(
    pipe_path: &str,
    read: bool,
    write: bool,
) -> Result<std::fs::File, TransportError> {
    let deadline = Instant::now() + CONNECT_BUDGET;
    let mut not_found = 0;
    loop {
        match std::fs::OpenOptions::new()
            .read(read)
            .write(write)
            .open(pipe_path)
        {
            Ok(file) => return Ok(file),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                not_found += 1;
                if not_found >= NOT_FOUND_TRIES {
                    return Err(TransportError::NotRunning);
                }
            }
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                if Instant::now() >= deadline {
                    return Err(TransportError::Io(err));
                }
            }
            Err(err) => return Err(TransportError::Io(err)),
        }
        std::thread::sleep(RETRY_PAUSE);
    }
}

/// How long a *connected* conversation may take before the caller gives up.
///
/// [`open`] is defensive — `NotFound` retries, a busy-instance budget — and the
/// read used to be trusting: `read_line` with no deadline. That asymmetry froze
/// a real cabinet: quit the daemon, and for a moment the pipe object outlives
/// the thread that served it, so a client connects into a conversation nobody
/// is having and blocks forever. The cabinet window makes that call on its UI
/// thread, so "forever" was a Not Responding window whose X did nothing.
///
/// Ten seconds, not two: the budget has to clear the *slowest honest verb*
/// (a session start plugs every pad before it answers), because a false
/// timeout on a slow success tells the user it failed while it quietly
/// succeeded — strictly worse than a few extra seconds in the pathological
/// case. The common failure is faster than either: a daemon that *dies*
/// closes the pipe, and the read returns immediately with EOF.
const RESPONSE_BUDGET: Duration = Duration::from_secs(10);

/// One raw request line in, one raw response value out.
///
/// Kept public and untyped for the callers that legitimately have no business
/// with the typed layer — a diagnostic that wants to send a verb this build
/// does not know, and the daemon's own tests.
pub fn request_json(
    pipe_path: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value, TransportError> {
    // Opening stays on the caller's thread: its failures (`NotRunning`, busy)
    // are already budgeted, and they are the ones whose latency a surface
    // shows off — "no daemon" must stay a millisecond answer.
    let pipe = open(pipe_path)?;
    let mut line = request.to_string();
    line.push('\n');
    exchange(pipe, line, RESPONSE_BUDGET)
}

/// Run one write-then-read conversation on a worker thread, giving up after
/// `budget`.
///
/// Generic over the stream for one reason: the timeout path can then be tested
/// against a stream that simply never answers, with a millisecond budget,
/// instead of against a real named pipe and a ten-second wait.
///
/// # The thread this leaks, on purpose
///
/// A synchronous pipe read cannot be cancelled without FFI, and staying
/// FFI-free is this file's design premise (see the module docs). So on timeout
/// the worker is *abandoned*, still blocked in `read_line`, holding the pipe
/// handle. That is bounded, not reckless: the moment the daemon's side closes
/// — including the kernel closing every handle of a daemon that exits — the
/// read unblocks and the worker ends. A worker outlives its timeout only while
/// a daemon is alive, connected and silent, and a named pipe has finitely many
/// instances for such zombies to occupy. The alternative was a UI thread
/// blocked forever; a parked worker is the cheaper end of that trade.
fn exchange<S>(
    stream: S,
    line: String,
    budget: Duration,
) -> Result<serde_json::Value, TransportError>
where
    S: Read + Write + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ksx-pipe-io".into())
        .spawn(move || {
            // The receiver may be long gone (timeout) — that is fine, and
            // `send`'s error says exactly that.
            let _ = tx.send(converse(stream, line));
        })
        .map_err(TransportError::Io)?;
    match rx.recv_timeout(budget) {
        Ok(outcome) => outcome,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(TransportError::TimedOut),
        // The worker panicked before sending. `converse` has no panic path of
        // its own, so treat it as the conversation failing, not as a bug to
        // hide: name it.
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(TransportError::Protocol(
            "the pipe worker thread died mid-conversation".into(),
        )),
    }
}

/// The blocking half: write the line, read the answer. Runs on the worker.
fn converse<S: Read + Write>(
    mut stream: S,
    line: String,
) -> Result<serde_json::Value, TransportError> {
    stream
        .write_all(line.as_bytes())
        .map_err(TransportError::Io)?;
    stream.flush().map_err(TransportError::Io)?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(TransportError::Io)?;
    if response.trim().is_empty() {
        return Err(TransportError::Protocol(
            "the daemon closed the connection without a response".into(),
        ));
    }
    serde_json::from_str(response.trim())
        .map_err(|err| TransportError::Protocol(format!("unparsable response: {err}")))
}

/// The pipe as a [`crate::VerbSink`]: a typed request in, a typed response
/// out, one connection per call.
pub struct PipeTransport {
    path: String,
}

impl PipeTransport {
    /// Talk to the well-known daemon pipe.
    pub fn new() -> Self {
        Self::at(crate::wire::PIPE_NAME)
    }

    /// Talk to a named pipe — the daemon's tests use throwaway names.
    pub fn at(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    /// The pipe this transport talks to.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Default for PipeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::VerbSink for PipeTransport {
    fn call(&self, request: &Request) -> Result<Response, Refusal> {
        let wire = serde_json::to_value(request).map_err(|err| {
            Refusal::new(
                codes::PIPE_ERROR,
                format!("the request could not be serialized: {err}"),
            )
        })?;
        let answer = request_json(&self.path, &wire).map_err(Refusal::from)?;
        Response::parse(request, answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe nobody is serving is "no daemon", not an I/O mystery — and the
    /// refusal it becomes carries the command that would fix it.
    #[test]
    fn an_unserved_pipe_is_the_no_channel_refusal_with_a_remedy() {
        let refusal = Refusal::from(TransportError::NotRunning);
        assert!(refusal.is_no_channel());
        assert_eq!(refusal.message, NO_CHANNEL);
        assert_eq!(refusal.remedy.as_deref(), Some("ksx daemon"));

        // The CLI's wording is a different sentence, deliberately: `ksx
        // session` is not a page with a banner, and it says where to start one.
        assert!(TransportError::NotRunning
            .to_string()
            .contains("start one with `ksx daemon`"));
    }

    #[test]
    fn a_broken_conversation_is_a_pipe_error_not_a_missing_daemon() {
        let refusal = Refusal::from(TransportError::Protocol("torn line".into()));
        assert_eq!(refusal.code, codes::PIPE_ERROR);
        assert!(!refusal.is_no_channel());
    }

    /// A connection that accepts and never answers. `Write` succeeds — the
    /// dying daemon's pipe buffers the request happily — and `Read` blocks
    /// forever, which is exactly what a real pipe does while its server
    /// tears down.
    struct Silent;

    impl Read for Silent {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            // `park` may wake spuriously; a silent server never does.
            loop {
                std::thread::park();
            }
        }
    }

    impl Write for Silent {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A server with a script: swallows the request, answers from `response`.
    struct Scripted(std::io::Cursor<Vec<u8>>);

    impl Scripted {
        fn answering(response: &str) -> Self {
            Self(std::io::Cursor::new(response.as_bytes().to_vec()))
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// **The cabinet freeze, reduced to its mechanism.** Quit the daemon and
    /// click X: the window's control call had connected into a conversation
    /// nobody was having, and `read_line` had no deadline, so the UI thread
    /// blocked forever and Windows painted the window Not Responding.
    ///
    /// Against the pre-budget transport this test does not fail — it HANGS,
    /// which is the whole finding; the harness timeout is what would flag it.
    /// Against any budget that fires, the answer must be `TimedOut`, because
    /// mislabelling it `Io`/`Protocol` would send someone reading pipe docs
    /// instead of noticing their daemon is half-dead.
    #[test]
    fn a_server_that_accepts_and_never_answers_is_a_timeout_not_a_hang() {
        let outcome = exchange(Silent, "{}\n".into(), Duration::from_millis(50));
        assert!(
            matches!(outcome, Err(TransportError::TimedOut)),
            "expected TimedOut, got {outcome:?}"
        );
        // ...and the surface string says what to actually do about it.
        let text = TransportError::TimedOut.to_string();
        assert!(text.contains("did not answer"), "{text}");
        assert!(text.contains("shutting down"), "{text}");
    }

    /// The worker machinery must be invisible on the happy path: same parsed
    /// value out as the pre-budget transport produced.
    #[test]
    fn an_answering_server_is_unaffected_by_the_budget() {
        let outcome = exchange(
            Scripted::answering("{\"ok\":true}\n"),
            "{}\n".into(),
            Duration::from_secs(5),
        )
        .expect("an answered conversation");
        assert_eq!(outcome, serde_json::json!({"ok": true}));
    }

    /// A daemon that *dies* closes the pipe, and EOF must stay the fast
    /// "closed without a response" protocol error — not wait out the budget,
    /// and not masquerade as a timeout. This is the common real-world case:
    /// the kernel closes a dead process's handles immediately.
    #[test]
    fn a_server_that_hangs_up_is_a_protocol_error_immediately_not_a_timeout() {
        let started = Instant::now();
        let outcome = exchange(
            Scripted::answering(""),
            "{}\n".into(),
            Duration::from_secs(30),
        );
        assert!(
            matches!(outcome, Err(TransportError::Protocol(ref what)) if what.contains("without a response")),
            "expected the closed-without-response protocol error, got {outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "EOF must not wait out the budget"
        );
    }
}
