//! Windows file-mapping storage for a [`crate::seqlock::Latch`].
//!
//! This is the transport half: a named section opened read/write, handed to the
//! same [`LatchStorage`] trait the heap test double implements, so the seqlock
//! discipline above it is byte-identical in tests and in production.
//!
//! The **section name is a parameter, not a constant**, and that is deliberate:
//! the audit documents the transport shape (shared section + event signalling,
//! seqlocked latch, consumer-driven cadence) but not HIDMaestro's naming
//! convention, because PadForge consumes the SDK in-process and never opens the
//! section itself. Hardcoding a guessed name here would be exactly the kind of
//! invented fact this crate is trying not to ship. The name comes from the
//! driver implementation when there is one.

use std::sync::atomic::AtomicU8;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
    MEMORY_MAPPED_VIEW_ADDRESS,
};

use crate::error::HmError;
use crate::seqlock::LatchStorage;

/// A mapped view of a named shared section.
///
/// `Send` but not `Sync`-by-default beyond what the atomics provide: the view is
/// shared memory, every access goes through `AtomicU8`/`AtomicU32`, and the
/// seqlock discipline is what makes concurrent access *correct* rather than
/// merely defined.
#[derive(Debug)]
pub struct MappedStorage {
    mapping: HANDLE,
    view: *mut u8,
    len: usize,
}

// SAFETY: the only state is a kernel handle and a pointer to a shared view.
// Neither has thread affinity: `CloseHandle`/`UnmapViewOfFile` may be called
// from any thread, and every read/write of the view goes through atomics in
// `LatchStorage::bytes`. Moving the value between threads therefore cannot
// create a data race that the atomics do not already define.
unsafe impl Send for MappedStorage {}
// SAFETY: `bytes()` hands out `&[AtomicU8]`, so all shared access is atomic;
// there is no interior `&mut` path to the view anywhere in this crate.
unsafe impl Sync for MappedStorage {}

impl MappedStorage {
    /// Opens an existing named section and maps `len` bytes of it read/write.
    ///
    /// Fails with [`HmError::Win32`] when the section does not exist — which,
    /// on a machine without HIDMaestro, is what always happens.
    pub fn open(name: &str, len: usize) -> Result<Self, HmError> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the
        // call. `OpenFileMappingW` only reads it, and returns a null handle on
        // failure, which is checked immediately below.
        let mapping = unsafe { OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, wide.as_ptr()) };
        if mapping.is_null() {
            // SAFETY: `GetLastError` reads this thread's last-error slot; it
            // has no preconditions and cannot fail.
            let code = unsafe { GetLastError() };
            return Err(HmError::Win32 {
                op: "OpenFileMappingW",
                code,
            });
        }
        // SAFETY: `mapping` is a valid, non-null section handle from the call
        // above, opened with exactly the access requested here. A zero offset
        // with an explicit length is in range for any section at least `len`
        // bytes; a shorter section makes the call fail, which is checked below.
        let view: MEMORY_MAPPED_VIEW_ADDRESS =
            unsafe { MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, len) };
        if view.Value.is_null() {
            // SAFETY: see above.
            let code = unsafe { GetLastError() };
            // SAFETY: `mapping` is a valid handle we own and have not closed;
            // nothing else references it, and we return immediately after.
            unsafe { CloseHandle(mapping) };
            return Err(HmError::Win32 {
                op: "MapViewOfFile",
                code,
            });
        }
        Ok(Self {
            mapping,
            view: view.Value.cast::<u8>(),
            len,
        })
    }
}

impl LatchStorage for MappedStorage {
    fn bytes(&self) -> &[AtomicU8] {
        // SAFETY: `self.view` is a live mapped view of at least `self.len`
        // bytes (checked at `open`, unmapped only in `Drop`, and never handed
        // out elsewhere). `AtomicU8` is `#[repr(C)]` over `u8` with the same
        // size and alignment, and every access through the returned slice is an
        // atomic operation — which is what makes the concurrent writes the
        // driver performs *defined* rather than UB. The lifetime is tied to
        // `&self`, so the slice cannot outlive the mapping.
        unsafe { std::slice::from_raw_parts(self.view.cast::<AtomicU8>(), self.len) }
    }
}

impl Drop for MappedStorage {
    fn drop(&mut self) {
        // SAFETY: both handles are ours, still valid (nothing else closes
        // them), and unreachable after this — `Drop` runs once, and `bytes()`
        // borrows `&self`, so no slice into the view can still be alive.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
            CloseHandle(self.mapping);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_section_that_does_not_exist_is_a_named_win32_error() {
        // ERROR_FILE_NOT_FOUND (2) is the "HIDMaestro is not running" answer.
        let err = MappedStorage::open("ksx-hidmaestro-no-such-section-9f3a", 4096).unwrap_err();
        match err {
            HmError::Win32 { op, code } => {
                assert_eq!(op, "OpenFileMappingW");
                assert_eq!(code, 2, "expected ERROR_FILE_NOT_FOUND");
            }
            other => panic!("expected a Win32 error, got {other:?}"),
        }
    }
}
