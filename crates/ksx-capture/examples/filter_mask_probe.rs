//! Diagnostic: which keyboard filter masks does the INSTALLED driver accept?
//! Sets each candidate mask and reads it back — no strokes, no key presses, and
//! the filter is cleared to NONE before exit.
//! Run: cargo run -p ksx-capture --example filter_mask_probe
#![cfg(windows)]

use kanata_interception::raw;

fn main() {
    unsafe {
        let ctx = raw::interception_create_context();
        assert!(!ctx.is_null(), "create_context failed");

        // KEY_DOWN=0x01, KEY_UP=0x02, E0=0x04, E1=0x08, TERMSRV_*=0x10..0x40, ALL=0xFFFF
        for mask in [
            0x0001u16, 0x0002, 0x0003, 0x000F, 0x007F, 0x00FF, 0x0FFF, 0xFFFF,
        ] {
            raw::interception_set_filter(ctx, Some(raw::interception_is_keyboard), mask);
            let readback: Vec<String> = (1..=3)
                .map(|d| format!("{:#06x}", raw::interception_get_filter(ctx, d)))
                .collect();
            println!("set {mask:#06x} -> dev1..3 read back {readback:?}");
        }

        // Per-device rather than predicate-wide, in case the predicate is the issue.
        raw::interception_set_filter(ctx, Some(raw::interception_is_keyboard), 0);
        for dev in 1..=3 {
            raw::interception_set_filter(ctx, Some(raw::interception_is_keyboard), 0x0003);
            println!(
                "after per-call set 0x0003: dev{dev} = {:#06x}",
                raw::interception_get_filter(ctx, dev)
            );
        }

        raw::interception_set_filter(ctx, Some(raw::interception_is_keyboard), 0);
        println!(
            "cleared; final dev1 = {:#06x}",
            raw::interception_get_filter(ctx, 1)
        );
        raw::interception_destroy_context(ctx);
    }
}
