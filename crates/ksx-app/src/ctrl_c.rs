//! Ctrl+C via `SetConsoleCtrlHandler` + one `AtomicBool` — deliberately no
//! ctrlc crate. Returning TRUE from the handler claims the event, so the
//! process survives long enough for the explicit cleanup path (unplug pads,
//! shut the capture thread down, print a summary).
//!
//! Shared by `ksx pads` and `ksx monitor`. One process runs one command, so a
//! single process-wide latch is exactly right; `requested()` stays `true` once
//! tripped.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::TRUE;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

static REQUESTED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn handler(_ctrl_type: u32) -> windows_sys::core::BOOL {
    REQUESTED.store(true, Ordering::SeqCst);
    TRUE
}

/// Installs the handler; `false` means Ctrl+C keeps its default behavior.
pub fn install() -> bool {
    // SAFETY: `handler` is a valid PHANDLER_ROUTINE for the process's
    // lifetime (a fn item), and only touches an atomic.
    unsafe { SetConsoleCtrlHandler(Some(handler), TRUE) != 0 }
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}
