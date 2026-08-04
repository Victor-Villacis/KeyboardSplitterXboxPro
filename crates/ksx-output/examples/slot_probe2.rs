//! Diagnostic 2: which user-index sources actually work on this driver?
//! (a) LED notifications after plug; (b) active button correlation via XInput.
//! Run on the cabinet: cargo run -p ksx-output --example slot_probe2
#![cfg(windows)]

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

fn main() {
    let client = Arc::new(Client::connect().expect("ViGEmBus not reachable"));
    let xinput = rusty_xinput::XInputHandle::load_default().expect("xinput load");

    let mut targets = Vec::new();
    let mut rxs = Vec::new();
    for i in 0..4u8 {
        let mut t = Xbox360Wired::new(client.clone(), TargetId::XBOX360_WIRED);
        t.plugin().expect("plugin");
        // Register the notification listener BEFORE wait_ready to catch the
        // LED command the XUSB stack sends on binding.
        let notif = t.request_notification().expect("request_notification");
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        notif.spawn_thread(move |_r, n| {
            let _ = tx.send((t0.elapsed(), n.large_motor, n.small_motor, n.led_number));
        });
        t.wait_ready().expect("wait_ready");
        rxs.push(rx);
        targets.push(t);
        println!("pad {i}: plugged, notification registered");
    }

    println!("\n-- collecting LED notifications for 3s --");
    std::thread::sleep(Duration::from_secs(3));
    for (i, rx) in rxs.iter().enumerate() {
        let events: Vec<_> = rx.try_iter().collect();
        println!("pad {i}: {} notification(s): {:?}", events.len(), events);
    }

    println!("\n-- active correlation: unique button per pad --");
    for (i, t) in targets.iter_mut().enumerate() {
        let pressed = XGamepad {
            buttons: XButtons(XButtons::A),
            ..Default::default()
        };
        t.update(&pressed).expect("update");
        std::thread::sleep(Duration::from_millis(30));
        let mut found = Vec::new();
        for slot in 0..4u32 {
            if let Ok(st) = xinput.get_state(slot) {
                if st.raw.Gamepad.wButtons & 0x1000 != 0 {
                    found.push(slot);
                }
            }
        }
        t.update(&XGamepad::default()).expect("update");
        std::thread::sleep(Duration::from_millis(30));
        println!("pad {i}: A-press visible on XInput slot(s) {found:?}");
    }

    println!("\n-- late LED notifications after correlation --");
    std::thread::sleep(Duration::from_millis(500));
    for (i, rx) in rxs.iter().enumerate() {
        let events: Vec<_> = rx.try_iter().collect();
        if !events.is_empty() {
            println!("pad {i}: {} more: {:?}", events.len(), events);
        }
    }
}
