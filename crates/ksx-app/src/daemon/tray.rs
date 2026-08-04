//! The notification-area icon: a hand-rolled `Shell_NotifyIconW` on a thread
//! of its own.
//!
//! # Why not a tray crate
//!
//! `ksx-app` already links `windows-sys`, and the whole API surface needed here
//! is five functions and one message loop. A crate for this would add a
//! dependency tree (and, in most cases, an event-loop framework) to a project
//! whose entire premise is that the input path must be boring and auditable.
//! It would also blur the one property that matters below.
//!
//! # The property that matters
//!
//! This thread cannot reach the hot path. Structurally — not by convention:
//!
//! - it owns no channel to the capture thread, the engine thread or the output
//!   thread. Its only outbound edge is a `Sender<DaemonCommand>`;
//! - it never calls into `ksx-capture`, `ksx-core` or `ksx-output`;
//! - it reads state through one `Mutex<DaemonState>` it holds for the length of
//!   a `clone()`.
//!
//! That is deliberate and it is the lesson of the legacy app, which dispatched
//! every keystroke onto its WPF UI thread: a stalled UI froze every keyboard on
//! the machine until reboot. Here, a tray thread that hangs costs you a menu.
//! Emulation keeps running, and `LeftCtrl ×5` still frees the keyboards,
//! because escapes are evaluated inside the capture thread.
//!
//! # Testing
//!
//! There is nothing here to unit-test — it is a message pump around six Win32
//! calls. Everything with a decision in it (which menu items are enabled, what
//! the tooltip says, what each command does) lives in the parent module and is
//! tested there. This file's correctness is established by running it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crossbeam_channel::Sender;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadIconW, PostQuitMessage,
    RegisterClassW, SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage,
    IDI_APPLICATION, MFS_DISABLED, MFS_ENABLED, MF_BYPOSITION, MF_STRING, MSG, TPM_BOTTOMALIGN,
    TPM_RIGHTALIGN, WM_APP, WM_COMMAND, WM_DESTROY, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
    WS_OVERLAPPED,
};

use super::{DaemonCommand, DaemonState, SharedState};

/// Our private notification-icon callback message.
const WM_KSX_TRAY: u32 = WM_APP + 1;
/// Tooltip-refresh timer.
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 500;
/// Menu command ids — index into [`DaemonState::menu`] plus this base.
const MENU_BASE: usize = 0x100;

/// Process-wide, because a `WNDPROC` is a bare `extern "system" fn` with no
/// user pointer we set up before the first message arrives. There is exactly
/// one tray thread per process, so a `OnceLock` is the honest representation —
/// not a global standing in for per-instance state.
static TRAY: OnceLock<TrayContext> = OnceLock::new();

struct TrayContext {
    commands: Sender<DaemonCommand>,
    state: SharedState,
}

// SAFETY: both fields are already Send+Sync (`Sender` is Send+Sync for a Send
// message type; `SharedState` is an Arc<Mutex<_>>). The wrapper adds nothing.
unsafe impl Send for TrayContext {}
unsafe impl Sync for TrayContext {}

/// A live tray icon: created, on screen, not yet pumping messages.
///
/// The split between [`create`] and [`Tray::pump`] exists for one caller and one
/// reason: `ksx daemon` releases its console window (`crate::console`), and it
/// may only do that once the icon is definitely on screen. Detaching first and
/// discovering afterwards that the tray could not be created (Session 0, no
/// shell, a locked-down desktop) would leave a daemon with no icon, no console
/// and no stdin — unreachable by any means except `taskkill`.
pub struct Tray {
    hwnd: HWND,
    icon: NOTIFYICONDATAW,
}

// SAFETY: the tray is created and pumped on the same thread — `create` and
// `pump` are called back to back by `daemon::run`, which never moves it. The
// marker exists only so the value can be held across that (non-Send) sequence
// without the caller needing to prove it; the fields are raw Win32 handles.
unsafe impl Send for Tray {}

/// Register the class, make the hidden window, and put the icon on screen.
///
/// Returns `None` — after logging why — if any of that fails. Nothing is
/// pumped yet, so no menu works and no tooltip updates until [`Tray::pump`].
pub fn create(commands: Sender<DaemonCommand>, state: SharedState) -> Option<Tray> {
    if TRAY.set(TrayContext { commands, state }).is_err() {
        tracing::error!("a tray is already running in this process");
        return None;
    }

    let class_name: Vec<u16> = "ksx_tray\0".encode_utf16().collect();
    let window_name: Vec<u16> = "ksx\0".encode_utf16().collect();

    // SAFETY: a zeroed WNDCLASSW with the two fields Windows requires
    // (lpfnWndProc, lpszClassName) filled in; the name outlives the call.
    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        lpszClassName: class_name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `class` is fully initialised above.
    if unsafe { RegisterClassW(&class) } == 0 {
        tracing::error!("RegisterClassW failed; falling back to headless");
        return None;
    }

    // A message-only-ish hidden window: never shown, exists solely to receive
    // the icon's callback messages.
    // SAFETY: both strings are NUL-terminated; every other argument is the
    // documented "no parent, no menu, no creation parameter" form.
    let hwnd: HWND = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        tracing::error!("CreateWindowExW failed; falling back to headless");
        return None;
    }

    let mut icon = notify_data(hwnd);
    icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    icon.uCallbackMessage = WM_KSX_TRAY;
    // SAFETY: IDI_APPLICATION is a system resource id; a null instance is the
    // documented way to load it.
    icon.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    write_tip(&mut icon, &tooltip());
    // SAFETY: `icon` is a fully initialised NOTIFYICONDATAW for `hwnd`.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &icon) } == 0 {
        tracing::error!("Shell_NotifyIconW(NIM_ADD) failed; falling back to headless");
        // SAFETY: `hwnd` is ours and has not been destroyed yet.
        unsafe { DestroyWindow(hwnd) };
        return None;
    }

    // The icon is on screen from here on: this is the earliest moment the
    // caller may safely give up its console.
    Some(Tray { hwnd, icon })
}

impl Tray {
    /// Pump messages until `WM_QUIT`, then remove the icon and the window.
    ///
    /// Consumes the tray, because after this returns there is no icon left.
    pub fn pump(self) {
        let hwnd = self.hwnd;
        // SAFETY: `hwnd` is a live window we own.
        unsafe { SetTimer(hwnd, TIMER_ID, TIMER_MS, None) };

        // SAFETY: standard message pump; `msg` is zeroed and re-filled per
        // iteration. GetMessageW returns 0 on WM_QUIT and -1 on error.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if got <= 0 {
                break;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // SAFETY: the same icon identity we added, and a window we own.
        unsafe {
            KillTimer(hwnd, TIMER_ID);
            Shell_NotifyIconW(NIM_DELETE, &self.icon);
            DestroyWindow(hwnd);
        }
    }
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is a plain C struct; zero is a valid starting
    // state and every field we rely on is set explicitly.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data
}

fn write_tip(data: &mut NOTIFYICONDATAW, text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let n = units.len().min(data.szTip.len() - 1);
    data.szTip[..n].copy_from_slice(&units[..n]);
    data.szTip[n] = 0;
    for slot in data.szTip.iter_mut().skip(n + 1) {
        *slot = 0;
    }
}

fn snapshot() -> DaemonState {
    TRAY.get()
        .and_then(|tray| tray.state.lock().ok().map(|s| s.clone()))
        .unwrap_or_default()
}

fn tooltip() -> String {
    snapshot().tooltip()
}

fn send(command: DaemonCommand) {
    if let Some(tray) = TRAY.get() {
        if tray.commands.send(command).is_err() {
            // The control loop is gone; nothing left to drive.
            // SAFETY: PostQuitMessage takes a scalar and cannot fail.
            unsafe { PostQuitMessage(0) };
        }
    }
}

/// One-shot latch so the menu is never opened re-entrantly by a second
/// right-click delivered while `TrackPopupMenu` is still running.
static MENU_OPEN: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_KSX_TRAY => {
            // The low word of lParam is the mouse message.
            if (lparam as u32) & 0xFFFF == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            0
        }
        WM_TIMER => {
            let mut icon = notify_data(hwnd);
            icon.uFlags = NIF_TIP;
            write_tip(&mut icon, &tooltip());
            // SAFETY: modifying the icon we added, with the same hWnd/uID.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &icon) };
            if matches!(snapshot().run, super::RunState::Quitting) {
                // SAFETY: scalar-only call.
                unsafe { PostQuitMessage(0) };
            }
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            if id >= MENU_BASE {
                let items = snapshot().menu();
                if let Some((command, _, enabled)) = items.get(id - MENU_BASE) {
                    if *enabled {
                        send(*command);
                        if *command == DaemonCommand::Quit {
                            // SAFETY: scalar-only call.
                            unsafe { PostQuitMessage(0) };
                        }
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: scalar-only call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: forwarding untouched arguments to the default handler.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn show_menu(hwnd: HWND) {
    if MENU_OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let items = snapshot().menu();
    // SAFETY: CreatePopupMenu takes nothing; the handle is destroyed below on
    // every path.
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        MENU_OPEN.store(false, Ordering::SeqCst);
        return;
    }
    for (index, (_, label, enabled)) in items.iter().enumerate() {
        let text: Vec<u16> = label
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>();
        let flags = MF_STRING | if *enabled { MFS_ENABLED } else { MFS_DISABLED };
        // SAFETY: `menu` is live, `text` is NUL-terminated and outlives the call
        // (AppendMenuW copies the string).
        unsafe { AppendMenuW(menu, flags, MENU_BASE + index, text.as_ptr()) };
    }

    // SAFETY: `point` is written by GetCursorPos before it is read.
    let mut point = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut point) };
    // Documented shell requirement: foreground the owner window first, or the
    // menu never dismisses when the user clicks away from it.
    // SAFETY: `hwnd` is our live window.
    unsafe {
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu,
            TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        DestroyMenu(menu);
    }
    let _ = MF_BYPOSITION; // documented alternative form; not used here
    MENU_OPEN.store(false, Ordering::SeqCst);
}
