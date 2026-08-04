//! M6.5 spike, step 2: why does `IOCTL_DS4_SUBMIT_REPORT` return 259, and what
//! makes it stop?
//!
//! `ds4_slot_probe` proved DS4 targets dodge the XInput 4-slot cap but could not
//! drive them: every submit failed with `ERROR_NO_MORE_ITEMS` (259). Reading
//! ViGEmBus' kernel source explains it — `EmulationTargetDS4::SubmitReportImpl`
//! (sys/Ds4Pdo.cpp) *starts* by dequeuing a pending interrupt-IN request and
//! bails out with that queue's status if there is none:
//!
//! ```text
//! status = WdfIoQueueRetrieveNextRequest(this->_PendingUsbInRequests, &usbRequest);
//! if (!NT_SUCCESS(status)) return status;   // STATUS_NO_MORE_ENTRIES -> 259
//! ```
//!
//! Requests only land in that queue once the HID stack above the PDO starts
//! polling, which it does not do by the time `IOCTL_WAIT_DEVICE_READY` returns.
//! So 259 is not a marshalling bug and not a permissions problem: it is a
//! **startup race**, and the spike lost it by submitting exactly once.
//!
//! Measured here on ViGEmBus 1.21.442.0 (unfixed crate, one pad):
//!
//! ```text
//! [plug] plugin 310.9µs, wait_ready returned at 1.7544ms
//! [0   ] first report ACCEPTED at 4.3417ms (2 refusals with 259 before it)
//! [A   ] no reader open : 50 ok, 0 x 259   <- steady state never refuses
//! [C   ] 200 unpaced submits: 0 refused
//! ```
//!
//! `DualShock4Wired::wait_ready` now absorbs that window, so phase 0 should show
//! zero refusals. The phases:
//!   0. time from plug to the first accepted report
//!   A. submit with no reader open       -> the queue is fed by hidclass anyway
//!   B/C. paced and unpaced submits with a reader open
//!   then press/release verified byte-for-byte on the wire
//!
//! Run on the cabinet: cargo run -p ksx-output --example ds4_report_probe
//! Opens one HID handle for reading (what joy.cpl does); claims nothing, binds
//! nothing, installs nothing. The pad is unplugged on exit.
#![cfg(windows)]

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vigem_client::{Client, DS4Report, DualShock4Wired, TargetId};
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// GUID_DEVINTERFACE_HID. Hardcoded rather than pulling in hid.dll for one call.
const GUID_DEVINTERFACE_HID: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// A wired DS4 sends 64-byte input reports, report id 0x01 in byte 0
/// (`DS4_REPORT_SIZE = 0x40`, sys/Ds4Pdo.hpp:140).
const DS4_REPORT_SIZE: usize = 64;

/// How many submits each phase attempts. Enough to see a rate, not a single roll
/// of the dice: the driver flushes its pending-IN queue every 5 ms
/// (`DS4_QUEUE_FLUSH_PERIOD`), so a lone success or failure proves nothing.
const ATTEMPTS: usize = 50;

/// Enumerate the device paths of every present HID interface belonging to a
/// Sony 054C:05C4. Read-only: SetupAPI enumeration opens no device.
fn ds4_hid_paths() -> Vec<String> {
    let mut paths = Vec::new();
    unsafe {
        let set = SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_HID,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        // HDEVINFO is an isize in windows-sys; INVALID_HANDLE_VALUE is -1.
        if set == -1 {
            return paths;
        }

        let mut index = 0u32;
        let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

        // Big enough for any device interface path.
        let mut buffer = [0u8; 1024];
        while SetupDiEnumDeviceInterfaces(
            set,
            ptr::null(),
            &GUID_DEVINTERFACE_HID,
            index,
            &mut iface,
        ) != 0
        {
            index += 1;

            let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            // cbSize describes the *header*, not the buffer (SetupAPI quirk).
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

            let mut required = 0u32;
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                &iface,
                detail,
                buffer.len() as u32,
                &mut required,
                ptr::null_mut(),
            ) == 0
            {
                continue;
            }

            let wide = ptr::addr_of!((*detail).DevicePath) as *const u16;
            let mut len = 0usize;
            while *wide.add(len) != 0 {
                len += 1;
            }
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            if path.to_ascii_lowercase().contains("vid_054c&pid_05c4") {
                paths.push(path);
            }
        }

        SetupDiDestroyDeviceInfoList(set);
    }
    paths
}

/// Owned HID read handle. `HANDLE` is a raw pointer, so it needs a Send wrapper
/// to be moved into the reader thread.
struct HidHandle(HANDLE);
unsafe impl Send for HidHandle {}
impl Drop for HidHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn open_hid(path: &str) -> Result<HidHandle, u32> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return Err(GetLastError());
        }
        Ok(HidHandle(h))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The report the probe drives the pad with: stick hard right, CROSS held,
/// D-pad neutral (0x8 is "released" for a DS4 hat, not 0x0).
fn pressed_report() -> DS4Report {
    DS4Report {
        thumb_lx: 0xFF,
        buttons: 0x0028, // DS4_BUTTON_CROSS (0x20) | DS4_BUTTON_DPAD_NONE (0x8)
        ..Default::default()
    }
}

/// Submit `ATTEMPTS` copies of `report`, returning (ok, err259, other).
fn submit_burst(
    pad: &mut DualShock4Wired<Arc<Client>>,
    report: &DS4Report,
) -> (usize, usize, Vec<String>) {
    let (mut ok, mut busy, mut other) = (0, 0, Vec::new());
    for _ in 0..ATTEMPTS {
        match pad.update(report) {
            Ok(()) => ok += 1,
            Err(vigem_client::Error::WinError(259)) => busy += 1,
            Err(e) => other.push(format!("{e:?}")),
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    (ok, busy, other)
}

fn main() {
    println!("== ksx DS4 report probe ==\n");

    let client = Arc::new(Client::connect().expect("ViGEmBus not reachable"));
    let before = ds4_hid_paths();
    println!(
        "[hid ] {} DS4 HID interface(s) present before plug",
        before.len()
    );

    let mut pad = DualShock4Wired::new(client.clone(), TargetId::DUALSHOCK4_WIRED);
    let t_plug = Instant::now();
    pad.plugin().expect("plugin");
    let t_plugged = t_plug.elapsed();
    pad.wait_ready().expect("wait_ready");
    let t_ready = t_plug.elapsed();
    println!("[plug] plugin {t_plugged:?}, wait_ready returned at {t_ready:?}");

    // --- Phase 0: how long after `wait_ready` before a report is accepted? ----
    // This is the spike's exact sequence (plugin -> wait_ready -> update), which
    // failed with 259 on all six pads. Measure the window instead of guessing.
    // With the fix in place `wait_ready` has already paid for it, so this should
    // report the first attempt accepted and zero refusals.
    let mut refused = 0usize;
    let mut first_ok = None;
    while t_plug.elapsed() < Duration::from_secs(10) {
        match pad.update(&DS4Report::default()) {
            Ok(()) => {
                first_ok = Some(t_plug.elapsed());
                break;
            }
            Err(vigem_client::Error::WinError(259)) => refused += 1,
            Err(e) => {
                println!("[0   ] unexpected error: {e:?}");
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    match first_ok {
        Some(t) => println!(
            "[0   ] first report ACCEPTED at {t:?} ({refused} refusals with 259 before it)"
        ),
        None => println!("[0   ] no report accepted within 10s ({refused} refusals)"),
    }

    // Enumeration is asynchronous; give the HID stack a moment to build the node.
    let mut fresh = Vec::new();
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(100));
        fresh = ds4_hid_paths()
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect::<Vec<_>>();
        if !fresh.is_empty() {
            break;
        }
    }
    if fresh.is_empty() {
        println!("[hid ] no new DS4 HID interface appeared — cannot verify, aborting");
        return;
    }
    for p in &fresh {
        println!("[hid ] new interface: {p}");
    }

    // --- Phase A: submit with nothing reading the pad -------------------------
    let (ok_a, busy_a, other_a) = submit_burst(&mut pad, &pressed_report());
    println!(
        "\n[A   ] no reader open : {ok_a} ok, {busy_a} x 259, {} other",
        other_a.len()
    );
    for e in &other_a {
        println!("[A   ]   unexpected: {e}");
    }

    // --- Phase B: hold a read open, then submit the same thing ---------------
    let path = fresh[0].clone();
    let handle = match open_hid(&path) {
        Ok(h) => h,
        Err(e) => {
            println!("[open] FAILED to open {path}: win error {e}");
            return;
        }
    };
    println!("\n[open] holding a read handle on the pad's HID interface");

    let latest = Arc::new(Mutex::new([0u8; DS4_REPORT_SIZE]));
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let latest = latest.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let handle = handle; // moved in; closed when this thread ends
            let mut buf = [0u8; DS4_REPORT_SIZE];
            let mut reads = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let mut read = 0u32;
                let ok = unsafe {
                    ReadFile(
                        handle.0,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut read,
                        ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    break;
                }
                reads += 1;
                *latest.lock().unwrap() = buf;
            }
            reads
        })
    };

    // Let the reader prime the queue, then capture the idle report.
    std::thread::sleep(Duration::from_millis(300));
    let idle = *latest.lock().unwrap();
    println!("[read] idle report   : {}", hex(&idle[..11]));

    let (ok_b, busy_b, other_b) = submit_burst(&mut pad, &pressed_report());
    println!(
        "[B   ] reader open   : {ok_b} ok, {busy_b} x 259, {} other",
        other_b.len()
    );
    for e in &other_b {
        println!("[B   ]   unexpected: {e}");
    }

    // --- Phase C: back-to-back submits, no pacing ----------------------------
    // A daemon can emit two state changes inside one millisecond. If the driver
    // refuses submits faster than the host polls, `update` must not drop them.
    let mut burst_refused = 0usize;
    for i in 0..200u32 {
        let mut r = pressed_report();
        r.thumb_ly = (i % 200) as u8;
        if let Err(vigem_client::Error::WinError(259)) = pad.update(&r) {
            burst_refused += 1;
        }
    }
    println!("[C   ] 200 unpaced submits: {burst_refused} refused with 259");

    std::thread::sleep(Duration::from_millis(100));
    let _ = pad.update(&pressed_report());
    std::thread::sleep(Duration::from_millis(100));
    let pressed = *latest.lock().unwrap();
    println!("[read] after submit  : {}", hex(&pressed[..11]));

    // Release, so a stuck-button false positive is impossible.
    let _ = pad.update(&DS4Report::default());
    std::thread::sleep(Duration::from_millis(100));
    let released = *latest.lock().unwrap();
    println!("[read] after release : {}", hex(&released[..11]));

    stop.store(true, Ordering::Relaxed);

    // Wire layout: [0] report id, [1..5] LX LY RX RY, [5..7] buttons (LE).
    let lx = pressed[1];
    let buttons = u16::from_le_bytes([pressed[5], pressed[6]]);
    let lx_rel = released[1];
    let buttons_rel = u16::from_le_bytes([released[5], released[6]]);

    println!("\n--- RESULT ---");
    println!("phase A (no reader) : {ok_a}/{ATTEMPTS} submits accepted");
    println!("phase B (reader)    : {ok_b}/{ATTEMPTS} submits accepted");
    println!("pressed  on the wire: thumb_lx {lx:#04x} (want 0xff), buttons {buttons:#06x} (want 0x0028)");
    println!("released on the wire: thumb_lx {lx_rel:#04x} (want 0x80), buttons {buttons_rel:#06x} (want 0x0008)");

    let delivered = lx == 0xFF && buttons == 0x0028 && lx_rel == 0x80 && buttons_rel == 0x0008;
    println!(
        "\nVERDICT: {}",
        if delivered && ok_b > 0 && ok_a == 0 {
            "confirmed — 259 means 'no reader'; with a reader open the report reaches the wire"
        } else if delivered {
            "reports reach the wire, but the phase A/B split is not what the kernel source predicts"
        } else {
            "reports did NOT reach the wire — inspect the bytes above"
        }
    );

    drop(pad); // unplug before the reader's handle closes
    let _ = reader.join();
    println!("(pad unplugged)");
}
