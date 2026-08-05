//! `RawInputIdentify` — the "press a key on Player 1's panel" picker primitive,
//! and the M7 learn-key observer built on the same sink.
//!
//! A message-only window registers for keyboard Raw Input with
//! `RIDEV_INPUTSINK` (input reaches us even without focus) and reports which
//! device produced the next key *press* within a timeout. Identify-only by
//! design: Raw Input cannot block system-wide, and the RawInput+`WH_KEYBOARD_LL`
//! correlation hack is explicitly rejected (research §3 — its ambiguity case is
//! exactly two identical I-PACs mashed simultaneously).
//!
//! Two entry points share the sink:
//!
//! - [`wait_for_keypress`] — the original picker: first press on any keyboard,
//!   reported as instance path + vkey. Semantics unchanged since M4.
//! - [`observe_next_key`] — the learn-key observer (`ksx map` / Studio's
//!   mapper): first press on any keyboard, reported as instance path + the
//!   **logical [`Key`]** presets are written against (scancode + E0/E1 through
//!   [`crate::keymap::corrected_key`] — the same correction the capture path
//!   applies, so the learned name is exactly what a preset binds). Adds the
//!   two behaviours a binding recorder needs (constants and edge cases from
//!   PadForge's recorder, docs/research/padforge-code-audit.md §1.2):
//!   **wait-for-release re-baselining** — keys already held when the observe
//!   starts (previous learn's finger still down, autorepeat) are ignored until
//!   seen released — and **cancellation** (a flag polled every
//!   [`OBSERVE_SLICE`], PadForge's 33 ms recorder tick).
//!
//! Injected input (null `hDevice` — e.g. the daemon's own typethrough
//! SendInput) is always ignored: a learned binding must come from a real
//! device. Which is also the honest limit: a WinUSB-*claimed* interface is not
//! in the keyboard stack, so Raw Input never sees it — learn-key can only hear
//! devices that are still keyboards to Windows (the Interception backend's
//! between-session state).
//!
//! # Identity-namespace note (why the caller does the correlation)
//!
//! Raw Input yields the **device interface path** (e.g.
//! `\\?\HID#VID_D209&PID_0430&MI_00#8&2a0d0500&0&0000#{884b96c3-…}`), which is
//! port-topology-stable. Interception can only report the **hardware id**
//! (`HID\VID_D209&PID_0430&REV_0056&MI_00`) — it has no instance-path API, and
//! identical boards share a hardware id. The two namespaces cannot be joined
//! from inside either API, so this module returns everything it knows
//! (instance path + key) and leaves the HWID↔path correlation to the caller
//! (M4 does it by timing correlation while observing both streams).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetLastError, HWND, LPARAM, LRESULT, WAIT_FAILED, WAIT_TIMEOUT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, HRAWINPUT, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE, RIDI_DEVICENAME, RID_INPUT,
    RIM_TYPEKEYBOARD,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MsgWaitForMultipleObjects,
    PeekMessageW, RegisterClassExW, UnregisterClassW, HWND_MESSAGE, MSG, PM_REMOVE, QS_ALLINPUT,
    RI_KEY_BREAK, RI_KEY_E0, RI_KEY_E1, WM_INPUT, WNDCLASSEXW,
};

use crate::backend::CaptureError;
use crate::guard::ResetGuard;
use crate::keymap;
use ksx_core::Key;

/// HID usage page/usage for generic desktop keyboards.
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_GENERIC_KEYBOARD: u16 = 0x06;

/// The observer's wake slice while a cancel flag is in play: PadForge's
/// recorder polls at 33 ms (~30 Hz, `RecorderService.PollIntervalMs` —
/// docs/research/padforge-code-audit.md §1.2); a cancel is honoured within
/// one slice.
const OBSERVE_SLICE: Duration = Duration::from_millis(33);

/// What the next key press within the timeout looked like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentifiedPress {
    /// PnP device **instance path** (e.g.
    /// `HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000`), normalized from the
    /// Raw Input interface path to the exact uppercase-backslash form the
    /// ksx-core `DeviceId` convention uses. Stable per physical port. NOT
    /// comparable to an Interception hardware id — see the module docs.
    pub instance_path: String,
    /// The virtual-key code of the pressed key (useful for "press START"
    /// prompts that want to validate which key was hit).
    pub vkey: u16,
}

/// What [`observe_next_key`] heard: the press mapped into the preset key
/// vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedKey {
    /// Same normalized instance path as [`IdentifiedPress::instance_path`].
    pub instance_path: String,
    /// The logical key, via the same scancode+E0/E1 correction the capture
    /// path applies — `key.name()` is a spelling `Key::from_name` accepts,
    /// i.e. exactly what a preset file stores.
    pub key: Key,
    /// The raw virtual-key code, for diagnostics.
    pub vkey: u16,
}

/// Normalize a Raw Input device *interface* path to the PnP *instance* path:
/// strip the `\\?\` (or `\\.\`) prefix, drop the trailing
/// `#{interface-class-guid}` segment, turn `#` separators back into `\`, and
/// uppercase (`CM_Get_Device_ID` returns uppercase; ksx-core `DeviceId`
/// comparison is exact).
fn interface_path_to_instance_path(interface: &str) -> String {
    let s = interface
        .strip_prefix(r"\\?\")
        .or_else(|| interface.strip_prefix(r"\\.\"))
        .unwrap_or(interface);
    let s = match s.rfind("#{") {
        Some(i) => &s[..i],
        None => s,
    };
    s.replace('#', "\\").to_ascii_uppercase()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Block up to `timeout` waiting for a key press on any keyboard; report which
/// device produced it. Returns `Ok(None)` on timeout.
///
/// Purely observational: registers an input *sink*, never suppresses anything,
/// and unregisters + destroys its window on every exit path (guard-based).
/// Original M4 picker semantics: no re-baseline (a held key's autorepeat
/// counts), no cancellation.
pub fn wait_for_keypress(timeout: Duration) -> Result<Option<IdentifiedPress>, CaptureError> {
    observe(timeout, None, false).map(|hit| {
        hit.map(|press| IdentifiedPress {
            instance_path: press.instance_path,
            vkey: press.vkey,
        })
    })
}

/// Block up to `timeout` for the next key press, mapped into the preset key
/// vocabulary; `Ok(None)` on timeout *or* cancellation (the caller holds the
/// flag and knows which).
///
/// Recorder semantics on top of [`wait_for_keypress`]:
/// - **wait-for-release re-baseline** — every key reported down by
///   `GetAsyncKeyState` when the observe starts is ignored (autorepeat
///   included) until a break for it is seen;
/// - **cancellable** — `cancel` is polled every [`OBSERVE_SLICE`];
/// - presses that map to [`Key::Unknown`] (a scancode outside the preset
///   vocabulary) are skipped, not reported — a learner must never hand back a
///   name a preset cannot store.
pub fn observe_next_key(
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Option<ObservedKey>, CaptureError> {
    observe(timeout, Some(cancel), true).map(|hit| {
        hit.map(|press| ObservedKey {
            instance_path: press.instance_path,
            key: press.key,
            vkey: press.vkey,
        })
    })
}

/// One decoded WM_INPUT keyboard packet.
enum SinkEvent {
    /// A make from a real device, fully resolved.
    Press(RawPress),
    /// A break (any device) — only the vkey matters (re-baseline bookkeeping).
    Release(u16),
    /// Injected/unreadable/non-keyboard — nothing to do.
    Ignored,
}

struct RawPress {
    instance_path: String,
    key: Key,
    vkey: u16,
}

/// The shared sink loop behind both public entry points.
fn observe(
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    rebaseline: bool,
) -> Result<Option<RawPress>, CaptureError> {
    let class_name = wide("ksx-rawinput-identify");
    // SAFETY: null module name = handle of the current module.
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    // If a previous call in this process already registered the class, this
    // fails benignly — CreateWindowExW below is the real gate.
    // SAFETY: fully-initialized WNDCLASSEXW that outlives the call.
    let _ = unsafe { RegisterClassExW(&wc) };
    let class_name_for_guard = class_name.clone();
    let _class_guard = ResetGuard::new(move || {
        // SAFETY: unregistering our own class after all windows are gone (the
        // window guard drops first — declared later, dropped earlier).
        unsafe { UnregisterClassW(class_name_for_guard.as_ptr(), hinstance) };
    });

    // SAFETY: registered class; HWND_MESSAGE parent = message-only window.
    let hwnd: HWND = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(CaptureError::Os {
            context: "CreateWindowExW(message-only)",
            // SAFETY: immediately after the failing call.
            code: unsafe { GetLastError() },
        });
    }
    let _window_guard = ResetGuard::new(move || {
        // SAFETY: our own window, destroyed exactly once.
        unsafe { DestroyWindow(hwnd) };
    });

    let rid = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_KEYBOARD,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    // SAFETY: one fully-initialized RAWINPUTDEVICE with the documented size.
    let registered =
        unsafe { RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
    if registered == 0 {
        return Err(CaptureError::Os {
            context: "RegisterRawInputDevices(keyboard, INPUTSINK)",
            // SAFETY: immediately after the failing call.
            code: unsafe { GetLastError() },
        });
    }
    let _unregister_guard = ResetGuard::new(|| {
        let rid_remove = RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: HID_USAGE_GENERIC_KEYBOARD,
            dwFlags: RIDEV_REMOVE,
            hwndTarget: std::ptr::null_mut(), // REMOVE requires a null target
        };
        // SAFETY: symmetric removal of the registration above.
        unsafe {
            RegisterRawInputDevices(&rid_remove, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
        };
    });

    // Wait-for-release re-baseline: snapshot what is held RIGHT NOW; those
    // vkeys stay invisible until a break is seen for them. PadForge re-arms
    // chained recordings the same way — only after all devices are neutral.
    let mut held = [false; 256];
    if rebaseline {
        for vk in 1..256 {
            // SAFETY: plain state query, no pointers.
            if (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0 {
                held[vk as usize] = true;
            }
        }
    }

    let deadline = Instant::now() + timeout;
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        // With a cancel flag in play, wake at least every OBSERVE_SLICE so
        // cancellation is honoured promptly; without one, sleep the remainder.
        let wait = if cancel.is_some() {
            remaining.min(OBSERVE_SLICE)
        } else {
            remaining
        };
        let wait_ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: zero handles is valid — wakes on message arrival only.
        let wake =
            unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait_ms, QS_ALLINPUT) };
        if wake == WAIT_FAILED {
            return Err(CaptureError::Os {
                context: "MsgWaitForMultipleObjects",
                // SAFETY: immediately after the failing call.
                code: unsafe { GetLastError() },
            });
        }
        if wake == WAIT_TIMEOUT {
            continue; // slice elapsed — loop re-checks cancel and deadline
        }

        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: valid MSG out-pointer; null hwnd = any window of this thread.
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            let event = if msg.message == WM_INPUT {
                read_key_event(msg.lParam as HRAWINPUT)
            } else {
                SinkEvent::Ignored
            };
            // Always dispatch — DefWindowProc performs the system's WM_INPUT
            // cleanup, including for events we ignored.
            // SAFETY: msg came from PeekMessageW just above.
            unsafe { DispatchMessageW(&msg) };
            match event {
                SinkEvent::Press(press) => {
                    if rebaseline && held[usize::from(press.vkey) & 0xFF] {
                        continue; // still held from before the observe started
                    }
                    if rebaseline && press.key == Key::Unknown {
                        continue; // outside the preset vocabulary — keep listening
                    }
                    return Ok(Some(press));
                }
                SinkEvent::Release(vkey) => {
                    held[usize::from(vkey) & 0xFF] = false;
                }
                SinkEvent::Ignored => {}
            }
        }
    }
    // Guards run on every return path: raw-input unregistered, window
    // destroyed, class unregistered — in that order (reverse declaration).
}

/// Decode one WM_INPUT message. Releases are reported (vkey only) so the
/// re-baseline can clear held keys; presses from injected input (null
/// `hDevice`) or non-keyboard packets are `Ignored`.
fn read_key_event(handle: HRAWINPUT) -> SinkEvent {
    let mut raw: RAWINPUT = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    // SAFETY: keyboard RAWINPUT always fits the fixed-size struct (only RAWHID
    // packets are variable-size).
    let copied = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            (&mut raw as *mut RAWINPUT).cast(),
            &mut size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if copied == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD {
        return SinkEvent::Ignored;
    }
    // SAFETY: dwType checked — the union holds the keyboard variant.
    let kb = unsafe { raw.data.keyboard };
    if u32::from(kb.Flags) & RI_KEY_BREAK != 0 {
        return SinkEvent::Release(kb.VKey);
    }
    if raw.header.hDevice.is_null() {
        return SinkEvent::Ignored; // injected input has no source device
    }

    // Two-call pattern for the device interface path.
    let mut chars = 0u32;
    // SAFETY: size query with null buffer per the API contract.
    unsafe {
        GetRawInputDeviceInfoW(
            raw.header.hDevice,
            RIDI_DEVICENAME,
            std::ptr::null_mut(),
            &mut chars,
        )
    };
    if chars == 0 {
        return SinkEvent::Ignored;
    }
    let mut buf = vec![0u16; chars as usize];
    // SAFETY: buffer sized by the query above.
    let written = unsafe {
        GetRawInputDeviceInfoW(
            raw.header.hDevice,
            RIDI_DEVICENAME,
            buf.as_mut_ptr().cast(),
            &mut chars,
        )
    };
    if written == u32::MAX || written == 0 {
        return SinkEvent::Ignored;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(written as usize);
    let interface_path = String::from_utf16_lossy(&buf[..end]);

    // Raw Input's E0/E1 flag bits are numerically the Interception state bits
    // (RI_KEY_E0 = 0x02 = KEY_E0, RI_KEY_E1 = 0x04 = KEY_E1), so the make
    // code + extended bits feed the same correction table the capture path
    // uses — one vocabulary, one mapping.
    let state = kb.Flags & ((RI_KEY_E0 | RI_KEY_E1) as u16);
    SinkEvent::Press(RawPress {
        instance_path: interface_path_to_instance_path(&interface_path),
        key: keymap::corrected_key(kb.MakeCode, state),
        vkey: kb.VKey,
    })
}

/// Everything is handled in the pump; the wndproc just defaults.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_path_normalizes_to_instance_path() {
        assert_eq!(
            interface_path_to_instance_path(
                r"\\?\HID#VID_D209&PID_0430&MI_00#8&2a0d0500&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"
            ),
            r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000"
        );
        // \\.\ prefix and no GUID segment are tolerated.
        assert_eq!(
            interface_path_to_instance_path(r"\\.\ACPI#PNP0303#4&1a2b3c4d&0"),
            r"ACPI\PNP0303\4&1A2B3C4D&0"
        );
        // Already-normal input passes through (uppercased).
        assert_eq!(
            interface_path_to_instance_path(r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000"),
            r"HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000"
        );
    }

    /// Live but harmless: registers an input sink, waits 50 ms with (almost
    /// certainly) no keypress, and must time out cleanly with all cleanup
    /// guards run. Never blocks or suppresses anything.
    #[test]
    fn times_out_cleanly_with_no_keypress() {
        let result = wait_for_keypress(Duration::from_millis(50));
        match result {
            Ok(None) => {}
            // If the developer happens to be typing during the 50 ms window,
            // a press is a legitimate outcome.
            Ok(Some(press)) => assert!(!press.instance_path.is_empty()),
            Err(e) => panic!("identify failed: {e}"),
        }
    }

    /// The observer honours its cancel flag within a slice — a pre-set flag
    /// returns immediately, well inside the 10 s learn timeout.
    #[test]
    fn observe_returns_promptly_when_cancelled() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let result = observe_next_key(Duration::from_secs(10), &cancel).expect("observe");
        assert_eq!(result, None);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a cancelled observe must not sit out its timeout"
        );
    }

    /// Raw Input's extended-key bits must remain numerically identical to the
    /// Interception state bits — `corrected_key` is shared on that basis.
    #[test]
    fn rawinput_flag_bits_match_the_interception_state_bits() {
        assert_eq!(RI_KEY_E0, u32::from(keymap::KEY_E0));
        assert_eq!(RI_KEY_E1, u32::from(keymap::KEY_E1));
        assert_eq!(RI_KEY_BREAK, u32::from(keymap::KEY_UP));
    }
}
