//! Diagnostic: is `interception_wait_with_timeout` the reason we never receive
//! strokes on this machine? Phase 1 uses the timeout variant, phase 2 the plain
//! blocking `interception_wait` the vendor samples use. Same filter for both.
//! Every stroke is re-sent (passthrough) and the filter is cleared on exit; a
//! guard thread hard-clears + exits after 60 s no matter what.
//! Run: cargo run -p ksx-capture --example wait_probe  — then PRESS KEYS.
#![cfg(windows)]

use std::time::{Duration, Instant};

use kanata_interception::raw;

/// KEY_DOWN | KEY_UP — exactly what the vendor's hardwareid sample sets.
const FILTER_DOWN_UP: u16 = 0x0003;

struct Ctx(raw::InterceptionContext);
unsafe impl Send for Ctx {}
unsafe impl Sync for Ctx {}

fn main() {
    unsafe {
        let ctx = Ctx(raw::interception_create_context());
        assert!(!ctx.0.is_null(), "create_context failed");
        raw::interception_set_filter(ctx.0, Some(raw::interception_is_keyboard), FILTER_DOWN_UP);
        println!("filter set to {FILTER_DOWN_UP:#06x} (KEY_DOWN|KEY_UP)\n");

        // Hard safety net: never leave a filter set if anything goes sideways.
        let guard_ctx = ctx.0 as usize;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(60));
            let c = guard_ctx as raw::InterceptionContext;
            raw::interception_set_filter(c, Some(raw::interception_is_keyboard), 0);
            println!("\n[guard] 60s elapsed — filter cleared, exiting");
            std::process::exit(0);
        });

        let mut buf = [raw::InterceptionKeyStroke::default(); 16];

        println!("PHASE 1: interception_wait_with_timeout(500ms) for 12s — PRESS KEYS NOW");
        let mut phase1 = 0u32;
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(12) {
            let dev = raw::interception_wait_with_timeout(ctx.0, 500);
            if dev == 0 || raw::interception_is_keyboard(dev) == 0 {
                continue;
            }
            let n = raw::interception_receive(ctx.0, dev, buf.as_mut_ptr().cast(), 16);
            if n > 0 {
                raw::interception_send(ctx.0, dev, buf.as_ptr().cast(), n as u32);
                phase1 += n as u32;
            }
        }
        println!("PHASE 1 result: {phase1} stroke(s)\n");

        println!("PHASE 2: plain interception_wait() — press ~10 keys (ESC ends early)");
        let mut phase2 = 0u32;
        while phase2 < 20 {
            let dev = raw::interception_wait(ctx.0);
            if dev == 0 || raw::interception_is_keyboard(dev) == 0 {
                continue;
            }
            let n = raw::interception_receive(ctx.0, dev, buf.as_mut_ptr().cast(), 16);
            if n <= 0 {
                continue;
            }
            raw::interception_send(ctx.0, dev, buf.as_ptr().cast(), n as u32);
            for s in &buf[..n as usize] {
                println!("  dev {dev}: code={} state={:#04x}", s.code, s.state);
            }
            phase2 += n as u32;
            if buf[..n as usize].iter().any(|s| s.code == 1) {
                break; // ESC
            }
        }
        println!("\nPHASE 2 result: {phase2} stroke(s)");

        raw::interception_set_filter(ctx.0, Some(raw::interception_is_keyboard), 0);
        raw::interception_destroy_context(ctx.0);
        println!("filter cleared, context destroyed");
        println!(
            "\nVERDICT: wait_with_timeout={}  plain_wait={}",
            if phase1 > 0 { "WORKS" } else { "BROKEN" },
            if phase2 > 0 { "WORKS" } else { "BROKEN" }
        );
    }
}
