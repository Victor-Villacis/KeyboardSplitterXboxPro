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
    ///
    /// Exit codes: 0 = clean import, 1 = error, 3 = imported with warnings.
    ImportLegacy {
        /// Directory containing the legacy XML files (default: alongside the legacy exe)
        #[arg(long)]
        from: Option<std::path::PathBuf>,
        /// Print the rendered TOML to stdout instead of writing files
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable JSON output (for automation)
        #[arg(long)]
        json: bool,
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
        Command::ImportLegacy {
            from,
            dry_run,
            json,
        } => import_legacy(from, dry_run, json),
        Command::Doctor { .. } => not_yet("doctor", "M3"),
        Command::InstallDrivers => not_yet("install-drivers", "M5"),
        Command::Autostart { .. } => not_yet("autostart", "M5"),
    }
}

fn not_yet(cmd: &str, milestone: &str) -> anyhow::Result<()> {
    anyhow::bail!("`ksx {cmd}` lands in milestone {milestone} — scaffold only for now")
}

/// Exit code 3 = import completed but produced warnings (0 = clean, 1 = error).
const EXIT_IMPORT_WARNINGS: i32 = 3;

fn import_legacy(
    from: Option<std::path::PathBuf>,
    dry_run: bool,
    json: bool,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_legacy_import::json_escape;

    let dir = from.unwrap_or_else(ksx_legacy_import::default_legacy_dir);
    let import = ksx_legacy_import::import_dir(&dir)
        .with_context(|| format!("importing legacy XML from '{}'", dir.display()))?;

    if dry_run {
        let files = import.rendered_files()?;
        if json {
            let rendered: Vec<String> = files
                .iter()
                .map(|f| {
                    format!(
                        "{{\"path\":\"{}\",\"content\":\"{}\"}}",
                        json_escape(&f.path),
                        json_escape(&f.content)
                    )
                })
                .collect();
            println!(
                "{{\"dry_run\":true,\"legacy_dir\":\"{}\",\"report\":{},\"files\":[{}]}}",
                json_escape(&dir.display().to_string()),
                import.report.to_json(),
                rendered.join(",")
            );
        } else {
            for file in &files {
                println!("==== {} ====", file.path);
                println!("{}", file.content);
            }
            // Report on stderr so stdout stays pipeable TOML.
            eprintln!("{}", import.report);
        }
    } else {
        let root = ksx_legacy_import::default_config_root()
            .context("cannot resolve the ksx config root (%APPDATA% is not set)")?;
        let written = import
            .write_outputs(&root)
            .with_context(|| format!("writing TOML into '{}'", root.display()))?;
        if json {
            let paths: Vec<String> = written
                .iter()
                .map(|p| format!("\"{}\"", json_escape(&p.display().to_string())))
                .collect();
            println!(
                "{{\"dry_run\":false,\"legacy_dir\":\"{}\",\"config_root\":\"{}\",\"report\":{},\"written\":[{}]}}",
                json_escape(&dir.display().to_string()),
                json_escape(&root.display().to_string()),
                import.report.to_json(),
                paths.join(",")
            );
        } else {
            println!("{}", import.report);
            for path in &written {
                println!("wrote {}", path.display());
            }
        }
    }

    if !import.report.warnings.is_empty() {
        std::process::exit(EXIT_IMPORT_WARNINGS);
    }
    Ok(())
}
