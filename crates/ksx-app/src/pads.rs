//! `ksx pads` — plug N virtual pads, show each pad's XInput identity, run a
//! visible test pattern, then unplug cleanly.
//!
//! Exit code [`EXIT_DRIVER_MISSING`] (2) = ViGEmBus is not installed: the
//! human-readable install hint goes to stderr; `--json` puts
//! `{"error":{code,message}}` on stdout instead. Ctrl+C is handled with
//! `SetConsoleCtrlHandler` + an `AtomicBool` the pattern loop polls — no ctrlc
//! crate. A `taskkill /f` mid-pattern runs no destructors; that case is covered
//! by driver-side client-handle cleanup (see `VigemBackend` docs).
//!
//! The pattern math is pure and cross-platform (tested everywhere); only
//! [`run`]'s driver plumbing is Windows-only.

// Off Windows only the stub `run` is reachable outside tests; the pure pattern
// + JSON helpers stay compiled (and tested) but would trip dead_code.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::Duration;

use ksx_core::pad::{XButtons, AXIS_MAX};
use ksx_core::PadState;
use ksx_platform::BusDriverReport;

/// Exit code when ViGEmBus is not installed (documented in `--help`).
pub const EXIT_DRIVER_MISSING: i32 = 2;

/// One pattern frame per tick. Coarse on purpose — this is the demo path, not
/// the input hot path.
pub const TICK: Duration = Duration::from_millis(100);

/// Each of A/B/X/Y is held this many ticks (400 ms).
const BUTTON_HOLD_TICKS: u64 = 4;
const BUTTON_CYCLE: [XButtons; 4] = [XButtons::A, XButtons::B, XButtons::X, XButtons::Y];
/// Full left-stick revolution every 20 ticks (2 s); right stick counter-rotates.
const STICK_PERIOD_TICKS: u64 = 20;
/// Trigger triangle-wave period (1.6 s); RT runs half a period behind LT.
const TRIGGER_PERIOD_TICKS: u64 = 16;

/// The state the test pattern shows at `tick` (one tick = [`TICK`]).
///
/// Pure and total: same tick, same state, on any platform.
fn pattern_state(tick: u64) -> PadState {
    let button = BUTTON_CYCLE[((tick / BUTTON_HOLD_TICKS) % 4) as usize];
    let angle =
        (tick % STICK_PERIOD_TICKS) as f64 / STICK_PERIOD_TICKS as f64 * std::f64::consts::TAU;
    let amp = f64::from(AXIS_MAX);
    PadState {
        buttons: button,
        lt: triangle(tick),
        rt: triangle(tick + TRIGGER_PERIOD_TICKS / 2),
        lx: (angle.cos() * amp) as i16,
        ly: (angle.sin() * amp) as i16,
        // Counter-rotating so the two sticks are visibly independent.
        rx: (angle.cos() * amp) as i16,
        ry: (-angle.sin() * amp) as i16,
    }
}

/// Triangle wave 0 → 255 → 0 over [`TRIGGER_PERIOD_TICKS`].
fn triangle(tick: u64) -> u8 {
    let pos = tick % TRIGGER_PERIOD_TICKS;
    let half = TRIGGER_PERIOD_TICKS / 2;
    let distance = if pos <= half {
        pos
    } else {
        TRIGGER_PERIOD_TICKS - pos
    };
    (distance * 255 / half) as u8
}

/// One plugged pad, as reported by `--json` (and mirrored by the human lines).
pub struct PadRow {
    /// 1-based plug order.
    pub slot: u8,
    /// Backend handle id ([`ksx_output::PadHandle::raw`]).
    pub handle: u32,
    /// XInput `dwUserIndex`; `None` beyond the 4 XInput slots.
    pub user_index: Option<u8>,
    /// LED number from the bus feedback, if one arrived during the drain.
    pub led_number: Option<u8>,
}

/// The single `--json` success object: `{driver, pads}`.
pub fn pads_json(driver: &BusDriverReport, pads: &[PadRow]) -> serde_json::Value {
    let pads: Vec<serde_json::Value> = pads
        .iter()
        .map(|p| {
            serde_json::json!({
                "slot": p.slot,
                "handle": p.handle,
                "user_index": p.user_index,
                "led_number": p.led_number,
            })
        })
        .collect();
    serde_json::json!({ "driver": driver, "pads": pads })
}

/// The single `--json` failure object: `{"error":{code,message}}`.
pub fn error_json(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

#[cfg(windows)]
pub fn run(count: u8, hold_secs: u64, json: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_output::{VigemBackend, VirtualPadBackend as _};

    let mut backend = match VigemBackend::connect() {
        Ok(backend) => backend,
        Err(err) if err.is_bus_missing() => {
            if json {
                println!("{}", error_json("vigembus-missing", &err.to_string()));
            } else {
                eprintln!("error: {err}");
            }
            std::process::exit(EXIT_DRIVER_MISSING);
        }
        Err(err) => return Err(err).context("connecting to ViGEmBus"),
    };

    let mut handles = Vec::new();
    let mut rows = Vec::new();
    for slot in 1..=count {
        let handle = backend
            .plug()
            .with_context(|| format!("plugging pad {slot} of {count}"))?;
        let user_index = backend.user_index(handle);
        let led_number = drain_led(&mut backend, handle);
        if !json {
            let ui = match user_index {
                Some(i) => i.to_string(),
                None => "none".to_string(),
            };
            let led = match led_number {
                Some(n) => n.to_string(),
                None => "?".to_string(),
            };
            println!("pad {slot}: user index {ui} (led {led})");
        }
        handles.push(handle);
        rows.push(PadRow {
            slot,
            handle: handle.raw(),
            user_index,
            led_number,
        });
    }

    if json {
        // The driver section reuses doctor's collector, scoped to ViGEmBus.
        let driver = ksx_platform::collect().vigembus;
        println!("{}", pads_json(&driver, &rows));
    } else {
        println!("running test pattern for {hold_secs}s (Ctrl+C to unplug and exit)...");
        animate(&mut backend, &handles, hold_secs);
    }

    for (handle, row) in handles.iter().zip(&rows) {
        backend
            .unplug(*handle)
            .with_context(|| format!("unplugging pad {}", row.slot))?;
    }
    drop(backend);
    if !json {
        println!("unplugged {count} pad(s)");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(_count: u8, _hold_secs: u64, _json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx pads` is Windows-only (it drives the ViGEmBus kernel driver)")
}

/// Polls feedback briefly after a plug so the bus's initial LED notification is
/// reported. Returns the last LED seen, or `None` if nothing arrived in time.
#[cfg(windows)]
fn drain_led(
    backend: &mut dyn ksx_output::VirtualPadBackend,
    handle: ksx_output::PadHandle,
) -> Option<u8> {
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    let mut led = None;
    loop {
        while let Some(feedback) = backend.poll_feedback(handle) {
            led = Some(feedback.led_number);
        }
        if led.is_some() || std::time::Instant::now() >= deadline {
            return led;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drives [`pattern_state`] on every pad until the hold expires or Ctrl+C.
#[cfg(windows)]
fn animate(
    backend: &mut ksx_output::VigemBackend,
    handles: &[ksx_output::PadHandle],
    hold_secs: u64,
) {
    use ksx_output::VirtualPadBackend as _;

    if !ctrl_c::install() {
        tracing::warn!("SetConsoleCtrlHandler failed; the pattern runs the full hold time");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(hold_secs);
    let mut tick: u64 = 0;
    while std::time::Instant::now() < deadline && !ctrl_c::requested() {
        let state = pattern_state(tick);
        for &handle in handles {
            if let Err(err) = backend.update(handle, &state) {
                tracing::warn!(%err, ?handle, "pattern update failed");
            }
        }
        tick += 1;
        std::thread::sleep(TICK);
    }
    // Release everything before the unplug so nothing is left "pressed".
    for &handle in handles {
        let _ = backend.update(handle, &PadState::default());
    }
}

#[cfg(windows)]
mod ctrl_c {
    //! Ctrl+C via `SetConsoleCtrlHandler` + one `AtomicBool` — deliberately no
    //! ctrlc crate. Returning TRUE from the handler claims the event, so the
    //! process survives long enough for the explicit unplug + drop path.

    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    static REQUESTED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn handler(_ctrl_type: u32) -> windows_sys::core::BOOL {
        REQUESTED.store(true, Ordering::SeqCst);
        TRUE
    }

    /// Installs the handler; `false` means Ctrl+C keeps its default behavior.
    pub fn install() -> bool {
        // SAFETY: `handler` is a valid PHANDLER_ROUTINE for the process's
        // lifetime (a fn item), and only touches an atomic.
        unsafe { SetConsoleCtrlHandler(Some(handler), TRUE) != 0 }
    }

    pub fn requested() -> bool {
        REQUESTED.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use ksx_platform::{DriverFileReport, ServiceInfo, ServiceState, StartType};

    use super::*;

    #[test]
    fn exit_code_for_missing_driver_is_2() {
        assert_eq!(EXIT_DRIVER_MISSING, 2);
    }

    #[test]
    fn pattern_is_deterministic() {
        for tick in 0..200 {
            assert_eq!(pattern_state(tick), pattern_state(tick), "tick {tick}");
        }
    }

    #[test]
    fn buttons_cycle_a_b_x_y_at_400ms_each() {
        let expect = [
            (0, XButtons::A),
            (3, XButtons::A),
            (4, XButtons::B),
            (7, XButtons::B),
            (8, XButtons::X),
            (11, XButtons::X),
            (12, XButtons::Y),
            (15, XButtons::Y),
            (16, XButtons::A), // wraps
        ];
        for (tick, button) in expect {
            assert_eq!(pattern_state(tick).buttons, button, "tick {tick}");
        }
    }

    #[test]
    fn exactly_one_face_button_at_a_time() {
        for tick in 0..64 {
            let buttons = pattern_state(tick).buttons;
            assert_eq!(buttons.bits().count_ones(), 1, "tick {tick}: {buttons:?}");
            assert!(
                (XButtons::A | XButtons::B | XButtons::X | XButtons::Y).contains(buttons),
                "tick {tick}: {buttons:?}"
            );
        }
    }

    #[test]
    fn triggers_pulse_full_range_out_of_phase() {
        assert_eq!(triangle(0), 0);
        assert_eq!(triangle(8), 255);
        assert_eq!(triangle(16), 0);
        for tick in 0..64 {
            let state = pattern_state(tick);
            // RT is LT delayed by half a period.
            assert_eq!(state.rt, triangle(tick + 8), "tick {tick}");
            // Periodicity.
            assert_eq!(state.lt, pattern_state(tick + 16).lt, "tick {tick}");
        }
    }

    #[test]
    fn stick_sweep_is_circular_and_periodic() {
        for tick in 0..40 {
            let state = pattern_state(tick);
            // Constant magnitude (within integer-truncation slack).
            let mag = (f64::from(state.lx).powi(2) + f64::from(state.ly).powi(2)).sqrt();
            let amp = f64::from(AXIS_MAX);
            assert!((mag - amp).abs() < amp * 0.01, "tick {tick}: |l| = {mag}");
            // Right stick mirrors the left (counter-rotation). `ly` is never
            // i16::MIN (amplitude is AXIS_MAX = 32767), so the negation is safe.
            assert_eq!(state.rx, state.lx, "tick {tick}");
            assert_eq!(state.ry, -state.ly, "tick {tick}");
            // Periodicity.
            let next = pattern_state(tick + STICK_PERIOD_TICKS);
            assert_eq!((state.lx, state.ly), (next.lx, next.ly), "tick {tick}");
        }
    }

    #[test]
    fn stick_sweep_visits_all_four_quadrants() {
        let mut quadrants = [false; 4];
        for tick in 0..STICK_PERIOD_TICKS {
            let state = pattern_state(tick);
            if state.lx != 0 && state.ly != 0 {
                let q = match (state.lx > 0, state.ly > 0) {
                    (true, true) => 0,
                    (false, true) => 1,
                    (false, false) => 2,
                    (true, false) => 3,
                };
                quadrants[q] = true;
            }
        }
        assert_eq!(quadrants, [true; 4]);
    }

    fn fixture_driver() -> BusDriverReport {
        BusDriverReport {
            installed: true,
            service: Some(ServiceInfo {
                start_type: StartType::Demand,
                image_path: Some("System32\\drivers\\ViGEmBus.sys".into()),
                display_name: Some("Nefarius Virtual Gamepad Emulation Bus Driver".into()),
                state: ServiceState::Running,
            }),
            driver_file: Some(DriverFileReport {
                path: "C:\\Windows\\System32\\drivers\\ViGEmBus.sys".into(),
                file_version: Some("1.21.442.0".into()),
                file_version_string: None,
                company: Some("Nefarius Software Solutions e.U.".into()),
                description: None,
                signature: None,
            }),
        }
    }

    #[test]
    fn pads_json_shape() {
        let rows = [
            PadRow {
                slot: 1,
                handle: 0,
                user_index: Some(0),
                led_number: Some(2),
            },
            PadRow {
                slot: 2,
                handle: 1,
                user_index: None,
                led_number: None,
            },
        ];
        let v = pads_json(&fixture_driver(), &rows);
        insta::assert_snapshot!(serde_json::to_string_pretty(&v).unwrap());
    }

    #[test]
    fn error_json_shape() {
        let v = error_json("vigembus-missing", "ViGEmBus is not installed");
        assert_eq!(
            v.pointer("/error/code"),
            Some(&serde_json::json!("vigembus-missing"))
        );
        assert_eq!(
            v.pointer("/error/message"),
            Some(&serde_json::json!("ViGEmBus is not installed"))
        );
    }
}
