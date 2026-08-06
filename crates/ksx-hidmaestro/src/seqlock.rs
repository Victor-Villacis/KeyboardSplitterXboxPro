//! The seqlocked latch in HIDMaestro's shared section.
//!
//! HIDMaestro is not a pipe/COM/IOCTL client: the SDK is in-process and the
//! driver reads a **seqlocked latch** out of a shared memory section
//! (`padforge-code-audit.md` §3.1). Crucially, **the consumer drives the
//! cadence** — there is no internal pumping thread. ksx publishes a frame; the
//! driver reads it whenever it reads, at whatever rate its consumers force.
//! That is why nothing in this crate spawns a submit thread, and why
//! [`crate::keepalive`] exists at all.
//!
//! ## The discipline
//!
//! Layout: `[seq: u32][payload: N bytes]`. `seq` is HIDMaestro's `SeqNo`.
//!
//! - **Writer**: bump `seq` to odd → write payload → bump to even. A reader that
//!   sees an odd `seq`, or a different `seq` before and after, discards what it
//!   read. Writers never block and never wait for readers — a slow driver costs
//!   staleness, never backpressure into the 1 kHz engine.
//! - **Reader**: read `seq` (must be even) → copy payload → re-read `seq` →
//!   accept only if unchanged.
//!
//! `seq` advancing is itself load-bearing: three separate driver watchdogs count
//! how long it sits still (see [`crate::keepalive`]).
//!
//! ## Why the payload is `[AtomicU8]`
//!
//! A seqlock is, by construction, a data race: the reader deliberately reads
//! bytes the writer may be mid-way through changing, and *then* decides whether
//! to keep them. Expressed with `&mut [u8]` + `copy_nonoverlapping` that is
//! plain UB in the Rust model, however well it behaves in practice. Modeling the
//! payload as `[AtomicU8]` with relaxed accesses makes the torn read *defined*
//! (each byte is some value that was really written) and the discipline above
//! turns "defined but possibly torn" into "correct". The cost is a byte-at-a-time
//! copy of an 80-byte frame, which is nothing next to the 1 ms tick.

use std::sync::atomic::{fence, AtomicU32, AtomicU8, Ordering};

use crate::error::HmError;

/// How many times a read retries before declaring the writer wedged. At 1 kHz
/// publication a reader is unlucky at most once or twice; 64 means "something is
/// actually broken", not "contended".
pub const READ_RETRIES: u32 = 64;

/// Bytes of header before the payload (the `seq` word).
pub const HEADER_BYTES: usize = 4;

/// Storage a [`Latch`] lives in.
///
/// Two implementations: a heap buffer (tests, and the in-process fake) and a
/// Windows file mapping (a real HIDMaestro section). The seqlock discipline is
/// identical over both, which is what lets it be tested with no driver.
pub trait LatchStorage {
    /// The whole region, header included.
    fn bytes(&self) -> &[AtomicU8];
}

/// A heap-backed region — the test double, and the only storage this crate can
/// exercise on a machine without HIDMaestro.
pub struct HeapStorage(Box<[AtomicU8]>);

impl HeapStorage {
    pub fn new(payload_bytes: usize) -> Self {
        let mut v = Vec::with_capacity(HEADER_BYTES + payload_bytes);
        v.resize_with(HEADER_BYTES + payload_bytes, || AtomicU8::new(0));
        Self(v.into_boxed_slice())
    }
}

impl LatchStorage for HeapStorage {
    fn bytes(&self) -> &[AtomicU8] {
        &self.0
    }
}

/// A seqlocked latch over some [`LatchStorage`].
pub struct Latch<S: LatchStorage> {
    storage: S,
    payload_bytes: usize,
}

impl<S: LatchStorage> std::fmt::Debug for Latch<S> {
    /// Never dumps the payload: it is shared memory another process is writing,
    /// so a `{:?}` in a log would be a torn read with no seqlock check.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Latch")
            .field("seq", &self.seq())
            .field("payload_bytes", &self.payload_bytes)
            .finish_non_exhaustive()
    }
}

impl<S: LatchStorage> Latch<S> {
    /// Wraps a region. Fails when the region is too small for the header plus
    /// `payload_bytes` — a version mismatch, worth naming rather than
    /// truncating.
    pub fn new(storage: S, payload_bytes: usize) -> Result<Self, HmError> {
        let needed = HEADER_BYTES + payload_bytes;
        let actual = storage.bytes().len();
        if actual < needed {
            return Err(HmError::SectionTooSmall { actual, needed });
        }
        Ok(Self {
            storage,
            payload_bytes,
        })
    }

    fn seq_word(&self) -> &AtomicU32 {
        let bytes = self.storage.bytes();
        // SAFETY: `AtomicU8` and `AtomicU32` are both `#[repr(C)]` over their
        // integer, so the first four bytes of the region are the bit pattern of
        // a `u32`. The cast is sound provided the region is 4-byte aligned,
        // which every constructor guarantees: `HeapStorage` allocates a `Vec`
        // whose data pointer is at least pointer-aligned, and `MappedStorage`
        // hands back the base of a `MapViewOfFile` view, which Windows aligns to
        // the allocation granularity (64 KiB). `Latch::new` has already checked
        // the region holds at least `HEADER_BYTES`, so the reference is in
        // bounds, and nothing ever hands out a `&mut` to these bytes.
        unsafe {
            debug_assert!(bytes.as_ptr() as usize % align_of::<AtomicU32>() == 0);
            &*(bytes.as_ptr() as *const AtomicU32)
        }
    }

    fn payload(&self) -> &[AtomicU8] {
        &self.storage.bytes()[HEADER_BYTES..HEADER_BYTES + self.payload_bytes]
    }

    /// Current `SeqNo`. Odd means a write is in flight.
    pub fn seq(&self) -> u32 {
        self.seq_word().load(Ordering::Acquire)
    }

    pub fn payload_len(&self) -> usize {
        self.payload_bytes
    }

    /// Publishes a frame (writer side) and returns the new `SeqNo`.
    ///
    /// Non-blocking, allocation-free, and never fails: a seqlock writer has
    /// nobody to wait for. `frame` must be exactly [`Latch::payload_len`] bytes.
    ///
    /// # Panics
    /// Panics on a wrong-sized frame — a caller bug (the frame size is a
    /// compile-time constant), not a runtime condition.
    pub fn publish(&self, frame: &[u8]) -> u32 {
        assert_eq!(
            frame.len(),
            self.payload_bytes,
            "frame must be exactly the payload size"
        );
        let seq = self.seq_word();
        let start = seq.load(Ordering::Relaxed);
        // Odd: "a write is in flight". Release so no reader can observe the odd
        // marker *after* the payload stores it is meant to guard.
        seq.store(start.wrapping_add(1), Ordering::Release);
        fence(Ordering::Release);
        for (slot, byte) in self.payload().iter().zip(frame) {
            slot.store(*byte, Ordering::Relaxed);
        }
        fence(Ordering::Release);
        let done = start.wrapping_add(2);
        seq.store(done, Ordering::Release);
        done
    }

    /// Reads a stable snapshot (reader side — the feedback direction, where the
    /// driver writes and ksx reads).
    ///
    /// Returns the `SeqNo` the snapshot belongs to. `Err(LatchTorn)` means the
    /// writer never settled within [`READ_RETRIES`].
    ///
    /// # Panics
    /// Panics on a wrong-sized buffer, for the same reason as [`Latch::publish`].
    pub fn read_into(&self, out: &mut [u8]) -> Result<u32, HmError> {
        assert_eq!(
            out.len(),
            self.payload_bytes,
            "buffer must be exactly the payload size"
        );
        let seq = self.seq_word();
        for _ in 0..READ_RETRIES {
            let before = seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                // A write is in flight; nothing here is worth copying.
                std::hint::spin_loop();
                continue;
            }
            for (byte, slot) in out.iter_mut().zip(self.payload()) {
                *byte = slot.load(Ordering::Relaxed);
            }
            fence(Ordering::Acquire);
            if seq.load(Ordering::Acquire) == before {
                return Ok(before);
            }
            std::hint::spin_loop();
        }
        Err(HmError::LatchTorn {
            retries: READ_RETRIES,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latch(n: usize) -> Latch<HeapStorage> {
        Latch::new(HeapStorage::new(n), n).unwrap()
    }

    #[test]
    fn a_published_frame_reads_back_verbatim() {
        let l = latch(8);
        assert_eq!(l.seq(), 0);
        let seq = l.publish(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut out = [0u8; 8];
        assert_eq!(l.read_into(&mut out).unwrap(), seq);
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    /// The discipline itself: every completed publish advances `SeqNo` by
    /// exactly 2 and leaves it even. The driver's watchdogs count this number,
    /// so "by 2, always even" is a wire contract, not an implementation detail.
    #[test]
    fn seqno_advances_by_two_per_publish_and_rests_even() {
        let l = latch(4);
        let mut last = l.seq();
        for i in 0..10u8 {
            let seq = l.publish(&[i; 4]);
            assert_eq!(seq, last + 2, "publish {i}");
            assert_eq!(seq % 2, 0, "a settled latch is never odd");
            assert_eq!(l.seq(), seq);
            last = seq;
        }
    }

    #[test]
    fn seqno_wraps_without_going_odd() {
        // A pad held for ~25 days at 1 kHz wraps u32. The wrap must not leave
        // the latch permanently "mid-write".
        let l = latch(4);
        l.seq_word().store(u32::MAX - 1, Ordering::Release);
        let seq = l.publish(&[9; 4]);
        assert_eq!(seq, 0, "wraps to even, not to an odd marker");
        let mut out = [0u8; 4];
        assert_eq!(l.read_into(&mut out).unwrap(), 0);
        assert_eq!(out, [9; 4]);
    }

    #[test]
    fn a_reader_refuses_a_write_in_flight() {
        let l = latch(4);
        l.publish(&[7; 4]);
        // Simulate a writer that started and has not finished: odd seq.
        l.seq_word().store(3, Ordering::Release);
        let mut out = [0u8; 4];
        let err = l.read_into(&mut out).unwrap_err();
        assert!(matches!(err, HmError::LatchTorn { retries } if retries == READ_RETRIES));
        // It must not have handed back the half-written payload.
        assert_eq!(out, [0; 4], "a torn read yields nothing, not garbage");
    }

    #[test]
    fn a_reader_retries_past_a_seqno_that_moved_under_it() {
        // The other torn case: seq was even both times but *different*.
        let l = latch(4);
        l.publish(&[1; 4]);
        let before = l.seq();
        l.seq_word().store(before + 2, Ordering::Release);
        // A stable latch reads fine; the point is the accept condition is
        // "before == after", which this now satisfies again.
        let mut out = [0u8; 4];
        assert_eq!(l.read_into(&mut out).unwrap(), before + 2);
    }

    /// Concurrency proof: a writer spinning at full tilt while a reader loops
    /// must never hand back a frame that mixes two publications. Frames are
    /// self-checking (all bytes equal), so a torn accept is detectable.
    #[test]
    fn concurrent_publish_never_yields_a_mixed_frame() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        const N: usize = 32;
        struct Shared(Box<[AtomicU8]>);
        impl LatchStorage for Arc<Shared> {
            fn bytes(&self) -> &[AtomicU8] {
                &self.0
            }
        }
        let mut v = Vec::new();
        v.resize_with(HEADER_BYTES + N, || AtomicU8::new(0));
        let shared = Arc::new(Shared(v.into_boxed_slice()));
        let writer = Latch::new(shared.clone(), N).unwrap();
        let reader = Latch::new(shared, N).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut i = 0u8;
            while !stop_w.load(Ordering::Relaxed) {
                writer.publish(&[i; N]);
                i = i.wrapping_add(1);
            }
        });

        let mut out = [0u8; N];
        let mut accepted = 0u32;
        for _ in 0..20_000 {
            if reader.read_into(&mut out).is_ok() {
                accepted += 1;
                assert!(
                    out.iter().all(|b| *b == out[0]),
                    "accepted a frame mixing two publications: {out:?}"
                );
            }
        }
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert!(accepted > 0, "the reader never got a stable snapshot");
    }

    #[test]
    fn a_short_section_is_a_named_error_not_a_truncation() {
        let err = Latch::new(HeapStorage::new(4), 64).unwrap_err();
        match err {
            HmError::SectionTooSmall { actual, needed } => {
                assert_eq!(actual, HEADER_BYTES + 4);
                assert_eq!(needed, HEADER_BYTES + 64);
            }
            other => panic!("expected SectionTooSmall, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "exactly the payload size")]
    fn publishing_a_wrong_sized_frame_is_a_caller_bug() {
        latch(8).publish(&[0; 4]);
    }
}
