//! The tray's "Open Studio": get a page on screen, or say why not.
//!
//! docs/M9-DECISION.md §4 item 1 is emphatic about this and it is the whole of
//! the requirement: **"it must never be possible to reach
//! `ERR_CONNECTION_REFUSED` by clicking a ksx shortcut."** A menu item that
//! opens a browser at a port nothing is listening on has not opened Studio; it
//! has produced an error page with ksx's name on it.
//!
//! So the order is: probe, start if needed, WAIT for the port to answer, and
//! only then hand the URL to the shell. If any of that fails the daemon says
//! so in its log and its console rather than opening a browser to a dead end.
//!
//! This is deliberately NOT the full `ksx open` of that document — no App
//! Paths resolution, no `--app=` window, no ksx-owned browser profile, no
//! single-instance focus. Those belong to the launcher pass. What is here is
//! the one property that makes the menu item honest.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Studio's default port — the one `ksx studio` binds and the one this dials.
const PORT: u16 = 4460;
/// How long to wait for a freshly started Studio to answer.
const READY_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_EVERY: Duration = Duration::from_millis(150);
/// A connect attempt that hangs is a machine problem, not a busy server.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// Open Studio, starting one if nothing is listening.
///
/// Never blocks the caller: everything below happens on a thread of its own,
/// because "poll a port for eight seconds" is not something the daemon's
/// control loop may do while a session is waiting to be reaped.
pub fn open(out: &mut dyn Write) {
    let _ = writeln!(out, "opening ksx Studio…");
    let _ = std::thread::Builder::new()
        .name("ksx-open-studio".into())
        .spawn(|| {
            if let Err(why) = ensure_and_open() {
                // The console may be gone (the daemon detaches once the tray
                // icon exists), so the log is the surface that always exists.
                tracing::error!("could not open ksx Studio: {why}");
            }
        });
}

fn ensure_and_open() -> Result<(), String> {
    if !answering() {
        start_studio()?;
        let deadline = Instant::now() + READY_TIMEOUT;
        while !answering() {
            if Instant::now() >= deadline {
                return Err(format!(
                    "started `ksx studio` but nothing answered 127.0.0.1:{PORT} within {}s — \
                     run `ksx studio` in a console to see why",
                    READY_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(PROBE_EVERY);
        }
    }
    let url = format!("http://127.0.0.1:{PORT}/");
    ksx_platform::process::shell_open(&url).map_err(|err| format!("{err} ({url})"))
}

/// Is something serving Studio's port right now?
///
/// A TCP connect, not an HTTP request: the daemon links no HTTP client and is
/// not about to grow one for a liveness probe. A listener on loopback that
/// accepts is Studio for this purpose — and if it is not, the browser shows
/// whatever it is, which is a truthful outcome rather than a refused
/// connection.
fn answering() -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok()
}

/// Start `ksx studio` as a child of this process, using OUR OWN executable.
///
/// `current_exe`, never a `PATH` lookup: a daemon started from a build tree, a
/// staging folder or an installer must launch the binary it IS, not whichever
/// ksx happens to be first on the path.
fn start_studio() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("cannot find my own path: {err}"))?;
    std::process::Command::new(&exe)
        .arg("studio")
        .arg("--port")
        .arg(PORT.to_string())
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("could not start `{} studio`: {err}", exe.display()))
}
