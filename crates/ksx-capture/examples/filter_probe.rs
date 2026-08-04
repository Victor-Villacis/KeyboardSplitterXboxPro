//! Diagnostic: is the Interception filter actually applied, and does the driver
//! deliver strokes? Passthrough-only (every stroke re-sent verbatim), 15 s hard
//! limit. Run: cargo run -p ksx-capture --example filter_probe
//! PRESS KEYS on any keyboard while it runs.
#![cfg(windows)]

use std::time::Instant;

use kanata_interception::raw;

const FILTER_KEY_ALL: u16 = 0xFFFF;

fn main() {
    unsafe {
        let ctx = raw::interception_create_context();
        assert!(!ctx.is_null(), "create_context failed");
        println!("context created");

        for dev in 1..=10 {
            if raw::interception_is_invalid(dev) == 0 {
                let f = raw::interception_get_filter(ctx, dev);
                println!("dev {dev}: filter before = {f:#06x}");
            }
        }

        raw::interception_set_filter(ctx, Some(raw::interception_is_keyboard), FILTER_KEY_ALL);
        println!("set_filter(keyboard, ALL) done");

        for dev in 1..=10 {
            if raw::interception_is_invalid(dev) == 0 {
                let f = raw::interception_get_filter(ctx, dev);
                println!("dev {dev}: filter after  = {f:#06x}");
            }
        }

        println!("waiting 15s — PRESS KEYS NOW (passthrough, keys keep working)");
        let t0 = Instant::now();
        let mut strokes = 0u32;
        let mut buf = [raw::InterceptionKeyStroke::default(); 32];
        while t0.elapsed().as_secs() < 15 {
            let dev = raw::interception_wait_with_timeout(ctx, 500);
            if dev == 0 {
                continue;
            }
            if raw::interception_is_keyboard(dev) == 0 {
                continue;
            }
            let n = raw::interception_receive(ctx, dev, buf.as_mut_ptr().cast(), 32);
            if n <= 0 {
                println!("wait signaled dev {dev} but receive returned {n}");
                continue;
            }
            for s in &buf[..n as usize] {
                println!(
                    "dev {dev}: code={} state={:#06x} info={}",
                    s.code, s.state, s.information
                );
            }
            raw::interception_send(ctx, dev, buf.as_ptr().cast(), n as u32);
            strokes += n as u32;
        }
        println!("total strokes: {strokes}");
        raw::interception_set_filter(ctx, Some(raw::interception_is_keyboard), 0);
        raw::interception_destroy_context(ctx);
        println!("filter reset, context destroyed — done");
    }
}
