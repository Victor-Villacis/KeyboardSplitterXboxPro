//! Full HTTP round trips against the real server: GET / with the session
//! panel, and the POST → 303 → flash loop. Raw `TcpStream` HTTP/1.1 on
//! purpose — no client dependency, and what a browser sends is exactly this.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ksx_studio::{
    BindConflict, BindOutcome, BindRequest, ControlSource, LearnView, MapperSlot, MapperSnapshot,
    PadRow, ProfileRow, SessionView, StatusSnapshot, StatusSource,
};

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

    fn mapper(&self) -> MapperSnapshot {
        let mut bindings = std::collections::BTreeMap::new();
        bindings.insert("A".to_owned(), vec!["G".to_owned()]);
        MapperSnapshot {
            generated_at: "test".into(),
            source: "slots of profile \"Steam\" (games.toml)".into(),
            config_root: "C:\\cfg".into(),
            slots: vec![MapperSlot {
                number: 1,
                persona: "xbox360".into(),
                persona_label: "Xbox 360".into(),
                preset: "IPAC P1".into(),
                keyboard: "HID\\TEST".into(),
                bindings,
            }],
        }
    }
}

/// Scriptable control: session() flips between idle and running; start()
/// records the profile it was given and either succeeds or refuses.
struct ScriptedControl {
    running: AtomicBool,
    refuse_start: bool,
    started_with: std::sync::Mutex<Option<Option<String>>>,
    learning: AtomicBool,
    bound_with: std::sync::Mutex<Option<BindRequest>>,
    restored_with: std::sync::Mutex<Option<(String, String)>>,
}

impl ScriptedControl {
    fn new(refuse_start: bool) -> Self {
        Self {
            running: AtomicBool::new(false),
            refuse_start,
            started_with: std::sync::Mutex::new(None),
            learning: AtomicBool::new(false),
            bound_with: std::sync::Mutex::new(None),
            restored_with: std::sync::Mutex::new(None),
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

    fn learn_start(&self) -> LearnView {
        self.learning.store(true, Ordering::SeqCst);
        LearnView {
            ok: true,
            state: "listening".into(),
            remaining_ms: Some(10_000),
            device: None,
            key: None,
            error: None,
        }
    }

    fn learn_poll(&self) -> LearnView {
        if self.learning.load(Ordering::SeqCst) {
            LearnView {
                ok: true,
                state: "listening".into(),
                remaining_ms: Some(9_000),
                device: None,
                key: None,
                error: None,
            }
        } else {
            LearnView {
                ok: true,
                state: "idle".into(),
                remaining_ms: None,
                device: None,
                key: None,
                error: None,
            }
        }
    }

    fn learn_cancel(&self) -> LearnView {
        self.learning.store(false, Ordering::SeqCst);
        LearnView {
            ok: true,
            state: "cancelled".into(),
            remaining_ms: None,
            device: None,
            key: None,
            error: None,
        }
    }

    fn restore(&self, preset: &str, mode: &str) -> Result<String, String> {
        *self.restored_with.lock().unwrap() = Some((preset.to_owned(), mode.to_owned()));
        if mode == "session-backup" {
            Err(format!("no session backup for \"{preset}\""))
        } else {
            Ok(format!(
                "\"{preset}\": bindings restored to the built-in defaults"
            ))
        }
    }

    fn bind(&self, request: &BindRequest) -> BindOutcome {
        *self.bound_with.lock().unwrap() = Some(request.clone());
        if request.key.as_deref() == Some("G") && !request.force {
            BindOutcome {
                ok: false,
                message: None,
                error: Some("refusing to bind G: G is \"IPAC P2\"'s A".into()),
                code: Some("conflict".into()),
                conflicts: vec![BindConflict {
                    scope: "profile".into(),
                    preset: "IPAC P2".into(),
                    function: "A".into(),
                    profile: Some("Steam".into()),
                    slot: Some(2),
                }],
                reloaded: false,
            }
        } else {
            BindOutcome {
                ok: true,
                message: Some(format!(
                    "\"{}\": {} = {}",
                    request.preset,
                    request.function,
                    request.key.as_deref().unwrap_or("None")
                )),
                error: None,
                code: None,
                conflicts: Vec::new(),
                reloaded: request.reload,
            }
        }
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
        fn learn_start(&self) -> LearnView {
            self.0.learn_start()
        }
        fn learn_poll(&self) -> LearnView {
            self.0.learn_poll()
        }
        fn learn_cancel(&self) -> LearnView {
            self.0.learn_cancel()
        }
        fn bind(&self, request: &BindRequest) -> BindOutcome {
            self.0.bind(request)
        }
        fn restore(&self, preset: &str, mode: &str) -> Result<String, String> {
            self.0.restore(preset, mode)
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

fn post_json(addr: SocketAddr, path: &str, body: &str) -> String {
    http(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

/// The response body (everything after the blank line).
fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
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

/// The whole mapper loop over real HTTP: the page renders zones with real
/// bindings, the learn flow answers listening → cancel, and /api/bind
/// round-trips the conflict → Replace(force) decision.
#[test]
fn the_mapper_page_learn_flow_and_bind_round_trip() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // The page: slot context, art, a zone with its binding tag, credit line.
    let page = get(addr, "/map");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("P1 · Xbox 360 · IPAC P1"), "{page}");
    assert!(page.contains("/_assets/pad-xbox.svg"), "{page}");
    assert!(page.contains(r#"data-fn="A""#), "{page}");
    assert!(page.contains(">G<"), "{page}");
    assert!(
        page.contains("Gamepad-Asset-Pack (MIT) by AL2009man"),
        "{page}"
    );
    // Ledger #13(a): the CSP header must allow inline STYLE attributes (the
    // zone geometry rides them) while scripts stay nonce-locked.
    let headers = page.split("\r\n\r\n").next().unwrap_or("");
    assert!(
        headers.contains("style-src 'self' 'unsafe-inline'"),
        "{headers}"
    );
    assert!(headers.contains("script-src 'nonce-"), "{headers}");

    // The art itself is served with the right type, recolored for the theme
    // (the palette sheet build.mjs injects) — never the source's black blob.
    let art = get(addr, "/_assets/pad-xbox.svg");
    assert!(art.starts_with("HTTP/1.1 200"), "{art}");
    assert!(art.contains("image/svg+xml"), "{art}");
    assert!(art.contains("<svg"), "{art}");
    assert!(art.contains("pad-body"), "recolor classes missing: {art}");
    assert!(!art.contains("fill:#000000"), "source black leaked: {art}");

    // /api/map serves the payload the page embeds.
    let api = get(addr, "/api/map");
    let payload: serde_json::Value = serde_json::from_str(body_of(&api)).expect("json");
    assert_eq!(payload["mapper"]["slots"][0]["preset"], "IPAC P1");
    assert_eq!(payload["mapper"]["slots"][0]["bindings"]["A"][0], "G");
    assert_eq!(payload["selected"], 1);
    assert_eq!(payload["learn"]["state"], "idle");

    // Learn: start → listening with the countdown, poll agrees, cancel ends.
    let started = post_json(addr, "/api/learn/start", "");
    let learn: serde_json::Value = serde_json::from_str(body_of(&started)).expect("json");
    assert_eq!(learn["state"], "listening");
    assert_eq!(learn["remaining_ms"], 10_000);
    let polled = get(addr, "/api/learn");
    let learn: serde_json::Value = serde_json::from_str(body_of(&polled)).expect("json");
    assert_eq!(learn["state"], "listening");
    let cancelled = post_json(addr, "/api/learn/cancel", "");
    let learn: serde_json::Value = serde_json::from_str(body_of(&cancelled)).expect("json");
    assert_eq!(learn["state"], "cancelled");

    // Bind: the scripted conflict comes back structured; Replace (force)
    // succeeds and reports the reload.
    let refused = post_json(
        addr,
        "/api/bind",
        r#"{"preset":"IPAC P1","function":"B","key":"G","force":false,"reload":true}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&refused)).expect("json");
    assert_eq!(outcome["ok"], false);
    assert_eq!(outcome["code"], "conflict");
    assert_eq!(outcome["conflicts"][0]["preset"], "IPAC P2");
    assert_eq!(outcome["conflicts"][0]["slot"], 2);

    let forced = post_json(
        addr,
        "/api/bind",
        r#"{"preset":"IPAC P1","function":"B","key":"G","force":true,"reload":true}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&forced)).expect("json");
    assert_eq!(outcome["ok"], true, "{outcome}");
    assert_eq!(outcome["reloaded"], true);
    let bound = control
        .bound_with
        .lock()
        .unwrap()
        .clone()
        .expect("bind reached control");
    assert_eq!(bound.preset, "IPAC P1");
    assert_eq!(bound.function, "B");
    assert!(bound.force);
    assert!(bound.reload);

    // Preset restore: defaults succeeds, session-backup surfaces the honest
    // "nothing to undo", and a junk mode never reaches the control source.
    let restored = post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"defaults"}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&restored)).expect("json");
    assert_eq!(outcome["ok"], true, "{outcome}");
    assert!(
        outcome["message"]
            .as_str()
            .unwrap()
            .contains("built-in defaults"),
        "{outcome}"
    );
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "defaults".to_owned()))
    );

    let refused = post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"session-backup"}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&refused)).expect("json");
    assert_eq!(outcome["ok"], false);
    assert!(
        outcome["error"]
            .as_str()
            .unwrap()
            .contains("no session backup"),
        "{outcome}"
    );

    let junk = post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"yolo"}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&junk)).expect("json");
    assert_eq!(outcome["ok"], false);
    assert!(
        outcome["error"]
            .as_str()
            .unwrap()
            .contains("unknown restore mode"),
        "{outcome}"
    );
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "session-backup".to_owned())),
        "the junk mode must have been rejected before the control source"
    );
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
