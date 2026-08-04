//! ksx — split keyboards (I-PAC arcade encoders) into virtual Xbox 360 controllers.

mod doctor;
mod pads;

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
    ///
    /// Plugs N virtual Xbox 360 pads through ViGEmBus, prints each pad's
    /// XInput user index + LED number, runs a visible test pattern
    /// (A/B/X/Y cycle, circular stick sweep, trigger pulses) until
    /// --hold-secs elapses or Ctrl+C, then unplugs cleanly.
    ///
    /// Exit codes: 0 = pads plugged and unplugged cleanly, 1 = error,
    /// 2 = ViGEmBus driver is not installed.
    Pads {
        /// Pads to plug (XInput has 4 slots; pads 5..=8 are DirectInput-only)
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=8))]
        count: u8,
        /// Seconds to run the test pattern before unplugging
        #[arg(long, default_value_t = 10)]
        hold_secs: u64,
        /// One JSON object {driver, pads} on stdout; skips the test pattern
        #[arg(long)]
        json: bool,
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
    ///
    /// Checks ViGEmBus, legacy ScpVBus, the Interception class filters and
    /// their Authenticode state, and the 2026 cross-signed-trust-removal CI
    /// policy, then prints verdicts with stable codes.
    ///
    /// Exit codes: 0 = healthy or warnings only, 1 = error, 2 = at least one
    /// critical problem (something will not work).
    Doctor {
        /// Capture-to-submit latency histogram (lands in M4)
        #[arg(long)]
        latency: bool,
        /// One JSON object {report, advice} on stdout
        #[arg(long)]
        json: bool,
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
        Command::Pads {
            count,
            hold_secs,
            json,
        } => pads::run(count, hold_secs, json),
        Command::ImportLegacy {
            from,
            dry_run,
            json,
        } => import_legacy(from, dry_run, json),
        Command::Doctor { latency, json } => {
            if latency {
                not_yet("doctor --latency", "M4")
            } else {
                doctor::run(json)
            }
        }
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn pads_defaults() {
        let cli = Cli::try_parse_from(["ksx", "pads"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Pads {
                count: 4,
                hold_secs: 10,
                json: false,
            }
        ));
    }

    #[test]
    fn pads_flags_parse() {
        let cli =
            Cli::try_parse_from(["ksx", "pads", "--count", "2", "--hold-secs", "2", "--json"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Command::Pads {
                count: 2,
                hold_secs: 2,
                json: true,
            }
        ));
    }

    #[test]
    fn pads_count_range_is_1_to_8() {
        assert!(Cli::try_parse_from(["ksx", "pads", "--count", "0"]).is_err());
        assert!(Cli::try_parse_from(["ksx", "pads", "--count", "9"]).is_err());
        for ok in ["1", "8"] {
            assert!(
                Cli::try_parse_from(["ksx", "pads", "--count", ok]).is_ok(),
                "{ok}"
            );
        }
    }

    #[test]
    fn pads_help_documents_exit_codes() {
        let mut cmd = Cli::command();
        let pads = cmd.find_subcommand_mut("pads").unwrap();
        let help = pads.render_long_help().to_string();
        assert!(
            help.contains("2 = ViGEmBus driver is not installed"),
            "{help}"
        );
    }

    #[test]
    fn doctor_parses_and_documents_exit_codes() {
        let cli = Cli::try_parse_from(["ksx", "doctor", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Doctor {
                latency: false,
                json: true,
            }
        ));
        let mut cmd = Cli::command();
        let doctor = cmd.find_subcommand_mut("doctor").unwrap();
        let help = doctor.render_long_help().to_string();
        assert!(help.contains("0 = healthy or warnings only"), "{help}");
        assert!(help.contains("2 = at least one"), "{help}");
    }

    /// M1 regression: the exact invocation the milestone gate smoke-runs.
    #[test]
    fn import_legacy_still_parses() {
        let cli = Cli::try_parse_from([
            "ksx",
            "import-legacy",
            "--from",
            "crates/ksx-legacy-import/tests/fixtures",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::ImportLegacy {
                from,
                dry_run,
                json,
            } => {
                assert_eq!(
                    from.as_deref(),
                    Some(std::path::Path::new(
                        "crates/ksx-legacy-import/tests/fixtures"
                    ))
                );
                assert!(dry_run);
                assert!(!json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }
}
