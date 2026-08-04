//! ksx — split keyboards (I-PAC arcade encoders) into virtual Xbox 360 controllers.

mod autostart;
#[cfg(windows)]
mod capture;
mod console;
#[cfg(windows)]
mod ctrl_c;
mod daemon;
mod devices;
mod doctor;
mod install;
mod logging;
mod monitor;
mod pads;
mod run;
mod winusb;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ksx", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start emulation: plug the pads, capture the assigned keyboards, translate
    ///
    /// Resolves `[[slot]]` entries (or a `--game` profile) into virtual Xbox 360
    /// pads, then blocks input ONLY for the keyboards those slots are bound to —
    /// every other keyboard keeps typing. Emergency escapes are printed as a
    /// banner before any blocking starts and are evaluated inside the capture
    /// thread, so they work even if the rest of ksx wedges: LeftCtrl x5 toggles
    /// keyboard capture, RightCtrl x5 is reserved for mice (logged only),
    /// Ctrl+Alt+Del stops emulation.
    ///
    /// Getting out: with every keyboard captured, use LeftCtrl x5 or
    /// Ctrl+Alt+Del. Ctrl+C canNOT work from a captured keyboard — Interception
    /// suppresses the keystrokes below win32k, so Windows never raises a console
    /// break event; it works only from an uncaptured keyboard or before blocking
    /// is enabled. `taskkill /f /im ksx.exe` works too, but needs a keyboard or
    /// mouse you can still act from (M4 never captures the mouse). A thread
    /// panic or process death also returns every keyboard — blocking needs no
    /// cleanup to be undone.
    ///
    /// With --game, the profile's program is started AFTER the pads are plugged
    /// and capture is armed (a game started earlier sees zero controllers), and
    /// emulation stops when it exits. A process that exits within 10 s is
    /// treated as a launcher, not the game: ksx then watches for the profile's
    /// `process_name` for 60 s and follows that instead. (Legacy used 3 s;
    /// Steam takes 5 s to hand off, and being too tight stops emulation while a
    /// launch is still in progress. Override per profile with
    /// `launcher_grace_ms` — lower it to notice a short session sooner, raise
    /// it for a slower launcher.) ksx never kills a game it started — stopping
    /// emulation leaves the game running.
    ///
    /// Exit codes: 0 = clean stop (Ctrl+Alt+Del, the game exiting, Ctrl+C where
    /// it can be delivered, --dry-run), 1 = error, 2 = refused to start
    /// (invalid config, unknown --game, a --game profile whose exe is missing,
    /// missing driver, two keyboards sharing one hardware id; nothing was
    /// plugged and no filter was set), 3 = started then torn down by a runtime
    /// failure, including a game that failed to launch (keyboards were released
    /// first).
    Run {
        /// Take the slot layout and block flags from this games.toml profile
        #[arg(long, value_name = "TITLE")]
        game: Option<String>,
        /// Apply the --game profile's slots and flags without starting the game
        #[arg(long, requires = "game")]
        no_launch: bool,
        /// Resolve and print the plan, then exit without touching any driver
        #[arg(long)]
        dry_run: bool,
        /// Print a rolling capture-to-submit latency summary every 5 s
        #[arg(long)]
        latency: bool,
        /// JSON on stdout: the plan with --dry-run, otherwise the final summary
        #[arg(long)]
        json: bool,
    },
    /// List every keyboard ksx could capture, on either backend
    ///
    /// Read-only on both halves: keyboards as the Interception driver sees them
    /// (hardware id, slot, friendly name, slot-budget health), and USB
    /// interfaces as WinUSB candidates (instance path, VID/PID, interface, and
    /// whether the winusb.sys rebind is present). Each device is shown with the
    /// backend its `[[device]]` entry selects. Nothing is opened, claimed or
    /// rebound, and no keyboard filter is ever set — this cannot affect the
    /// machine's keyboards.
    ///
    /// A missing Interception driver is reported, not fatal: after the M6
    /// rebind, running with it uninstalled is the target state.
    ///
    /// Exit codes: 0 = listed, 1 = error, 2 = nothing could be enumerated at
    /// all (run `ksx doctor`).
    Devices {
        /// One JSON object {backend, keyboards, mice_visible, health} on stdout
        #[arg(long)]
        json: bool,
    },
    /// Live per-device key monitor (passthrough-only — never blocks)
    ///
    /// Streams one `<alias> <Key> down|up` line per keystroke on every
    /// keyboard. Every stroke is re-sent to the OS: this command has no way
    /// to suppress input (blocking lives in `ksx run`). Runs until Ctrl+C
    /// unless --for-secs is given.
    ///
    /// Exit codes: 0 = clean stop, 1 = error, 2 = Interception driver
    /// unavailable (run `ksx doctor`).
    Monitor {
        /// Hard-stop after N seconds (default: run until Ctrl+C)
        #[arg(long, value_name = "N")]
        for_secs: Option<u64>,
        /// Write JSONL {t_ms, device, key, down} per event (replay-oracle corpus)
        #[arg(long, value_name = "FILE")]
        record: Option<std::path::PathBuf>,
        /// JSONL on stdout: warning lines, event lines, one final {"summary":...}
        #[arg(long)]
        json: bool,
    },
    /// Manage / test virtual pads (plug N pads, LED order, kill-recovery)
    ///
    /// Plugs N virtual pads through ViGEmBus, prints each pad's
    /// XInput user index + LED number, runs a visible test pattern
    /// (A/B/X/Y cycle, circular stick sweep, trigger pulses) until
    /// --hold-secs elapses or Ctrl+C, then unplugs cleanly.
    ///
    /// Exit codes: 0 = pads plugged and unplugged cleanly, 1 = error,
    /// 2 = ViGEmBus driver is not installed.
    Pads {
        /// Pads to plug (XInput has 4 slots; pads 5..=8 need --persona playstation)
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=8))]
        count: u8,
        /// Controller type for every pad: xbox360 (default) or playstation
        /// (aliases ds4/ps4 accepted). PlayStation pads are HID/DirectInput —
        /// no XInput user index, no LED, and joy.cpl shows a "Wireless
        /// Controller".
        #[arg(long, default_value = "xbox360", value_parser = parse_persona)]
        persona: ksx_core::Persona,
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
        /// Explain the capture-to-submit latency histogram (measured by `ksx run`)
        #[arg(long)]
        latency: bool,
        /// One JSON object {report, advice} on stdout
        #[arg(long)]
        json: bool,
    },
    /// Stay resident with a tray icon; start/stop emulation on demand
    ///
    /// The tray runs on its own thread with its own message pump and has NO
    /// path to the capture, engine or output threads — it can only enqueue a
    /// command. A wedged tray therefore costs you a menu, not your keyboards
    /// (which is exactly what went wrong in the legacy app's WPF UI thread).
    ///
    /// Menu: Start emulation, Stop emulation, Reload config, Open config
    /// folder, Quit. The tooltip shows the current state plus any capture
    /// health problem (reboot required, watchdog tripped, dropped events) —
    /// polled from the RUNNING session, so a mid-session problem appears while
    /// it is happening, and the last finished session's verdict is shown only
    /// once nothing is running.
    ///
    /// --headless offers the identical commands on stdin: start | stop |
    /// reload | config | status | quit.
    ///
    /// THE CONSOLE: once the tray icon is on screen, ksx releases the console
    /// window it was started from, so the tray is the whole interface and there
    /// is no terminal to close by accident (closing it would kill the daemon)
    /// and none on a cabinet's game screen at logon. Logging survives that:
    /// every line, a panic included, also goes to the daily rotating log file
    /// under the config root (its path is printed at startup and again just
    /// before the console is released). Use --console to keep it and watch a
    /// session live. --headless always keeps it: stdin is its control surface.
    ///
    /// Exit codes: 0 = clean exit, 1 = error, 2 = the configuration does not
    /// resolve (nothing was started).
    Daemon {
        /// Use this games.toml profile for each session
        #[arg(long, value_name = "TITLE")]
        game: Option<String>,
        /// With --game, apply the profile but never start its program
        #[arg(long, requires = "game")]
        no_launch: bool,
        /// No tray icon; take the same commands on stdin (keeps the console)
        #[arg(long)]
        headless: bool,
        /// Keep the console window attached (debugging; watch a session live)
        #[arg(long)]
        console: bool,
        /// Start emulation immediately instead of waiting for a command
        #[arg(long)]
        start: bool,
    },
    /// Install/verify the bundled ViGEmBus driver (needs administrator)
    ///
    /// Reports what is installed, then verifies the bundled installer against
    /// two independent pins — its SHA-256 and its Authenticode signer — before
    /// offering to run it. The file is opened ONCE with writers locked out and
    /// stays open across execution, so the bytes that were checked are the
    /// bytes that run. A file that fails either pin is refused, and ksx will
    /// not print a command line for it either.
    ///
    /// ksx never downloads anything and never self-elevates: if an admin token
    /// is needed it says so and stops. Interception is reported but never
    /// installed (non-commercial licence — see docs/DRIVERS.md).
    ///
    /// Exit codes: 0 = nothing to do or the install succeeded, 1 = error,
    /// 2 = refused (verification failed, installer missing, elevation needed),
    /// 3 = the installer ran and returned a failure.
    InstallDrivers {
        /// Report and verify without executing anything
        #[arg(long)]
        dry_run: bool,
        /// Actually run the verified installer (otherwise this is a report)
        #[arg(long)]
        yes: bool,
        /// Run setup again even when ViGEmBus is already installed
        #[arg(long)]
        repair: bool,
        /// One JSON object {action, verdict, installer, installed, ...} on stdout
        #[arg(long)]
        json: bool,
    },
    /// Start ksx at logon via a per-user Task Scheduler task
    ///
    /// Registers `ksx daemon` (add --game <TITLE> to give every session a
    /// profile) as a logon-triggered task for the current user only:
    /// InteractiveToken, LeastPrivilege, never elevated. Idempotent — enabling
    /// twice replaces the task.
    ///
    /// The default is the tray daemon, not a session, and that is deliberate:
    /// a registered `ksx run` captures the assigned keyboards unconditionally
    /// at every logon — a hostile default on a machine that is also a desktop
    /// PC — while the daemon sits in the tray until a session is asked for.
    /// `--mode run` keeps the kiosk shape (logon straight into the game) for
    /// cabinets that want exactly that. Changing the default was safe: no
    /// cabinet has ever run the M5 gate, so no deployed registration relied on
    /// `run` (and --status still reports both shapes correctly).
    ///
    /// --enable validates first: the config must pass the same checks `ksx run`
    /// applies, the --game profile must exist, and its executable must be
    /// present. A typo caught here is a one-line error; the same typo
    /// registered is a cabinet that cold-boots to nothing.
    ///
    /// --status also reports a STALE registration (ksx moved, task did not).
    ///
    /// Exit codes: 0 = done, 1 = error, 2 = refused (validation failed) or a
    /// stale registration was found by --status.
    Autostart {
        /// Register the logon task (validates the configuration first)
        #[arg(long, conflicts_with_all = ["disable", "status"])]
        enable: bool,
        /// Remove the logon task (safe to run when nothing is registered)
        #[arg(long, conflicts_with = "status")]
        disable: bool,
        /// Report what is registered (the default when no verb is given)
        #[arg(long)]
        status: bool,
        /// What the task starts: the tray daemon (default) or a full session
        #[arg(long, value_enum, default_value = "daemon")]
        mode: AutostartMode,
        /// Give the registered command a games.toml profile: the daemon uses
        /// it for every session; `--mode run` starts it at logon
        #[arg(long, value_name = "TITLE")]
        game: Option<String>,
        /// Seconds to wait after logon before starting
        #[arg(long, default_value_t = 10)]
        delay_secs: u32,
        /// Override the scheduled-task name (default: ksx\autostart)
        #[arg(long, value_name = "NAME")]
        task_name: Option<String>,
        /// Print the exact XML and schtasks invocation; register nothing
        #[arg(long)]
        dry_run: bool,
        /// JSON on stdout
        #[arg(long)]
        json: bool,
    },
    /// Manage the WinUSB claim: which interfaces ksx can take, and how to give them back
    ///
    /// Claiming an interface rebinds it from the keyboard stack to Microsoft's
    /// in-box winusb.sys. Blocking then costs nothing and cannot be bypassed —
    /// the interface is not in the keyboard stack at all — and there is no
    /// third-party kernel driver left to expire. That is what M6 is for: the
    /// Interception driver this project shipped on is cross-signed with a
    /// certificate that expired in 2012.
    ///
    /// THE TRADE, stated plainly: a claimed panel is no longer a keyboard.
    /// It types only while ksx is running — the daemon re-injects its keys
    /// with SendInput whenever emulation is stopped, so frontend menus keep
    /// working. If ksx is not running, a claimed panel does nothing. Injected
    /// keys also cannot reach the lock screen, a UAC prompt or Ctrl+Alt+Del.
    /// Keep one ordinary keyboard on another port; `claim` refuses to take the
    /// last one.
    ///
    /// `status` is read-only. `claim` and `release` are dry runs by default:
    /// they print the exact INF and the exact pnputil command line and change
    /// nothing until you add --yes (which also needs an administrator token).
    ///
    /// Exit codes: 0 = reported or done, 1 = error, 2 = refused (unknown or
    /// ambiguous device, not a keyboard interface, already claimed, elevation
    /// needed, or it is the only keyboard on the machine), 3 = pnputil ran and
    /// failed.
    Winusb {
        #[command(subcommand)]
        command: WinusbCommand,
    },
}

#[derive(Subcommand)]
enum WinusbCommand {
    /// List USB interfaces, their current driver, and whether ksx could claim them
    ///
    /// Read-only: reads the PnP device tree and the registry. Opens nothing,
    /// claims nothing, changes nothing.
    Status {
        /// One JSON object {keyboard_count, keyboards, candidates} on stdout
        #[arg(long)]
        json: bool,
    },
    /// Rebind an interface to winusb.sys (DRY RUN unless --yes)
    ///
    /// DEVICE is an instance path from `ksx winusb status`, or any unique
    /// substring of one. An ambiguous match is refused, never guessed — two
    /// identical I-PACs differ only in their instance path.
    Claim {
        /// Instance path (or a unique substring) from `ksx winusb status`
        device: String,
        /// Print the INF and the commands; change nothing (the default)
        #[arg(long)]
        dry_run: bool,
        /// Actually write the INF and run pnputil (needs administrator)
        #[arg(long)]
        yes: bool,
        /// JSON on stdout
        #[arg(long)]
        json: bool,
    },
    /// Give an interface back to the keyboard driver (DRY RUN unless --yes)
    ///
    /// The rollback: pnputil /remove-device, delete the ksx INF from the driver
    /// store (without which a rescan re-binds WinUSB straight back), then
    /// /scan-devices.
    Release {
        /// Instance path (or a unique substring) from `ksx winusb status`
        device: String,
        /// Print the commands; change nothing (the default)
        #[arg(long)]
        dry_run: bool,
        /// Actually run pnputil (needs administrator)
        #[arg(long)]
        yes: bool,
        /// Release a device that is not currently WinUSB-bound (recovery)
        #[arg(long)]
        force: bool,
        /// JSON on stdout
        #[arg(long)]
        json: bool,
    },
}

/// Clap adapter for [`ksx_core::Persona`]'s lenient `FromStr` (ksx-core carries
/// no clap dependency). The error already names the valid values.
fn parse_persona(s: &str) -> Result<ksx_core::Persona, ksx_core::UnknownPersona> {
    s.parse()
}

/// What `ksx autostart` registers as the logon task. The clap-facing twin of
/// [`ksx_platform::autostart::TaskMode`] (the platform crate stays clap-free);
/// the rationale for `daemon` being the default lives on that type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum AutostartMode {
    /// Tray icon at logon; sessions are started from the tray or a wrapper
    Daemon,
    /// Capture keyboards and start a session immediately at logon (kiosk)
    Run,
}

impl From<AutostartMode> for ksx_platform::autostart::TaskMode {
    fn from(mode: AutostartMode) -> Self {
        match mode {
            AutostartMode::Daemon => Self::Daemon,
            AutostartMode::Run => Self::Run,
        }
    }
}

fn main() -> anyhow::Result<()> {
    // Logging first, and for **every** command — not just the daemon. A
    // `ksx run` started by the cabinet's logon task has no console either, and
    // the whole point of the file sink is that something is left behind when a
    // session ends badly. A config root that cannot be discovered degrades to
    // stderr rather than failing the command: `ksx --version` must still work.
    //
    // The returned `LogSink` is *not* a guard — `crate::logging` keeps the
    // writer's `WorkerGuard` in a `static` precisely so that no future edit to
    // this function can drop it and silently stop logging.
    let sink = logging::init(ksx_config::ConfigRoot::discover().ok().as_ref());
    logging::announce(&sink);
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            game,
            no_launch,
            dry_run,
            latency,
            json,
        } => run::run(game, no_launch, dry_run, latency, json),
        Command::Devices { json } => devices::run(json),
        Command::Monitor {
            for_secs,
            record,
            json,
        } => monitor::run(for_secs, record, json),
        Command::Pads {
            count,
            persona,
            hold_secs,
            json,
        } => pads::run(count, persona, hold_secs, json),
        Command::ImportLegacy {
            from,
            dry_run,
            json,
        } => import_legacy(from, dry_run, json),
        Command::Doctor { latency, json } => {
            if latency {
                doctor::run_latency(json)
            } else {
                doctor::run(json)
            }
        }
        Command::Daemon {
            game,
            no_launch,
            headless,
            console,
            start,
        } => daemon::run(game, no_launch, headless, console, start),
        Command::InstallDrivers {
            dry_run,
            yes,
            repair,
            json,
        } => install::run(install::Options {
            dry_run,
            json,
            yes,
            repair,
        }),
        Command::Autostart {
            enable,
            disable,
            status: _,
            mode,
            game,
            delay_secs,
            task_name,
            dry_run,
            json,
        } => autostart::run(autostart::Options {
            // No verb means `--status`: the read-only answer is the only safe
            // default for a command that can rewrite what a machine does at
            // every logon.
            action: match (enable, disable) {
                (true, _) => autostart::Action::Enable,
                (_, true) => autostart::Action::Disable,
                _ => autostart::Action::Status,
            },
            mode: mode.into(),
            game,
            delay_secs,
            task_name,
            extra_args: Vec::new(),
            dry_run,
            json,
        }),
        Command::Winusb { command } => match command {
            WinusbCommand::Status { json } => winusb::run(winusb::Options {
                action: winusb::Action::Status,
                dry_run: true,
                yes: false,
                json,
            }),
            WinusbCommand::Claim {
                device,
                dry_run,
                yes,
                json,
            } => winusb::run(winusb::Options {
                action: winusb::Action::Claim { device },
                dry_run,
                yes,
                json,
            }),
            WinusbCommand::Release {
                device,
                dry_run,
                yes,
                force,
                json,
            } => winusb::run(winusb::Options {
                action: winusb::Action::Release { device, force },
                dry_run,
                yes,
                json,
            }),
        },
    }
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
                persona: ksx_core::Persona::Xbox360,
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
                persona: ksx_core::Persona::Xbox360,
                hold_secs: 2,
                json: true,
            }
        ));
    }

    #[test]
    fn pads_persona_accepts_aliases_and_rejects_unknowns() {
        for (arg, want) in [
            ("playstation", ksx_core::Persona::PlayStation),
            ("ds4", ksx_core::Persona::PlayStation),
            ("PS4", ksx_core::Persona::PlayStation),
            ("xbox360", ksx_core::Persona::Xbox360),
        ] {
            let cli = Cli::try_parse_from(["ksx", "pads", "--persona", arg]).unwrap();
            assert!(
                matches!(cli.command, Command::Pads { persona, .. } if persona == want),
                "{arg}"
            );
        }
        let err = Cli::try_parse_from(["ksx", "pads", "--persona", "gamecube"])
            .err()
            .expect("an unknown persona must be a parse error");
        let msg = err.to_string();
        assert!(msg.contains("playstation"), "must name the options: {msg}");
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

    #[test]
    fn devices_parses_with_and_without_json() {
        let cli = Cli::try_parse_from(["ksx", "devices"]).unwrap();
        assert!(matches!(cli.command, Command::Devices { json: false }));
        let cli = Cli::try_parse_from(["ksx", "devices", "--json"]).unwrap();
        assert!(matches!(cli.command, Command::Devices { json: true }));
    }

    #[test]
    fn devices_help_documents_exit_codes() {
        let mut cmd = Cli::command();
        let devices = cmd.find_subcommand_mut("devices").unwrap();
        let help = devices.render_long_help().to_string();
        assert!(
            help.contains("2 = nothing could be enumerated at all"),
            "{help}"
        );
        assert!(help.contains("ksx doctor"), "{help}");
        // M6 changed what a missing Interception driver means here: it is the
        // expected end state, not a failure, and the help has to say so or
        // someone will read exit 0 with an empty keyboard list as a bug.
        assert!(
            help.contains("A missing Interception driver is reported, not fatal"),
            "{help}"
        );
        assert!(
            help.contains("Nothing is opened, claimed or rebound"),
            "{help}"
        );
    }

    #[test]
    fn monitor_defaults_run_until_ctrl_c() {
        let cli = Cli::try_parse_from(["ksx", "monitor"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Monitor {
                for_secs: None,
                record: None,
                json: false,
            }
        ));
    }

    /// The exact bounded live-smoke invocation the M3 gate runs.
    #[test]
    fn monitor_flags_parse() {
        let cli = Cli::try_parse_from(["ksx", "monitor", "--for-secs", "5"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Monitor {
                for_secs: Some(5),
                record: None,
                json: false,
            }
        ));
        let cli = Cli::try_parse_from([
            "ksx",
            "monitor",
            "--for-secs",
            "10",
            "--record",
            "corpus.jsonl",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Monitor {
                for_secs,
                record,
                json,
            } => {
                assert_eq!(for_secs, Some(10));
                assert_eq!(
                    record.as_deref(),
                    Some(std::path::Path::new("corpus.jsonl"))
                );
                assert!(json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    #[test]
    fn monitor_help_promises_passthrough_only() {
        let mut cmd = Cli::command();
        let monitor = cmd.find_subcommand_mut("monitor").unwrap();
        let help = monitor.render_long_help().to_string();
        assert!(help.contains("passthrough-only"), "{help}");
        assert!(help.contains("re-sent to the OS"), "{help}");
        assert!(help.contains("2 = Interception driver"), "{help}");
    }

    #[test]
    fn run_defaults_to_the_config_layout_and_no_flags() {
        let cli = Cli::try_parse_from(["ksx", "run"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Run {
                game: None,
                no_launch: false,
                dry_run: false,
                latency: false,
                json: false,
            }
        ));
    }

    #[test]
    fn run_flags_parse() {
        let cli = Cli::try_parse_from([
            "ksx",
            "run",
            "--game",
            "Street Fighter",
            "--dry-run",
            "--latency",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Run {
                game,
                no_launch,
                dry_run,
                latency,
                json,
            } => {
                assert!(!no_launch);
                assert_eq!(game.as_deref(), Some("Street Fighter"));
                assert!(dry_run);
                assert!(latency);
                assert!(json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    /// The escape hotkeys and the exit-code contract are documented where a
    /// user (or an agent) will actually look: `ksx run --help`.
    #[test]
    fn run_help_documents_escapes_and_exit_codes() {
        let mut cmd = Cli::command();
        let run = cmd.find_subcommand_mut("run").unwrap();
        let help = run.render_long_help().to_string();
        for needle in [
            "LeftCtrl x5",
            "RightCtrl x5",
            "Ctrl+Alt+Del",
            "taskkill",
            "0 = clean stop",
            "2 = refused to start",
            "3 = started then torn down",
        ] {
            assert!(help.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    /// `--help` must not promise an escape hatch that cannot exist: with every
    /// keyboard captured, Interception suppresses the keystrokes below win32k,
    /// so no CTRL_C_EVENT is ever generated and the console handler never runs.
    #[test]
    fn run_help_is_honest_about_ctrl_c() {
        let mut cmd = Cli::command();
        let run = cmd.find_subcommand_mut("run").unwrap();
        let help = run.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("Ctrl+C canNOT work from a captured keyboard"),
            "the Ctrl+C limitation must be stated plainly:\n{help}"
        );
        assert!(
            flat.contains("uncaptured keyboard or before blocking is enabled"),
            "the help must say when Ctrl+C DOES work:\n{help}"
        );
        assert!(
            flat.contains("needs a keyboard or mouse you can still act from"),
            "taskkill needs an input device you can still use:\n{help}"
        );
        assert!(
            !flat.contains("Ctrl+C, a thread panic, or `taskkill /f` all return every keyboard"),
            "the old claim that Ctrl+C always works must be gone:\n{help}"
        );
    }

    #[test]
    fn doctor_latency_is_no_longer_a_stub() {
        let cli = Cli::try_parse_from(["ksx", "doctor", "--latency"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Doctor {
                latency: true,
                json: false,
            }
        ));
        let text = doctor::render_latency();
        assert!(text.contains("ksx run --latency"), "{text}");
        assert!(text.contains("p99"), "{text}");
        assert!(
            !text.contains("M4"),
            "the not-yet stub must be gone: {text}"
        );
    }

    // -----------------------------------------------------------------
    // M5 rest: ksx autostart --mode
    // -----------------------------------------------------------------

    /// The default registration is the tray daemon. A bare `--enable` on a
    /// machine that is also a desktop PC must NOT produce a task that captures
    /// the keyboards at every logon.
    #[test]
    fn autostart_defaults_to_the_daemon_mode() {
        let cli = Cli::try_parse_from(["ksx", "autostart", "--enable"]).unwrap();
        let Command::Autostart { mode, game, .. } = cli.command else {
            panic!("parsed to the wrong subcommand");
        };
        assert_eq!(mode, AutostartMode::Daemon);
        assert_eq!(game, None);
        assert_eq!(
            ksx_platform::autostart::TaskMode::from(mode),
            ksx_platform::autostart::TaskMode::Daemon
        );
    }

    #[test]
    fn autostart_mode_parses_both_values_composes_with_game_and_rejects_unknowns() {
        for (arg, want) in [
            ("daemon", AutostartMode::Daemon),
            ("run", AutostartMode::Run),
        ] {
            let cli = Cli::try_parse_from([
                "ksx",
                "autostart",
                "--enable",
                "--mode",
                arg,
                "--game",
                "Street Fighter",
            ])
            .unwrap();
            let Command::Autostart { mode, game, .. } = cli.command else {
                panic!("parsed to the wrong subcommand");
            };
            assert_eq!(mode, want, "{arg}");
            assert_eq!(game.as_deref(), Some("Street Fighter"), "{arg}");
        }
        let err = Cli::try_parse_from(["ksx", "autostart", "--enable", "--mode", "kiosk"])
            .err()
            .expect("an unknown mode must be a parse error");
        let msg = err.to_string();
        assert!(
            msg.contains("daemon") && msg.contains("run"),
            "must name the valid modes: {msg}"
        );
    }

    /// The why lives in `--help`, where the person about to register a logon
    /// task is actually looking.
    #[test]
    fn autostart_help_says_why_the_daemon_is_the_default() {
        let mut cmd = Cli::command();
        let autostart = cmd.find_subcommand_mut("autostart").unwrap();
        let help = autostart.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "captures the assigned keyboards unconditionally",
            "also a desktop PC",
            "sits in the tray until a session is asked for",
            "--mode run",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    // -----------------------------------------------------------------
    // M6: ksx winusb
    // -----------------------------------------------------------------

    #[test]
    fn winusb_status_parses_and_is_the_read_only_verb() {
        let cli = Cli::try_parse_from(["ksx", "winusb", "status", "--json"]).unwrap();
        match cli.command {
            Command::Winusb {
                command: WinusbCommand::Status { json },
            } => assert!(json),
            _ => panic!("parsed to the wrong subcommand"),
        }
        // `status` takes no --yes: there is nothing for it to consent to.
        assert!(Cli::try_parse_from(["ksx", "winusb", "status", "--yes"]).is_err());
    }

    #[test]
    fn winusb_claim_and_release_take_a_device_and_default_to_not_acting() {
        let cli = Cli::try_parse_from(["ksx", "winusb", "claim", "MI_00"]).unwrap();
        match cli.command {
            Command::Winusb {
                command:
                    WinusbCommand::Claim {
                        device,
                        dry_run,
                        yes,
                        json,
                    },
            } => {
                assert_eq!(device, "MI_00");
                // `yes` unset is what makes this a report; the command layer
                // requires `yes && !dry_run` before it touches pnputil.
                assert!(!yes, "claim must never act without an explicit --yes");
                assert!(!dry_run);
                assert!(!json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        let cli =
            Cli::try_parse_from(["ksx", "winusb", "release", "MI_00", "--force", "--yes"]).unwrap();
        match cli.command {
            Command::Winusb {
                command:
                    WinusbCommand::Release {
                        device, force, yes, ..
                    },
            } => {
                assert_eq!(device, "MI_00");
                assert!(force);
                assert!(yes);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        // A device argument is not optional: `ksx winusb claim` with no target
        // must not be able to mean "whatever you think is best".
        assert!(Cli::try_parse_from(["ksx", "winusb", "claim"]).is_err());
        assert!(Cli::try_parse_from(["ksx", "winusb", "release"]).is_err());
    }

    /// The trade-off — a claimed panel types only while ksx runs — is the one
    /// thing a user must know before running this, so it lives in `--help`,
    /// not only in a doc they have not opened.
    #[test]
    fn winusb_help_states_the_trade_and_the_exit_codes() {
        let mut cmd = Cli::command();
        let winusb = cmd.find_subcommand_mut("winusb").unwrap();
        let help = winusb.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "no longer a keyboard",
            "types only while ksx is running",
            "If ksx is not running, a claimed panel does nothing",
            "cannot reach the lock screen",
            "refuses to take the last one",
            "dry runs by default",
            "2 = refused",
            "3 = pnputil ran and failed",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    // -----------------------------------------------------------------
    // The daemon's console
    // -----------------------------------------------------------------

    /// Plain `ksx daemon` must detach; `--console` and `--headless` must not.
    /// This is the flag-to-policy wiring — the policy itself is tested in
    /// `crate::console`.
    #[test]
    fn daemon_console_flags_parse_and_select_the_right_policy() {
        let cli = Cli::try_parse_from(["ksx", "daemon"]).unwrap();
        let Command::Daemon {
            headless, console, ..
        } = cli.command
        else {
            panic!("parsed to the wrong subcommand");
        };
        assert!(!headless);
        assert!(!console);
        assert!(
            console::mode(headless, console).detaches(),
            "a bare `ksx daemon` must release its console: a stray terminal window on a \
             cabinet is one click away from killing emulation"
        );

        for args in [
            vec!["ksx", "daemon", "--console"],
            vec!["ksx", "daemon", "--headless"],
            vec!["ksx", "daemon", "--headless", "--console"],
        ] {
            let cli = Cli::try_parse_from(&args).unwrap();
            let Command::Daemon {
                headless, console, ..
            } = cli.command
            else {
                panic!("parsed to the wrong subcommand");
            };
            assert!(
                !console::mode(headless, console).detaches(),
                "{args:?} must keep the console"
            );
        }
    }

    /// The trade has to be in `--help`, because it is the only place somebody
    /// looks after their daemon vanished.
    #[test]
    fn daemon_help_states_what_happens_to_the_console() {
        let mut cmd = Cli::command();
        let daemon = cmd.find_subcommand_mut("daemon").unwrap();
        let help = daemon.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "releases the console window",
            // The file log is the answer to "where did my daemon go", so the
            // help has to name it — and must no longer claim the opposite.
            "daily rotating log file",
            "a panic included",
            "--console to keep it",
            "--headless always keeps it",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
        assert!(
            !flat.contains("log output stops at that moment"),
            "the pre-file-log claim must be gone:\n{help}"
        );
    }

    /// The tooltip's promise, in `--help`: health is live, not post-mortem.
    #[test]
    fn daemon_help_promises_live_health_in_the_tooltip() {
        let mut cmd = Cli::command();
        let daemon = cmd.find_subcommand_mut("daemon").unwrap();
        let help = daemon.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("polled from the RUNNING session"), "{help}");
        assert!(flat.contains("while it is happening"), "{help}");
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
