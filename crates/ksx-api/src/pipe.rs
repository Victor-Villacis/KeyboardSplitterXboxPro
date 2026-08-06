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

use std::io::{BufRead as _, BufReader, Write as _};
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
    let deadline = Instant::now() + CONNECT_BUDGET;
    let mut not_found = 0;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
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

/// One raw request line in, one raw response value out.
///
/// Kept public and untyped for the callers that legitimately have no business
/// with the typed layer — a diagnostic that wants to send a verb this build
/// does not know, and the daemon's own tests.
pub fn request_json(
    pipe_path: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value, TransportError> {
    let mut pipe = open(pipe_path)?;
    let mut line = request.to_string();
    line.push('\n');
    pipe.write_all(line.as_bytes())
        .map_err(TransportError::Io)?;
    pipe.flush().map_err(TransportError::Io)?;

    let mut response = String::new();
    BufReader::new(pipe)
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
}
