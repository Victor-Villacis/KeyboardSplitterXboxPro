//! Full HTTP round trips against the real server: GET / with the session
//! panel, and the POST → 303 → flash loop. Raw `TcpStream` HTTP/1.1 on
//! purpose — no client dependency, and what a browser sends is exactly this.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ksx_studio::{ControlSource, PadRow, ProfileRow, SessionView, StatusSnapshot, StatusSource};

struct FixedStatus;

impl StatusSource for FixedStatus {
    fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "test".into(),
            vigem: "installed".into(),
            interception: "installed".into(),
            daemon_running: true,
            daemon_detail: "test".into(),
            autostart: "not registered".into(),
            pads: vec![PadRow {
                persona: "Xbox 360 pad".into(),
                instance: "USB\\TEST\\1".into(),
            }],
            profiles: vec![ProfileRow {
                title: "Street Fighter".into(),
                detail: "C:\\sf.exe — 2 slots".into(),
            }],
            config_root: "C:\\cfg".into(),
        }
    }
}

/// Scriptable control: session() flips between idle and running; start()
/// records the profile it was given and either succeeds or refuses.
struct ScriptedControl {
    running: AtomicBool,
    refuse_start: bool,
    started_with: std::sync::Mutex<Option<Option<String>>>,
}

impl ScriptedControl {
    fn new(refuse_start: bool) -> Self {
        Self {
            running: AtomicBool::new(false),
            refuse_start,
            started_with: std::sync::Mutex::new(None),
        }
    }
}

impl ControlSource for ScriptedControl {
    fn session(&self) -> SessionView {
        if self.running.load(Ordering::SeqCst) {
            SessionView {
                reachable: true,
                running: true,
                line: "running — 4 pad(s)".into(),
            }
        } else {
            SessionView {
                reachable: true,
                running: false,
                line: "idle".into(),
            }
        }
    }

    fn start(&self, profile: Option<&str>) -> Result<String, String> {
        *self.started_with.lock().unwrap() = Some(profile.map(str::to_owned));
        if self.refuse_start {
            Err("no ksx daemon control channel at the pipe".into())
        } else {
            self.running.store(true, Ordering::SeqCst);
            Ok("running (4 slot(s))".into())
        }
    }

    fn stop(&self) -> Result<String, String> {
        self.running.store(false, Ordering::SeqCst);
        Ok("stopped".into())
    }

    fn reload(&self) -> Result<String, String> {
        Ok("running (4 slot(s))".into())
    }
}

/// Bind port 0 to learn a free port, release it, and serve there. The tiny
/// race is acceptable in a local test.
fn start_server(control: Arc<ScriptedControl>) -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    struct SharedControl(Arc<ScriptedControl>);
    impl ControlSource for SharedControl {
        fn session(&self) -> SessionView {
            self.0.session()
        }
        fn start(&self, profile: Option<&str>) -> Result<String, String> {
            self.0.start(profile)
        }
        fn stop(&self) -> Result<String, String> {
            self.0.stop()
        }
        fn reload(&self) -> Result<String, String> {
            self.0.reload()
        }
    }
    std::thread::spawn(move || {
        let _ = ksx_studio::serve(
            addr,
            Box::new(FixedStatus),
            Box::new(SharedControl(control)),
        );
    });
    // Wait until it accepts.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(addr).is_ok() {
            return addr;
        }
        assert!(Instant::now() < deadline, "server never came up on {addr}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn http(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn get(addr: SocketAddr, path: &str) -> String {
    http(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    )
}

fn post_form(addr: SocketAddr, path: &str, body: &str) -> String {
    http(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

#[test]
fn the_session_panel_round_trips_start_stop_and_the_flash() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // Idle: the Start form and the profile dropdown render.
    let page = get(addr, "/");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains(r#"action="/session/start""#), "{page}");
    assert!(page.contains("Street Fighter"), "{page}");
    assert!(page.contains("idle"), "{page}");

    // Start with a profile: 303 back to / with the outcome flashed.
    let response = post_form(addr, "/session/start", "profile=Street+Fighter");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("location: /?flash=running"), "{response}");
    assert_eq!(
        control.started_with.lock().unwrap().clone(),
        Some(Some("Street Fighter".to_owned())),
        "the form's profile field must reach the control verb"
    );

    // Following the redirect renders the flash and, now, Stop/Reload.
    let page = get(addr, "/?flash=running%20%284%20slot%28s%29%29");
    assert!(page.contains("running (4 slot(s))"), "{page}");
    assert!(page.contains(r#"action="/session/stop""#), "{page}");
    assert!(page.contains(r#"action="/config/reload""#), "{page}");
    assert!(!page.contains(r#"action="/session/start""#), "{page}");

    // The empty sentinel option means "no profile override".
    let response = post_form(addr, "/session/stop", "");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let response = post_form(addr, "/session/start", "profile=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert_eq!(control.started_with.lock().unwrap().clone(), Some(None));
}

#[test]
fn a_refused_action_comes_back_as_an_error_flash_never_silence() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);

    let response = post_form(addr, "/session/start", "profile=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /?flash=error%3A%20no%20ksx%20daemon"),
        "{response}"
    );

    // And the redirect target renders it.
    let page = get(
        addr,
        "/?flash=error%3A%20no%20ksx%20daemon%20control%20channel",
    );
    assert!(
        page.contains("error: no ksx daemon control channel"),
        "{page}"
    );
}
