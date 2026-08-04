//! M6.5 spike, step 1: do ViGEm DS4 targets dodge the 4-slot XInput cap?
//!
//! X360 targets become XUSB devices and Windows exposes exactly four of them
//! (measured in `docs/research/m2-xinput-findings.md`). DS4 targets advertise
//! Sony's VID/PID and have no XUSB IOCTLs at all, which *implies* they are
//! plain HID devices with no such cap — but nothing in this repo or its
//! research has ever tested it. That one fact decides whether 6–8 player
//! cabinets are a two-day feature (ViGEm DS4) or a multi-week HIDMaestro
//! project, so measure it before estimating anything.
//!
//! Plugs six DS4 pads, then reports what each subsystem can see:
//!   * XInput  — expected to stay at 4 slots (or 0, if DS4 is invisible to it)
//!   * HID     — the real count, via a raw enumeration of Sony 054C:05C4
//!
//! Run on the cabinet: cargo run -p ksx-output --example ds4_slot_probe
//! Nothing is claimed, no driver is installed; pads are unplugged on exit.
#![cfg(windows)]

use std::sync::Arc;
use std::time::Duration;

use vigem_client::{Client, DS4Report, DualShock4Wired, TargetId};

/// More than four on purpose: four proves nothing, six is the first count that
/// can only be explained by "no XInput cap applies here".
const PADS: usize = 6;

fn hid_ds4_count() -> usize {
    // Count present Sony DualShock 4 device nodes. Deliberately shelling out to
    // PnP rather than adding a HID dependency to a throwaway spike.
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match 'VID_054C&PID_05C4' }).Count",
        ])
        .output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

fn main() {
    let xinput = rusty_xinput::XInputHandle::load_default().expect("XInput");
    let client = Arc::new(Client::connect().expect("ViGEmBus not reachable"));

    println!("baseline: {} Sony DS4 HID node(s) present", hid_ds4_count());
    let baseline_xinput = (0..4).filter(|s| xinput.get_state(*s).is_ok()).count();
    println!("baseline: {baseline_xinput} XInput slot(s) occupied\n");

    let mut pads = Vec::new();
    for i in 0..PADS {
        let mut pad = DualShock4Wired::new(client.clone(), TargetId::DUALSHOCK4_WIRED);
        match pad.plugin().and_then(|()| pad.wait_ready()) {
            Ok(()) => {
                // A neutral report proves the submit IOCTL works at all — the
                // vendored DS4 path has never been exercised by anyone.
                match pad.update(&DS4Report::default()) {
                    Ok(()) => println!("pad {i}: plugged, ready, update OK"),
                    Err(e) => println!("pad {i}: plugged, ready, update FAILED: {e:?} (raw {e})"),
                }
                pads.push(pad);
            }
            Err(e) => println!("pad {i}: FAILED to plug: {e:?}"),
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    std::thread::sleep(Duration::from_secs(2));
    let hid = hid_ds4_count();
    let xin = (0..4).filter(|s| xinput.get_state(*s).is_ok()).count();

    println!("\n--- RESULT ---");
    println!("DS4 targets plugged : {}", pads.len());
    println!("Sony HID nodes seen : {hid}");
    println!("XInput slots taken  : {xin} (baseline {baseline_xinput})");
    println!(
        "\nVERDICT: {}",
        if pads.len() > 4 && hid >= pads.len() {
            "DS4 pads are NOT capped at 4 — >4 players is a ViGEm feature, amend E4"
        } else if pads.len() > 4 {
            "plugged >4 but Windows did not enumerate them all — inconclusive, inspect above"
        } else {
            "could not plug more than 4 — DS4 is capped like X360"
        }
    );
    println!("(pads unplug as this process exits)");
}
