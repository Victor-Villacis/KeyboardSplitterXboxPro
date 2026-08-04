//! Cabinet-only XInput loopback (M2 exit criteria).
//!
//! Requires the ViGEmBus driver → gated behind the `cab-tests` feature and never
//! part of default `cargo test` / CI:
//!
//! ```text
//! cargo test -p ksx-output --features cab-tests --test loopback -- --nocapture
//! ```
//!
//! Compiles (empty) without the feature so `cargo check --workspace` stays green
//! on driverless machines; verify the gated build with
//! `cargo check -p ksx-output --features cab-tests --all-targets`.
//!
//! Kill-recovery note: dropping the backend unplugs all pads (asserted below —
//! this is the `taskkill` graceful path and the panic-unwind path). A hard
//! process kill (`taskkill /f`) runs no destructors; ViGEm targets then linger
//! until the driver reaps the dead client's handles. If a ghost pad ever
//! survives, remove it under Devices and Printers (vendored lib.rs documents
//! this) — `ksx doctor` should eventually detect the condition.
#![cfg(all(windows, feature = "cab-tests"))]

use std::thread;
use std::time::{Duration, Instant};

use ksx_core::pad::{PadState, XButtons};
use ksx_output::{VigemBackend, VirtualPadBackend};
use rusty_xinput::XInputHandle;

const SETTLE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);

fn connect() -> VigemBackend {
    match VigemBackend::connect() {
        Ok(b) => b,
        Err(e) => panic!("cannot run cab-tests: {e}"),
    }
}

/// Polls XInput until the read-back state matches `want` or the deadline hits.
fn wait_for_state(xi: &XInputHandle, user_index: u32, want: &PadState) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
        if let Ok(state) = xi.get_state(user_index) {
            let gp = state.raw.Gamepad;
            if gp.wButtons == want.buttons.bits()
                && gp.bLeftTrigger == want.lt
                && gp.bRightTrigger == want.rt
                && gp.sThumbLX == want.lx
                && gp.sThumbLY == want.ly
                && gp.sThumbRX == want.rx
                && gp.sThumbRY == want.ry
            {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

fn wait_for_disconnect(xi: &XInputHandle, user_index: u32) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
        if xi.get_state(user_index).is_err() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// Every button bit XInputGetState reports (GUIDE is filtered by the public
/// XInput API, so it round-trips only via get_state_ex — out of scope here).
const VISIBLE_BUTTONS: [XButtons; 14] = [
    XButtons::DPAD_UP,
    XButtons::DPAD_DOWN,
    XButtons::DPAD_LEFT,
    XButtons::DPAD_RIGHT,
    XButtons::START,
    XButtons::BACK,
    XButtons::LEFT_THUMB,
    XButtons::RIGHT_THUMB,
    XButtons::LEFT_BUMPER,
    XButtons::RIGHT_BUMPER,
    XButtons::A,
    XButtons::B,
    XButtons::X,
    XButtons::Y,
];

/// One monolithic test on purpose: cargo runs `#[test]`s concurrently, and two
/// tests plugging pads at once would race for XInput slots.
#[test]
fn four_pads_full_loopback() {
    let xi = XInputHandle::load_default().expect("XInput DLL");
    let mut backend = connect();

    // --- Plug 4, assert distinct user indices 0..=3. ---
    let handles: Vec<_> = (0..4)
        .map(|i| backend.plug().unwrap_or_else(|e| panic!("plug #{i}: {e}")))
        .collect();
    let mut indices: Vec<u8> = handles
        .iter()
        .map(|&h| backend.user_index(h).expect("pad got no XInput slot"))
        .collect();
    let plug_order = indices.clone();
    indices.sort_unstable();
    assert_eq!(indices, [0, 1, 2, 3], "4 pads must own the 4 XInput slots");
    eprintln!("plug order -> user indices: {plug_order:?}");

    // --- State round-trip per pad: every visible button bit, then axes. ---
    for &h in &handles {
        let idx = u32::from(backend.user_index(h).unwrap());

        for bit in VISIBLE_BUTTONS {
            let state = PadState {
                buttons: bit,
                ..PadState::default()
            };
            backend.update(h, &state).unwrap();
            assert!(
                wait_for_state(&xi, idx, &state),
                "pad {idx}: button {bit:?} did not round-trip"
            );
        }

        let extremes = PadState {
            buttons: XButtons::A | XButtons::DPAD_UP | XButtons::RIGHT_BUMPER,
            lt: 255,
            rt: 1,
            lx: i16::MIN,
            ly: i16::MAX,
            rx: -12345,
            ry: 12345,
        };
        backend.update(h, &extremes).unwrap();
        assert!(
            wait_for_state(&xi, idx, &extremes),
            "pad {idx}: combined extremes did not round-trip"
        );

        let neutral = PadState::default();
        backend.update(h, &neutral).unwrap();
        assert!(
            wait_for_state(&xi, idx, &neutral),
            "pad {idx}: neutral state did not round-trip"
        );
    }

    // --- Feedback: drive rumble via XInputSetState, read it back through the
    // notification queue. Motor bytes are the high byte of the u16 speed. ---
    for &h in &handles {
        let idx = u32::from(backend.user_index(h).unwrap());
        xi.set_state(idx, 0x9C00, 0x4E00).unwrap();
        let deadline = Instant::now() + SETTLE;
        let feedback = loop {
            assert!(
                Instant::now() < deadline,
                "pad {idx}: rumble feedback never arrived"
            );
            match backend.poll_feedback(h) {
                Some(f) if f.large_motor == 0x9C && f.small_motor == 0x4E => break f,
                Some(_) => {} // earlier LED-only notifications
                None => thread::sleep(POLL),
            }
        };
        // LED semantics are recorded here (not asserted) until verified across
        // driver versions; user_index() remains the slot source of truth.
        eprintln!("pad {idx}: feedback {feedback:?}");
        xi.set_state(idx, 0, 0).unwrap();
    }

    // --- Stale-handle contract. ---
    let first = handles[0];
    let first_idx = u32::from(backend.user_index(first).unwrap());
    backend.unplug(first).unwrap();
    assert!(
        wait_for_disconnect(&xi, first_idx),
        "unplugged pad still visible to XInput"
    );
    assert!(
        backend.update(first, &PadState::default()).is_err(),
        "update on a dead handle must fail"
    );

    // --- Explicit unplug for the rest. ---
    for &h in &handles[1..] {
        let idx = u32::from(backend.user_index(h).unwrap());
        backend.unplug(h).unwrap();
        assert!(wait_for_disconnect(&xi, idx), "pad {idx} still connected");
    }

    // --- Kill recovery (graceful path): drop the backend → pads vanish. ---
    let mut backend = connect();
    let h = backend.plug().unwrap();
    let idx = u32::from(backend.user_index(h).unwrap());
    assert!(xi.get_state(idx).is_ok(), "pad should be live before drop");
    drop(backend);
    assert!(
        wait_for_disconnect(&xi, idx),
        "dropping the backend must unplug its pads"
    );
}
