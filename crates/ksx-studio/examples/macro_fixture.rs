//! A standalone ksx Studio serving FIXED data, for browser-level tests of the
//! client island (`studio-ui/pwtest`).
//!
//! The macro editor's state lives entirely in the browser — the draft, which
//! step the duration editor points at, the authored unit — and none of it is
//! reachable from a Rust test. So the DOM-level tests drive a real page, and
//! this is the backend they drive it against: the same `ksx_studio::serve` the
//! app uses, wired to a preset that never changes underfoot.
//!
//! Saves are kept in memory and served back by the next `/api/map` poll, which
//! is what makes "the unit survives save and reload" testable at all.
//!
//! Loopback only, and the port is an argument so it can never collide with the
//! user's own `ksx studio` (4460):
//!
//! ```text
//! cargo run -p ksx-studio --example macro_fixture -- 4476
//! ```

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use ksx_studio::{
    ControlSource, MacroOutcome, MacroSnapshot, MacroStepView, MacroView, MacroWrite, MapperSlot,
    MapperSnapshot, PadRow, ProfileRow, SessionView, StatusSnapshot, StatusSource,
};

const PRESET: &str = "IPAC P1";

fn mac(name: &str, steps: Vec<MacroStepView>) -> MacroView {
    MacroView {
        name: name.into(),
        steps,
        on_release: "finish".into(),
        retrigger: "ignore".into(),
        interrupt: "none".into(),
        repeat: "once".into(),
        turbo_hz: None,
        gap_ms: None,
        triggers: vec!["P".into()],
        disabled: false,
    }
}

fn ms_step(hold: &[&str], ms: u32) -> MacroStepView {
    MacroStepView {
        hold: hold.iter().map(|s| (*s).to_owned()).collect(),
        ms: Some(ms),
        frames: None,
        allow_short: false,
    }
}

/// The preset the page opens on.
///
/// `piano` — step 1 authored in `ms`, step 2 authored in `frames`: the two
/// spellings §1c keeps apart, so a test can watch either one round-trip.
///
/// `written-by-hand` — steps NOBODY MADE THROUGH THIS PAGE. Named to sort
/// AFTER `piano`: `save_macro` re-sorts the table, and the page opens on the
/// FIRST macro, so a fixture macro that sorted first would silently become what
/// every other test is looking at once anything is saved. This is the round trip
/// the diagonal lens promises: a hold that names two ordinary bindings must
/// DISPLAY as the diagonal, including when it is spelled at a partial
/// deflection, when a button rides along with it, and (never) when it
/// contradicts itself.
fn seed_macros() -> Vec<MacroView> {
    vec![
        mac(
            "piano",
            vec![
                ms_step(&["dpad.down"], 50),
                MacroStepView {
                    hold: vec!["A".into()],
                    ms: None,
                    frames: Some(3),
                    allow_short: false,
                },
                ms_step(&["B"], 80),
            ],
        ),
        mac(
            "written-by-hand",
            vec![
                // The canonical pair — reads back as D-pad ↘.
                ms_step(&["dpad.down", "dpad.right"], 50),
                // A hand-written partial deflection — still LS ↘, labelled.
                ms_step(&["ly.-16384", "lx.max"], 50),
                // The single most common macro step in existence.
                ms_step(&["dpad.down", "dpad.right", "A"], 50),
                // Contradictory: never folded, never guessed.
                ms_step(&["dpad.down", "dpad.right", "dpad.up"], 50),
                // The hat+stick double-binding every in-box template writes.
                ms_step(&["dpad.down", "dpad.right", "ly.min", "lx.max"], 50),
            ],
        ),
    ]
}

/// The one piece of state the fixture keeps: what Save wrote, so the poll that
/// follows serves it back exactly as a real preset file would.
#[derive(Clone)]
struct Store(Arc<Mutex<Vec<MacroView>>>);

impl Store {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(seed_macros())))
    }
}

impl StatusSource for Store {
    fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "fixture".into(),
            vigem: "installed".into(),
            interception: "installed".into(),
            daemon_running: true,
            daemon_detail: "fixture".into(),
            autostart: "not registered".into(),
            pads: vec![PadRow {
                persona: "Xbox 360 pad".into(),
                instance: "USB\\FIXTURE\\1".into(),
            }],
            profiles: vec![ProfileRow {
                title: "Fixture".into(),
                detail: "C:\\fixture.exe — 1 slot".into(),
            }],
            config_root: "C:\\fixture".into(),
        }
    }

    fn mapper(&self) -> MapperSnapshot {
        MapperSnapshot {
            generated_at: "fixture".into(),
            source: "fixture".into(),
            config_root: "C:\\fixture".into(),
            slots: vec![MapperSlot {
                number: 1,
                persona: "xbox360".into(),
                persona_label: "Xbox 360".into(),
                preset: PRESET.into(),
                keyboard: "HID\\FIXTURE".into(),
                bindings: BTreeMap::from([("A".to_owned(), vec!["G".to_owned()])]),
                backup: None,
                turbo: BTreeMap::new(),
                macros_off: false,
            }],
        }
    }

    fn macros(&self, preset: &str) -> MacroSnapshot {
        MacroSnapshot::read(preset, self.0.lock().unwrap().clone())
    }
}

impl ControlSource for Store {
    fn session(&self) -> SessionView {
        SessionView {
            reachable: true,
            running: false,
            line: "idle".into(),
            profile: None,
        }
    }

    fn start(&self, _profile: Option<&str>) -> Result<String, String> {
        Ok("running (1 slot(s))".into())
    }

    fn stop(&self) -> Result<String, String> {
        Ok("stopped".into())
    }

    fn reload(&self) -> Result<String, String> {
        Ok("running (1 slot(s))".into())
    }

    /// Whole table in, whole table out — the same shape `mapping::save_macro`
    /// writes, so what the next poll serves is what the grid sent.
    fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
        let mut held = self.0.lock().unwrap();
        held.retain(|m| !m.name.eq_ignore_ascii_case(&request.name));
        if !request.delete {
            held.push(MacroView {
                name: request.name.clone(),
                steps: request.steps.clone(),
                on_release: request.on_release.clone(),
                retrigger: request.retrigger.clone(),
                interrupt: request.interrupt.clone(),
                repeat: request.repeat.clone(),
                turbo_hz: request.turbo_hz,
                gap_ms: request.gap_ms,
                // Triggers live in `[bindings]`, not in the macro table — the
                // real writer does not touch them either.
                triggers: vec!["P".into()],
                // A whole-table write carries the flag like any field.
                disabled: request.enabled == Some(false),
            });
            held.sort_by(|a, b| a.name.cmp(&b.name));
        }
        MacroOutcome {
            ok: true,
            message: Some(format!("\"{PRESET}\": macro \"{}\" saved", request.name)),
            deleted: request.delete,
            reloaded: request.reload,
            ..MacroOutcome::default()
        }
    }
}

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(4476);
    let bind: SocketAddr = ([127, 0, 0, 1], port).into();
    let store = Store::new();
    println!("macro fixture on http://{bind}/map");
    if let Err(err) = ksx_studio::serve(bind, Box::new(store.clone()), Box::new(store)) {
        eprintln!("fixture failed: {err}");
        std::process::exit(1);
    }
}
