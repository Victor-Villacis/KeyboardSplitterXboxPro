//! ksx — split keyboards (I-PAC arcade encoders) into virtual Xbox 360 controllers.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ksx", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start emulation (foreground; later also tray-daemon mode)
    Run {
        /// Launch a configured game/profile and stop emulation when it exits
        #[arg(long)]
        game: Option<String>,
    },
    /// List input devices with stable instance-path identities
    Devices,
    /// Live per-device key monitor
    Monitor,
    /// Manage / test virtual pads (plug N pads, LED order, kill-recovery)
    Pads {
        #[arg(default_value_t = 4)]
        count: u8,
    },
    /// Import legacy splitter_presets.xml / splitter_games.xml into ksx TOML
    ImportLegacy {
        /// Directory containing the legacy XML files (default: alongside the legacy exe)
        #[arg(long)]
        from: Option<std::path::PathBuf>,
    },
    /// Diagnostics: driver health, CI-policy state, latency histogram
    Doctor {
        #[arg(long)]
        latency: bool,
    },
    /// Install/verify bundled drivers (admin)
    InstallDrivers,
    /// Configure start-at-boot via Task Scheduler
    Autostart {
        #[arg(long)]
        disable: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { .. } => not_yet("run", "M4"),
        Command::Devices => not_yet("devices", "M3"),
        Command::Monitor => not_yet("monitor", "M3"),
        Command::Pads { .. } => not_yet("pads", "M2"),
        Command::ImportLegacy { .. } => not_yet("import-legacy", "M1"),
        Command::Doctor { .. } => not_yet("doctor", "M3"),
        Command::InstallDrivers => not_yet("install-drivers", "M5"),
        Command::Autostart { .. } => not_yet("autostart", "M5"),
    }
}

fn not_yet(cmd: &str, milestone: &str) -> anyhow::Result<()> {
    anyhow::bail!("`ksx {cmd}` lands in milestone {milestone} — scaffold only for now")
}
