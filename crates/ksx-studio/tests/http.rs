//! Full HTTP round trips against the real server: GET / with the session
//! panel, and the POST → 303 → flash loop. Raw `TcpStream` HTTP/1.1 on
//! purpose — no client dependency, and what a browser sends is exactly this.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ksx_studio::{
    BindConflict, BindOutcome, BindRequest, ControlSource, LearnView, MacroOutcome, MacroSnapshot,
    MacroStepView, MacroView, MacroWrite, MapperSlot, MapperSnapshot, PadRow, ProfileRow,
    SessionView, StatusSnapshot, StatusSource,
};

struct FixedStatus;

/// The newest timestamped backup this preset has, as `collect_mapper` reads
/// it off disk — the label the mapper's third restore button wears.
const BACKUP_LABEL: &str = "2026-08-05 14:32:07 UTC";

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
        // MANY KEYS → ONE CONTROL, exactly as a preset file can hold it
        // (docs/INPUT-TRANSFORMS.md §1a) — the shape Victor's imported preset
        // already had, and the one the add/remove-one routes are computed
        // against.
        bindings.insert("B".to_owned(), vec!["S".to_owned(), "Enter".to_owned()]);
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
                backup: Some(BACKUP_LABEL.to_owned()),
                // One AUTO-FIRING control, so the legend badge is covered by
                // the ordinary page assertions.
                turbo: std::collections::BTreeMap::from([("B".to_owned(), 12)]),
            }],
        }
    }

    /// The preset's `[macros]` tables, in the file's own shape — what
    /// ksx-app's collector reads off disk (`ms` and `frames` kept apart, the
    /// `macro.<name>` rows resolved into `triggers`).
    fn macros(&self, preset: &str) -> MacroSnapshot {
        MacroSnapshot::read(
            preset,
            vec![MacroView {
                name: "hadouken".into(),
                steps: vec![
                    MacroStepView {
                        hold: vec!["dpad.down".into()],
                        ms: Some(50),
                        frames: None,
                        allow_short: false,
                    },
                    MacroStepView {
                        hold: vec!["A".into()],
                        ms: None,
                        frames: Some(3),
                        allow_short: false,
                    },
                ],
                on_release: "finish".into(),
                retrigger: "ignore".into(),
                interrupt: "none".into(),
                repeat: "once".into(),
                turbo_hz: None,
                gap_ms: None,
                triggers: vec!["P".into()],
            }],
        )
    }
}

/// Scriptable control: session() flips between idle and running; start()
/// records the profile it was given and either succeeds or refuses.
struct ScriptedControl {
    running: AtomicBool,
    refuse_start: bool,
    /// Every ControlSource call fails the way an absent daemon fails.
    no_daemon: bool,
    started_with: std::sync::Mutex<Option<Option<String>>>,
    learning: AtomicBool,
    bound_with: std::sync::Mutex<Option<BindRequest>>,
    restored_with: std::sync::Mutex<Option<(String, String)>>,
    cleared: std::sync::Mutex<Option<String>>,
    saved_macro: std::sync::Mutex<Option<MacroWrite>>,
}

impl ScriptedControl {
    fn new(refuse_start: bool) -> Self {
        Self {
            running: AtomicBool::new(false),
            refuse_start,
            no_daemon: false,
            started_with: std::sync::Mutex::new(None),
            learning: AtomicBool::new(false),
            bound_with: std::sync::Mutex::new(None),
            restored_with: std::sync::Mutex::new(None),
            cleared: std::sync::Mutex::new(None),
            saved_macro: std::sync::Mutex::new(None),
        }
    }

    /// Nothing answers the pipe — the state Victor hit when he quit the tray
    /// daemon and then clicked around /map.
    fn dead() -> Self {
        Self {
            no_daemon: true,
            ..Self::new(true)
        }
    }
}

/// What every verb says when there is no daemon, matching the real
/// PipeControlSource's `NO_CHANNEL`.
const NO_CHANNEL: &str = "no daemon control channel — start the daemon (tray, or `ksx daemon`)";

impl ControlSource for ScriptedControl {
    fn session(&self) -> SessionView {
        if self.no_daemon {
            // The profile still comes from the config, so the banner can print
            // a command that actually starts THIS cabinet.
            return SessionView {
                profile: Some("Steam".into()),
                ..SessionView::unreachable(NO_CHANNEL)
            };
        }
        if self.running.load(Ordering::SeqCst) {
            SessionView {
                reachable: true,
                running: true,
                line: "running — 4 pad(s)".into(),
                profile: Some("Street Fighter".into()),
            }
        } else {
            SessionView {
                reachable: true,
                running: false,
                line: "idle".into(),
                profile: None,
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
        if self.no_daemon {
            return Err(NO_CHANNEL.to_owned());
        }
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
        if self.no_daemon {
            return Err(NO_CHANNEL.to_owned());
        }
        match mode {
            "session-backup" => Err(format!("no session backup for \"{preset}\"")),
            "latest-backup" => Ok(format!(
                "\"{preset}\": bindings restored from the newest timestamped backup"
            )),
            _ => Ok(format!(
                "\"{preset}\": bindings reset to the generic keyboard layout (S/D/A/W…)"
            )),
        }
    }

    fn clear_all(&self, preset: &str) -> Result<String, String> {
        *self.cleared.lock().unwrap() = Some(preset.to_owned());
        if self.no_daemon {
            return Err(NO_CHANNEL.to_owned());
        }
        Ok(format!("\"{preset}\": every binding cleared"))
    }

    fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
        *self.saved_macro.lock().unwrap() = Some(request.clone());
        if self.no_daemon {
            return MacroOutcome::failed(NO_CHANNEL);
        }
        // The one refusal the daemon answers with rows rather than a sentence.
        if request
            .steps
            .iter()
            .any(|s| s.hold.iter().any(|f| f == "warp"))
        {
            return MacroOutcome {
                code: Some("macro-invalid".into()),
                problems: vec!["macro 'hadouken' step 1 holds 'warp'".into()],
                ..MacroOutcome::failed("refusing to write macro \"hadouken\"")
            };
        }
        MacroOutcome {
            ok: true,
            message: Some(if request.delete {
                format!("\"{}\": macro \"{}\" deleted", request.preset, request.name)
            } else {
                format!(
                    "\"{}\": macro \"{}\" = {} step(s)",
                    request.preset,
                    request.name,
                    request.steps.len()
                )
            }),
            warnings: vec!["step 2 asks for 5 ms and was raised to 33 ms".into()],
            deleted: request.delete,
            backup: Some(BACKUP_LABEL.to_owned()),
            reloaded: request.reload,
            ..MacroOutcome::default()
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
                also_drives: Vec::new(),
                turbo_hz: None,
                turbo_effective_hz: None,
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
                // A same-preset duplicate is a multi-bind, not a refusal: the
                // write succeeds and names the controls the key also drives.
                also_drives: match request.key.as_deref() {
                    Some("P") => vec!["A".into(), "B".into()],
                    _ => Vec::new(),
                },
                turbo_hz: None,
                turbo_effective_hz: None,
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
        fn clear_all(&self, preset: &str) -> Result<String, String> {
            self.0.clear_all(preset)
        }
        fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
            self.0.save_macro(request)
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
            .contains("generic keyboard layout"),
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

/// FIX 1, over real HTTP: the exact failure Victor hit. Quit the daemon, load
/// either page, and the FIRST thing on it must be the banner — with the
/// command that starts one, profile flag included.
#[test]
fn a_dead_daemon_is_loud_on_both_pages_with_a_runnable_command() {
    let addr = start_server(Arc::new(ScriptedControl::dead()));

    for path in ["/", "/map"] {
        let page = get(addr, path);
        assert!(page.starts_with("HTTP/1.1 200"), "{path}: {page}");
        let body = body_of(&page);
        assert!(
            body.contains("No daemon — ksx Studio can see your config but cannot change anything."),
            "{path} has no banner: {body}"
        );
        assert!(body.contains("tray icon"), "{path}: {body}");
        assert!(
            body.contains("ksx daemon --game &quot;Steam&quot;")
                || body.contains(r#"ksx daemon --game "Steam""#),
            "{path} must print the command that actually starts THIS cabinet: {body}"
        );
        // Unmissable = above everything it is about. On both pages the banner
        // must precede the <main> content it warns you off touching.
        let banner = body.find("No daemon —").expect("banner");
        let footer = body.find("<footer").expect("footer");
        assert!(banner < footer, "{path}: banner is below the fold: {body}");
        let first_other_card = body[banner..]
            .find(r#"class="card"#)
            .map(|i| banner + i)
            .expect("another card follows the banner");
        assert!(
            banner < first_other_card,
            "{path}: the banner is not first inside <main>: {body}"
        );
    }

    // The mapper additionally renders every control visibly inert…
    let map = body_of(&get(addr, "/map")).to_owned();
    assert!(map.contains("z-dead"), "{map}");
    assert!(map.contains("l-dead"), "{map}");
    assert!(map.contains("card pactions off"), "{map}");
    // …and keeps the prefilled shell fallback, so the page is still useful.
    assert!(map.contains("ksx map --preset"), "{map}");
}

/// FIX 0 over HTTP: the mapper's own session controls are the same
/// ControlSource verbs the status page's forms use — one pipe verb each.
#[test]
fn the_mapper_can_pause_and_resume_emulation_over_json() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // Start something, then pause it from the mapper.
    let started = post_json(
        addr,
        "/api/session/start",
        r#"{"profile":"Street Fighter"}"#,
    );
    let out: serde_json::Value = serde_json::from_str(body_of(&started)).expect("json");
    assert_eq!(out["ok"], true, "{out}");

    let map = body_of(&get(addr, "/map")).to_owned();
    assert!(map.contains("Emulation is running"), "{map}");
    assert!(map.contains(r#"data-act="pause-map""#), "{map}");
    // v9: and it is a real form, so the pause is not a dead button on a page
    // without JavaScript — same `stop` verb, 303'd back to /map.
    assert!(map.contains(r#"action="/map/session/stop""#), "{map}");
    let response = post_form(addr, "/map/session/stop", "slot=1");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /map?slot=1&flash=stopped"),
        "{response}"
    );
    assert!(
        !control.session().running,
        "the form POST really stopped it"
    );
    let started = post_json(
        addr,
        "/api/session/start",
        r#"{"profile":"Street Fighter"}"#,
    );
    let out: serde_json::Value = serde_json::from_str(body_of(&started)).expect("json");
    assert_eq!(out["ok"], true, "{out}");

    let paused = post_json(addr, "/api/session/stop", "");
    let out: serde_json::Value = serde_json::from_str(body_of(&paused)).expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["message"], "stopped");

    // Resume names the profile the page remembered.
    let resumed = post_json(
        addr,
        "/api/session/start",
        r#"{"profile":"Street Fighter"}"#,
    );
    let out: serde_json::Value = serde_json::from_str(body_of(&resumed)).expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(
        control.started_with.lock().unwrap().clone(),
        Some(Some("Street Fighter".to_owned())),
        "Resume must restart the SAME profile that was paused"
    );
}

/// FIX 2 over HTTP: three destinations, and the label the third one wears.
#[test]
fn the_three_restore_destinations_and_clear_all_round_trip() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    let map = body_of(&get(addr, "/map")).to_owned();
    assert!(
        map.contains(&format!("Restore backup from {BACKUP_LABEL}")),
        "the newest backup's timestamp belongs in the label: {map}"
    );
    assert!(
        map.contains("Reset to generic keyboard layout (S/D/A/W…)"),
        "{map}"
    );
    assert!(!map.contains("Restore built-in defaults"), "{map}");

    let out: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"latest-backup"}"#,
    )))
    .expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "latest-backup".to_owned()))
    );

    let out: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/preset/clear-all",
        r#"{"preset":"IPAC P1"}"#,
    )))
    .expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(
        control.cleared.lock().unwrap().clone(),
        Some("IPAC P1".to_owned())
    );
}

/// v11 over HTTP: the macro editor READS a real preset and SAVES the whole
/// table back through the one control verb.
///
/// The read half proves the card is no longer a blank draft — the file's own
/// numbers reach the page, in the unit they were authored in — and the write
/// half proves the save is `ControlSource::save_macro` (= the daemon's
/// `map-macro`), carrying the toast, the advisories a successful write still
/// has to say, and the backup label that IS the undo.
#[test]
fn the_macro_editor_reads_a_preset_and_saves_the_whole_table() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // READ: the payload the island polls carries the file's shape.
    let map: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/map?slot=1"))).expect("json");
    assert_eq!(map["macros"]["available"], true, "{map}");
    assert_eq!(map["macros"]["preset"], "IPAC P1");
    assert_eq!(map["macros"]["macros"][0]["name"], "hadouken");
    assert_eq!(map["macros"]["macros"][0]["triggers"][0], "P");
    assert_eq!(map["macros"]["macros"][0]["steps"][0]["ms"], 50);
    // A duration authored in frames stays frames all the way to the client.
    assert_eq!(map["macros"]["macros"][0]["steps"][1]["frames"], 3);
    assert_eq!(
        map["macros"]["macros"][0]["steps"][1]["ms"],
        serde_json::Value::Null
    );
    // ...and the SSR paint says the same thing without any JavaScript.
    let page = body_of(&get(addr, "/map?slot=1")).to_owned();
    assert!(page.contains("hadouken"), "{page}");
    assert!(page.contains("started by P"), "{page}");

    // WRITE: one POST, one whole table.
    let saved: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","on_release":"abort",
            "steps":[{"hold":["dpad.down"],"ms":50},{"hold":["A"],"frames":3}]}"#,
    )))
    .expect("json");
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["backup"], BACKUP_LABEL, "the undo, named: {saved}");
    assert_eq!(
        saved["warnings"][0], "step 2 asks for 5 ms and was raised to 33 ms",
        "an advisory is never swallowed: {saved}"
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(write.preset, "IPAC P1");
    assert_eq!(write.name, "hadouken");
    assert_eq!(write.on_release, "abort");
    assert_eq!(write.steps.len(), 2);
    assert_eq!(write.steps[1].frames, Some(3));
    assert!(!write.delete);
    assert!(
        write.reload,
        "a macro body is a binding change: the running session takes it in place"
    );

    // A refusal comes back as rows a page can list, not a sentence to parse.
    let refused: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","steps":[{"hold":["warp"],"ms":50}]}"#,
    )))
    .expect("json");
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["code"], "macro-invalid", "{refused}");
    assert!(
        refused["problems"][0].as_str().unwrap().contains("warp"),
        "{refused}"
    );

    // DELETE is the same route with the explicit word.
    let deleted: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","delete":true}"#,
    )))
    .expect("json");
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert_eq!(deleted["deleted"], true, "{deleted}");
    assert!(control.saved_macro.lock().unwrap().as_ref().unwrap().delete);
}

/// END TO END over HTTP for the field that broke: `repeat`, and the turbo
/// rate that hangs off it.
///
/// The user set `repeat = while-held` in the card, clicked Save, was told
/// "saved", and watched the control snap back to `once` — because the value
/// was dropped between the wire and the preset file. Nothing about that was
/// visible from the outside: the POST returned `ok`. So this test asserts what
/// the POST actually DELIVERS, not just that it succeeded — the `MacroWrite`
/// that reached `ControlSource::save_macro` must carry the policy the request
/// asked for, in every spelling the card can produce.
#[test]
fn the_repeat_policy_and_its_rate_reach_the_control_source_over_http() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // The read half serves the field at all — an absent `repeat` on the wire
    // would leave the card with nothing to show.
    let map: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/map?slot=1"))).expect("json");
    assert_eq!(map["macros"]["macros"][0]["repeat"], "once", "{map}");

    // while-held: the exact edit that was reported lost.
    let saved: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","repeat":"while-held",
            "steps":[{"hold":["A"],"ms":50}]}"#,
    )))
    .expect("json");
    assert_eq!(saved["ok"], true, "{saved}");
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(
        write.repeat, "while-held",
        "the repeat policy must reach the writer, or Save is a lie: {saved}"
    );

    // turbo authored in hertz: the rate travels in the unit it was written in.
    let _ = post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","repeat":"turbo","turbo_hz":12,
            "steps":[{"hold":["A"],"ms":50}]}"#,
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(write.repeat, "turbo");
    assert_eq!(write.turbo_hz, Some(12));
    assert_eq!(write.gap_ms, None, "the other spelling is not invented");

    // ...and the same rate said the other way.
    let _ = post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","repeat":"turbo","gap_ms":50,
            "steps":[{"hold":["A"],"frames":2,"allow_short":true}]}"#,
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(write.gap_ms, Some(50));
    assert_eq!(write.turbo_hz, None);
    // The step's own fields ride along untouched, in the author's unit.
    assert_eq!(write.steps[0].frames, Some(2));
    assert_eq!(write.steps[0].ms, None);
    assert!(write.steps[0].allow_short);

    // An omitted `repeat` is the file's own default, not an empty string the
    // daemon would refuse.
    let _ = post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","steps":[{"hold":["A"],"ms":50}]}"#,
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert!(
        write.repeat.is_empty(),
        "blank = the file's omitted-field rule"
    );
}

/// Clearing ONE binding is the plain `map` verb with a null key — no second
/// writer, no GUI-only path.
#[test]
fn clearing_one_binding_goes_through_the_bind_verb_with_a_null_key() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());
    let out: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/bind",
        r#"{"preset":"IPAC P1","function":"A","key":null,"force":false,"reload":true}"#,
    )))
    .expect("json");
    assert_eq!(out["ok"], true, "{out}");
    let bound = control.bound_with.lock().unwrap().clone().expect("bind");
    assert_eq!(bound.function, "A");
    assert_eq!(bound.key, None, "a null key is a CLEAR");
}

/// v9, over real HTTP and with no JavaScript anywhere in sight: the mapper
/// page ships forms, and posting one form-encoded body writes a binding and
/// 303s back to /map with the outcome flashed. This is the whole no-JS
/// contract — if it holds here, a browser with scripting off can map a
/// cabinet.
#[test]
fn the_mapper_is_fully_operable_with_form_posts_only() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // The page a scripting-off browser gets: real forms, real action URLs,
    // real key options, and slot switching as links.
    let page = body_of(&get(addr, "/map")).to_owned();
    assert!(page.contains(r#"action="/map/bind""#), "{page}");
    assert!(page.contains(r#"formaction="/map/clear""#), "{page}");
    assert!(page.contains(r#"action="/map/preset/restore""#), "{page}");
    assert!(page.contains(r#"action="/map/preset/clear-all""#), "{page}");
    assert!(
        page.contains(r#"<select class="keysel" name="key""#),
        "{page}"
    );
    assert!(page.contains("<option>NumpadEnter</option>"), "{page}");
    assert!(page.contains(r#"href="/map?slot=1""#), "{page}");

    // Bind: form-encoded in, 303 back to the slot we were on, outcome flashed.
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=H");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /map?slot=1&flash=B%20is%20now%20H."),
        "{response}"
    );
    let bound = control.bound_with.lock().unwrap().clone().expect("bind");
    assert_eq!(bound.preset, "IPAC P1", "the slot resolved to its preset");
    assert_eq!(bound.function, "B");
    assert_eq!(bound.key.as_deref(), Some("H"));
    assert!(
        bound.reload,
        "a binding edit is hot-swapped, pads stay plugged"
    );
    assert!(!bound.force, "the row form never forces on its own");

    // Clear: the same `map` verb with a null key — no second unbind path.
    let response = post_form(addr, "/map/clear", "slot=1&function=A");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /map?slot=1&flash=A%20is%20now%20unbound."),
        "{response}"
    );
    assert_eq!(
        control.bound_with.lock().unwrap().clone().unwrap().key,
        None,
        "a null key is a CLEAR"
    );

    // The empty placeholder is refused in words, never read as a clear.
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("flash=error%3A%20no%20key%20picked"),
        "{response}"
    );

    // Cross-slot refusal: the flash names the other slot AND the checkbox
    // that says yes to it — a form's version of the Replace dialog.
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=G");
    assert!(
        response.contains("location: /map?slot=1&flash=error"),
        "{response}"
    );
    assert!(response.contains("IPAC%20P2"), "{response}");
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=G&force=1");
    assert!(response.contains("flash=B%20is%20now%20G."), "{response}");
    assert!(control.bound_with.lock().unwrap().clone().unwrap().force);

    // The preset writes and the pause, same shape.
    let response = post_form(addr, "/map/preset/clear-all", "slot=1");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert_eq!(
        control.cleared.lock().unwrap().clone(),
        Some("IPAC P1".to_owned())
    );
    let response = post_form(addr, "/map/preset/restore", "slot=1&mode=latest-backup");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "latest-backup".to_owned()))
    );
    // A junk mode is refused before the daemon is ever asked.
    let response = post_form(addr, "/map/preset/restore", "slot=1&mode=yolo");
    assert!(
        response.contains("flash=error%3A%20unknown%20restore%20mode"),
        "{response}"
    );
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "latest-backup".to_owned())),
        "the junk mode must not have reached the control source"
    );

    // Following the redirect renders the outcome — the no-JS feedback loop
    // closed, exactly like the status page's.
    let page = body_of(&get(addr, "/map?slot=1&flash=B%20is%20now%20H.")).to_owned();
    assert!(page.contains("B is now H."), "{page}");
    assert!(page.contains("flash flash-ok"), "{page}");
    let page = body_of(&get(addr, "/map?slot=1&flash=error%3A%20nope")).to_owned();
    assert!(page.contains("flash flash-err"), "{page}");
}

/// v10, MANY KEYS → ONE CONTROL over real HTTP and with no JavaScript: the
/// same row form that binds can also ADD the picked key to what a control
/// already holds, and REMOVE just one of the keys it holds. Both are
/// read-modify-write on the key list the page already read, so a form body
/// never carries a key list it made up.
#[test]
fn the_no_js_forms_add_and_remove_one_key_at_a_time() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    let page = body_of(&get(addr, "/map")).to_owned();
    assert!(page.contains(r#"formaction="/map/add""#), "{page}");
    assert!(page.contains(r#"formaction="/map/key/remove""#), "{page}");
    // The fixture's B holds two keys: both are on the page, each with its own
    // remove payload, and neither reader spells them as a chord.
    assert!(page.contains(r#"data-rmkey="B|S""#), "{page}");
    assert!(page.contains(r#"data-rmkey="B|Enter""#), "{page}");
    assert!(!page.contains("S+Enter"), "{page}");

    // REMOVE ONE: B keeps S, loses Enter — and because one key is left, this
    // daemon's single-key `map` verb can express it exactly.
    let response = post_form(addr, "/map/key/remove", "slot=1&function=B&key=Enter");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("flash=Enter%20removed.%20B%20is%20now%20S."),
        "{response}"
    );
    assert_eq!(
        control
            .bound_with
            .lock()
            .unwrap()
            .clone()
            .unwrap()
            .key
            .as_deref(),
        Some("S"),
        "the survivor is what gets written"
    );

    // A key the control does not have is a refusal that names what it DOES
    // have — never a silent no-op, and never a write.
    let response = post_form(addr, "/map/key/remove", "slot=1&function=B&key=J");
    assert!(response.contains("flash=error%3A"), "{response}");
    assert!(
        response.contains("it%20has%20S%20%C2%B7%20Enter"),
        "{response}"
    );

    // ADD onto an UNBOUND control is an ordinary bind: nothing to keep.
    let response = post_form(addr, "/map/add", "slot=1&function=X&key=J");
    assert!(response.contains("flash=X%20is%20now%20J."), "{response}");

    // ADD onto a control that already has keys is the OR-chain the engine
    // executes — and the honest limit of today's wire: the map verb writes one
    // key per control and would drop the rest, so the write is REFUSED in
    // words rather than made silently lossy. (The day the verb takes a key
    // list, `ControlSource::bind_keys` writes it and this flash becomes the
    // success sentence, with nothing else on the page changing.)
    let before = control.bound_with.lock().unwrap().clone();
    let response = post_form(addr, "/map/add", "slot=1&function=B&key=J");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("flash=error%3A"), "{response}");
    assert!(response.contains("ONE%20key%20per%20control"), "{response}");
    assert_eq!(
        control.bound_with.lock().unwrap().clone().map(|b| b.key),
        before.map(|b| b.key),
        "a refused multi-key write must not have written anything"
    );

    // Adding a key the control already has changes nothing and says so.
    let response = post_form(addr, "/map/add", "slot=1&function=A&key=G");
    assert!(response.contains("already%20has%20G"), "{response}");

    // No key picked: the same honest refusal the Bind button gives.
    let response = post_form(addr, "/map/add", "slot=1&function=B&key=");
    assert!(
        response.contains("flash=error%3A%20no%20key%20picked"),
        "{response}"
    );
}

/// The JSON twin: the island computes the SET it wants and posts it whole, so
/// add, remove-one and undo all land through one writer.
#[test]
fn the_key_list_route_writes_a_whole_set() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // One key: an ordinary bind.
    let response = post_json(
        addr,
        "/api/bind/keys",
        r#"{"preset":"IPAC P1","function":"B","keys":["H"],"force":false,"reload":true}"#,
    );
    assert!(body_of(&response).contains(r#""ok":true"#), "{response}");
    let bound = control.bound_with.lock().unwrap().clone().unwrap();
    assert_eq!(bound.key.as_deref(), Some("H"));
    assert!(bound.reload);

    // No keys: a clear, through the same `map` verb.
    let response = post_json(
        addr,
        "/api/bind/keys",
        r#"{"preset":"IPAC P1","function":"B","keys":[],"reload":true}"#,
    );
    assert!(body_of(&response).contains(r#""ok":true"#), "{response}");
    assert_eq!(
        control.bound_with.lock().unwrap().clone().unwrap().key,
        None
    );

    // Two keys: refused, in words that name the missing wire field — and
    // nothing was written.
    let response = post_json(
        addr,
        "/api/bind/keys",
        r#"{"preset":"IPAC P1","function":"B","keys":["S","Enter"],"reload":true}"#,
    );
    let body = body_of(&response);
    assert!(body.contains(r#""ok":false"#), "{response}");
    assert!(body.contains("ONE key per control"), "{response}");
    assert_eq!(
        control.bound_with.lock().unwrap().clone().unwrap().key,
        None,
        "the refusal must not have written the first key"
    );
}

/// No daemon: the forms are still there (dimmed by CSS, never removed) and a
/// post still answers with the reason — the no-JS half of FIX 1's "never a
/// silent no-op".
#[test]
fn a_no_js_post_without_a_daemon_flashes_the_reason() {
    let addr = start_server(Arc::new(ScriptedControl::dead()));
    let page = body_of(&get(addr, "/map")).to_owned();
    assert!(page.contains(r#"class="lbind nojs off""#), "{page}");
    assert!(page.contains(r#"action="/map/bind""#), "{page}");

    let response = post_form(addr, "/map/preset/clear-all", "slot=1");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("flash=error%3A"), "{response}");
    assert!(
        response.contains("no%20daemon%20control%20channel"),
        "{response}"
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
