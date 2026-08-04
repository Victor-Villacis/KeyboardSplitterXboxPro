//! Diagnostic: can an x64 client filter at all on this machine — with no human
//! at the keyboard? One context sets a filter and waits; a second context
//! injects a harmless F13 stroke. If the filtering context sees it, x64 capture
//! works and earlier zero-stroke runs just had nobody pressing keys.
//!
//! F13 (scancode 0x64) is chosen because Windows ignores it; and if the filter
//! captures it we never re-send, so nothing reaches the OS either way.
//! Run: cargo run -p ksx-capture --example selftest_loop
#![cfg(windows)]

use std::time::{Duration, Instant};

use kanata_interception::raw;

const FILTER_DOWN_UP: u16 = 0x0003;
const SCANCODE_F13: u16 = 0x64;

fn main() {
    unsafe {
        let rx = raw::interception_create_context();
        let tx = raw::interception_create_context();
        assert!(!rx.is_null() && !tx.is_null(), "create_context failed");

        raw::interception_set_filter(rx, Some(raw::interception_is_keyboard), FILTER_DOWN_UP);
        println!("receiver: filter {FILTER_DOWN_UP:#06x} set on keyboard devices");

        // Inject from a separate thread so the receiver is already waiting.
        let tx_addr = tx as usize;
        std::thread::spawn(move || {
            let tx = tx_addr as raw::InterceptionContext;
            std::thread::sleep(Duration::from_millis(600));
            for dev in 1..=10 {
                if raw::interception_is_invalid(dev) != 0 {
                    continue;
                }
                let down = raw::InterceptionKeyStroke {
                    code: SCANCODE_F13,
                    state: 0, // KEY_DOWN
                    information: 0,
                };
                let up = raw::InterceptionKeyStroke {
                    code: SCANCODE_F13,
                    state: 1, // KEY_UP
                    information: 0,
                };
                raw::interception_send(
                    tx,
                    dev,
                    (&down as *const raw::InterceptionKeyStroke).cast(),
                    1,
                );
                raw::interception_send(
                    tx,
                    dev,
                    (&up as *const raw::InterceptionKeyStroke).cast(),
                    1,
                );
                println!("sender: injected F13 down/up on device {dev}");
                std::thread::sleep(Duration::from_millis(120));
            }
        });

        let mut buf = [raw::InterceptionKeyStroke::default(); 16];
        let mut seen = 0u32;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            let dev = raw::interception_wait_with_timeout(rx, 300);
            if dev == 0 || raw::interception_is_keyboard(dev) == 0 {
                continue;
            }
            let n = raw::interception_receive(rx, dev, buf.as_mut_ptr().cast(), 16);
            if n <= 0 {
                continue;
            }
            for s in &buf[..n as usize] {
                println!("receiver: dev {dev} code={} state={:#04x}", s.code, s.state);
                // Pass through anything that is not our synthetic probe key.
                if s.code != SCANCODE_F13 {
                    raw::interception_send(
                        rx,
                        dev,
                        (s as *const raw::InterceptionKeyStroke).cast(),
                        1,
                    );
                }
            }
            seen += n as u32;
        }

        raw::interception_set_filter(rx, Some(raw::interception_is_keyboard), 0);
        raw::interception_destroy_context(rx);
        raw::interception_destroy_context(tx);
        println!(
            "\nreceiver saw {seen} stroke(s) — x64 filtering {}",
            if seen > 0 { "WORKS" } else { "did NOT deliver" }
        );
    }
}
