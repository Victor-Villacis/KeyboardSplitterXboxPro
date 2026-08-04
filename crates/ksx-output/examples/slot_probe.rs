//! Diagnostic: plug 4 ViGEm pads and watch XInput slot binding settle over time.
//! Run on the cabinet: cargo run -p ksx-output --example slot_probe
#![cfg(windows)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use vigem_client::{Client, TargetId, Xbox360Wired};

fn main() {
    let client = Arc::new(Client::connect().expect("ViGEmBus not reachable"));
    let xinput = rusty_xinput::XInputHandle::load_default().expect("xinput load");

    let mut targets: Vec<Xbox360Wired<Arc<Client>>> = Vec::new();
    for i in 0..4u8 {
        let mut t = Xbox360Wired::new(client.clone(), TargetId::XBOX360_WIRED);
        t.plugin().expect("plugin");
        t.wait_ready().expect("wait_ready");
        println!("[{:>6.2?}] pad {i} plugged + ready", Instant::now());
        targets.push(t);
    }

    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(8) {
        let indices: Vec<String> = targets
            .iter_mut()
            .map(|t| match t.get_user_index() {
                Ok(i) => i.to_string(),
                Err(e) => format!("err:{e:?}"),
            })
            .collect();
        let xstates: Vec<&str> = (0..4)
            .map(|s| {
                if xinput.get_state(s).is_ok() {
                    "conn"
                } else {
                    "-"
                }
            })
            .collect();
        println!(
            "t={:>5}ms  user_index={:?}  xinput_slots={:?}",
            t0.elapsed().as_millis(),
            indices,
            xstates
        );
        std::thread::sleep(Duration::from_millis(400));
    }
    println!("unplugging");
}
