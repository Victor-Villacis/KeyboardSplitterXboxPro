//! `ksx monitor` — live per-device key monitor, **passthrough-only**.
//!
//! # Safety contract (M3, absolute)
//!
//! This command NEVER sends [`CaptureCtl::SetCaptured`]; the only control
//! message it ever emits is `Shutdown`. The backend starts in passthrough and
//! stays there, so every stroke is re-sent to the OS — `ksx monitor` has no
//! code path that can suppress a keystroke. Blocking arrives in M4 through
//! `ksx run`, supervised. The capture thread's drop guard (filters → NONE) and
//! the consumer-stall watchdog are active for the whole session.
//!
//! # Output
//!
//! Human mode: one `<alias> <Key> down|up` line per event (alias = `kb<slot>`,
//! legend printed at startup), then a summary. `--json`: JSONL on stdout —
//! each line is exactly one JSON object of one of three shapes:
//!
//! 1. zero or more startup warnings: `{"warning":{"code","message"}}`
//!    (the doctor's `interception-borrowed-time` advice, verbatim);
//! 2. event lines in the replay-oracle corpus shape (below);
//! 3. exactly one final `{"summary":{events,dropped,per_device,exit_reason,
//!    watchdog_tripped,reboot_required,panicked}}`.
//!
//! # Replay-oracle corpus format (`--record FILE`, JSONL, stable)
//!
//! One JSON object per event, one event per line — nothing else is ever
//! written to the record file:
//!
//! ```json
//! {"t_ms": 1042, "device": "HID\\VID_D209&PID_0430&REV_0056&MI_00", "key": "A", "down": true}
//! ```
//!
//! - `t_ms` (u64): milliseconds since monitor start, measured when the CLI
//!   *received* the event — replay pacing, not driver-latency analysis;
//! - `device` (string): the Interception hardware id, the same identity
//!   ksx-config persists;
//! - `key` (string): the legacy `InterceptionKey` spelling ([`Key::name`],
//!   exact and case-sensitive — presets parse it back);
//! - `down` (bool): true = press, false = release.
//!
//! The field *set* is stable; field order within a line is not guaranteed.
//! Serialization lives here in ksx-app (the capture crate stays serde-free).
//!
//! Exit codes: 0 = clean stop (Ctrl+C or `--for-secs` elapsed), 1 = error,
//! [`crate::devices::EXIT_DRIVER_MISSING`] (2) = Interception unavailable.

// Off Windows only the stub `run` is reachable outside tests; the pure loop
// + formatting stay compiled (and mock-tested) but would trip dead_code.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossbeam_channel::RecvTimeoutError;
use ksx_capture::{CaptureBackend, CaptureCtl, DeviceInfo, DeviceKind, ExitReason};
use ksx_core::KeyEvent;

use crate::devices::vendor_tag;

/// How often the loop wakes to check Ctrl+C / the deadline even when no keys
/// arrive. Also bounds shutdown latency.
const POLL: Duration = Duration::from_millis(50);

/// Event channel capacity. The monitor drains promptly; if it ever stalls the
/// backend's watchdog force-flips to passthrough rather than black-holing.
const CHANNEL_CAPACITY: usize = 1024;

/// Knobs for [`run_loop`], separated from I/O so tests can drive the loop with
/// a mock backend and captured writers.
pub(crate) struct LoopCfg {
    pub json: bool,
    /// Hard stop (`--for-secs`).
    pub deadline: Option<Instant>,
    /// Test-only early stop after N events — never set from the CLI. Keeps the
    /// mock-backend tests deterministic and fast instead of waiting out a
    /// wall-clock deadline.
    pub max_events: Option<u64>,
}

/// End-of-session accounting (also the shape of the `--json` summary line).
pub(crate) struct Summary {
    pub events: u64,
    /// Keyed by hardware id (not alias): the stable identity.
    pub per_device: BTreeMap<String, u64>,
    pub dropped: u64,
    pub exit_reason: &'static str,
    pub watchdog_tripped: bool,
    pub reboot_required: bool,
    pub panicked: bool,
}

fn reason_str(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::Shutdown => "shutdown",
        ExitReason::ChannelClosed => "channel-closed",
        ExitReason::ScriptExhausted => "script-exhausted",
        ExitReason::Panicked => "panicked",
        // M6: a claimed board was unplugged. Deliberately its own string and
        // not folded into "shutdown" — the session ended because hardware
        // left, which is a different thing to explain to a user than "you
        // asked me to stop".
        ExitReason::DeviceLost => "device-lost",
    }
}

/// One event in the replay-oracle corpus shape (used for both `--json` stdout
/// event lines and `--record` lines — one format, documented above).
pub(crate) fn event_json(t_ms: u64, ev: &KeyEvent) -> serde_json::Value {
    serde_json::json!({
        "t_ms": t_ms,
        "device": ev.device.as_str(),
        "key": ev.key.name(),
        "down": ev.down,
    })
}

/// The human event line: `kb1 A down`.
pub(crate) fn human_line(alias: &str, ev: &KeyEvent) -> String {
    format!(
        "{alias} {} {}",
        ev.key.name(),
        if ev.down { "down" } else { "up" }
    )
}

/// hwid → short alias (`kb<slot>`). Identical boards share a hwid (risk review
/// R2) so twins collapse onto one alias here; M4's instance-path correlation
/// is what disambiguates them.
pub(crate) fn alias_map(devices: &[DeviceInfo]) -> BTreeMap<String, String> {
    let mut keyboards: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Keyboard)
        .collect();
    keyboards.sort_by_key(|d| d.interception_slot);
    let mut map = BTreeMap::new();
    for (i, d) in keyboards.iter().enumerate() {
        let alias = match d.interception_slot {
            Some(slot) => format!("kb{slot}"),
            None => format!("kb?{}", i + 1),
        };
        map.insert(d.id.as_str().to_owned(), alias);
    }
    map
}

fn emit_event(
    ev: &KeyEvent,
    t_ms: u64,
    json: bool,
    aliases: &BTreeMap<String, String>,
    out: &mut dyn Write,
    record: &mut Option<&mut dyn Write>,
) -> anyhow::Result<()> {
    if json {
        writeln!(out, "{}", event_json(t_ms, ev))?;
    } else {
        let alias = aliases
            .get(ev.device.as_str())
            .map(String::as_str)
            .unwrap_or_else(|| ev.device.as_str());
        writeln!(out, "{}", human_line(alias, ev))?;
    }
    if let Some(w) = record.as_mut() {
        writeln!(w, "{}", event_json(t_ms, ev))?;
    }
    Ok(())
}

fn summary_json(s: &Summary) -> serde_json::Value {
    serde_json::json!({
        "summary": {
            "events": s.events,
            "dropped": s.dropped,
            "per_device": s.per_device,
            "exit_reason": s.exit_reason,
            "watchdog_tripped": s.watchdog_tripped,
            "reboot_required": s.reboot_required,
            "panicked": s.panicked,
        }
    })
}

fn render_summary(s: &Summary, aliases: &BTreeMap<String, String>) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "monitor summary: {} event(s), {} dropped",
        s.events, s.dropped
    );
    for (id, n) in &s.per_device {
        match aliases.get(id) {
            Some(alias) => {
                let _ = writeln!(out, "  {alias} {id}: {n}");
            }
            None => {
                let _ = writeln!(out, "  {id}: {n}");
            }
        }
    }
    if s.watchdog_tripped {
        let _ = writeln!(
            out,
            "  [WARN] watchdog tripped — the consumer stalled; the backend forced passthrough"
        );
    }
    if s.reboot_required {
        let _ = writeln!(
            out,
            "  [FAIL] Interception slot exhaustion detected — REBOOT REQUIRED"
        );
    }
    if s.panicked {
        let _ = writeln!(
            out,
            "  [FAIL] capture thread panicked (the drop guard reset the filters)"
        );
    }
    if s.exit_reason != "shutdown" {
        let _ = writeln!(out, "  [WARN] capture thread exit: {}", s.exit_reason);
    }
    out
}

/// The whole monitor session against any [`CaptureBackend`]: legend, capture
/// thread, event pump, shutdown, drain, summary. Pure with respect to the OS —
/// the clock and stop conditions are injected, so the mock backend exercises
/// exactly the code the live command runs.
pub(crate) fn run_loop(
    mut backend: Box<dyn CaptureBackend>,
    cfg: &LoopCfg,
    stop_requested: &dyn Fn() -> bool,
    now_ms: &mut dyn FnMut() -> u64,
    out: &mut dyn Write,
    mut record: Option<&mut dyn Write>,
) -> anyhow::Result<Summary> {
    let devices = backend.devices();
    let aliases = alias_map(&devices);

    if !cfg.json {
        let mut keyboards: Vec<&DeviceInfo> = devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Keyboard)
            .collect();
        keyboards.sort_by_key(|d| d.interception_slot);
        writeln!(
            out,
            "monitoring {} keyboard(s) — passthrough only, no keys are blocked",
            keyboards.len()
        )?;
        for d in keyboards {
            let alias = aliases
                .get(d.id.as_str())
                .map(String::as_str)
                .unwrap_or("kb?");
            let friendly = d.friendly.as_deref().unwrap_or("n/a");
            let tag = vendor_tag(d.id.as_str()).map_or_else(String::new, |n| format!("  [{n}]"));
            writeln!(out, "  {alias} = {} \"{friendly}\"{tag}", d.id.as_str())?;
        }
    }

    let health = backend.health();
    let (tx, rx) = crossbeam_channel::bounded(CHANNEL_CAPACITY);
    let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
    let handle = backend
        .run(tx, ctl_rx)
        .context("spawning the capture thread")?;

    let mut events: u64 = 0;
    let mut per_device: BTreeMap<String, u64> = BTreeMap::new();
    loop {
        if stop_requested() {
            break;
        }
        if cfg.deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        if cfg.max_events.is_some_and(|m| events >= m) {
            break;
        }
        match rx.recv_timeout(POLL) {
            Ok(ev) => {
                let t_ms = now_ms();
                events += 1;
                *per_device.entry(ev.device.as_str().to_owned()).or_insert(0) += 1;
                emit_event(&ev, t_ms, cfg.json, &aliases, out, &mut record)?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The capture thread ended on its own; join below says why.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    // Stop the capture thread. This is the ONLY control message the monitor
    // ever sends — SetCaptured does not exist on any `ksx monitor` code path.
    let _ = ctl_tx.send(CaptureCtl::Shutdown);
    let exit_reason = match handle.join() {
        Ok(reason) => reason_str(reason),
        Err(_) => "join-panic",
    };
    // The thread is gone (tx dropped): drain whatever was still buffered so
    // events from inside the window are never silently lost.
    while let Ok(ev) = rx.try_recv() {
        let t_ms = now_ms();
        events += 1;
        *per_device.entry(ev.device.as_str().to_owned()).or_insert(0) += 1;
        emit_event(&ev, t_ms, cfg.json, &aliases, out, &mut record)?;
    }

    let snap = health.snapshot();
    let summary = Summary {
        events,
        per_device,
        dropped: snap.dropped_events,
        exit_reason,
        watchdog_tripped: snap.watchdog_tripped,
        reboot_required: snap.reboot_required,
        panicked: snap.panicked,
    };
    if cfg.json {
        writeln!(out, "{}", summary_json(&summary))?;
    } else {
        write!(out, "{}", render_summary(&summary, &aliases))?;
    }
    Ok(summary)
}

#[cfg(windows)]
pub fn run(
    for_secs: Option<u64>,
    record: Option<std::path::PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    use ksx_capture::{CaptureError, InterceptionBackend};

    // Safe: creating the context sets no filter. The keyboard filter opens
    // only inside `run_loop`'s capture thread — in passthrough, guarded.
    let backend = match InterceptionBackend::new() {
        Ok(backend) => backend,
        // Both "the driver is not answering" and "the DLL is not on this
        // machine at all" are the same thing to `ksx monitor`: an Interception-
        // only command on a machine that cannot do Interception. Exit 2 with the
        // message, never a loader dialog and never an anyhow backtrace.
        Err(err @ (CaptureError::DriverUnavailable | CaptureError::DllMissing { .. })) => {
            let message = format!("{err}; run `ksx doctor` for driver diagnostics");
            if json {
                println!(
                    "{}",
                    crate::pads::error_json("interception-unavailable", &message)
                );
            } else {
                eprintln!("error: {message}");
            }
            std::process::exit(crate::devices::EXIT_DRIVER_MISSING);
        }
        Err(err) => return Err(err).context("creating the Interception context"),
    };

    // The doctor's borrowed-time warning, same code + message, at startup.
    let advice = ksx_platform::summarize(&ksx_platform::collect());
    for a in advice
        .iter()
        .filter(|a| a.code == "interception-borrowed-time")
    {
        if json {
            println!(
                "{}",
                serde_json::json!({"warning": {"code": a.code, "message": a.message}})
            );
        } else {
            println!("[WARN] {}: {}", a.code, a.message);
        }
    }

    if !crate::ctrl_c::install() {
        tracing::warn!("SetConsoleCtrlHandler failed; Ctrl+C will abort without a summary");
    }
    if !json {
        match for_secs {
            Some(s) => println!("monitoring for {s}s (Ctrl+C to stop early)..."),
            None => println!("monitoring until Ctrl+C..."),
        }
    }

    let mut record_file = match &record {
        Some(path) => Some(std::io::BufWriter::new(
            std::fs::File::create(path)
                .with_context(|| format!("creating record file '{}'", path.display()))?,
        )),
        None => None,
    };

    let start = Instant::now();
    let cfg = LoopCfg {
        json,
        deadline: for_secs.map(|s| start + Duration::from_secs(s)),
        max_events: None,
    };
    let mut now_ms = move || start.elapsed().as_millis() as u64;
    let mut out = std::io::stdout().lock();
    let summary = run_loop(
        Box::new(backend),
        &cfg,
        &crate::ctrl_c::requested,
        &mut now_ms,
        &mut out,
        record_file.as_mut().map(|w| w as &mut dyn Write),
    )?;
    drop(out);
    if let Some(mut w) = record_file {
        w.flush().context("flushing the record file")?;
        if !json {
            // `record` is Some whenever `record_file` is.
            if let Some(path) = &record {
                println!("recorded {} event(s) to {}", summary.events, path.display());
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(
    _for_secs: Option<u64>,
    _record: Option<std::path::PathBuf>,
    _json: bool,
) -> anyhow::Result<()> {
    anyhow::bail!("`ksx monitor` is Windows-only (it captures via the Interception driver)")
}

#[cfg(test)]
mod tests {
    use ksx_capture::keymap::{KEY_DOWN, KEY_E0, KEY_UP};
    use ksx_capture::{MockCaptureBackend, MockStroke};
    use ksx_core::DeviceId;

    use super::*;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const LOGI: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";

    fn two_keyboards() -> Vec<DeviceInfo> {
        vec![
            DeviceInfo {
                id: DeviceId::from(IPAC),
                interception_slot: Some(1),
                friendly: Some("I-PAC Arcade Control Interface".into()),
                kind: DeviceKind::Keyboard,
            },
            DeviceInfo {
                id: DeviceId::from(LOGI),
                interception_slot: Some(2),
                friendly: None,
                kind: DeviceKind::Keyboard,
            },
        ]
    }

    /// A down (I-PAC), Left down (desk keyboard, E0-corrected), A up (I-PAC).
    fn script() -> Vec<MockStroke> {
        vec![
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_DOWN,
            },
            MockStroke {
                device: 1,
                code: 75,
                state: KEY_E0 | KEY_DOWN,
            },
            MockStroke {
                device: 0,
                code: 30,
                state: KEY_UP,
            },
        ]
    }

    /// Deterministic clock: 10, 20, 30, ... ms.
    fn ticking_clock() -> impl FnMut() -> u64 {
        let mut t = 0u64;
        move || {
            t += 10;
            t
        }
    }

    #[test]
    fn jsonl_stream_and_record_shape_via_mock() {
        let backend = MockCaptureBackend::new(two_keyboards(), script());
        let mut out = Vec::new();
        let mut rec = Vec::new();
        let mut now_ms = ticking_clock();
        let cfg = LoopCfg {
            json: true,
            deadline: None,
            max_events: Some(3),
        };
        let summary = run_loop(
            Box::new(backend),
            &cfg,
            &|| false,
            &mut now_ms,
            &mut out,
            Some(&mut rec),
        )
        .unwrap();

        assert_eq!(summary.events, 3);
        assert_eq!(summary.dropped, 0);
        assert_eq!(summary.exit_reason, "shutdown");
        assert_eq!(summary.per_device.get(IPAC), Some(&2));
        assert_eq!(summary.per_device.get(LOGI), Some(&1));

        let stdout = String::from_utf8(out).unwrap();
        // The record file is exactly the event lines: stdout is those same
        // lines plus the trailing summary object.
        let rec = String::from_utf8(rec).unwrap();
        assert!(stdout.starts_with(&rec), "record must prefix stdout");
        assert_eq!(rec.lines().count(), 3);
        // Every line (events + summary) is valid standalone JSON.
        for line in stdout.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        insta::assert_snapshot!(stdout);
    }

    #[test]
    fn human_stream_shape_via_mock() {
        let backend = MockCaptureBackend::new(two_keyboards(), script());
        let mut out = Vec::new();
        let mut now_ms = ticking_clock();
        let cfg = LoopCfg {
            json: false,
            deadline: None,
            max_events: Some(3),
        };
        run_loop(
            Box::new(backend),
            &cfg,
            &|| false,
            &mut now_ms,
            &mut out,
            None,
        )
        .unwrap();
        insta::assert_snapshot!(String::from_utf8(out).unwrap());
    }

    #[test]
    fn event_json_is_the_documented_corpus_shape() {
        let ev = KeyEvent {
            device: DeviceId::from(IPAC),
            key: ksx_core::Key::A,
            down: true,
            t: 12345,
        };
        let v = event_json(1042, &ev);
        let obj = v.as_object().unwrap();
        // Exactly the documented field set — nothing extra sneaks into the
        // corpus format.
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["device", "down", "key", "t_ms"]);
        assert_eq!(v.pointer("/t_ms"), Some(&serde_json::json!(1042)));
        assert_eq!(v.pointer("/device"), Some(&serde_json::json!(IPAC)));
        assert_eq!(v.pointer("/key"), Some(&serde_json::json!("A")));
        assert_eq!(v.pointer("/down"), Some(&serde_json::json!(true)));
        // Backslashes in the hwid survive a serialize/parse round trip.
        let round: serde_json::Value = serde_json::from_str(&v.to_string()).unwrap();
        assert_eq!(round.pointer("/device"), Some(&serde_json::json!(IPAC)));
    }

    #[test]
    fn aliases_use_interception_slots() {
        let map = alias_map(&two_keyboards());
        assert_eq!(map.get(IPAC).map(String::as_str), Some("kb1"));
        assert_eq!(map.get(LOGI).map(String::as_str), Some("kb2"));
    }

    #[test]
    fn human_line_reads_naturally() {
        let ev = KeyEvent {
            device: DeviceId::from(IPAC),
            key: ksx_core::Key::Left,
            down: false,
            t: 0,
        };
        assert_eq!(human_line("kb1", &ev), "kb1 Left up");
    }
}
