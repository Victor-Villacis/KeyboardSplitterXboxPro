//! Hot-path allocation sanity check: `handle()`/`release_device()` on the
//! 4-slot I-PAC configuration must not heap-allocate after warmup.
//!
//! This file deliberately contains a single `#[test]` — with one test in the
//! binary no concurrent test can touch the process-global counter while the
//! measured section runs. Events are pre-built because constructing a
//! `KeyEvent` clones the device-id string (an allocation that belongs to the
//! capture layer's arena, not to the engine).

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ksx_core::KeyEvent;

struct CountingAlloc;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: delegates to `System`, only adding a relaxed counter bump.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[test]
fn handle_does_not_allocate_after_warmup() {
    let mut engine = common::ipac_engine();
    let dev = common::ipac_device();

    // Full press+release sweep of every key bound anywhere in the 4 presets,
    // plus a mid-sweep partial release for the opposite-axis path.
    let mut events: Vec<KeyEvent> = Vec::new();
    let keys = common::ipac_bound_keys();
    for &k in &keys {
        events.push(common::ev(&dev, k, true));
    }
    for &k in &keys {
        events.push(common::ev(&dev, k, false));
    }

    let mut sink = 0u64; // fold delta output so it cannot be optimized away

    // Warmup pass.
    for e in &events {
        for d in engine.handle(e) {
            sink = sink.wrapping_add(d.slot as u64);
        }
    }
    engine.release_device(&dev);

    let before = ALLOCATIONS.load(Ordering::SeqCst);
    // Guard against a vacuous pass: engine + event construction above must
    // have gone through the counting hook.
    assert!(before > 0, "counting allocator is not installed");
    for _ in 0..100 {
        for e in &events {
            for d in engine.handle(e) {
                sink = sink.wrapping_add(d.state.buttons.bits() as u64);
            }
        }
        for e in events.iter().take(events.len() / 2) {
            engine.handle(e); // re-press half, then unplug mid-press
        }
        engine.release_device(&dev);
    }
    let after = ALLOCATIONS.load(Ordering::SeqCst);

    assert_eq!(
        before,
        after,
        "engine hot path allocated {} time(s) (sink={sink})",
        after - before
    );
}
