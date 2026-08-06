//! Hot-path allocation check for the CHORD path (docs/ARCHITECTURE.md's
//! hot-path rule, docs/INPUT-TRANSFORMS.md §3 "hot path stays pure").
//!
//! Chord evaluation runs on the engine thread for every event of a chorded
//! slot, so it has to be allocation-free like everything else there: the guard
//! is bit tests over precompiled dense ids, and every buffer it touches
//! (`held`, `consumed`, `blocked`, `scan`) is sized in `EngineTables::build`.
//!
//! Single `#[test]` per binary, for the same reason as `engine_alloc.rs`: the
//! allocation counter is process-global.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{ev, ipac_device, preset_with_chords};
use ksx_core::{Binding, Chord, Key, KeyEvent, Trigger, XButton};

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
fn chord_evaluation_does_not_allocate_after_warmup() {
    let dev = ipac_device();
    let preset = preset_with_chords(
        "chords",
        vec![
            (Key::A, Binding::Button(XButton::X)),
            (Key::B, Binding::Button(XButton::Y)),
            (Key::G, Binding::Button(XButton::A)),
        ],
        vec![
            Chord::new(Key::A, Binding::Trigger(Trigger::Right), vec![Key::B]),
            Chord::new(
                Key::A,
                Binding::Button(XButton::LeftBumper),
                vec![Key::B, Key::C],
            ),
            Chord {
                key: Key::G,
                binding: Binding::Button(XButton::Start),
                when: vec![Key::D],
                unless: vec![Key::LeftShift],
            },
        ],
    );
    let mut engine = common::engine_for(preset);

    // Every key the preset touches, bound or guard-only.
    let keys = [
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::G,
        Key::LeftShift,
        // An unbound key: the "not in this slot at all" path.
        Key::Z,
    ];
    let mut events: Vec<KeyEvent> = Vec::new();
    for &k in &keys {
        events.push(ev(&dev, k, true));
    }
    for &k in &keys {
        events.push(ev(&dev, k, false));
    }

    let mut sink = 0u64;
    for e in &events {
        for d in engine.handle(e) {
            sink = sink.wrapping_add(d.slot as u64);
        }
    }
    engine.release_device(&dev);

    let before = ALLOCATIONS.load(Ordering::SeqCst);
    assert!(before > 0, "counting allocator is not installed");
    for _ in 0..100 {
        for e in &events {
            for d in engine.handle(e) {
                sink = sink.wrapping_add(d.state.buttons.bits() as u64);
            }
        }
        // Half-pressed, then unplugged: the full-resync path with chords held.
        for e in events.iter().take(events.len() / 2) {
            engine.handle(e);
        }
        engine.release_device(&dev);
    }
    let after = ALLOCATIONS.load(Ordering::SeqCst);

    assert_eq!(
        before,
        after,
        "chord hot path allocated {} time(s) (sink={sink})",
        after - before
    );
}
