//! `ksx studio` — serve the cabinet status page on localhost (feature
//! `studio`).
//!
//! Builds a [`StatusSource`] from the EXISTING collectors — `ksx-platform`'s
//! driver report (which includes the bus's current children), the autostart
//! query, the games store, and a tasklist-style process check — and hands it
//! to `ksx-studio` to serve. Every page load takes a fresh point-in-time
//! snapshot; there is no daemon IPC yet (docs/CONTROL-SURFACE.md), so this
//! command cannot see inside a running session and does not pretend to.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use ksx_platform::autostart;
use ksx_platform::{BusDriverReport, InterceptionReport, ServiceState};
use ksx_studio::{PadRow, ProfileRow, StatusSnapshot, StatusSource};

pub fn run(port: u16) -> anyhow::Result<()> {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    println!("ksx Studio: http://{bind}/  (localhost only; Ctrl+C or close the window to stop)");
    println!("Snapshots are point-in-time, re-read on each request — no live session data yet.");
    ksx_studio::serve(bind, Box::new(CollectorSource))?;
    Ok(())
}

/// The real snapshot provider: nothing cached, nothing owned — each call
/// re-runs the same read-only collectors `ksx doctor` and `ksx autostart
/// --status` use.
struct CollectorSource;

impl StatusSource for CollectorSource {
    fn snapshot(&self) -> StatusSnapshot {
        collect_snapshot()
    }
}

fn collect_snapshot() -> StatusSnapshot {
    let report = ksx_platform::collect();
    let (daemon_running, daemon_detail) = daemon_check();
    let (profiles, config_root) = load_profiles();

    StatusSnapshot {
        generated_at: now_utc(),
        vigem: bus_line(&report.vigembus),
        interception: interception_line(&report.interception),
        daemon_running,
        daemon_detail,
        autostart: autostart_line(),
        pads: report
            .virtual_pads
            .pads
            .iter()
            .map(|p| PadRow {
                persona: p.persona_guess.label().to_owned(),
                instance: p.instance_id.clone(),
            })
            .collect(),
        profiles,
        config_root,
    }
}

fn now_utc() -> String {
    let t = ksx_config::Timestamp::now_utc();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

fn service_state_label(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "service running",
        ServiceState::Stopped => "service stopped",
        ServiceState::StartPending => "service start pending",
        ServiceState::StopPending => "service stop pending",
        ServiceState::Paused => "service paused",
        ServiceState::PausePending => "service pause pending",
        ServiceState::ContinuePending => "service continue pending",
        ServiceState::NotRegistered => "service not registered with the SCM",
        ServiceState::Unknown => "service state unknown",
    }
}

fn bus_line(bus: &BusDriverReport) -> String {
    if !bus.installed {
        return "not installed".to_owned();
    }
    let state = bus
        .service
        .as_ref()
        .map_or("service state unknown", |s| service_state_label(s.state));
    match bus
        .driver_file
        .as_ref()
        .and_then(|f| f.file_version.as_deref())
    {
        Some(version) => format!("installed — {state} — driver v{version}"),
        None => format!("installed — {state} — driver version unknown"),
    }
}

fn interception_line(icpt: &InterceptionReport) -> String {
    if !icpt.installed {
        return "not installed (the M6 target state once WinUSB capture lands)".to_owned();
    }
    let filter = if icpt.keyboard.filter_active {
        "keyboard filter active"
    } else {
        "keyboard filter NOT in the class stack"
    };
    match icpt
        .keyboard
        .driver_file
        .as_ref()
        .and_then(|f| f.file_version.as_deref())
    {
        Some(version) => format!("installed — {filter} — driver v{version}"),
        None => format!("installed — {filter}"),
    }
}

/// Tasklist-style liveness check: any OTHER `ksx.exe` process. Honest about
/// its own limits — without daemon IPC it cannot tell a tray daemon from a
/// `ksx run` session, only that some ksx is alive.
fn daemon_check() -> (bool, String) {
    let self_pid = std::process::id();
    let ksx: Vec<_> = ksx_platform::process::snapshot()
        .into_iter()
        .filter(|p| p.pid != self_pid && p.name_matches("ksx.exe"))
        .collect();
    if ksx.is_empty() {
        (
            false,
            "no other ksx.exe process (process-list check — this cannot see \
             inside a session and there is no daemon IPC yet)"
                .to_owned(),
        )
    } else {
        let pids: Vec<String> = ksx.iter().map(|p| p.pid.to_string()).collect();
        (
            true,
            format!(
                "ksx.exe alive (pid {}) — daemon or session, indistinguishable \
                 without daemon IPC",
                pids.join(", ")
            ),
        )
    }
}

fn autostart_line() -> String {
    match autostart::query(autostart::DEFAULT_TASK_NAME) {
        Ok(autostart::Status::NotRegistered) => "not registered".to_owned(),
        Ok(autostart::Status::Registered(task)) => {
            let mode = task.mode().map_or("unrecognized command", |m| m.describe());
            match task.game() {
                Some(game) => format!("registered — {mode} — profile \"{game}\""),
                None => format!("registered — {mode}"),
            }
        }
        Err(err) => format!("query failed: {err}"),
    }
}

fn load_profiles() -> (Vec<ProfileRow>, String) {
    let root = match ksx_config::ConfigRoot::discover() {
        Ok(root) => root,
        Err(err) => return (Vec::new(), format!("(config root not found: {err})")),
    };
    let root_display = root.dir().display().to_string();
    let profiles = match ksx_config::Store::new(root).load_games() {
        Ok(loaded) => loaded
            .value
            .games
            .iter()
            .map(|g| ProfileRow {
                title: g.title.clone(),
                detail: match g.slots.len() {
                    1 => format!("{} — 1 slot", g.path),
                    n => format!("{} — {n} slots", g.path),
                },
            })
            .collect(),
        Err(err) => vec![ProfileRow {
            title: "(games.toml unreadable)".to_owned(),
            detail: err.to_string(),
        }],
    };
    (profiles, root_display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_line_includes_the_driver_version() {
        use ksx_platform::{DriverFileReport, ServiceInfo, StartType};
        let bus = BusDriverReport {
            installed: true,
            service: Some(ServiceInfo {
                start_type: StartType::Demand,
                image_path: None,
                display_name: None,
                state: ServiceState::Running,
            }),
            driver_file: Some(DriverFileReport {
                path: "C:\\Windows\\System32\\drivers\\ViGEmBus.sys".into(),
                file_version: Some("1.21.442.0".into()),
                file_version_string: None,
                company: None,
                description: None,
                signature: None,
            }),
        };
        assert_eq!(
            bus_line(&bus),
            "installed — service running — driver v1.21.442.0"
        );
        assert_eq!(
            bus_line(&BusDriverReport {
                installed: false,
                service: None,
                driver_file: None
            }),
            "not installed"
        );
    }

    #[test]
    fn daemon_check_is_honest_about_the_ipc_gap() {
        // Cannot assert liveness (depends on the machine), but the wording
        // must always disclose the mechanism's limit.
        let (_, detail) = daemon_check();
        assert!(detail.contains("daemon IPC"), "{detail}");
    }
}
