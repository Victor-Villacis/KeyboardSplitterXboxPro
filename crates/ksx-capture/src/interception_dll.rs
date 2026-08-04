//! Runtime loader for `interception.dll` — the reason `ksx.exe` starts on a
//! machine that has never heard of Interception.
//!
//! # The bug this exists to kill
//!
//! `interception-sys` links `interception.lib`, an **import library**: every
//! `interception_*` call it resolves becomes a static entry in `ksx.exe`'s
//! import table, and the Windows loader resolves those *before `main` runs*.
//! Ship `ksx.exe` without the DLL beside it and the process dies at load time
//! with a modal loader dialog — "The code execution cannot proceed because
//! interception.dll was not found" — for `ksx --version`, for `ksx devices`,
//! for everything. Two consequences, both bad:
//!
//! 1. the installer has to ship a third-party DLL to run `ksx doctor`; and
//! 2. a cabinet migrated to the WinUSB backend — the entire point of M6, whose
//!    exit criterion is *the Interception driver uninstalled* — still could not
//!    start ksx.
//!
//! # The fix
//!
//! Nothing in ksx references an `interception_*` symbol any more. The linker
//! therefore pulls **no** member out of `interception.lib` and emits no import
//! descriptor for the DLL (asserted over the real binary's PE import table by
//! `ksx-app/tests/no_interception_dll.rs`). The functions are instead looked up
//! once, lazily, by [`api`] — which is called only from
//! [`crate::InterceptionBackend`]'s constructors, i.e. only when a session
//! genuinely needs the Interception backend.
//!
//! Types still come from `kanata_interception::raw`: a `#[repr(C)]` struct
//! declaration creates no linkage, and re-typing `InterceptionKeyStroke` by hand
//! would be a second source of truth for a layout the hot path depends on.
//!
//! # Why `LoadLibrary`, not `/DELAYLOAD:interception.dll`
//!
//! A delay-load stub whose DLL is missing raises a **structured exception**
//! (`VcppException(ERROR_MOD_NOT_FOUND)`) at the first call. Rust cannot catch
//! that, so "the DLL is not installed" would still be a crash — just a later,
//! stranger one — and it would only work under the MSVC linker. An explicit
//! `GetProcAddress` table fails where the failure belongs: as a `Result`, with a
//! message naming both ways out ([`CaptureError::DllMissing`]).

use std::ffi::{c_int, c_uint, c_ulong, c_void};
use std::sync::OnceLock;

use kanata_interception::raw::{
    InterceptionContext, InterceptionDevice, InterceptionFilter, InterceptionPredicate,
    InterceptionStroke,
};

use crate::backend::CaptureError;

/// The DLL ksx looks for, by the name the Interception installer gives it.
///
/// Searched with the ordinary loader search path: the directory of `ksx.exe`
/// first, then `System32`, then `PATH`. Deliberately not an absolute path — an
/// existing Interception installation puts it wherever it likes, and the app
/// directory is what the ksx installer will populate.
pub const DLL_NAME: &str = "interception.dll";

// Each entry point's C prototype, transcribed from interception-sys 0.1.3's
// bindgen output (itself generated from `interception.h`). Named aliases rather
// than inline types for one reason: `load` transmutes a raw address into each of
// them, and a transmute whose target is spelled out is a transmute that can be
// reviewed. Getting one of these wrong is a corrupted stroke or a wild call.
type CreateContext = unsafe extern "C" fn() -> InterceptionContext;
type DestroyContext = unsafe extern "C" fn(InterceptionContext);
type SetFilter =
    unsafe extern "C" fn(InterceptionContext, InterceptionPredicate, InterceptionFilter);
type WaitWithTimeout = unsafe extern "C" fn(InterceptionContext, c_ulong) -> InterceptionDevice;
type Send_ = unsafe extern "C" fn(
    InterceptionContext,
    InterceptionDevice,
    *const InterceptionStroke,
    c_uint,
) -> c_int;
type Receive = unsafe extern "C" fn(
    InterceptionContext,
    InterceptionDevice,
    *mut InterceptionStroke,
    c_uint,
) -> c_int;
type GetHardwareId =
    unsafe extern "C" fn(InterceptionContext, InterceptionDevice, *mut c_void, c_uint) -> c_uint;
type Predicate = unsafe extern "C" fn(InterceptionDevice) -> c_int;

/// Every `interception_*` entry point ksx uses, resolved once.
///
/// Field order mirrors the header. Function pointers are `Copy + Send + Sync`,
/// so one `&'static Api` is shared by the capture thread and its owner without
/// synchronisation — the hot loop calls through it with the same cost as a
/// direct import (one indirect call, exactly what an import thunk is anyway).
pub struct Api {
    pub create_context: CreateContext,
    pub destroy_context: DestroyContext,
    pub set_filter: SetFilter,
    pub wait_with_timeout: WaitWithTimeout,
    pub send: Send_,
    pub receive: Receive,
    pub get_hardware_id: GetHardwareId,
    pub is_invalid: Predicate,
    pub is_keyboard: Predicate,
    pub is_mouse: Predicate,
}

/// The symbols [`load`] resolves, in the order it resolves them. Public so a
/// test can assert the list did not silently shrink.
pub const SYMBOLS: &[&str] = &[
    "interception_create_context",
    "interception_destroy_context",
    "interception_set_filter",
    "interception_wait_with_timeout",
    "interception_send",
    "interception_receive",
    "interception_get_hardware_id",
    "interception_is_invalid",
    "interception_is_keyboard",
    "interception_is_mouse",
];

/// The loaded table, or the reason it could not be loaded.
///
/// Cached either way: a machine without the DLL must not pay a failed
/// `LoadLibraryW` per call, and a machine with it must not load it twice.
static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// The resolved entry points, loading the DLL on first use.
///
/// **This is the only place `interception.dll` is ever touched.** Call it from
/// backend construction, never from an enumeration or diagnostic path that is
/// expected to work on a machine without the driver.
pub fn api() -> Result<&'static Api, CaptureError> {
    match API.get_or_init(load) {
        Ok(api) => Ok(api),
        Err(detail) => Err(CaptureError::DllMissing {
            detail: detail.clone(),
        }),
    }
}

/// Has the DLL been loaded, without loading it? Diagnostics only.
pub fn is_loaded() -> bool {
    matches!(API.get(), Some(Ok(_)))
}

fn load() -> Result<Api, String> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let wide: Vec<u16> = DLL_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        // SAFETY: no arguments; reads this thread's last-error slot.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(format!(
            "LoadLibraryW(\"{DLL_NAME}\") failed, win32 error {code}"
        ));
    }

    // One helper, ten calls: a mistyped symbol name must be a clean error
    // naming the symbol, not a null pointer that segfaults on first use.
    let sym = |name: &str| -> Result<*const c_void, String> {
        let mut cname: Vec<u8> = name.as_bytes().to_vec();
        cname.push(0);
        // SAFETY: live module handle; `cname` is a NUL-terminated ASCII name.
        let addr = unsafe { GetProcAddress(module, cname.as_ptr()) };
        match addr {
            Some(f) => Ok(f as *const c_void),
            None => Err(format!(
                "{DLL_NAME} loaded but exports no '{name}' — the file is not Interception's \
                 DLL, or it is an incompatible version"
            )),
        }
    };

    // SAFETY: each address is the export whose C prototype is transcribed in
    // `Api` (verified against interception-sys 0.1.3's bindgen output, which is
    // itself generated from interception.h). Transmuting a function address to
    // its own declared signature is the documented use of GetProcAddress.
    unsafe {
        Ok(Api {
            create_context: std::mem::transmute::<*const c_void, CreateContext>(sym(SYMBOLS[0])?),
            destroy_context: std::mem::transmute::<*const c_void, DestroyContext>(sym(SYMBOLS[1])?),
            set_filter: std::mem::transmute::<*const c_void, SetFilter>(sym(SYMBOLS[2])?),
            wait_with_timeout: {
                let addr = sym(SYMBOLS[3])?;
                std::mem::transmute::<*const c_void, WaitWithTimeout>(addr)
            },
            send: std::mem::transmute::<*const c_void, Send_>(sym(SYMBOLS[4])?),
            receive: std::mem::transmute::<*const c_void, Receive>(sym(SYMBOLS[5])?),
            get_hardware_id: std::mem::transmute::<*const c_void, GetHardwareId>(sym(SYMBOLS[6])?),
            is_invalid: std::mem::transmute::<*const c_void, Predicate>(sym(SYMBOLS[7])?),
            is_keyboard: std::mem::transmute::<*const c_void, Predicate>(sym(SYMBOLS[8])?),
            is_mouse: std::mem::transmute::<*const c_void, Predicate>(sym(SYMBOLS[9])?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message a WinUSB-only user meets if something still asks for the
    /// Interception backend. It has to name **both** ways forward, because
    /// "install the driver" is the wrong advice for the cabinet M6 exists for.
    #[test]
    fn the_missing_dll_error_names_both_ways_out() {
        let err = CaptureError::DllMissing {
            detail: "LoadLibraryW(\"interception.dll\") failed, win32 error 126".into(),
        };
        let text = err.to_string();
        assert!(text.contains("not installed"), "{text}");
        assert!(text.contains("backend = \"winusb\""), "{text}");
        assert!(text.contains("ksx install-drivers"), "{text}");
        // The raw win32 detail survives: "error 126" is what makes a support
        // conversation short.
        assert!(text.contains("win32 error 126"), "{text}");
    }

    /// The table is exactly the set of entry points the backend calls. If a
    /// symbol is added to `Api` without being added here the loader would leave
    /// a field unresolved, so the two lists are pinned together.
    #[test]
    fn every_symbol_is_an_interception_export_name() {
        assert_eq!(SYMBOLS.len(), 10);
        for name in SYMBOLS {
            assert!(name.starts_with("interception_"), "{name}");
        }
        let mut sorted = SYMBOLS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SYMBOLS.len(), "duplicate symbol in the table");
    }

    /// Loading is idempotent and never panics, on a machine with the DLL or
    /// without it — `ksx devices` calls this on both.
    #[test]
    fn asking_twice_is_stable() {
        let first = api().is_ok();
        let second = api().is_ok();
        assert_eq!(first, second);
        assert_eq!(is_loaded(), first);
    }
}
