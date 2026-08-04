//! End-to-end tests for the `ksx run` supervisor, driver-free.
//!
//! Every test here drives the **real** [`supervise`] — real threads, real
//! channels, the real engine, the real teardown order — with
//! `MockCaptureBackend` in front and `MockBackend` behind. Nothing is stubbed
//! except the two drivers, so these run in CI on a machine with neither
//! Interception nor ViGEmBus installed.
//!
//! What they pin:
//! - the recorded cabinet session replays through the whole pipeline and
//!   produces exactly the pad updates a bare `Engine` would (all four slots);
//! - LeftCtrl ×5 toggles capture off and on again;
//! - Ctrl+Alt+Del tears the session down cleanly, exit 0;
//! - teardown always uncaptures **before** it unplugs — asserted against the
//!   real pad log interleaved with the real capture-control log, not against a
//!   step the supervisor writes down after the fact;
//! - an escape still fires with the output thread wedged (the lockout case);
//! - an engine-thread panic still uncaptures (keyboards come back);
//! - a bound device disappearing invalidates its slots and the run continues;
//! - every failure before capture starts exits 2, and duplicate hardware ids
//!   are refused before anything is plugged.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ksx_capture::keymap::{corrected_key, KEY_DOWN, KEY_E0, KEY_UP};
use ksx_capture::{
    CaptureBackend, CaptureCtl, DeviceInfo, DeviceKind, MockCaptureBackend, MockStroke,
    PresenceHandle,
};
use ksx_config::PresetFile;
use ksx_core::{
    DeviceId, Engine, InvalidationReason, Key, KeyEvent, PadState, ResolvedSlot, SlotSpec,
};
use ksx_output::{Feedback, MockBackend, OutputError, PadHandle, VirtualPadBackend};

use super::latency::Clock;
use super::plan::{PlanSource, RunPlan};
use super::supervisor::{supervise, Outcome, RunOptions, Step, StopReason, Trace, Wiring};

const IPAC: &str = r"HID\VID_D209&PID_0430&REV_0056&MI_00";
const DESK: &str = r"HID\VID_046D&PID_C31C&REV_6402&MI_00";

// ---------------------------------------------------------------------------
// Observable pad backend
// ---------------------------------------------------------------------------

/// What the output thread did to the pads, in order. Wrapping `MockBackend`
/// (rather than extending it) keeps the observation local to these tests while
/// still exercising its real XInput-slot assignment.
#[derive(Clone, Debug, PartialEq)]
enum PadEvent {
    Plug(u32),
    Update(u32, PadState),
    Unplug(u32),
}

/// A control message as the *capture backend* applied it. Recorded into the
/// same timeline as the pad events so teardown ordering can be asserted against
/// what actually happened to the two drivers, in one total order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtlKind {
    Captured,
    Passthrough,
    Shutdown,
}

impl CtlKind {
    fn of(message: &CaptureCtl) -> Self {
        match message {
            CaptureCtl::SetCaptured(_) => CtlKind::Captured,
            CaptureCtl::SetPassthrough => CtlKind::Passthrough,
            CaptureCtl::Shutdown => CtlKind::Shutdown,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum TraceEvent {
    Pad(PadEvent),
    Ctl(CtlKind),
}

/// One shared, mutex-ordered timeline of everything both mock drivers did.
#[derive(Clone, Default)]
struct PadLog(Arc<Mutex<Vec<TraceEvent>>>);

impl PadLog {
    fn push(&self, event: TraceEvent) {
        self.0.lock().expect("pad log poisoned").push(event);
    }

    fn timeline(&self) -> Vec<TraceEvent> {
        self.0.lock().expect("pad log poisoned").clone()
    }

    fn events(&self) -> Vec<PadEvent> {
        self.timeline()
            .into_iter()
            .filter_map(|e| match e {
                TraceEvent::Pad(pad) => Some(pad),
                TraceEvent::Ctl(_) => None,
            })
            .collect()
    }
}

/// `update` fails from the Nth call on (0 = never) — for the "output thread
/// dies mid-session" ordering test.
struct RecordingBackend {
    inner: MockBackend,
    log: PadLog,
    fail_update_after: Option<usize>,
    updates: usize,
}

impl RecordingBackend {
    fn new(log: PadLog) -> Self {
        Self {
            inner: MockBackend::new(),
            log,
            fail_update_after: None,
            updates: 0,
        }
    }

    fn failing_after(log: PadLog, updates: usize) -> Self {
        Self {
            fail_update_after: Some(updates),
            ..Self::new(log)
        }
    }
}

impl VirtualPadBackend for RecordingBackend {
    fn plug(&mut self) -> Result<PadHandle, OutputError> {
        let handle = self.inner.plug()?;
        self.log.push(TraceEvent::Pad(PadEvent::Plug(handle.raw())));
        Ok(handle)
    }

    fn persona(&self, handle: PadHandle) -> Option<ksx_core::Persona> {
        self.inner.persona(handle)
    }

    fn user_index(&self, handle: PadHandle) -> Option<u8> {
        self.inner.user_index(handle)
    }

    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
        self.updates += 1;
        if self.fail_update_after.is_some_and(|n| self.updates > n) {
            return Err(OutputError::PlugTimeout(Duration::from_millis(1)));
        }
        self.inner.update(handle, state)?;
        self.log
            .push(TraceEvent::Pad(PadEvent::Update(handle.raw(), *state)));
        Ok(())
    }

    fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
        self.inner.poll_feedback(handle)
    }

    fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
        self.inner.unplug(handle)?;
        self.log
            .push(TraceEvent::Pad(PadEvent::Unplug(handle.raw())));
        Ok(())
    }
}

/// Index of the first pad unplug in the timeline.
fn first_unplug(timeline: &[TraceEvent]) -> usize {
    timeline
        .iter()
        .position(|e| matches!(e, TraceEvent::Pad(PadEvent::Unplug(_))))
        .unwrap_or_else(|| panic!("no pad was ever unplugged in {timeline:?}"))
}

/// The teardown-order contract, asserted against what the drivers actually did:
/// the capture backend was put back in passthrough before the first pad went
/// away, and nothing re-captured in between.
fn assert_uncaptured_before_unplug(timeline: &[TraceEvent]) {
    let unplug = first_unplug(timeline);
    let release = timeline[..unplug]
        .iter()
        .rposition(|e| *e == TraceEvent::Ctl(CtlKind::Passthrough))
        .unwrap_or_else(|| {
            panic!("the pads were unplugged while keyboards were still captured: {timeline:?}")
        });
    assert!(
        !timeline[release..unplug].contains(&TraceEvent::Ctl(CtlKind::Captured)),
        "capture was re-armed between the release and the unplug: {timeline:?}"
    );
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn keyboard(id: &str, slot: u8) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::from(id),
        interception_slot: Some(slot),
        friendly: None,
        kind: DeviceKind::Keyboard,
    }
}

/// A one-slot plan on an inert preset: escape tests want zero pad traffic.
fn escape_plan() -> RunPlan {
    let preset = ksx_core::Preset::builtin_empty();
    RunPlan {
        source: PlanSource::Config,
        config_path: PathBuf::from("test"),
        slots: vec![ResolvedSlot {
            spec: SlotSpec::new(1, Some(DeviceId::from(IPAC)), None, preset.name.clone())
                .expect("valid slot"),
            preset,
        }],
        block_keyboards: true,
        block_mice: false,
        captureable: vec![DeviceId::from(IPAC)],
        // These plans drive mock backends; no board is WinUSB-claimed, so the
        // pipeline runs entirely through the Interception path.
        winusb: Vec::new(),
        notes: Vec::new(),
    }
}

/// The cabinet's real layout: one I-PAC feeding four slots through the disjoint
/// `IPAC P1..P4` presets imported from the legacy XML — same fixture the
/// `tests/replay.rs` oracle uses.
pub(super) fn cabinet_slots() -> Vec<ResolvedSlot> {
    let fixtures = repo_root().join("crates/ksx-legacy-import/tests/fixtures");
    let import = ksx_legacy_import::import_dir(&fixtures).expect("import fixtures");
    let ipac = DeviceId::from(IPAC);
    let by_name: BTreeMap<String, _> = import
        .presets
        .iter()
        .map(|p: &PresetFile| (p.name.clone(), p.to_core().expect("preset converts")))
        .collect();
    (1..=4u8)
        .map(|n| {
            let preset = by_name
                .get(&format!("IPAC P{n}"))
                .unwrap_or_else(|| panic!("preset IPAC P{n} in fixtures"))
                .clone();
            ResolvedSlot {
                spec: SlotSpec::new(n, Some(ipac.clone()), None, preset.name.clone())
                    .expect("valid slot"),
                preset,
            }
        })
        .collect()
}

fn cabinet_plan() -> RunPlan {
    RunPlan {
        source: PlanSource::Config,
        config_path: PathBuf::from("test"),
        slots: cabinet_slots(),
        block_keyboards: true,
        block_mice: false,
        captureable: vec![DeviceId::from(IPAC)],
        // These plans drive mock backends; no board is WinUSB-claimed, so the
        // pipeline runs entirely through the Interception path.
        winusb: Vec::new(),
        notes: Vec::new(),
    }
}

/// Reverse of [`corrected_key`]: the (scancode, state-prefix) a driver would
/// have to deliver to produce `key`.
///
/// Built by brute force over the real correction table rather than a
/// hand-written twin, so it can never drift from what the capture layer does.
/// Plain scancodes win over E0 forms when both exist (that is how a keyboard
/// actually sends them).
fn scancode_table() -> BTreeMap<Key, (u16, u16)> {
    let mut table = BTreeMap::new();
    for prefix in [KEY_DOWN, KEY_E0, ksx_capture::keymap::KEY_E1] {
        for code in 0u16..=255 {
            let key = corrected_key(code, prefix);
            if key != Key::Unknown && key != Key::None {
                table.entry(key).or_insert((code, prefix));
            }
        }
    }
    table
}

pub(super) struct CorpusEvent {
    pub(super) device: DeviceId,
    pub(super) key: Key,
    pub(super) down: bool,
}

/// The recorded cabinet session (`ksx monitor --record`), 392 real events.
pub(super) fn load_corpus() -> Vec<CorpusEvent> {
    let path = repo_root().join("crates/ksx-capture/tests/corpus/ipac-session-001.jsonl");
    let text = std::fs::read_to_string(&path).expect("corpus present");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            // Hand-parsed on purpose: the corpus format is a stable contract
            // (documented in monitor.rs), not a serde type.
            let field = |name: &str| -> String {
                let key = format!("\"{name}\":");
                let rest = &line[line.find(&key).expect("field") + key.len()..];
                let rest = rest.trim_start();
                if let Some(stripped) = rest.strip_prefix('"') {
                    let end = stripped.find('"').expect("closing quote");
                    stripped[..end].replace("\\\\", "\\")
                } else {
                    rest.split([',', '}'])
                        .next()
                        .expect("value")
                        .trim()
                        .to_string()
                }
            };
            CorpusEvent {
                device: DeviceId::from(field("device")),
                key: Key::from_name(&field("key")).expect("key in the ksx-core vocabulary"),
                down: field("down") == "true",
            }
        })
        .collect()
}

/// Every distinct device the corpus mentions, in first-seen order. The session
/// was recorded with the cabinet's I-PAC *and* a plain desk keyboard, which is
/// exactly the interesting case: only the I-PAC is bound to slots, so the desk
/// keyboard must be seen by the engine and never captured.
fn corpus_devices(events: &[CorpusEvent]) -> Vec<DeviceInfo> {
    let mut devices: Vec<DeviceInfo> = Vec::new();
    for event in events {
        if !devices.iter().any(|d| d.id == event.device) {
            let slot = devices.len() as u8 + 1;
            devices.push(DeviceInfo {
                id: event.device.clone(),
                interception_slot: Some(slot),
                friendly: None,
                kind: DeviceKind::Keyboard,
            });
        }
    }
    devices
}

/// Turn corpus events into driver-level strokes for `MockCaptureBackend`.
fn corpus_script(events: &[CorpusEvent], devices: &[DeviceInfo]) -> Vec<MockStroke> {
    let table = scancode_table();
    events
        .iter()
        .map(|e| {
            let (code, prefix) = *table
                .get(&e.key)
                .unwrap_or_else(|| panic!("no scancode encodes {:?}", e.key));
            let index = devices
                .iter()
                .position(|d| d.id == e.device)
                .unwrap_or_else(|| panic!("corpus device {} not in the fixture", e.device));
            MockStroke {
                device: index,
                code,
                state: if e.down { prefix } else { prefix | KEY_UP },
            }
        })
        .collect()
}

fn key_stroke(device: usize, key: Key, down: bool) -> MockStroke {
    let (code, prefix) = *scancode_table()
        .get(&key)
        .unwrap_or_else(|| panic!("no scancode encodes {key:?}"));
    MockStroke {
        device,
        code,
        state: if down { prefix } else { prefix | KEY_UP },
    }
}

/// One full supervised session against the mock drivers.
struct Session {
    outcome: Outcome,
    trace: Vec<Step>,
    pads: Vec<PadEvent>,
    /// Pad events and capture-control messages in one total order.
    timeline: Vec<TraceEvent>,
    output: String,
    /// Strokes the capture backend re-sent to "the OS" — i.e. everything that
    /// was NOT blocked.
    resent: Vec<ksx_capture::ResentStroke>,
}

fn run_session(
    plan: &RunPlan,
    devices: Vec<DeviceInfo>,
    script: Vec<MockStroke>,
    pace: Option<Duration>,
    tweak: impl FnOnce(&mut RunOptions, &PresenceHandle),
) -> Session {
    run_session_with(plan, devices, script, pace, tweak, RecordingBackend::new)
}

fn run_session_with(
    plan: &RunPlan,
    devices: Vec<DeviceInfo>,
    script: Vec<MockStroke>,
    pace: Option<Duration>,
    tweak: impl FnOnce(&mut RunOptions, &PresenceHandle),
    pads: impl FnOnce(PadLog) -> RecordingBackend,
) -> Session {
    let log = PadLog::default();
    let mut capture = MockCaptureBackend::new(devices, script).with_ctl_observer({
        let log = log.clone();
        Box::new(move |message| log.push(TraceEvent::Ctl(CtlKind::of(message))))
    });
    if let Some(pace) = pace {
        capture = capture.with_pace(pace);
    }
    let presence = capture.presence();
    let resent = capture.resent_log();
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));

    let mut options = RunOptions {
        clock: Clock::monotonic_nanos(),
        trace: Some(trace.clone()),
        plug_deadline: Duration::from_secs(10),
        ..RunOptions::default()
    };
    tweak(&mut options, &presence);

    let wiring = Wiring {
        capture: Box::new(capture),
        pads: Box::new(pads(log.clone())),
    };
    let mut out: Vec<u8> = Vec::new();
    let outcome = supervise(plan, wiring, &mut options, &mut out).expect("supervisor ran");

    let steps = trace.lock().expect("trace poisoned").clone();
    let resent = resent.lock().expect("resent log poisoned").clone();
    Session {
        outcome,
        trace: steps,
        pads: log.events(),
        timeline: log.timeline(),
        output: String::from_utf8(out).expect("utf-8 output"),
        resent,
    }
}

fn index_of(trace: &[Step], step: Step) -> usize {
    trace
        .iter()
        .position(|s| *s == step)
        .unwrap_or_else(|| panic!("{step:?} never happened in {trace:?}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn recorded_cabinet_session_drives_all_four_pads_through_the_real_pipeline() {
    let events = load_corpus();
    assert!(events.len() > 300, "corpus should hold the full session");
    let devices = corpus_devices(&events);
    assert!(
        devices.len() > 1,
        "the recorded session included an unassigned keyboard; that is the point"
    );
    let script = corpus_script(&events, &devices);
    let plan = cabinet_plan();

    let session = run_session(&plan, devices.clone(), script, None, |opts, _| {
        opts.max_events = Some(events.len() as u64);
    });

    assert_eq!(session.outcome.stop, StopReason::EventLimit);
    assert_eq!(session.outcome.exit_code(), 0);
    assert_eq!(session.outcome.events, events.len() as u64);

    // Four pads, four distinct XInput user indices, reported by slot number.
    assert_eq!(session.outcome.pads.len(), 4);
    assert_eq!(
        session
            .outcome
            .pads
            .iter()
            .map(|p| (p.slot, p.user_index))
            .collect::<Vec<_>>(),
        vec![(1, Some(0)), (2, Some(1)), (3, Some(2)), (4, Some(3))]
    );
    assert!(
        session.output.contains("slot 1 -> XInput user index 0"),
        "the slot→user-index mapping must be printed: {}",
        session.output
    );

    // The pipeline's pad updates must equal, transition for transition, what a
    // bare Engine produces from the same events — the replay oracle, but end to
    // end through three threads and two channels.
    let handle_of: BTreeMap<u8, u32> = session
        .outcome
        .pads
        .iter()
        .map(|p| (p.slot, p.handle))
        .collect();
    let mut engine = Engine::new(cabinet_slots());
    let expected: Vec<(u32, PadState)> = events
        .iter()
        .enumerate()
        .flat_map(|(i, e)| {
            engine
                .handle(&KeyEvent {
                    device: e.device.clone(),
                    key: e.key,
                    down: e.down,
                    t: i as u64,
                })
                .into_iter()
                .map(|d| (handle_of[&d.slot], d.state))
                .collect::<Vec<_>>()
        })
        .collect();

    let mut actual = session.pads.iter().filter_map(|e| match e {
        PadEvent::Update(handle, state) => Some((*handle, *state)),
        _ => None,
    });
    // The output thread also writes one neutral state per pad at teardown, so
    // compare the session's transitions as a prefix.
    let observed: Vec<(u32, PadState)> = actual.by_ref().take(expected.len()).collect();
    if session.outcome.deltas_coalesced == 0 {
        assert_eq!(
            observed, expected,
            "the live pipeline diverged from the pure engine"
        );
    } else {
        // Coalescing is level-triggered and therefore ALLOWED to drop
        // intermediate states — that is the M4 fix that stops a stalled output
        // thread from blocking the engine. Under parallel CI load it does fire
        // here (observed 305 of 311 once, agreeing for the first 301 and then
        // simply shorter), so demanding exact equality makes this test a
        // load-sensitive flake rather than a correctness check.
        //
        // What must still hold is the property coalescing actually promises:
        // every state that survived is one the pure engine produced, in order,
        // and each pad ends where an un-squashed run would have left it. The
        // stronger equality above still runs whenever nothing was coalesced,
        // which is the ordinary case.
        let mut it = expected.iter();
        assert!(
            observed.iter().all(|o| it.any(|e| e == o)),
            "coalescing dropped intermediate states, but what survived is not an \
             in-order subsequence of what the pure engine produced"
        );
        let final_of = |seq: &[(u32, PadState)]| -> BTreeMap<u32, PadState> {
            let mut last = BTreeMap::new();
            for (handle, state) in seq {
                last.insert(*handle, *state);
            }
            last
        };
        assert_eq!(
            final_of(&observed),
            final_of(&expected),
            "a coalesced burst left a pad in a different state than an un-squashed one"
        );
    }
    assert!(expected.len() > 100, "expected a busy session");

    // Every player panel drove its own pad: the one-keyboard→many-slots fan-out
    // this project exists for.
    for slot in 1..=4u8 {
        assert!(
            expected.iter().any(|(h, _)| *h == handle_of[&slot]),
            "slot {slot} saw no pad updates; fan-out is broken"
        );
    }

    // Balanced session: every pad ends neutral, and every pad is unplugged.
    for pad in &session.outcome.pads {
        assert_eq!(
            session.pads.iter().rev().find_map(|e| match e {
                PadEvent::Update(h, s) if *h == pad.handle => Some(*s),
                _ => None,
            }),
            Some(PadState::default()),
            "slot {} did not return to neutral before unplug",
            pad.slot
        );
        assert!(
            session.pads.contains(&PadEvent::Unplug(pad.handle)),
            "slot {} was never unplugged",
            pad.slot
        );
    }

    assert_eq!(session.outcome.latency.count, session.outcome.updates);

    // Blocking scope (risk review §3 item 3): the bound I-PAC was swallowed,
    // the unassigned desk keyboard kept typing. `SetCaptured` races the first
    // few strokes of an unpaced script, so assert the shape, not exact counts.
    let ipac = DeviceId::from(IPAC);
    let desk_events = events.iter().filter(|e| e.device != ipac).count();
    let desk_resent = session.resent.iter().filter(|r| r.device != ipac).count();
    let ipac_resent = session.resent.iter().filter(|r| r.device == ipac).count();
    assert!(
        desk_events > 0,
        "the corpus has unassigned-keyboard traffic"
    );
    assert_eq!(
        desk_resent, desk_events,
        "every stroke from an unassigned keyboard must reach Windows"
    );
    assert!(
        ipac_resent < events.len() - desk_events,
        "the bound I-PAC's strokes must be swallowed once blocking is on \
         ({ipac_resent} of {} re-sent)",
        events.len() - desk_events
    );
}

#[test]
fn left_ctrl_x5_toggles_capture_off_then_on() {
    let devices = vec![keyboard(IPAC, 1)];
    // Ten LeftCtrl press+release cycles: the detector fires on the 5th release
    // and again on the 10th (counters reset after each hit). The trailing
    // padding keeps the session alive past the second toggle, so the supervisor
    // observes both of them in its loop rather than in the final
    // reconciliation — this test is about the mirrored ctl state, not counting.
    let mut script: Vec<MockStroke> = (0..10)
        .flat_map(|_| {
            [
                key_stroke(0, Key::LeftControl, true),
                key_stroke(0, Key::LeftControl, false),
            ]
        })
        .collect();
    script.extend((0..5).flat_map(|_| [key_stroke(0, Key::A, true), key_stroke(0, Key::A, false)]));
    let plan = escape_plan();

    let session = run_session(
        &plan,
        devices,
        script,
        Some(Duration::from_millis(10)),
        |opts, _| {
            opts.max_events = Some(30);
        },
    );

    assert_eq!(session.outcome.stop, StopReason::EventLimit);
    assert_eq!(session.outcome.toggles, 2);
    // Startup enabled it, the first hit disabled it, the second re-enabled it,
    // and teardown disabled it again.
    assert_eq!(
        session.trace,
        vec![
            Step::PadsPlugged,
            Step::CaptureStarted,
            Step::BlockingEnabled,
            Step::BlockingDisabled,
            Step::BlockingEnabled,
            Step::BlockingDisabled,
            Step::PadsUnplugged,
        ]
    );
    assert!(
        session.output.contains("keyboard capture OFF")
            && session.output.contains("keyboard capture ON"),
        "both toggle directions must be announced: {}",
        session.output
    );
}

#[test]
fn right_ctrl_x5_is_logged_but_changes_nothing() {
    let devices = vec![keyboard(IPAC, 1)];
    let script: Vec<MockStroke> = (0..5)
        .flat_map(|_| {
            [
                key_stroke(0, Key::RightControl, true),
                key_stroke(0, Key::RightControl, false),
            ]
        })
        .collect();

    let session = run_session(&escape_plan(), devices, script, None, |opts, _| {
        opts.max_events = Some(10);
    });

    assert_eq!(
        session.outcome.toggles, 0,
        "mouse escape must not toggle keyboards"
    );
    assert!(
        session.output.contains("RightCtrl x5"),
        "the reserved mouse escape must still be reported: {}",
        session.output
    );
    // Capture state never changed between startup and teardown.
    assert_eq!(
        session.trace,
        vec![
            Step::PadsPlugged,
            Step::CaptureStarted,
            Step::BlockingEnabled,
            Step::BlockingDisabled,
            Step::PadsUnplugged,
        ]
    );
}

#[test]
fn ctrl_alt_del_stops_emulation_cleanly() {
    let devices = vec![keyboard(IPAC, 1)];
    let script = vec![
        key_stroke(0, Key::LeftControl, true),
        key_stroke(0, Key::LeftAlt, true),
        key_stroke(0, Key::Delete, true),
        // Padding the mock never reaches if the escape works.
        key_stroke(0, Key::A, true),
    ];

    let session = run_session(&escape_plan(), devices, script, None, |_, _| {});

    assert_eq!(session.outcome.stop, StopReason::EmergencyStop);
    assert_eq!(
        session.outcome.exit_code(),
        0,
        "an emergency stop is a clean exit"
    );
    assert!(
        index_of(&session.trace, Step::BlockingDisabled)
            < index_of(&session.trace, Step::PadsUnplugged),
        "{:?}",
        session.trace
    );
}

#[test]
fn an_escape_still_fires_while_its_own_device_is_being_blocked() {
    // The whole point of the escape hatch: the strokes are being swallowed
    // (nothing reaches Windows) and the gesture still works, because the
    // capture layer *reports* suppressed strokes and escapes are evaluated on
    // every reported event (risk review §3 item 2).
    let devices = vec![keyboard(IPAC, 1)];
    let script: Vec<MockStroke> = (0..5)
        .flat_map(|_| {
            [
                key_stroke(0, Key::LeftControl, true),
                key_stroke(0, Key::LeftControl, false),
            ]
        })
        .collect();

    // Paced, so `SetCaptured` is definitely in force before the first stroke.
    let session = run_session(
        &escape_plan(),
        devices,
        script,
        Some(Duration::from_millis(15)),
        |opts, _| opts.max_events = Some(10),
    );

    assert_eq!(session.outcome.toggles, 1);
    // Nine of the ten strokes were swallowed. The tenth — the one that completed
    // the gesture — passes through, because escapes are evaluated BEFORE the
    // pass/suppress decision (legacy `CheckForEmergencyHit` order): by the time
    // that stroke is decided, the capture thread has already freed itself.
    assert_eq!(
        session.resent.len(),
        1,
        "only the gesture-completing stroke may reach Windows: {:?}",
        session.resent
    );
    assert_eq!(session.resent[0].state, KEY_UP);
}

/// The lockout this whole fix exists for: the output thread is wedged inside a
/// driver call (a ViGEm IOCTL that never returns), so the delta channel fills.
/// Before the fix the engine blocked on `deltas.send()`, stopped draining key
/// events, and the escape — evaluated on that same engine thread — never fired:
/// every cabinet keyboard stayed captured with no way back.
#[test]
fn a_wedged_output_thread_cannot_prevent_un_capturing() {
    // Release the wedge the moment the keyboards are freed. If un-capturing
    // required the output thread to make progress, this would deadlock; the
    // bounded wait below turns that into a failure instead of a hung test.
    let released = Arc::new(AtomicBool::new(false));
    let log = PadLog::default();

    struct Wedged {
        inner: MockBackend,
        released: Arc<AtomicBool>,
        log: PadLog,
    }
    impl VirtualPadBackend for Wedged {
        fn plug(&mut self) -> Result<PadHandle, OutputError> {
            let handle = self.inner.plug()?;
            self.log.push(TraceEvent::Pad(PadEvent::Plug(handle.raw())));
            Ok(handle)
        }
        fn persona(&self, handle: PadHandle) -> Option<ksx_core::Persona> {
            self.inner.persona(handle)
        }
        fn user_index(&self, handle: PadHandle) -> Option<u8> {
            self.inner.user_index(handle)
        }
        fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !self.released.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            self.inner.update(handle, state)?;
            self.log
                .push(TraceEvent::Pad(PadEvent::Update(handle.raw(), *state)));
            Ok(())
        }
        fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
            self.inner.poll_feedback(handle)
        }
        fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
            self.inner.unplug(handle)?;
            self.log
                .push(TraceEvent::Pad(PadEvent::Unplug(handle.raw())));
            Ok(())
        }
    }

    // Enough traffic to overrun the 1024-deep delta channel while the output
    // thread is stuck in its first update.
    let mut script: Vec<MockStroke> = (0..700)
        .flat_map(|_| [key_stroke(0, Key::A, true), key_stroke(0, Key::A, false)])
        .collect();
    script.extend((0..5).flat_map(|_| {
        [
            key_stroke(0, Key::LeftControl, true),
            key_stroke(0, Key::LeftControl, false),
        ]
    }));
    script.push(key_stroke(0, Key::LeftControl, true));
    script.push(key_stroke(0, Key::LeftAlt, true));
    script.push(key_stroke(0, Key::Delete, true));

    let capture = MockCaptureBackend::new(vec![keyboard(IPAC, 1)], script).with_ctl_observer({
        let log = log.clone();
        let released = Arc::clone(&released);
        Box::new(move |message| {
            log.push(TraceEvent::Ctl(CtlKind::of(message)));
            if matches!(message, CaptureCtl::SetPassthrough) {
                // Keyboards are free: let the "driver" return so the test can
                // finish. Nothing about freeing them waited on this thread.
                released.store(true, Ordering::SeqCst);
            }
        })
    });
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    // Backstop so a regression fails instead of hanging forever.
    let hard_deadline = Instant::now() + Duration::from_secs(15);
    let mut options = RunOptions {
        clock: Clock::monotonic_nanos(),
        trace: Some(trace.clone()),
        plug_deadline: Duration::from_secs(10),
        stop: Box::new(move || Instant::now() > hard_deadline),
        ..RunOptions::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let outcome = supervise(
        &cabinet_plan(),
        Wiring {
            capture: Box::new(capture),
            pads: Box::new(Wedged {
                inner: MockBackend::new(),
                released: Arc::clone(&released),
                log: log.clone(),
            }),
        },
        &mut options,
        &mut out,
    )
    .expect("supervisor ran");

    assert_eq!(
        outcome.stop,
        StopReason::EmergencyStop,
        "Ctrl+Alt+Del must be seen even with the output thread stuck in a driver call \
         (a CtrlC stop here means the session only ended on the test's own deadline)"
    );
    assert_eq!(outcome.exit_code(), 0);
    assert!(
        outcome.toggles >= 1,
        "LeftCtrl x5 must also have fired while wedged"
    );
    assert!(
        outcome.deltas_coalesced > 0,
        "the delta channel must genuinely have backed up — otherwise this test \
         is not exercising the wedge"
    );
    assert!(
        released.load(Ordering::SeqCst),
        "the keyboards were never released"
    );
    assert_uncaptured_before_unplug(&log.timeline());
}

/// F2, asserted on behaviour rather than on its own counter.
///
/// `a_wedged_output_thread_cannot_prevent_un_capturing` above is an F1 test: the
/// escape hatch survives a wedge because escapes moved into the capture thread,
/// and it keeps passing with a blocking `deltas.send()`. What a blocking send
/// actually costs is *pipeline progress* — the engine parks inside the send, so
/// it stops draining key events for as long as the driver call takes.
///
/// This pins that directly: the output thread is stuck in `update` until
/// teardown releases it, and the run is only allowed to end by reaching its
/// event limit. An engine that blocks on the delta channel stalls at the
/// channel's depth (1024), never reaches the limit, and the session can only end
/// on the test's own deadline — a different `StopReason`.
#[test]
fn a_wedged_output_thread_does_not_stall_the_engine() {
    /// Every `update` parks until the keyboards are released at teardown.
    struct Wedged {
        inner: MockBackend,
        released: Arc<AtomicBool>,
    }
    impl VirtualPadBackend for Wedged {
        fn plug(&mut self) -> Result<PadHandle, OutputError> {
            self.inner.plug()
        }
        fn persona(&self, handle: PadHandle) -> Option<ksx_core::Persona> {
            self.inner.persona(handle)
        }
        fn user_index(&self, handle: PadHandle) -> Option<u8> {
            self.inner.user_index(handle)
        }
        fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !self.released.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            self.inner.update(handle, state)
        }
        fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
            self.inner.poll_feedback(handle)
        }
        fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
            self.inner.unplug(handle)
        }
    }

    // Comfortably more strokes than the engine needs, so capture-side drops
    // cannot starve it of the limit; no escape gesture anywhere in the script.
    const LIMIT: u64 = 1_500; // > DELTA_CHANNEL_CAPACITY (1024)
    let script: Vec<MockStroke> = (0..2_500)
        .flat_map(|_| [key_stroke(0, Key::A, true), key_stroke(0, Key::A, false)])
        .collect();

    let released = Arc::new(AtomicBool::new(false));
    let capture = MockCaptureBackend::new(vec![keyboard(IPAC, 1)], script).with_ctl_observer({
        let released = Arc::clone(&released);
        Box::new(move |message| {
            if matches!(message, CaptureCtl::SetPassthrough) {
                released.store(true, Ordering::SeqCst);
            }
        })
    });
    // The only other way out: a blocking engine must FAIL here, not hang CI.
    // Generous on purpose — this deadline exists to convert a hang into a
    // failure, not to measure throughput. At 10 s it flaked on a machine
    // running concurrent cargo builds (the engine simply had not been
    // scheduled enough to push LIMIT events past a 2 ms-per-step wedge), which
    // is a false red on exactly the property this test does not assert.
    let hard_deadline = Instant::now() + Duration::from_secs(120);
    let mut options = RunOptions {
        clock: Clock::monotonic_nanos(),
        plug_deadline: Duration::from_secs(10),
        max_events: Some(LIMIT),
        stop: Box::new(move || Instant::now() > hard_deadline),
        ..RunOptions::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let outcome = supervise(
        &cabinet_plan(),
        Wiring {
            capture: Box::new(capture),
            pads: Box::new(Wedged {
                inner: MockBackend::new(),
                released: Arc::clone(&released),
            }),
        },
        &mut options,
        &mut out,
    )
    .expect("supervisor ran");

    assert_eq!(
        outcome.stop,
        StopReason::EventLimit,
        "the engine must keep consuming key events while the output thread is stuck \
         in a driver call (a CtrlC stop means it parked on the delta channel and the \
         run only ended on the test's deadline)"
    );
    assert!(
        outcome.events >= LIMIT,
        "{} event(s) reached the engine, expected at least {LIMIT}",
        outcome.events
    );
    assert!(
        outcome.deltas_coalesced > 0,
        "the delta channel must genuinely have backed up — otherwise the wedge is \
         not being exercised"
    );
}

#[test]
fn escapes_fire_from_an_unassigned_keyboard_too() {
    // Risk review §3 item 2: escapes are evaluated on ANY device, not just the
    // ones bound to slots — otherwise a mis-bound cabinet has no way out.
    let devices = vec![keyboard(IPAC, 1), keyboard(DESK, 2)];
    let script = vec![
        key_stroke(1, Key::LeftControl, true),
        key_stroke(1, Key::LeftAlt, true),
        key_stroke(1, Key::Delete, true),
    ];

    let session = run_session(&escape_plan(), devices, script, None, |_, _| {});
    assert_eq!(session.outcome.stop, StopReason::EmergencyStop);
}

#[test]
fn teardown_uncaptures_before_it_unplugs() {
    let devices = vec![keyboard(IPAC, 1)];
    let script = vec![key_stroke(0, Key::A, true), key_stroke(0, Key::A, false)];

    let session = run_session(&escape_plan(), devices, script, None, |opts, _| {
        opts.max_events = Some(2);
    });

    // Asserted against what the two mock DRIVERS actually saw, in one order.
    // The `Step` log cannot prove this on its own: `PadsUnplugged` is recorded
    // by the supervisor after it joins the output thread, so "BlockingDisabled
    // came first" is true by construction there no matter what the code does.
    assert_uncaptured_before_unplug(&session.timeline);
    assert!(
        session
            .timeline
            .iter()
            .any(|e| matches!(e, TraceEvent::Pad(PadEvent::Unplug(_)))),
        "nothing was unplugged, so the ordering claim is vacuous: {:?}",
        session.timeline
    );

    let disabled = index_of(&session.trace, Step::BlockingDisabled);
    let unplugged = index_of(&session.trace, Step::PadsUnplugged);
    assert!(
        disabled < unplugged,
        "keyboards must be released before the pads go away: {:?}",
        session.trace
    );
    // And blocking was only ever enabled after the pads were up.
    assert!(
        index_of(&session.trace, Step::PadsPlugged)
            < index_of(&session.trace, Step::BlockingEnabled),
        "{:?}",
        session.trace
    );
    assert!(
        index_of(&session.trace, Step::CaptureStarted)
            < index_of(&session.trace, Step::BlockingEnabled),
        "{:?}",
        session.trace
    );
}

#[test]
fn an_engine_thread_panic_still_frees_the_keyboards() {
    let devices = vec![keyboard(IPAC, 1)];
    // EXACTLY one stroke, and that is load-bearing rather than minimal.
    //
    // The engine panics on the event this stroke produces, which drops the
    // capture→engine receiver. A mock that is still feeding that channel then
    // sees `Disconnected` and exits `ChannelClosed` — and the supervisor checks
    // the capture thread *before* the engine thread, so it would report
    // `CaptureEnded` instead of `EngineThreadDied` depending on which thread the
    // scheduler ran first. That is what made this test fail about one run in
    // five under a loaded `cargo test` (and never in isolation).
    //
    // With one stroke the mock has provably finished sending before the panic
    // it causes can happen: it is parked in its idle `ctl.recv()` loop, never
    // touches the channel again, and the only thread that can die is the engine.
    // The recovery path under test — real panic, real teardown, real ordering —
    // is unchanged.
    let script = vec![key_stroke(0, Key::A, true)];

    // The panic message is printed by the default hook; that noise is the price
    // of proving the recovery path with a real panic instead of a simulated one.
    let session = run_session(&escape_plan(), devices, script, None, |opts, _| {
        opts.panic_after_events = Some(1);
    });

    assert_eq!(session.outcome.stop, StopReason::EngineThreadDied);
    assert_eq!(
        session.outcome.exit_code(),
        super::EXIT_RUNTIME_FAILURE,
        "a dead pipeline thread is a failure, not a clean stop"
    );
    assert!(
        index_of(&session.trace, Step::BlockingDisabled)
            < index_of(&session.trace, Step::PadsUnplugged),
        "{:?}",
        session.trace
    );
    // Pads still came back down.
    let plugged: Vec<u32> = session
        .pads
        .iter()
        .filter_map(|e| match e {
            PadEvent::Plug(h) => Some(*h),
            _ => None,
        })
        .collect();
    for handle in plugged {
        assert!(
            session.pads.contains(&PadEvent::Unplug(handle)),
            "pad {handle} leaked after the panic"
        );
    }
}

#[test]
fn unplugging_a_bound_device_invalidates_its_slots_without_crashing() {
    let devices = vec![keyboard(IPAC, 1)];
    let script: Vec<MockStroke> = (0..8)
        .flat_map(|_| [key_stroke(0, Key::A, true), key_stroke(0, Key::A, false)])
        .collect();
    let plan = cabinet_plan();

    let session = run_session(
        &plan,
        devices,
        script,
        Some(Duration::from_millis(20)),
        |opts, presence| {
            opts.max_events = Some(16);
            // The board vanishes: the capture backend republishes an empty
            // device set, exactly as the Interception idle rescan would.
            presence.publish(Vec::new());
        },
    );

    assert_eq!(
        session.outcome.stop,
        StopReason::EventLimit,
        "a hotplug removal must never end the session"
    );
    assert_eq!(
        session.outcome.invalidated,
        (1..=4)
            .map(|slot| (slot, InvalidationReason::KeyboardUnplugged))
            .collect::<Vec<_>>(),
        "every slot fed by the vanished board must be invalidated"
    );
    assert!(
        session.output.contains("device unplugged"),
        "the removal must be reported loudly: {}",
        session.output
    );
    // Capture was dropped to passthrough once the last bound device left.
    assert!(
        session.trace.contains(&Step::BlockingDisabled),
        "{:?}",
        session.trace
    );
}

/// A pad update failing mid-session must not make the output thread tear its
/// own pads down: that would unplug while the keyboards are still captured.
/// It reports, and the supervisor drives the order.
#[test]
fn an_output_error_still_uncaptures_before_the_pads_go_away() {
    let devices = vec![keyboard(IPAC, 1)];
    let script: Vec<MockStroke> = (0..40)
        .flat_map(|_| [key_stroke(0, Key::A, true), key_stroke(0, Key::A, false)])
        .collect();

    let session = run_session_with(
        &cabinet_plan(),
        devices,
        script,
        None,
        |_, _| {},
        // Let a few updates land, then fail every one after.
        |log| RecordingBackend::failing_after(log, 3),
    );

    assert!(
        matches!(session.outcome.stop, StopReason::OutputError(_)),
        "{:?}",
        session.outcome.stop
    );
    assert_eq!(session.outcome.exit_code(), super::EXIT_RUNTIME_FAILURE);
    assert_uncaptured_before_unplug(&session.timeline);
    // The pads did come back down — reporting instead of self-cleanup must not
    // leak handles.
    let plugged: Vec<u32> = session
        .pads
        .iter()
        .filter_map(|e| match e {
            PadEvent::Plug(h) => Some(*h),
            _ => None,
        })
        .collect();
    assert_eq!(plugged.len(), 4);
    for handle in plugged {
        assert!(
            session.pads.contains(&PadEvent::Unplug(handle)),
            "pad {handle} leaked after the output error"
        );
    }
}

/// F5 / risk review §3 items 3-4: two boards with one hardware id are
/// indistinguishable to Interception, so capturing "that id" captures both and
/// either board would drive all four slots. Refuse before anything is plugged.
#[test]
fn two_keyboards_sharing_a_hardware_id_refuse_to_start() {
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    let log = PadLog::default();
    // The plan binds the I-PAC; the backend enumerates TWO of them. The ctl
    // observer is what makes the "nothing was told to the capture backend"
    // assertion below mean something: without it the timeline only ever holds
    // pad events, so it would be empty even if a filter had been armed.
    let capture = MockCaptureBackend::new(
        vec![keyboard(IPAC, 1), keyboard(IPAC, 2), keyboard(DESK, 3)],
        Vec::new(),
    )
    .with_ctl_observer({
        let log = log.clone();
        Box::new(move |message| log.push(TraceEvent::Ctl(CtlKind::of(message))))
    });
    // Backstop: if the refusal ever regresses, this session would otherwise run
    // forever (nothing else stops it). Fail, never hang.
    let hard_deadline = Instant::now() + Duration::from_secs(5);
    let mut options = RunOptions {
        clock: Clock::monotonic_nanos(),
        trace: Some(trace.clone()),
        plug_deadline: Duration::from_secs(5),
        stop: Box::new(move || Instant::now() > hard_deadline),
        ..RunOptions::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let outcome = supervise(
        &cabinet_plan(),
        Wiring {
            capture: Box::new(capture),
            pads: Box::new(RecordingBackend::new(log.clone())),
        },
        &mut options,
        &mut out,
    )
    .expect("supervisor ran");

    match &outcome.stop {
        StopReason::AmbiguousDevices { ids } => assert_eq!(ids, &vec![DeviceId::from(IPAC)]),
        other => panic!("expected AmbiguousDevices, got {other:?}"),
    }
    assert_eq!(
        outcome.exit_code(),
        super::EXIT_CANNOT_START,
        "nothing was plugged and no filter was set, so this is a refusal"
    );
    assert_eq!(
        *trace.lock().expect("trace"),
        Vec::<Step>::new(),
        "an ambiguous binding must be caught before the pads are plugged"
    );
    assert!(log.timeline().is_empty(), "{:?}", log.timeline());
    let text = String::from_utf8(out).expect("utf-8");
    assert!(
        text.contains(IPAC) && text.contains("cannot tell identical boards apart"),
        "{text}"
    );
}

/// An unbound duplicate is none of our business: those boards are never
/// captured, so they cannot collide with anything.
#[test]
fn duplicate_ids_that_no_slot_binds_do_not_block_the_run() {
    let session = run_session(
        &escape_plan(),
        vec![keyboard(IPAC, 1), keyboard(DESK, 2), keyboard(DESK, 3)],
        vec![key_stroke(0, Key::A, true)],
        None,
        |opts, _| opts.max_events = Some(1),
    );
    assert_eq!(session.outcome.stop, StopReason::EventLimit);
    assert!(session.trace.contains(&Step::BlockingEnabled));
}

/// F9: once the watchdog has tripped, capture is being torn down. A device
/// coming back must not re-arm blocking on the way out.
#[test]
fn hotplug_does_not_re_arm_capture_after_the_watchdog_tripped() {
    // The backend sees no devices at startup (so blocking never turns on), then
    // republishes the I-PAC as "back" — with the watchdog already tripped. Both
    // conditions are true on the supervisor's very first loop iteration, so the
    // ordering of the two checks is what decides the outcome.
    //
    // The trip is published from `with_start_hook`, i.e. synchronously inside
    // `run()`, and that is load-bearing: a session baselines its health at
    // startup, so a flag set before `supervise()` is by definition a PREVIOUS
    // session's stall and this one is right to ignore it (R1). Setting it from
    // the capture thread instead would race the first poll and make this test
    // flaky rather than meaningful.
    let log = PadLog::default();
    let capture = MockCaptureBackend::new(Vec::new(), Vec::new())
        .with_ctl_observer({
            let log = log.clone();
            Box::new(move |message| log.push(TraceEvent::Ctl(CtlKind::of(message))))
        })
        .with_start_hook(Box::new(ksx_capture::HealthHandle::set_watchdog_tripped));
    let presence = capture.presence();
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));

    presence.publish(vec![DeviceId::from(IPAC)]);

    let mut options = RunOptions {
        clock: Clock::monotonic_nanos(),
        trace: Some(trace.clone()),
        plug_deadline: Duration::from_secs(5),
        ..RunOptions::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let outcome = supervise(
        &escape_plan(),
        Wiring {
            capture: Box::new(capture),
            pads: Box::new(RecordingBackend::new(log.clone())),
        },
        &mut options,
        &mut out,
    )
    .expect("supervisor ran");

    assert_eq!(outcome.stop, StopReason::WatchdogTripped);
    let steps = trace.lock().expect("trace").clone();
    assert!(
        !steps.contains(&Step::BlockingEnabled),
        "a stalled pipeline must never be handed keyboards again: {steps:?}"
    );
    assert!(
        !log.timeline().contains(&TraceEvent::Ctl(CtlKind::Captured)),
        "no SetCaptured may reach the capture backend after a trip: {:?}",
        log.timeline()
    );
    assert_uncaptured_before_unplug(&log.timeline());
}

/// F10: `--json` used to drop the plan's notes on the floor.
#[test]
fn the_json_summary_carries_the_plan_notes() {
    let mut plan = escape_plan();
    plan.notes = vec!["[WARN] block_mice is set but ignored in M4".to_owned()];
    let session = run_session(
        &plan,
        vec![keyboard(IPAC, 1)],
        vec![key_stroke(0, Key::A, true)],
        None,
        |opts, _| opts.max_events = Some(1),
    );
    let v = session.outcome.to_json();
    assert_eq!(
        v.pointer("/notes/0"),
        Some(&serde_json::json!(
            "[WARN] block_mice is set but ignored in M4"
        )),
        "{v}"
    );
}

#[test]
fn a_bus_with_no_pads_refuses_to_start_and_never_captures() {
    struct DeadBus;
    impl VirtualPadBackend for DeadBus {
        fn plug(&mut self) -> Result<PadHandle, OutputError> {
            Err(OutputError::BusNotFound)
        }
        fn persona(&self, _: PadHandle) -> Option<ksx_core::Persona> {
            Some(ksx_core::Persona::default())
        }
        fn user_index(&self, _: PadHandle) -> Option<u8> {
            None
        }
        fn update(&mut self, _: PadHandle, _: &PadState) -> Result<(), OutputError> {
            Ok(())
        }
        fn poll_feedback(&mut self, _: PadHandle) -> Option<Feedback> {
            None
        }
        fn unplug(&mut self, _: PadHandle) -> Result<(), OutputError> {
            Ok(())
        }
    }

    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    let capture = MockCaptureBackend::new(vec![keyboard(IPAC, 1)], Vec::new());
    let mut options = RunOptions {
        clock: Clock::monotonic_nanos(),
        trace: Some(trace.clone()),
        plug_deadline: Duration::from_secs(5),
        ..RunOptions::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let outcome = supervise(
        &escape_plan(),
        Wiring {
            capture: Box::new(capture),
            pads: Box::new(DeadBus),
        },
        &mut options,
        &mut out,
    )
    .expect("supervisor ran");

    assert!(matches!(
        outcome.stop,
        StopReason::PlugFailed {
            bus_missing: true,
            ..
        }
    ));
    assert_eq!(outcome.exit_code(), super::EXIT_CANNOT_START);
    assert_eq!(
        *trace.lock().expect("trace"),
        Vec::<Step>::new(),
        "a failed plug must never have started capture, let alone blocking"
    );
}

/// Every way the pads can fail to come up, end to end. All of them happen
/// before `SetCaptured` is ever sent, so all of them are exit 2 — not just the
/// missing-bus case (F7).
#[test]
fn every_pre_capture_plug_failure_refuses_with_exit_2() {
    /// `plug` fails with something that is NOT "bus missing" — e.g. ViGEm's
    /// no-free-slot error surfaced through a plug timeout.
    struct NoFreeSlot;
    /// `plug` never returns in time.
    struct SlowBus;
    /// The output thread dies before it can report anything.
    struct PanickingBus;

    macro_rules! stub_pad_backend {
        ($ty:ty, $plug:expr) => {
            impl VirtualPadBackend for $ty {
                fn plug(&mut self) -> Result<PadHandle, OutputError> {
                    #[allow(clippy::redundant_closure_call)]
                    ($plug)()
                }
                fn persona(&self, _: PadHandle) -> Option<ksx_core::Persona> {
                    Some(ksx_core::Persona::default())
                }
                fn user_index(&self, _: PadHandle) -> Option<u8> {
                    None
                }
                fn update(&mut self, _: PadHandle, _: &PadState) -> Result<(), OutputError> {
                    Ok(())
                }
                fn poll_feedback(&mut self, _: PadHandle) -> Option<Feedback> {
                    None
                }
                fn unplug(&mut self, _: PadHandle) -> Result<(), OutputError> {
                    Ok(())
                }
            }
        };
    }
    stub_pad_backend!(NoFreeSlot, || Err(OutputError::PlugTimeout(
        Duration::from_secs(5)
    )));
    stub_pad_backend!(SlowBus, || {
        std::thread::sleep(Duration::from_millis(800));
        Err(OutputError::BusNotFound)
    });
    stub_pad_backend!(PanickingBus, || panic!(
        "ksx test hook: deliberate output-thread panic before PadsReady"
    ));

    let cases: Vec<(&str, Box<dyn VirtualPadBackend>)> = vec![
        ("no free slot", Box::new(NoFreeSlot)),
        ("plug timeout", Box::new(SlowBus)),
        ("output thread died", Box::new(PanickingBus)),
    ];

    for (label, pads) in cases {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let log = PadLog::default();
        let capture = MockCaptureBackend::new(vec![keyboard(IPAC, 1)], Vec::new())
            .with_ctl_observer({
                let log = log.clone();
                Box::new(move |message| log.push(TraceEvent::Ctl(CtlKind::of(message))))
            });
        let mut options = RunOptions {
            clock: Clock::monotonic_nanos(),
            trace: Some(trace.clone()),
            plug_deadline: Duration::from_millis(200),
            ..RunOptions::default()
        };
        let mut out: Vec<u8> = Vec::new();
        let outcome = supervise(
            &escape_plan(),
            Wiring {
                capture: Box::new(capture),
                pads,
            },
            &mut options,
            &mut out,
        )
        .expect("supervisor ran");

        assert!(
            matches!(outcome.stop, StopReason::PlugFailed { .. }),
            "{label}: {:?}",
            outcome.stop
        );
        assert_eq!(
            outcome.exit_code(),
            super::EXIT_CANNOT_START,
            "{label}: no keyboard filter was ever set, so this cannot be a runtime failure"
        );
        assert_eq!(
            *trace.lock().expect("trace"),
            Vec::<Step>::new(),
            "{label}: capture must never have started"
        );
        assert!(
            log.timeline().is_empty(),
            "{label}: the capture backend was told nothing: {:?}",
            log.timeline()
        );
    }
}

#[test]
fn block_keyboards_false_runs_the_pads_without_ever_capturing() {
    let mut plan = escape_plan();
    plan.block_keyboards = false;
    let devices = vec![keyboard(IPAC, 1)];
    let script = vec![key_stroke(0, Key::A, true)];

    let session = run_session(&plan, devices, script, None, |opts, _| {
        opts.max_events = Some(1);
    });

    assert!(
        !session.trace.contains(&Step::BlockingEnabled),
        "block_keyboards = false must never capture anything: {:?}",
        session.trace
    );
    assert!(session.output.contains("passthrough"), "{}", session.output);
}

#[test]
fn a_bound_device_the_backend_cannot_see_is_reported_at_startup() {
    // The plan binds the I-PAC; the backend only sees the desk keyboard.
    let session = run_session(
        &escape_plan(),
        vec![keyboard(DESK, 1)],
        vec![key_stroke(0, Key::A, true)],
        None,
        |opts, _| opts.max_events = Some(1),
    );

    assert!(
        session.output.contains(IPAC) && session.output.contains("cannot see"),
        "a missing bound device must be called out: {}",
        session.output
    );
    // Nothing present to capture, so blocking never turns on — an absent board
    // must not cause the *other* keyboard to be blocked.
    assert!(
        !session.trace.contains(&Step::BlockingEnabled),
        "{:?}",
        session.trace
    );
}

#[test]
fn scancode_table_round_trips_every_key_the_corpus_uses() {
    let table = scancode_table();
    for event in load_corpus() {
        let (code, prefix) = *table
            .get(&event.key)
            .unwrap_or_else(|| panic!("no scancode encodes {:?}", event.key));
        assert_eq!(corrected_key(code, prefix), event.key);
        assert_eq!(corrected_key(code, prefix | KEY_UP), event.key);
    }
    // Spot-check the two shapes: a plain letter and an E0-extended arrow.
    assert_eq!(table[&Key::A], (30, KEY_DOWN));
    assert_eq!(table[&Key::Left], (75, KEY_E0));
}

// ---------------------------------------------------------------------------
// M5: game launching through the SessionHook
// ---------------------------------------------------------------------------

/// A [`GameHost`] the test thread can steer, and whose clock advances a fixed
/// amount per observation so the tracker's 3-second rule is crossed
/// deterministically rather than by sleeping.
#[derive(Clone)]
struct ScriptedGame(Arc<Mutex<GameScript>>);

#[derive(Default)]
struct GameScript {
    now_ms: u64,
    /// Virtual milliseconds added per `now_ms()` call.
    tick_ms: u64,
    alive: bool,
    spawn_ok: bool,
    /// The trace as it stood the moment the game was launched — this is how the
    /// ordering guarantee is asserted.
    trace_at_launch: Option<Vec<Step>>,
    launched: Vec<String>,
    trace: Option<Trace>,
}

impl ScriptedGame {
    fn new(script: GameScript) -> Self {
        Self(Arc::new(Mutex::new(script)))
    }

    fn get(&self) -> std::sync::MutexGuard<'_, GameScript> {
        self.0.lock().expect("game script poisoned")
    }
}

impl ksx_games::GameHost for ScriptedGame {
    fn now_ms(&mut self) -> u64 {
        let mut script = self.get();
        script.now_ms += script.tick_ms;
        script.now_ms
    }

    fn spawn(
        &mut self,
        exe: &std::path::Path,
        _args: &[String],
        _wd: Option<&std::path::Path>,
    ) -> Result<u32, String> {
        let mut script = self.get();
        script.launched.push(exe.display().to_string());
        script.trace_at_launch = script
            .trace
            .as_ref()
            .map(|t| t.lock().expect("trace poisoned").clone());
        if script.spawn_ok {
            Ok(4242)
        } else {
            Err("The system cannot find the file specified. (os error 2)".to_owned())
        }
    }

    fn activate(&mut self, url: &str) -> Result<(), String> {
        self.get().launched.push(url.to_owned());
        Ok(())
    }

    fn launched_alive(&mut self) -> Option<bool> {
        Some(self.get().alive)
    }

    fn processes(&mut self) -> Vec<ksx_platform::process::ProcessEntry> {
        Vec::new()
    }
}

fn game_spec() -> ksx_games::LaunchSpec {
    ksx_games::LaunchSpec::from_entry(&ksx_config::GameEntry {
        title: "Street Fighter".into(),
        notes: String::new(),
        path: r"C:\games\sf.exe".into(),
        arguments: String::new(),
        process_name: None,
        launcher_grace_ms: None,
        block_keyboards: true,
        block_mice: false,
        slots: Vec::new(),
    })
}

/// **The ordering contract.** A game started before the pads exist enumerates
/// zero controllers and never asks again, so the launch must happen strictly
/// after `PadsPlugged`, `CaptureStarted` and `BlockingEnabled` — asserted
/// against the trace as it stood at the instant `spawn` was called, not against
/// a note written afterwards.
#[test]
fn the_game_launches_only_after_the_pads_are_up_and_capture_is_armed() {
    let script = ScriptedGame::new(GameScript {
        // One virtual tick must carry the process past the launcher grace, or
        // its exit is a hand-off (which never ends a session) and this test
        // waits forever. `game_spec()` has no `process_name`, so a hand-off here
        // would be terminal-but-running by design.
        tick_ms: ksx_games::DEFAULT_LAUNCHER_GRACE_MS + 5_000,
        alive: false,
        spawn_ok: true,
        ..GameScript::default()
    });
    let session = run_session(
        &escape_plan(),
        vec![keyboard(IPAC, 1)],
        Vec::new(),
        None,
        |options, _| {
            script.get().trace = options.trace.clone();
            options.hook = Box::new(super::game::GameHook::new(
                game_spec(),
                script.clone(),
                PathBuf::from("games.toml"),
            ));
        },
    );

    let at_launch = script
        .get()
        .trace_at_launch
        .clone()
        .expect("the game was launched");
    assert_eq!(
        at_launch,
        vec![
            Step::PadsPlugged,
            Step::CaptureStarted,
            Step::BlockingEnabled
        ],
        "the game must not start until the pads are plugged AND blocking is armed"
    );
    assert_eq!(script.get().launched, vec![r"C:\games\sf.exe"]);

    // ...and the game exiting is a clean end of session.
    assert_eq!(session.outcome.stop, StopReason::GameExited);
    assert_eq!(session.outcome.exit_code(), 0);
    assert!(
        session.output.contains("exited — stopping emulation"),
        "{}",
        session.output
    );

    // Teardown order is unchanged: uncapture, then unplug.
    assert!(
        index_of(&session.trace, Step::BlockingDisabled)
            < index_of(&session.trace, Step::PadsUnplugged),
        "{:?}",
        session.trace
    );
}

/// A launch that fails happens after the pads are up, so it is a runtime
/// failure (exit 3) — and the keyboards must still be freed before the pads go.
#[test]
fn a_failed_game_launch_tears_down_in_order_and_exits_3() {
    let script = ScriptedGame::new(GameScript {
        tick_ms: 1_000,
        alive: true,
        spawn_ok: false,
        ..GameScript::default()
    });
    let session = run_session(
        &escape_plan(),
        vec![keyboard(IPAC, 1)],
        Vec::new(),
        None,
        |options, _| {
            options.hook = Box::new(super::game::GameHook::new(
                game_spec(),
                script.clone(),
                PathBuf::from("games.toml"),
            ));
        },
    );

    assert!(
        matches!(session.outcome.stop, StopReason::GameLaunchFailed(_)),
        "{:?}",
        session.outcome.stop
    );
    assert_eq!(session.outcome.exit_code(), super::EXIT_RUNTIME_FAILURE);
    assert!(
        !session.outcome.stop.is_clean(),
        "a game that never started is not a clean session"
    );
    assert!(
        index_of(&session.trace, Step::BlockingDisabled)
            < index_of(&session.trace, Step::PadsUnplugged),
        "even a launch failure frees the keyboards before it unplugs: {:?}",
        session.trace
    );
    // The pads really were unplugged — a failed launch must not leave four
    // ghost controllers in joy.cpl.
    assert!(
        session
            .pads
            .iter()
            .any(|e| matches!(e, PadEvent::Unplug(_))),
        "{:?}",
        session.pads
    );
    assert!(
        session
            .outcome
            .stop
            .message()
            .contains("cannot find the file"),
        "{}",
        session.outcome.stop.message()
    );
}

/// Stopping for any other reason (here: the Ctrl+C latch) must not end the
/// game — ksx has no kill primitive — and must say so, because the user's
/// keyboard is about to start typing into whatever is still on screen.
#[test]
fn stopping_emulation_leaves_the_game_running_and_says_so() {
    let script = ScriptedGame::new(GameScript {
        tick_ms: 10,
        alive: true,
        spawn_ok: true,
        ..GameScript::default()
    });
    let deadline = Instant::now() + Duration::from_millis(400);
    let session = run_session(
        &escape_plan(),
        vec![keyboard(IPAC, 1)],
        Vec::new(),
        None,
        |options, _| {
            options.stop = Box::new(move || Instant::now() > deadline);
            options.hook = Box::new(super::game::GameHook::new(
                game_spec(),
                script.clone(),
                PathBuf::from("games.toml"),
            ));
        },
    );

    assert_eq!(session.outcome.stop, StopReason::CtrlC);
    assert_eq!(session.outcome.exit_code(), 0);
    assert!(
        session.output.contains("never kills a game"),
        "{}",
        session.output
    );
    assert!(script.get().alive, "the game was never touched");
}
