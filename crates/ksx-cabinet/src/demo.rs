//! Fixture sources: the cabinet with a scripted machine behind it.
//!
//! This exists for one reason and it is not a shortcut. A 10-foot surface is a
//! **design artefact** — the type has to be legible from six feet, the focus
//! ring has to be findable across a room, and the two columns of the button
//! check have to hold a plausible amount of live data without collapsing. None
//! of that can be judged from a screenshot of an empty panel with no daemon
//! behind it, and none of it should require starting a real session on a real
//! cabinet with real keyboards captured.
//!
//! So: `ksx cabinet --demo` renders the same window, the same theme and the
//! same navigation against a scripted cabinet. It performs **no verb** — every
//! [`ksx_api::ControlSource`] write here answers with a refusal that says it is
//! a demo — so it can never touch a real machine, a real config, or a running
//! daemon.

use std::sync::Mutex;
use std::time::Instant;

use ksx_api::{
    codes, ControlSource, KeyHit, LiveFeed, LiveFrame, MachineSource, MapperSlot, MapperSnapshot,
    PadRow, PresetRow, PresetsView, ProfileRow, Refusal, SessionView, SlotLive, SlotOutcome,
    StatusSnapshot, StatusSource,
};

/// A cabinet mid-session: four slots, a game up, everything healthy.
pub struct DemoStatus;

impl StatusSource for DemoStatus {
    fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "2026-08-06 18:40:11 UTC".into(),
            vigem: "installed — service running — driver v1.22.0".into(),
            interception: "not installed — every bound board is WinUSB-claimed".into(),
            daemon_running: true,
            daemon_detail: "ksx.exe alive (pid 4242) — claim held since 09:12".into(),
            autostart: "registered — ksx daemon --game \"Steam\"".into(),
            pads: (1..=4)
                .map(|n| PadRow {
                    persona: "Xbox 360 pad".into(),
                    instance: format!(r"USB\VID_045E&PID_028E\2&AA&0&0{n}"),
                })
                .collect(),
            profiles: vec![
                ProfileRow {
                    title: "Steam".into(),
                    detail: r"C:\Program Files\Steam\steam.exe — 4 slots".into(),
                },
                ProfileRow {
                    title: "MAME".into(),
                    detail: r"G:\emulators\mame\mame.exe — 4 slots".into(),
                },
                ProfileRow {
                    title: "Street Fighter".into(),
                    detail: r"steam://rungameid/310950 — 2 slots".into(),
                },
            ],
            config_root: r"C:\Users\arcade\AppData\Roaming\ksx".into(),
        }
    }

    fn mapper(&self) -> MapperSnapshot {
        MapperSnapshot {
            generated_at: "2026-08-06 18:40:11 UTC".into(),
            source: "slots of profile \"Steam\" (games.toml)".into(),
            // The same profile the demo session reports, so the demo does not
            // show a mismatch warning that a real cabinet would not have.
            profile: Some("Steam".into()),
            config_root: r"C:\Users\arcade\AppData\Roaming\ksx".into(),
            slots: (1..=4)
                .map(|n| MapperSlot {
                    number: n,
                    persona: "xbox360".into(),
                    persona_label: "Xbox 360".into(),
                    preset: format!("IPAC P{n}"),
                    keyboard: if n <= 2 { "IPAC 2" } else { "IPAC 4" }.into(),
                    bindings: Default::default(),
                    backup: None,
                    turbo: Default::default(),
                    macros_off: false,
                })
                .collect(),
        }
    }
}

/// A control source that answers the reads and refuses every write **in
/// words**, naming itself. Nothing here can reach a machine.
pub struct DemoControl;

impl ControlSource for DemoControl {
    fn session(&self) -> SessionView {
        SessionView {
            reachable: true,
            running: true,
            line: "running — Steam — 4 pad(s)".into(),
            profile: Some("Steam".into()),
        }
    }

    fn start(&self, _profile: Option<&str>) -> Result<String, Refusal> {
        Err(demo_refusal("start emulation"))
    }

    fn stop(&self) -> Result<String, Refusal> {
        Err(demo_refusal("stop emulation"))
    }

    fn reload(&self) -> Result<String, Refusal> {
        Err(demo_refusal("reload the config"))
    }

    fn assign_slot(&self, request: &ksx_api::SlotAssignRequest) -> SlotOutcome {
        SlotOutcome {
            ok: false,
            error: Some(format!(
                "this is `ksx cabinet --demo` — nothing was written. The real verb is \
                 `ksx slot assign --slot {} --preset \"{}\"`",
                request.slot, request.preset
            )),
            code: Some(codes::NOT_HERE.to_owned()),
            ..SlotOutcome::default()
        }
    }
}

fn demo_refusal(what: &str) -> Refusal {
    Refusal::with_remedy(
        codes::NOT_HERE,
        format!("this is `ksx cabinet --demo` — it will not {what} on a real machine"),
        "run `ksx cabinet` against a live daemon",
    )
}

pub struct DemoMachine;

impl MachineSource for DemoMachine {
    fn presets(&self) -> Result<PresetsView, Refusal> {
        Ok(PresetsView {
            config_root: r"C:\Users\arcade\AppData\Roaming\ksx".into(),
            presets: [
                "IPAC P1",
                "IPAC P2",
                "IPAC P3",
                "IPAC P4",
                "Street Fighter P1",
                "default",
            ]
            .into_iter()
            .map(|name| PresetRow {
                name: name.into(),
                bound: 14,
                macros: 2,
                protected: name == "default",
                source: format!(r"C:\…\presets\{name}.toml"),
            })
            .collect(),
            templates: Vec::new(),
        })
    }
}

/// A scripted panel: four players pressing things, on a loop.
///
/// Deliberately irregular — a real panel does not produce one press per frame
/// on the beat, and a screenshot of a metronome would not tell us whether the
/// decay reads correctly.
pub struct DemoFeed {
    started: Instant,
    last: Mutex<u64>,
}

impl Default for DemoFeed {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            last: Mutex::new(0),
        }
    }
}

/// `(key, device alias, slot, control)` — one scripted press, with both
/// columns of the truth it produces.
const SCRIPT: [(&str, &str, u8, &str); 8] = [
    ("G", "IPAC P1", 1, "A"),
    ("H", "IPAC P1", 1, "B"),
    ("R", "IPAC P2", 2, "dpad.up"),
    ("D", "IPAC P2", 2, "lt"),
    ("A", "IPAC P3", 3, "X"),
    ("W", "IPAC P3", 3, "dpad.left"),
    ("K", "IPAC P4", 4, "Y"),
    ("L", "IPAC P4", 4, "start"),
];

impl LiveFeed for DemoFeed {
    fn poll(&mut self) -> LiveFrame {
        // One step every ~230 ms: slow enough to read, fast enough that a
        // screenshot almost always catches something lit.
        let step = self.started.elapsed().as_millis() as u64 / 230;
        let mut last = self.last.lock().expect("demo feed");
        let fired: Vec<u64> = ((*last + 1)..=step).collect();
        *last = step;
        drop(last);

        let mut frame = LiveFrame {
            running: true,
            ..LiveFrame::default()
        };
        let mut slots: Vec<SlotLive> = (1..=4)
            .map(|slot| SlotLive {
                slot,
                ..SlotLive::default()
            })
            .collect();
        for tick in fired {
            let (key, alias, slot, control) = SCRIPT[(tick as usize) % SCRIPT.len()];
            frame.keys.push(KeyHit {
                key: key.into(),
                device: format!(r"HID\VID_D209&PID_0430&MI_0{slot}\8&2A0D0500&0&000{slot}"),
                alias: alias.into(),
                down: true,
            });
            if let Some(entry) = slots.iter_mut().find(|s| s.slot == slot) {
                entry.hit.push(control.to_owned());
                // Every other press is still HELD when the frame is taken, so
                // both the "down now" and the "just happened" treatments are
                // on screen at once and can be compared.
                if tick % 2 == 0 {
                    entry.down.push(control.to_owned());
                }
            }
        }
        frame.slots = slots;
        frame
    }
}

/// The whole demo cabinet.
pub fn cabinet() -> crate::Cabinet {
    crate::Cabinet {
        status: std::sync::Arc::new(DemoStatus),
        control: std::sync::Arc::new(DemoControl),
        machine: std::sync::Arc::new(DemoMachine),
        feed: Box::new(DemoFeed::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo must be incapable of touching a real machine — every write is
    /// a worded refusal, never a silent success.
    #[test]
    fn the_demo_refuses_every_write_in_words() {
        let control = DemoControl;
        for refusal in [
            control.start(None).unwrap_err(),
            control.stop().unwrap_err(),
            control.reload().unwrap_err(),
        ] {
            assert_eq!(refusal.code, codes::NOT_HERE);
            assert!(refusal.message.contains("--demo"), "{refusal}");
            assert!(refusal.remedy.is_some(), "a refusal owes a way out");
        }
        let assigned = control.assign_slot(&ksx_api::SlotAssignRequest {
            slot: 1,
            preset: "IPAC P2".into(),
            profile: None,
            persona: None,
            reload: true,
        });
        assert!(!assigned.ok);
        assert!(assigned.headline().contains("ksx slot assign"));
    }

    /// The scripted feed produces BOTH columns for one press, which is the
    /// thing the screenshot has to show.
    #[test]
    fn the_demo_feed_produces_both_columns() {
        let mut feed = DemoFeed {
            started: Instant::now() - std::time::Duration::from_millis(2_000),
            last: Mutex::new(0),
        };
        let frame = feed.poll();
        assert!(frame.running);
        assert!(!frame.keys.is_empty(), "the panel column");
        assert!(
            frame.slots.iter().any(|s| !s.hit.is_empty()),
            "the pad column"
        );
    }
}
