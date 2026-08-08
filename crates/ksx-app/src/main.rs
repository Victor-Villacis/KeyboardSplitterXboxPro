//! ksx — split keyboards (I-PAC arcade encoders) into virtual Xbox 360 controllers.

// In the TEST harness build only, dead-code analysis loses `fn main` as a
// liveness root (rustc 1.97 sharpened this), so everything reachable solely
// through the runtime path — the `dyn MachineSource` chain Studio drives, the
// onboard/profile-edit glue, every view-conversion helper no unit test calls
// directly — reads as "never used" in that one target and nowhere else. CI
// caught it under `--features studio` the first time local and CI ran the same
// compiler version; the bin target in the very same clippy invocation checks
// all of that code with `main` as a root and stays the authority, so this
// silences a structural false positive, not a lint.
#![cfg_attr(test, allow(dead_code))]

mod autostart;
#[cfg(feature = "cabinet")]
mod cabinet;
#[cfg(windows)]
mod capture;
mod config_io;
mod console;
#[cfg(windows)]
mod ctrl_c;
mod daemon;
mod device_edit;
mod device_scan;
mod devices;
mod doctor;
mod feed;
mod install;
mod logging;
mod macro_cli;
mod macro_trace;
mod map;
mod mapping;
mod monitor;
// The first-run state and the path-free config in/out, for the surfaces that
// have a screen. Gated with `sources` for the same reason: the CLI reaches this
// machinery through `config_io` directly, so a default build would carry it as
// dead code — which `-D warnings` refuses.
#[cfg(any(feature = "studio", feature = "cabinet"))]
mod onboard;
mod pads;
mod preset_cli;
mod preset_edit;
// Gated exactly like `sources` below, and for the same `-D warnings` reason:
// this is the write half of games.toml and Studio's Profiles page is its only
// caller today, so a default build would carry it as dead code. The gate comes
// off the day a `ksx games new` CLI verb exists — which is where it belongs
// per docs/SURFACES.md §2, and which is the one thing this change-set did not
// also do.
#[cfg(any(feature = "studio", feature = "cabinet"))]
mod profile_edit;
mod run;
mod session;
mod setup;
mod slot_cli;
mod slots;
#[cfg(any(feature = "studio", feature = "cabinet"))]
mod sources;
#[cfg(feature = "studio")]
mod studio;
#[cfg(feature = "studio")]
mod studio_launch;
mod winusb;

use clap::{Parser, Subcommand};

/// Everything a slot-numbered flag needs, read off [`ksx_core::MAX_SLOTS`]
/// instead of repeating its value.
///
/// Clap rejects an out-of-range value before `main` is entered, which makes a
/// hardcoded range the *effective* ceiling no matter what the constant says:
/// with `1..=8` frozen at three call sites, raising `MAX_SLOTS` left `ksx
/// setup --slot 9` dying at the parser having read no file and consulted no
/// constant. The help text is derived for the same reason — a stale number in
/// `--help` is worse than no number, because it tells the owner of a
/// 16-player cabinet that ksx cannot drive it.
mod slot_arg {
    use std::sync::LazyLock;

    use ksx_core::MAX_SLOTS;

    /// The accepted range, in the `i64` clap's ranged integer parsers speak.
    pub fn range() -> std::ops::RangeInclusive<i64> {
        1..=i64::from(MAX_SLOTS)
    }

    /// These are `help =` rather than `///` doc comments because the number in
    /// them is computed; a doc comment can only hold a literal.
    pub static SETUP_SLOT: LazyLock<String> = LazyLock::new(|| {
        format!("Slot to set up first; later players continue from it (1..={MAX_SLOTS})")
    });

    pub static PADS_COUNT: LazyLock<String> = LazyLock::new(|| {
        format!(
            "Pads to plug (1..={MAX_SLOTS}; XInput has 4 slots, so pads 5 and up need \
             --persona playstation)"
        )
    });

    pub static ASSIGN_SLOT: LazyLock<String> =
        LazyLock::new(|| format!("Slot number (1..={MAX_SLOTS})"));
}

#[derive(Parser)]
#[command(name = "ksx", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// First contact: identify a panel by pressing it, map it, write the config
    ///
    /// The wizard for a machine that has never run ksx (docs/USE-CASES.md
    /// "adoption gaps"). It asks you to HOLD A KEY on the panel you want to set
    /// up — never to pick a device from a numbered list, because on a cabinet
    /// with two identical encoders that is not a question anyone can answer —
    /// and then walks one control at a time: press the button the prompt names,
    /// it binds and moves on.
    ///
    /// PROMPTS ARE POSITION-NAMED. It asks for SOUTH, not "A": the letter is in
    /// a different place on a Nintendo pad and most arcade panels are labelled
    /// by position anyway. What gets STORED is ksx's own function vocabulary
    /// (`A`, `dpad.up`, `lx.min`) — the prompt is for you, the name is for the
    /// file. Each stick direction binds the D-pad AND the left stick from one
    /// press, because some games read only one of them.
    ///
    /// SKIPPING: press nothing. Every prompt runs a visible countdown
    /// (--step-secs, default 6) and skips the control when it expires; two
    /// silent prompts in a row end the run and skip everything left, so bailing
    /// out of the optional tail costs about twelve seconds. A key that already
    /// drives another control in this run is refused inline ("ALREADY TAKEN")
    /// and the prompt stays put. `Escape` cancels the whole run and is the only
    /// reserved key.
    ///
    /// NOTHING IS WRITTEN UNTIL YOU SAY SO. The run ends at a review screen: a
    /// table of what was bound, then a completeness audit (it warns when the
    /// panel can reach neither START nor BACK — on a cabinet those are the exit
    /// keys), then a yes/no. "No" discards everything. Only after "yes" does it
    /// ask whether to point a slot at it, in config.toml or — with --profile —
    /// inside a games.toml profile. It asks rather than assumes.
    ///
    /// MULTI-PLAYER: after each slot it offers the next one, so P1 through P4
    /// is one continuous run.
    ///
    /// STOP EMULATION FIRST. While `ksx run` has a panel captured, Interception
    /// suppresses its keystrokes below win32k and the wizard's Raw Input
    /// observer hears nothing at all.
    ///
    /// Exit codes: 0 = written, discarded or a dry run, 1 = error, 2 = refused
    /// (no panel identified, nothing bound, an unknown --profile).
    Setup {
        #[arg(
            long,
            default_value_t = 1,
            value_parser = clap::value_parser!(u8).range(slot_arg::range()),
            help = slot_arg::SETUP_SLOT.as_str(),
        )]
        slot: u8,
        /// Name the preset it writes (default: "Player <N>")
        #[arg(long, value_name = "NAME")]
        preset: Option<String>,
        /// Wire finished slots into this games.toml profile instead of config.toml
        #[arg(long, value_name = "TITLE")]
        profile: Option<String>,
        /// Seconds each prompt listens before skipping that control
        #[arg(long, value_name = "N", default_value_t = setup::DEFAULT_STEP_SECS)]
        step_secs: u64,
        /// Run the whole wizard and report what it WOULD write; write nothing
        #[arg(long)]
        dry_run: bool,
        /// One JSON object on stdout instead of the prose summary (prompts stay
        /// on stderr)
        #[arg(long)]
        json: bool,
    },
    /// List presets, or start a new one from an in-box template
    ///
    /// `list` shows what is in the presets folder; `list --templates` shows the
    /// layouts that ship inside the binary. `new` turns one of those templates
    /// into an ordinary preset file you own and can edit.
    ///
    /// The templates exist so that a standard panel needs NO mapping session at
    /// all (docs/MAPPER-UX.md commandment 9): an I-PAC ships MAME-ready, so ksx
    /// ships the same chart. `arcade-6button` is the two-player, six-button
    /// fighting panel; `arcade-4way` is the four-player, two-button cabinet;
    /// `keyboard-wasd` is one ordinary keyboard; `keyboard-2p` splits ONE
    /// keyboard between two players (WASD against the arrows, no encoder);
    /// `default` and `empty` are the two built-ins, copied under a name of your
    /// choosing.
    ///
    /// Multi-player templates carry a key block per player — `--player 2`
    /// writes player 2's keys, which on an I-PAC is a different set of
    /// scancodes from the SAME encoder. That is the primary topology: one
    /// board, four slots.
    ///
    /// Exit codes: 0 = listed / written / dry run, 1 = error, 2 = refused
    /// (unknown template, no such player block, or a preset of that name
    /// already exists and --force was not given); nothing written.
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
    },
    /// Which preset a slot uses: list them, or point one somewhere else
    ///
    /// The gap `ksx map` never covered. `ksx map` edits the bindings INSIDE a
    /// preset; this says which preset slot 3 points at — in config.toml, or in
    /// one games.toml profile, so the same panel can be one thing for Steam
    /// and another for MAME.
    ///
    /// `assign` writes ONE field and takes a timestamped backup of the file
    /// first. The preset has to exist; a refusal lists the ones that do and
    /// writes nothing.
    ///
    /// Wiring a DEVICE to a slot is still `ksx setup`'s job — it identifies
    /// the board by pressing it, which is the only honest way to do it and is
    /// not something a preset name can imply.
    ///
    /// Exit codes: 0 = listed / assigned, 1 = error (or the daemon refused a
    /// --reload, in which case the FILE was still written and the message says
    /// so), 2 = refused (unknown preset, unknown profile, bad slot number);
    /// nothing written.
    Slot {
        #[command(subcommand)]
        command: SlotCommand,
    },
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
        #[arg(
            long,
            default_value_t = 4,
            value_parser = clap::value_parser!(u8).range(slot_arg::range()),
            help = slot_arg::PADS_COUNT.as_str(),
        )]
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
        /// Clear pads that outlived whatever made them, instead of plugging any
        ///
        /// Restarts the ViGEmBus devnode, which drops every child pad with it.
        /// A dry run unless `--yes` is given, and refused outright while a
        /// session is running — those pads belong to whoever is playing.
        #[arg(long, conflicts_with_all = ["count", "persona", "hold_secs"])]
        prune: bool,
        /// Actually prune. Without it, `--prune` only says what it would do.
        #[arg(long, requires = "prune")]
        yes: bool,
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
    /// Bind one preset function to one panel key — or to a list of them
    /// (writes the preset TOML)
    ///
    /// The non-interactive mapping verb (docs/CONTROL-SURFACE.md): validates
    /// the preset name against the files on disk, the function name against
    /// the preset vocabulary (A B X Y start back guide lb rb lthumb rthumb,
    /// lt rt, lx/ly/rx/ry with .min/.max/.<i16>, dpad.*), and the key against
    /// the legacy key-name spelling (`ksx monitor` shows the name for any key
    /// you press) — then rewrites exactly one preset file atomically.
    ///
    /// Binding REPLACES the function's keys AND NOTHING ELSE; --clear leaves
    /// the inert "None" placeholder. The write is canonical TOML: bindings
    /// come back sorted, dotted functions as quoted literals ("dpad.up"),
    /// hand-written comments do not survive — the trade for atomic, validated
    /// writes.
    ///
    /// MANY KEYS → ONE CONTROL: repeat --key (or comma-separate it) to give a
    /// control a whole key list in ONE write — `--function A --key S --key
    /// Enter` writes A = ["S", "Enter"], and pressing EITHER fires A (the
    /// OR-chain the engine has always run). The list is written in the order
    /// given, exact duplicates dropped (`--key s --key S` is one key); the
    /// list REPLACES what the control held, so an add is "the old keys plus
    /// the new one", which is what Studio's mapper sends.
    ///
    /// MULTI-BIND: binding a key that already drives another control adds a
    /// second driver; use --move-from to take it away instead. One key driving
    /// several controls is native to the engine — press it and all of them
    /// fire together — so `--function A --key P`, then `--function B --key P`,
    /// then `--function rt --key P` leaves ALL THREE on P, and each write
    /// reports the others ("P also drives A, B"). Nothing is stolen, no flag
    /// is needed, and --force has nothing to do with it. To hand a key over
    /// instead of sharing it, name the loser: `--function A --key P
    /// --move-from B` binds A, takes P off B ONLY, and says so (B keeps the
    /// inert "None" if that was its last key).
    ///
    /// CHORDS (--when / --unless): `--function rt --key A --when B` binds
    /// "A while B is held" to RT. While the chord is active its keys are
    /// CONSUMED — A's and B's own bindings produce nothing, so the game sees
    /// RT instead of the parts, and any output they were holding is released
    /// in the same instant. Lift either key and the chord releases; whatever
    /// is still down resumes its own binding immediately. A bigger guard wins
    /// over a smaller one (A+B+C beats A+B); two guards of the same size on
    /// the same key are refused by `ksx doctor` rather than raced.
    ///
    /// THE HONEST CAVEAT: ksx does not defer input — there is no timing
    /// window, and no press is ever held back. So if a chord key is ALSO
    /// bound on its own, the game sees that individual output for the moment
    /// between the first keypress and the second. The recommended shape is
    /// chord keys with no individual binding (a spare panel button): then
    /// consumption costs nothing and adds no latency. Binding a chord over an
    /// already-bound key is allowed and does not conflict — the response names
    /// the flash instead.
    ///
    /// CROSS-SLOT CONFLICTS block by default, and they are the only ones
    /// left: a key bound in ANOTHER slot's preset, inside a games.toml profile
    /// that also uses this preset, is reported and nothing is written. --force
    /// proceeds — it means "yes, both slots should see that key", writes this
    /// preset only, and keeps naming the double binding; other presets are
    /// NEVER edited, and --force removes no binding anywhere: the only flag
    /// that unbinds anything is --move-from, which names its one victim.
    ///
    /// A running session picks the change up when Studio's mapper (or the pipe
    /// `map` verb with "reload") applies it: a binding-only edit is hot-swapped
    /// into the live engine with the pads left plugged; a structural change
    /// still restarts the session. From a shell, `ksx session reload` is the
    /// blunt equivalent.
    ///
    /// RESTORE has three destinations, and the labels say which:
    ///   defaults        the GENERIC KEYBOARD layout (S=A, D=B, A=X, W=Y,
    ///                   Q/E triggers, arrows = left stick, Esc=Start) — NOT
    ///                   "this preset as it shipped". On an arcade panel this
    ///                   replaces the I-PAC map with a desktop-keyboard map.
    ///   session-backup  the preset as it was before the daemon's first change
    ///                   this session ("undo this session").
    ///   latest-backup   the preset as it was before the most recent restore.
    /// Every restore copies the current file to
    /// "<preset>.toml.bak-YYYYMMDD-HHMMSS" first; --list-backups shows them.
    ///
    /// Exit codes: 0 = written, 1 = error, 2 = refused (unknown
    /// preset/function/key, a cross-slot conflict without --force, a
    /// --move-from that would unbind a control the write was not about, or a
    /// restore with nothing to restore; nothing written).
    Map {
        /// Preset name (the file's `name` field, e.g. "IPAC P1")
        #[arg(long, value_name = "NAME")]
        preset: String,
        /// Function to bind, e.g. A, lt, dpad.up, lx.min
        #[arg(
            long,
            value_name = "FUNCTION",
            required_unless_present_any = ["restore", "list_backups", "clear_all"]
        )]
        function: Option<String>,
        /// Key name to bind (legacy spelling, e.g. G, Enter, Left). REPEAT IT
        /// (or comma-separate) for MANY KEYS → ONE CONTROL: --key S --key
        /// Enter, or --key S,Enter, writes A = ["S", "Enter"] in one write and
        /// the control fires on either. Order is kept, duplicates dropped
        #[arg(
            long,
            value_name = "KEY",
            value_delimiter = ',',
            required_unless_present_any = ["clear", "restore", "list_backups", "clear_all"],
            conflicts_with = "clear"
        )]
        key: Vec<String>,
        /// Unbind the function (leaves the inert "None" placeholder)
        #[arg(long)]
        clear: bool,
        /// CHORD: extra keys that must ALL be held for this binding to apply
        /// (comma-separated, e.g. --when B or --when B,C). While the chord is
        /// active its keys are CONSUMED: their own bindings produce nothing.
        #[arg(
            long,
            value_name = "KEYS",
            value_delimiter = ',',
            conflicts_with_all = ["clear", "restore", "list_backups", "clear_all"]
        )]
        when: Vec<String>,
        /// CHORD: keys that must NOT be held for this binding to apply
        /// (comma-separated) — MAME's NOT
        #[arg(
            long,
            value_name = "KEYS",
            value_delimiter = ',',
            conflicts_with_all = ["clear", "restore", "list_backups", "clear_all"]
        )]
        unless: Vec<String>,
        /// TURBO: make this control auto-fire while its key is held, at N
        /// press/release cycles a second (`--turbo-hz 12`). 0 turns it off.
        ///
        /// The rate belongs to the CONTROL, not to the key: several keys on
        /// one control share ONE clock, so holding two of them fires once, not
        /// twice out of phase. It composes with a chord — the guard decides
        /// whether the control is being driven, the rate decides what it does
        /// while it is.
        ///
        /// Omitting the flag leaves an existing rate alone, so rebinding an
        /// auto-fire button does not silently switch the auto-fire off;
        /// --clear clears the rate with the keys.
        ///
        /// THE CEILING IS ARITHMETIC: one cycle is a press AND a release, a
        /// game polling at 60 Hz sees state every ~16.7 ms, and each half must
        /// be sampled to exist — so ~15 Hz is the fastest that can actually be
        /// delivered and anything above it is capped. The response says both
        /// the number you asked for and the one you get; it is never silently
        /// substituted. For a timed SEQUENCE that repeats, use a macro with
        /// `repeat = "turbo"` instead
        #[arg(
            long,
            value_name = "N",
            conflicts_with_all = ["restore", "list_backups", "clear_all"]
        )]
        turbo_hz: Option<u32>,
        /// Bind anyway when the key is already used by ANOTHER SLOT's preset
        /// (cross-slot fan-out). Removes nothing, edits no other preset — a
        /// same-preset duplicate is a multi-bind and never needs this
        #[arg(long)]
        force: bool,
        /// Take the key away from exactly this one other control of the same
        /// preset instead of sharing it with it (the explicit move; that
        /// control keeps the inert "None" if it is left with nothing)
        #[arg(
            long,
            value_name = "FUNCTION",
            requires = "key",
            conflicts_with_all = ["clear", "when", "unless", "restore", "list_backups", "clear_all"]
        )]
        move_from: Option<String>,
        /// Restore the whole preset instead of binding one function:
        /// "defaults" (the generic keyboard layout — read the note above),
        /// "session-backup" (the daemon's session-start snapshot) or
        /// "latest-backup" (undo the previous restore)
        #[arg(
            long,
            value_name = "MODE",
            value_parser = ["defaults", "session-backup", "latest-backup"],
            conflicts_with_all = ["function", "key", "clear", "force", "list_backups", "clear_all"]
        )]
        restore: Option<String>,
        /// List this preset's timestamped backups, newest first, and exit
        /// (writes nothing)
        #[arg(long, conflicts_with_all = ["function", "key", "clear", "force", "restore"])]
        list_backups: bool,
        /// Unbind EVERY function of the preset (each one stays listed as the
        /// inert "None"), after taking a timestamped backup
        #[arg(
            long,
            conflicts_with_all = ["function", "key", "clear", "force", "restore", "list_backups"]
        )]
        clear_all: bool,
        /// One JSON object on stdout: {ok, path, preset, function, key, when,
        /// unless, chord, also_drives (the other controls this key drives now
        /// — information, not an error), moved_from ({function, remaining,
        /// unbound} or null), conflicts, flash}; on a refusal {ok:false, code,
        /// error, conflicts}
        #[arg(long)]
        json: bool,
    },
    /// Write (or delete) a preset's whole [macros.<name>] table — a timed
    /// sequence — from JSON
    ///
    /// `ksx map` binds the KEY that starts a macro; this writes the SEQUENCE
    /// itself: an ordered list of steps, each holding a SET of pad functions
    /// (everything simultaneous is free — the diagonal is one step holding
    /// two) for a duration given in `ms` or 60 Hz `frames`, plus the three
    /// policies (on_release, retrigger, interrupt).
    ///
    /// THE BODY IS JSON, on stdin or --from-json FILE, in exactly the shape
    /// the preset file uses (the same serde types `ksx config export` emits —
    /// there is no second macro schema):
    ///
    ///   ksx macro --preset "IPAC P1" --name hadouken --from-json hadouken.json
    ///
    ///   { "steps": [
    ///       { "hold": ["dpad.down"],               "ms": 50 },
    ///       { "hold": ["dpad.down","dpad.right"],  "ms": 50 },
    ///       { "hold": ["dpad.right"],              "ms": 50 },
    ///       { "hold": ["A"],                       "frames": 3 } ],
    ///     "on_release": "finish",     // or "abort"
    ///     "retrigger":  "ignore",     // or "restart"
    ///     "interrupt":  "none" }      // or "any-input" / "opposing"
    ///
    /// Each step gives EXACTLY ONE of "ms" or "frames"; an empty "hold" is a
    /// deliberate neutral gap. A step shorter than ~2 poll intervals (33 ms at
    /// 60 Hz) is RAISED to it — the game cannot see anything shorter — unless
    /// the step says "allow_short": true, in which case it runs as written and
    /// may be missed. Both outcomes are reported as warnings; neither is ever
    /// silent.
    ///
    /// WHOLE-MACRO, always: the table is replaced, never patched, so what you
    /// send is what the preset holds. Bindings, chords and the preset's OTHER
    /// macros are untouched.
    ///
    /// --delete removes the table AND the `macro.<name>` trigger rows that
    /// would otherwise dangle (a trigger for a macro that no longer exists
    /// does not load). Deletion is this explicit flag and never "a body with
    /// no steps" — an empty step list is refused, so a tool that lost its
    /// draft cannot delete a macro by omission.
    ///
    /// --disable switches a macro OFF without losing it: the steps and the
    /// `macro.<name>` trigger row stay exactly where they are and nothing
    /// runs. That is what you want to TEST (isolate one macro without deleting
    /// its neighbours, and get them back unchanged) and to COMPETE (a cabinet
    /// in a tournament wants macros off, not gone). --enable puts it back. Both
    /// read no body and change nothing else. To silence a WHOLE SLOT in one
    /// edit, set `macros = "off"` on that [[slot]] in config.toml — the
    /// tournament switch, which overrides every macro's own flag.
    ///
    /// A timestamped backup ("<preset>.toml.bak-YYYYMMDD-HHMMSS") is taken
    /// before the write and named in the answer; `ksx map --list-backups`
    /// shows them and `--restore latest-backup` walks one back.
    ///
    /// Exit codes: 0 = written, 1 = error, 2 = refused (unknown preset, a body
    /// validation names — an unknown function in a hold set, no steps, a step
    /// with two duration units or none — or --delete for a macro this preset
    /// does not define; nothing written, no backup taken).
    // Verbatim so the JSON sample above keeps its line breaks: a body shape
    // reflowed into one paragraph is a shape nobody can copy.
    ///
    /// `ksx macro trace` is the other half of this verb: it MEASURES a macro
    /// instead of writing one (see its own --help).
    #[command(
        verbatim_doc_comment,
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true
    )]
    Macro {
        /// `trace` — play the macro through the real output path and report
        /// what a 60 Hz poller would have seen
        #[command(subcommand)]
        command: Option<MacroCommand>,
        /// Preset name (the file's `name` field, e.g. "IPAC P1")
        #[arg(long, value_name = "NAME", required = true)]
        preset: Option<String>,
        /// The macro's name — the [macros.<name>] table, and the second half
        /// of the `macro.<name>` function that triggers it
        #[arg(long, value_name = "NAME", required = true)]
        name: Option<String>,
        /// Read the JSON body from this file instead of stdin
        #[arg(long, value_name = "FILE", conflicts_with = "delete")]
        from_json: Option<std::path::PathBuf>,
        /// Delete the macro (and its trigger rows) instead of writing one
        #[arg(long)]
        delete: bool,
        /// Switch this macro back ON (it keeps everything; only the flag moves)
        #[arg(long, conflicts_with_all = ["delete", "from_json", "disable"])]
        enable: bool,
        /// Switch this macro OFF: it keeps its steps AND its trigger row and
        /// simply never runs. Disable to TEST (isolate one macro without
        /// deleting it) and to COMPETE (a cabinet in a tournament wants macros
        /// off, not lost). For a whole slot at once, set `macros = "off"` on
        /// its [[slot]] in config.toml
        #[arg(long, conflicts_with_all = ["delete", "from_json"])]
        disable: bool,
        /// One JSON object on stdout: {ok, path, preset, name, steps,
        /// total_ms, deleted, enabled, toggled, triggers, warnings, backup};
        /// on a refusal {ok:false, code, error, problems}
        #[arg(long)]
        json: bool,
    },
    /// Control a running `ksx daemon`: status, start, stop, reload
    ///
    /// Talks to the daemon over its named pipe (\\.\pipe\ksx-daemon) — the
    /// same control surface as the tray menu, reachable from a script, an
    /// agent, or ksx Studio. Each verb is one request; the daemon enqueues
    /// the identical command a tray click would and answers with the result.
    /// The pipe uses the default same-user ACL: whoever runs the daemon (and
    /// administrators) can drive it, nobody else.
    ///
    /// `start --game TITLE` points this and every later session at that
    /// games.toml profile; the title is validated by the daemon's normal plan
    /// resolution, and a title that does not resolve fails the start and
    /// leaves the previous profile in place. `status --json` prints the full
    /// pipe response (state, game, profiles, last/live session health).
    ///
    /// Exit codes: 0 = done, 1 = error (the daemon refused — e.g. start while
    /// already running, stop while stopped, unknown profile — or the pipe
    /// failed mid-conversation), 2 = no daemon control channel (the daemon is
    /// not running, or it predates `ksx session`).
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Export / import the configuration as JSON (TOML stays canonical)
    ///
    /// TOML is the canonical format because it carries COMMENTS, and ksx
    /// config files are annotated — the 0..12 range next to a deadzone, why a
    /// cabinet's launcher_grace_ms is 20 s, which panel a [[device]] id
    /// belongs to. Those notes are what makes a file maintainable a year
    /// later, by a person or by an AI reading it, and JSON has no syntax that
    /// could keep them.
    ///
    /// JSON exists for the readers that are not people: preset sharing,
    /// AI-generated configs (docs/ENHANCEMENTS.md E5 — this verb is what makes
    /// "write me a cabinet config" a real workflow), and anything that wants a
    /// schema. Both formats go through the SAME serde types, so they cannot
    /// drift: a field added to a config type is in both the moment it
    /// compiles.
    ///
    /// The store also READS `.json` variants (`presets\Foo.json`,
    /// `config.json`, `games.json`) when no TOML of that name is there, and
    /// writes back whatever format the file already had. With both spellings
    /// present the TOML wins and the JSON is ignored with a warning — never
    /// merged.
    ///
    /// Exit codes: 0 = exported / imported / dry-run report, 1 = error,
    /// 2 = refused (validation faults, an unreadable document, an unknown
    /// --preset; nothing written), 3 = some files were written and then a
    /// write failed.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
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
    /// Choose which physical device ksx reads — scan for boards, then pick one
    ///
    /// `ksx devices` lists devnodes, which is right for diagnosing a backend
    /// and wrong for choosing: on a cabinet it prints 29 USB interfaces, of
    /// which three are one I-PAC. These verbs group interfaces into the
    /// physical boards they belong to and name each one.
    ///
    /// `pick` and `remove` write config and NOTHING else — neither one claims
    /// or releases a board. A WinUSB claim takes a keyboard off the Windows
    /// input stack, so it stays a separate, consented act (`ksx winusb claim`).
    ///
    /// Exit codes: 0 = done, 1 = error, 2 = refused (unknown or ambiguous
    /// device, not a keyboard interface, the alias is taken, no id can name the
    /// board uniquely, or slots still use the device being removed).
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Serve the ksx Studio page on 127.0.0.1: cabinet status + session control
    ///
    /// One auto-refreshing page. The SESSION panel talks to a running `ksx
    /// daemon` over its control pipe (the same surface as `ksx session` and
    /// the tray menu): current state, a games.toml profile dropdown, and
    /// Start / Stop / Reload buttons as plain HTML forms — every button is
    /// one backend verb, no GUI-only code paths. With no daemon on the pipe
    /// the controls render disabled and say so; this command never starts a
    /// daemon or captures anything itself.
    ///
    /// Below it, the status sections re-run the same read-only collectors
    /// `ksx doctor` uses per request: driver health, the virtual pads the
    /// bus is exposing, autostart registration, the games.toml profiles.
    /// Status rows are point-in-time snapshots; session state is live from
    /// the pipe. Localhost only — there is no LAN option; that arrives with
    /// the pairing token.
    ///
    /// Exit codes: 0 = clean stop, 1 = error (bind failed, embedded UI
    /// rejected).
    #[cfg(feature = "studio")]
    Studio {
        /// TCP port on 127.0.0.1
        #[arg(long, default_value_t = 4460)]
        port: u16,
    },
    /// Open the 10-foot cabinet panel: buttons, status, start/stop, slots
    ///
    /// The surface for the MACHINE — a screen you read from six feet away and
    /// drive with the arcade panel itself. It OPERATES: it chooses among
    /// things that already exist. There is no mapper here, no macro editor and
    /// no preset file management; that is authoring, it needs a keyboard, and
    /// ksx Studio does it.
    ///
    /// Five screens: press a button and watch BOTH what the panel sent and
    /// what the pad published (the wiring check); is ksx working; start and
    /// stop; which game profile; which preset each slot uses.
    ///
    /// Normally you open this from the tray ("Open cabinet UI"), which runs it
    /// INSIDE the daemon and is the only way the live button check has
    /// anything to show. Run as its own process it still starts, stops and
    /// re-wires over the control pipe — the recovery path when the in-daemon
    /// window cannot be created.
    ///
    /// Driving it: with emulation STOPPED the panel is an ordinary keyboard
    /// and arrow keys / Enter / Esc work. With emulation RUNNING the panel
    /// produces no keystrokes at all — so the window reads the virtual pads
    /// ksx itself is publishing. Stick or D-pad moves, SOUTH confirms, EAST
    /// goes back.
    ///
    /// Exit codes: 0 = the window was closed, 1 = it could not be opened.
    #[cfg(feature = "cabinet")]
    Cabinet {
        /// Draw the same window against a scripted cabinet, touching nothing.
        /// For reviewing the 10-foot design without starting a session.
        #[arg(long)]
        demo: bool,
    },
}

#[derive(Subcommand)]
enum MacroCommand {
    /// Measure a macro: play it through the REAL output path and report what a
    /// 60 Hz poller would have seen
    ///
    /// The question a macro's correctness actually turns on is not "did ksx
    /// emit the state" — that is a unit test — but "did the state live long
    /// enough for the game to sample it". XInput hands a game a SNAPSHOT, not
    /// a queue, and an Unreal-engine game polls it once per frame, so a state
    /// shorter than one poll interval is not unreliable: it is invisible.
    ///
    /// So this plugs a pad, plays one run through the same Engine and the same
    /// VirtualPadBackend the daemon uses, timestamps every submission to the
    /// microsecond, and — separately, on its own thread — samples the published
    /// state at --sample-hz and reports the DISTINCT states that consumer saw.
    /// Two lists, and the gap between them is the finding:
    ///
    ///   SUBMITTED  what ksx handed the driver, with dwell and driver-call time
    ///   OBSERVED   what a poller at the game's rate actually read
    ///
    /// Anything in the first list and not the second was too short to survive
    /// sampling. The verdict names those explicitly, plus how many samples the
    /// diagonal (any state deflecting two perpendicular directions) got.
    ///
    /// It captures no keyboard and writes nothing. --config-dir points it at a
    /// portable root so it cannot read or disturb the installed configuration,
    /// and the pad is its own — ViGEm targets are per-process, so it runs
    /// safely beside a live daemon, though it does occupy one XInput slot while
    /// it runs.
    ///
    /// Exit codes: 0 = traced, 1 = error, 2 = refused (unknown preset or macro)
    /// or ViGEmBus missing.
    #[command(verbatim_doc_comment)]
    Trace {
        /// Preset name (the file's `name` field, e.g. "IPAC P1")
        #[arg(long, value_name = "NAME")]
        preset: String,
        /// The macro to play
        #[arg(long, value_name = "NAME")]
        name: String,
        /// Rate of the simulated consumer, in hertz. 60 is the number that
        /// matters: it is what a 60 fps game polls at
        #[arg(long, value_name = "HZ", default_value_t = 60)]
        sample_hz: u32,
        /// Configuration directory to read the preset from. Use a portable
        /// root to keep a trace away from the installed configuration
        #[arg(long, value_name = "DIR")]
        config_dir: Option<std::path::PathBuf>,
        /// Controller the traced pad presents itself as
        #[arg(long, default_value = "xbox360", value_parser = parse_persona)]
        persona: ksx_core::Persona,
        /// Trace the scheduler with no driver and no pad (mock backend). The
        /// timing is real; the driver call is not measured
        #[arg(long)]
        dry_run: bool,
        /// How long the trigger key is held, in milliseconds. A tap by default
        #[arg(long, value_name = "MS", default_value_t = 33)]
        hold_ms: u64,
        /// One JSON object on stdout: {preset, macro, sample_hz, submits[],
        /// observed[], verdict}
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Daemon state, current session summary, and the games.toml profiles
    Status {
        /// Print the raw pipe response (one JSON object) on stdout
        #[arg(long)]
        json: bool,
    },
    /// Start emulation (optionally under a games.toml profile)
    Start {
        /// Use this profile for this and every later session
        #[arg(long, value_name = "TITLE")]
        game: Option<String>,
        /// Print the raw pipe response (one JSON object) on stdout
        #[arg(long)]
        json: bool,
    },
    /// Stop the current session (the game, if any, keeps running)
    Stop {
        /// Print the raw pipe response (one JSON object) on stdout
        #[arg(long)]
        json: bool,
    },
    /// Stop, re-read the configuration from disk, start again
    Reload {
        /// Print the raw pipe response (one JSON object) on stdout
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Write config / games / presets as one JSON document (stdout by default)
    ///
    /// The document goes to --out (stdout unless told otherwise), so
    /// `ksx config export | jq` and `ksx config export --out cabinet.json`
    /// both work; the human/`--json` SUMMARY goes to stderr so stdout stays
    /// exactly the document.
    Export {
        /// What to export (default: everything, or just presets with --preset)
        #[arg(long, value_enum, value_name = "PART")]
        what: Option<ConfigPart>,
        /// Export only this preset, by its `name` field (implies --what presets)
        #[arg(long, value_name = "NAME")]
        preset: Option<String>,
        /// Destination file, or `-` for stdout (the default)
        #[arg(long, value_name = "PATH", default_value = "-")]
        out: String,
        /// One line, no indentation (the default is pretty and diffable)
        #[arg(long)]
        compact: bool,
        /// Machine-readable summary on stderr
        #[arg(long)]
        json: bool,
    },
    /// Read a JSON document back into the config root (DRY RUN unless --yes)
    ///
    /// Accepts an enveloped document (one carrying `ksx_interop`, which
    /// describes itself) or a BARE `ConfigFile` / `GamesFile` / `PresetFile` /
    /// preset array — the kind an assistant writes — in which case --what must
    /// name exactly which one it is. Nothing is guessed.
    ///
    /// The import is validated through the same checks `ksx run` and
    /// `ksx doctor` apply, against the configuration it WOULD PRODUCE (imported
    /// presets layered over the ones already on disk), and refuses on any
    /// non-advisory finding — reported structurally, nothing written. --force
    /// writes anyway.
    ///
    /// What lands on disk is CANONICAL TOML unless the target file is already
    /// a `.json`, so hand-written comments in an overwritten file do not
    /// survive: every overwritten file is copied to
    /// `<file>.bak-YYYYMMDD-HHMMSS` first.
    Import {
        /// JSON file to read, or `-` for stdin
        #[arg(value_name = "PATH")]
        path: String,
        /// Import only this part; also names a bare document's type
        #[arg(long, value_enum, value_name = "PART")]
        what: Option<ConfigPart>,
        /// Validate and report; write nothing (the default, and it wins over --yes)
        #[arg(long)]
        dry_run: bool,
        /// Actually write the files (each overwrite is backed up first)
        #[arg(long)]
        yes: bool,
        /// Write even though validation found faults (advisories never block)
        #[arg(long)]
        force: bool,
        /// One JSON object on stdout
        #[arg(long)]
        json: bool,
    },
}

/// Which files `ksx config export|import` covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum ConfigPart {
    /// config.toml (settings, devices, slots)
    Config,
    /// games.toml (launch profiles)
    Games,
    /// presets/*.toml (bindings, chords, macros)
    Presets,
    /// all three
    All,
}

impl ConfigPart {
    /// `All` becomes the empty selection, which every consumer reads as
    /// "whatever is there" — one spelling of "no narrowing", so `--what all`
    /// and no flag at all cannot behave differently.
    fn parts(self) -> Vec<ksx_config::Part> {
        match self {
            Self::Config => vec![ksx_config::Part::Config],
            Self::Games => vec![ksx_config::Part::Games],
            Self::Presets => vec![ksx_config::Part::Presets],
            Self::All => Vec::new(),
        }
    }
}

#[derive(Subcommand, Debug, PartialEq)]
enum DeviceCommand {
    /// Show the boards ksx can see, grouped as physical devices
    ///
    /// Read-only and daemon-free: it enumerates and prints. Opens nothing,
    /// claims nothing, writes nothing — looking is never a commitment.
    ///
    /// Boards with no keyboard interface are hidden by default (they cannot be
    /// picked) but always COUNTED, so nothing disappears silently; `--all`
    /// lists them, which is the answer to "ksx cannot see my board".
    Scan {
        /// Include boards with no keyboard interface
        #[arg(long)]
        all: bool,
        /// The whole DevicesView as JSON — the same shape the UI reads
        #[arg(long)]
        json: bool,
    },
    /// Write a [[device]] entry for one board — this never claims it
    ///
    /// The id written is the WEAKEST identity that still names exactly one
    /// connected interface (docs/DEVICE-IDENTITY.md §2), so moving the board to
    /// another USB socket does not break the entry. When two identical boards
    /// leave no weaker choice, the socket is pinned and the output says so; when
    /// nothing can tell them apart at all, this refuses rather than writing an
    /// id that would name whichever board enumerated first.
    ///
    /// `backend = "winusb"` is written only for an interface that is ALREADY
    /// bound to winusb.sys. Anything else gets the Interception backend and the
    /// claim command as an explicit next step — flipping a working keyboard to
    /// a backend it is not on is a config that refuses to start.
    Pick {
        /// An instance path, an existing [[device]] alias, or a unique part of
        /// one — the same argument `ksx winusb claim` takes
        query: String,
        /// The name [[slot]] entries will use; defaults to the board's name
        #[arg(long)]
        alias: Option<String>,
        /// What was written, as JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a [[device]] entry
    ///
    /// Refuses while any [[slot]] — in config.toml or in a games.toml profile —
    /// still names the alias, and lists them; `--force` removes it anyway and
    /// says what it broke.
    ///
    /// Removing the entry does NOT release a WinUSB-claimed board: the driver
    /// binding is a property of the machine, not of the config. A claimed board
    /// is removed with a warning and the `ksx winusb release` command to undo
    /// it, because the alternative is a dead panel and no config explaining why.
    Remove {
        /// The [[device]] alias to forget
        alias: String,
        /// Remove it even though slots still name it
        #[arg(long)]
        force: bool,
        /// What was removed, as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug, PartialEq)]
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

#[derive(Subcommand)]
enum PresetCommand {
    /// What presets exist — on disk, or (--templates) in the box
    ///
    /// The disk listing names every preset the store can load and the file it
    /// came from. `--templates` switches to the in-box layouts instead, with
    /// the player blocks each one carries; add `--json` for their panel notes
    /// and the exact keys they expect.
    List {
        /// List the in-box templates instead of the presets on disk
        #[arg(long)]
        templates: bool,
        /// One JSON object on stdout
        #[arg(long)]
        json: bool,
    },
    /// Write a new preset from an in-box template
    ///
    /// One atomic write, validated the same way every other preset write is.
    /// The result is an ORDINARY preset: not protected, editable by `ksx map`,
    /// by Studio's mapper or by hand.
    ///
    /// Refuses to overwrite an existing preset of the same name; --force does
    /// it anyway and copies the old file to <preset>.toml.bak-YYYYMMDD-HHMMSS
    /// first (the same backups `ksx map --list-backups` shows).
    New {
        /// Name for the new preset (also its file name)
        #[arg(value_name = "NAME")]
        name: String,
        /// Template id — `ksx preset list --templates` names them
        #[arg(long, value_name = "ID")]
        from_template: String,
        /// Which player's key block to write, for templates that carry several
        #[arg(long, value_name = "N", default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=4))]
        player: u8,
        /// Overwrite an existing preset of this name (backup taken first)
        #[arg(long)]
        force: bool,
        /// Print the TOML it would write; write nothing
        #[arg(long)]
        dry_run: bool,
        /// One JSON object on stdout
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SlotCommand {
    /// What each slot uses — read-only, and needs no daemon
    List {
        /// Read a games.toml profile's slots instead of config.toml's
        #[arg(long, value_name = "TITLE")]
        profile: Option<String>,
        /// One JSON object on stdout
        #[arg(long)]
        json: bool,
    },
    /// Point a slot at a preset
    ///
    /// One field of one entry. `ksx config export | edit | import` can do the
    /// same thing and rewrites the WHOLE file, which loses every comment in
    /// it — and ksx config files are annotated on purpose. This writes the one
    /// line, after copying the file to <file>.bak-YYYYMMDD-HHMMSS.
    ///
    /// The preset must already exist (a slot pointing at a preset that is not
    /// there is a cabinet that refuses to start, at the next boot). A refusal
    /// lists the presets that do exist and writes nothing.
    ///
    /// THE PADS REPLUG. Every other write on this control surface is a
    /// key->function table and the live engine takes it in place with the pads
    /// left plugged; this one changes what the slot IS, so --reload means the
    /// blunt stop, re-read, start. Without --reload nothing is disturbed and
    /// the next session start reads the file.
    Assign {
        #[arg(
            long,
            value_name = "N",
            value_parser = clap::value_parser!(u8).range(slot_arg::range()),
            help = slot_arg::ASSIGN_SLOT.as_str(),
        )]
        slot: u8,
        /// Preset name — `ksx preset list` names them (case-insensitive)
        #[arg(long, value_name = "NAME")]
        preset: String,
        /// Write into this games.toml profile instead of config.toml
        #[arg(long, value_name = "TITLE")]
        profile: Option<String>,
        /// Ask a running daemon to take it now: the session RESTARTS and the
        /// pads replug
        #[arg(long)]
        reload: bool,
        /// One JSON object on stdout
        #[arg(long)]
        json: bool,
    },
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
        Command::Setup {
            slot,
            preset,
            profile,
            step_secs,
            dry_run,
            json,
        } => setup::run(setup::Options {
            slot,
            preset,
            profile,
            step_secs,
            dry_run,
            json,
        }),
        Command::Preset { command } => preset_cli::run(match command {
            PresetCommand::List { templates, json } => preset_cli::Options {
                action: preset_cli::Action::List { templates },
                json,
            },
            PresetCommand::New {
                name,
                from_template,
                player,
                force,
                dry_run,
                json,
            } => preset_cli::Options {
                action: preset_cli::Action::New {
                    name,
                    from_template,
                    player,
                    force,
                    dry_run,
                },
                json,
            },
        }),
        Command::Slot { command } => slot_cli::run(match command {
            SlotCommand::List { profile, json } => slot_cli::Options {
                action: slot_cli::Action::List { profile },
                json,
            },
            SlotCommand::Assign {
                slot,
                preset,
                profile,
                reload,
                json,
            } => slot_cli::Options {
                action: slot_cli::Action::Assign {
                    slot,
                    preset,
                    profile,
                    reload,
                },
                json,
            },
        }),
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
            prune,
            yes,
        } => {
            if prune {
                pads::prune(yes, json)
            } else {
                pads::run(count, persona, hold_secs, json)
            }
        }
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
        #[cfg(feature = "studio")]
        Command::Studio { port } => studio::run(port),
        #[cfg(feature = "cabinet")]
        Command::Cabinet { demo } => {
            // The cabinet is a 10-foot panel UI, and the installer puts it on
            // the Start menu (`packaging/ksx.iss`) — so for most people this
            // process starts from a double-click, not a terminal. Without this
            // a black console window sits behind the panel for the life of the
            // session, which on a cabinet running attract mode is the whole
            // screen. Same reasoning and same call as `ksx daemon`
            // (`daemon/mod.rs`), and the notice names the log file first
            // because it is the last console output this process can produce.
            //
            // `ksx.exe` stays a console subsystem binary deliberately — see
            // `crate::console` — so `ksx cabinet` run from a terminal still
            // prints everything up to this point.
            println!("{}", console::detach_notice(crate::logging::active_path()));
            console::detach();
            if demo {
                cabinet::run_demo()
            } else {
                cabinet::run()
            }
        }
        Command::Map {
            preset,
            function,
            key,
            clear: _,
            turbo_hz,
            force,
            move_from,
            when,
            unless,
            restore,
            list_backups,
            clear_all,
            json,
        } => map::run(map::Options {
            preset,
            action: match (list_backups, clear_all, restore.as_deref()) {
                (true, _, _) => map::Action::ListBackups,
                (_, true, _) => map::Action::ClearAll,
                // clap's value_parser pins the three spellings, so a parse
                // failure here is impossible rather than merely unlikely.
                (false, false, Some(mode)) => map::Action::Restore(
                    mapping::RestoreKind::parse(mode).expect("clap validated the restore mode"),
                ),
                (false, false, None) => map::Action::Bind {
                    // clap: --function is required without --restore, and
                    // either --key (once or many times) or --clear; an EMPTY
                    // key list IS the clear.
                    function: function.expect("clap requires --function without --restore"),
                    keys: key,
                    force,
                    move_from,
                    when,
                    unless,
                    turbo_hz,
                },
            },
            json,
        }),
        Command::Macro {
            command:
                Some(MacroCommand::Trace {
                    preset,
                    name,
                    sample_hz,
                    config_dir,
                    persona,
                    dry_run,
                    hold_ms,
                    json,
                }),
            ..
        } => macro_trace::run(macro_trace::Options {
            preset,
            name,
            sample_hz,
            config_dir,
            persona,
            dry_run,
            hold_ms,
            json,
        }),
        Command::Macro {
            command: None,
            preset,
            name,
            from_json,
            delete,
            enable,
            disable,
            json,
        } => macro_cli::run(macro_cli::Options {
            // `required = true` on both, negated only by a subcommand — which
            // the arm above already took, so clap has guaranteed these are here.
            preset: preset.expect("clap enforces --preset"),
            name: name.expect("clap enforces --name"),
            from_json,
            delete,
            // clap makes the two mutually exclusive, so at most one is set.
            set_enabled: enable.then_some(true).or(disable.then_some(false)),
            json,
        }),
        Command::Session { command } => match command {
            SessionCommand::Status { json } => session::run(session::Verb::Status, json),
            SessionCommand::Start { game, json } => {
                session::run(session::Verb::Start { game }, json)
            }
            SessionCommand::Stop { json } => session::run(session::Verb::Stop, json),
            SessionCommand::Reload { json } => session::run(session::Verb::Reload, json),
        },
        Command::Config { command } => match command {
            ConfigCommand::Export {
                what,
                preset,
                out,
                compact,
                json,
            } => config_io::export(config_io::ExportOptions {
                what: what.map(ConfigPart::parts).unwrap_or_default(),
                preset,
                out: if out == "-" {
                    config_io::Destination::Stdout
                } else {
                    config_io::Destination::File(out.into())
                },
                style: if compact {
                    ksx_config::JsonStyle::Compact
                } else {
                    ksx_config::JsonStyle::Pretty
                },
                json,
            }),
            ConfigCommand::Import {
                path,
                what,
                dry_run,
                yes,
                force,
                json,
            } => config_io::import(config_io::ImportOptions {
                source: if path == "-" {
                    config_io::Origin::Stdin
                } else {
                    config_io::Origin::File(path.into())
                },
                what: what.map(ConfigPart::parts).unwrap_or_default(),
                dry_run,
                yes,
                force,
                json,
            }),
        },
        Command::Device { command } => match command {
            DeviceCommand::Scan { all, json } => device_scan::run(all, json),
            DeviceCommand::Pick { query, alias, json } => {
                device_edit::pick(device_edit::PickSpec { query, alias }, json)
            }
            DeviceCommand::Remove { alias, force, json } => {
                device_edit::remove(device_edit::RemoveSpec { alias, force }, json)
            }
        },
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

    /// The highest slot number every slot flag must ACCEPT, and the lowest it
    /// must REFUSE — read off the constant, because a test that spells the
    /// ceiling out keeps passing after the ceiling moves.
    fn max_slot() -> String {
        ksx_core::MAX_SLOTS.to_string()
    }

    /// Widened to `u16` so this stays honest if `MAX_SLOTS` ever reaches the
    /// `u8` ceiling the engine's slot index imposes.
    fn past_max_slot() -> String {
        (u16::from(ksx_core::MAX_SLOTS) + 1).to_string()
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// Every flag that names a slot ranges over `MAX_SLOTS` — and prints it.
    ///
    /// Clap refuses an out-of-range value before `main` is entered, so these
    /// ranges ARE the ceiling regardless of what ksx-core says. They were
    /// frozen at `1..=8`, and raising `MAX_SLOTS` to 16 left `ksx setup --slot
    /// 9` failing at the parser against a config file it never opened. Nothing
    /// here names a number: the assertions are against the constant, so the
    /// next raise cannot quietly reopen the gap.
    #[test]
    fn every_slot_flag_ranges_over_the_constant_and_says_so() {
        let max = max_slot();
        let past = past_max_slot();

        let sites: [(&str, &[&str], &[&str]); 3] = [
            ("setup --slot", &["ksx", "setup", "--slot"], &[]),
            ("pads --count", &["ksx", "pads", "--count"], &[]),
            (
                "slot assign --slot",
                &["ksx", "slot", "assign", "--slot"],
                &["--preset", "P"],
            ),
        ];
        for (label, prefix, suffix) in sites {
            let argv = |n: &str| -> Vec<String> {
                prefix
                    .iter()
                    .copied()
                    .chain(std::iter::once(n))
                    .chain(suffix.iter().copied())
                    .map(str::to_owned)
                    .collect()
            };
            assert!(Cli::try_parse_from(argv("0")).is_err(), "{label} took 0");
            assert!(Cli::try_parse_from(argv("1")).is_ok(), "{label} refused 1");
            assert!(
                Cli::try_parse_from(argv(&max)).is_ok(),
                "{label} refused {max} — its range is a literal, not MAX_SLOTS"
            );
            assert!(
                Cli::try_parse_from(argv(&past)).is_err(),
                "{label} took {past}, one past MAX_SLOTS"
            );
        }

        // And each one SAYS the real bound. A `--help` frozen at the old
        // number is worse than a silent one: it tells the owner of a 16-player
        // cabinet that ksx stops at eight.
        let bound = format!("1..={}", ksx_core::MAX_SLOTS);
        let mut cmd = Cli::command();
        let setup = cmd
            .find_subcommand_mut("setup")
            .unwrap()
            .render_long_help()
            .to_string();
        let pads = cmd
            .find_subcommand_mut("pads")
            .unwrap()
            .render_long_help()
            .to_string();
        let assign = cmd
            .find_subcommand_mut("slot")
            .unwrap()
            .find_subcommand_mut("assign")
            .unwrap()
            .render_long_help()
            .to_string();
        for (label, help) in [("setup", setup), ("pads", pads), ("slot assign", assign)] {
            assert!(
                help.contains(&bound),
                "{label} --help never says {bound}:\n{help}"
            );
        }
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
                prune: false,
                yes: false,
            }
        ));
    }

    /// `--prune` is a different verb wearing the same command's name, so the
    /// flags that describe pads to PLUG must be rejected beside it rather than
    /// silently ignored — and `--yes` must be meaningless without it, or a
    /// stray `--yes` on a pad test would read as consent to something.
    #[test]
    fn prune_refuses_the_flags_that_belong_to_plugging() {
        let cli = Cli::try_parse_from(["ksx", "pads", "--prune"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Pads {
                prune: true,
                yes: false,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["ksx", "pads", "--prune", "--count", "4"]).is_err());
        assert!(Cli::try_parse_from(["ksx", "pads", "--prune", "--hold-secs", "2"]).is_err());
        assert!(
            Cli::try_parse_from(["ksx", "pads", "--yes"]).is_err(),
            "--yes without --prune must not parse: it would look like consent with no subject"
        );
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
                prune: false,
                yes: false,
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

    // `--count`'s range used to be pinned here, by a test whose NAME was the
    // literal ("pads_count_range_is_1_to_8") — which is how it survived a
    // grep for `1..=8` and outlived the constant it was describing. It lives
    // in `every_slot_flag_ranges_over_the_constant_and_says_so` now, next to
    // the two other flags that share the same failure.

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
    // ksx device
    // -----------------------------------------------------------------

    #[test]
    fn device_pick_takes_a_query_and_an_optional_alias() {
        let cli = Cli::try_parse_from(["ksx", "device", "pick", "MI_00", "--alias", "panel"])
            .expect("a query and a name");
        match cli.command {
            Command::Device {
                command: DeviceCommand::Pick { query, alias, json },
            } => {
                assert_eq!(query, "MI_00");
                assert_eq!(alias.as_deref(), Some("panel"));
                assert!(!json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        // A query is not optional: `ksx device pick` with no target must not be
        // able to mean "whatever you think is best" — it writes config.
        assert!(Cli::try_parse_from(["ksx", "device", "pick"]).is_err());
        assert!(Cli::try_parse_from(["ksx", "device", "remove"]).is_err());
    }

    #[test]
    fn device_remove_needs_force_spelled_out() {
        let cli = Cli::try_parse_from(["ksx", "device", "remove", "panel", "--force"]).unwrap();
        match cli.command {
            Command::Device {
                command: DeviceCommand::Remove { alias, force, .. },
            } => {
                assert_eq!(alias, "panel");
                assert!(force);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        let plain = Cli::try_parse_from(["ksx", "device", "remove", "panel"]).unwrap();
        assert!(matches!(
            plain.command,
            Command::Device {
                command: DeviceCommand::Remove { force: false, .. }
            }
        ));
    }

    /// The one thing a user must know before running these: neither verb
    /// touches a driver binding. If `--help` did not say so, "pick" would read
    /// as "start using", and on a one-keyboard machine that expectation is a
    /// lockout waiting to happen.
    #[test]
    fn device_help_says_picking_is_not_claiming() {
        let mut cmd = Cli::command();
        let device = cmd.find_subcommand_mut("device").unwrap();
        let help = device.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "neither one claims or releases a board",
            "separate, consented act",
            "2 = refused",
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

    // -----------------------------------------------------------------
    // M10 skeleton: ksx studio (feature-gated)
    // -----------------------------------------------------------------

    #[cfg(feature = "studio")]
    #[test]
    fn studio_parses_and_defaults_to_port_4460() {
        let cli = Cli::try_parse_from(["ksx", "studio"]).unwrap();
        assert!(matches!(cli.command, Command::Studio { port: 4460 }));
        let cli = Cli::try_parse_from(["ksx", "studio", "--port", "8099"]).unwrap();
        assert!(matches!(cli.command, Command::Studio { port: 8099 }));
    }

    /// The honest promises live in `--help`: controls go through the pipe
    /// (never a parallel path), degrade visibly, and the bind stays local.
    #[cfg(feature = "studio")]
    #[test]
    fn studio_help_states_the_control_path_and_localhost_limits() {
        let mut cmd = Cli::command();
        let studio = cmd.find_subcommand_mut("studio").unwrap();
        let help = studio.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "control pipe",
            "one backend verb, no GUI-only code paths",
            "controls render disabled",
            "point-in-time",
            "Localhost only",
            "no LAN option",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    // -----------------------------------------------------------------
    // M7 slice: ksx map (the non-interactive mapping verb)
    // -----------------------------------------------------------------

    #[test]
    fn map_parses_bind_clear_and_force() {
        let cli = Cli::try_parse_from([
            "ksx",
            "map",
            "--preset",
            "IPAC P1",
            "--function",
            "A",
            "--key",
            "G",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Map {
                preset,
                function,
                key,
                clear,
                turbo_hz: _,
                force,
                move_from,
                when,
                unless,
                restore,
                list_backups,
                clear_all,
                json,
            } => {
                assert_eq!(preset, "IPAC P1");
                assert_eq!(function.as_deref(), Some("A"));
                assert_eq!(key, ["G"], "one --key is a one-key list");
                assert!(!clear && !force && json && !list_backups && !clear_all);
                assert_eq!(restore, None);
                // The default write shares a key rather than moving it.
                assert_eq!(move_from, None);
                // No guard given: an ordinary binding, byte for byte as before.
                assert!(when.is_empty() && unless.is_empty());
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        let cli = Cli::try_parse_from([
            "ksx",
            "map",
            "--preset",
            "IPAC P1",
            "--function",
            "dpad.up",
            "--clear",
            "--force",
        ])
        .unwrap();
        match cli.command {
            Command::Map {
                key, clear, force, ..
            } => {
                assert!(key.is_empty());
                assert!(clear && force);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    /// `--move-from` is the explicit move: it takes a FUNCTION, needs a
    /// `--key` to move, and belongs to a plain bind only (a clear takes
    /// nothing from anyone; a chord layers instead of taking).
    #[test]
    fn map_parses_move_from_and_keeps_it_to_a_plain_bind() {
        let cli = Cli::try_parse_from([
            "ksx",
            "map",
            "--preset",
            "IPAC P1",
            "--function",
            "A",
            "--key",
            "P",
            "--move-from",
            "B",
        ])
        .unwrap();
        match cli.command {
            Command::Map {
                move_from, force, ..
            } => {
                assert_eq!(move_from.as_deref(), Some("B"));
                assert!(!force, "moving is never a spelling of --force");
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        for junk in [
            // no key to move
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--function",
                "A",
                "--clear",
                "--move-from",
                "B",
            ],
            // a chord
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--function",
                "A",
                "--key",
                "P",
                "--when",
                "F",
                "--move-from",
                "B",
            ],
            // a whole-preset write
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--restore",
                "defaults",
                "--move-from",
                "B",
            ],
        ] {
            assert!(
                Cli::try_parse_from(junk.clone()).is_err(),
                "must not parse: {junk:?}"
            );
        }
    }

    /// `--restore` stands alone: it parses without function/key, refuses to
    /// combine with the binding flags, and only accepts the three documented
    /// modes.
    #[test]
    fn map_restore_parses_alone_and_rejects_bind_flags() {
        let cli =
            Cli::try_parse_from(["ksx", "map", "--preset", "IPAC P1", "--restore", "defaults"])
                .unwrap();
        match cli.command {
            Command::Map {
                function,
                key,
                restore,
                ..
            } => {
                assert_eq!(function, None);
                assert!(key.is_empty());
                assert_eq!(restore.as_deref(), Some("defaults"));
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        assert!(Cli::try_parse_from([
            "ksx",
            "map",
            "--preset",
            "P",
            "--restore",
            "session-backup"
        ])
        .is_ok());
        for bad in [
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--restore",
                "defaults",
                "--function",
                "A",
            ],
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--restore",
                "defaults",
                "--key",
                "G",
            ],
            vec!["ksx", "map", "--preset", "P", "--restore", "everything"],
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--restore",
                "defaults",
                "--list-backups",
            ],
        ] {
            assert!(Cli::try_parse_from(bad.clone()).is_err(), "{bad:?}");
        }
        // The third destination (FIX 2): undo the previous restore.
        assert!(
            Cli::try_parse_from(["ksx", "map", "--preset", "P", "--restore", "latest-backup"])
                .is_ok()
        );
    }

    /// `--list-backups` is the read-only twin: no function, no key, no write.
    #[test]
    fn map_list_backups_parses_alone() {
        let cli = Cli::try_parse_from([
            "ksx",
            "map",
            "--preset",
            "IPAC P1",
            "--list-backups",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Map {
                preset,
                function,
                key,
                list_backups,
                json,
                ..
            } => {
                assert_eq!(preset, "IPAC P1");
                assert_eq!(function, None);
                assert!(key.is_empty());
                assert!(list_backups && json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    /// Exactly one of --key/--clear: neither and both are parse errors, so
    /// "bind to nothing" can never be expressed by accident.
    #[test]
    fn map_requires_exactly_one_of_key_or_clear() {
        assert!(
            Cli::try_parse_from(["ksx", "map", "--preset", "P", "--function", "A"]).is_err(),
            "no key and no clear must not parse"
        );
        assert!(
            Cli::try_parse_from([
                "ksx",
                "map",
                "--preset",
                "P",
                "--function",
                "A",
                "--key",
                "G",
                "--clear",
            ])
            .is_err(),
            "key AND clear must not parse"
        );
    }

    /// The semantics a user must know before running it live in --help:
    /// replace-per-function, the conflict gate, the canonical rewrite, and
    /// the exit codes.
    #[test]
    fn map_help_documents_conflicts_rewrite_and_exit_codes() {
        let mut cmd = Cli::command();
        let map = cmd.find_subcommand_mut("map").unwrap();
        let help = map.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "REPLACES the function's keys",
            // Multi-bind: the sentence a user needs before running it, in the
            // words the brief asked for.
            "binding a key that already drives another control adds a second \
             driver; use --move-from to take it away instead",
            "leaves ALL THREE on P",
            "CROSS-SLOT CONFLICTS block by default",
            "--force removes no binding anywhere",
            "other presets are NEVER edited",
            "canonical TOML",
            "comments do not survive",
            "ksx session reload",
            "2 = refused",
            // FIX 2: --help must never let "defaults" read as "how this preset
            // shipped". It names the layout it writes, and the road back.
            "the GENERIC KEYBOARD layout",
            "NOT \"this preset as it shipped\"",
            "latest-backup",
            "bak-YYYYMMDD-HHMMSS",
            // FIX 3: the honest description of what a live session does now.
            "hot-swapped into the live engine with the pads left plugged",
            // Chords: the feature, the consumption rule, and — above all —
            // the caveat, because a zero-deferral design has one.
            "--when",
            "keys are CONSUMED",
            "ksx does not defer input",
            "no timing window",
            "chord keys with no individual binding",
            "A bigger guard wins over a smaller one",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    /// `--when`/`--unless` take a comma-separated list and belong to the BIND
    /// action only — never to a restore or a clear.
    #[test]
    fn map_parses_chord_guards() {
        let cli = Cli::try_parse_from([
            "ksx",
            "map",
            "--preset",
            "IPAC P1",
            "--function",
            "rt",
            "--key",
            "A",
            "--when",
            "B,C",
            "--unless",
            "LeftShift",
        ])
        .unwrap();
        match cli.command {
            Command::Map {
                function,
                key,
                when,
                unless,
                ..
            } => {
                assert_eq!(function.as_deref(), Some("rt"));
                assert_eq!(key, ["A"]);
                assert_eq!(when, vec!["B".to_owned(), "C".to_owned()]);
                assert_eq!(unless, vec!["LeftShift".to_owned()]);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }

        for bad in [
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--function",
                "rt",
                "--clear",
                "--when",
                "B",
            ],
            vec![
                "ksx",
                "map",
                "--preset",
                "P",
                "--restore",
                "defaults",
                "--when",
                "B",
            ],
            vec!["ksx", "map", "--preset", "P", "--clear-all", "--when", "B"],
        ] {
            assert!(
                Cli::try_parse_from(bad.clone()).is_err(),
                "must not parse: {bad:?}"
            );
        }
    }

    // -----------------------------------------------------------------
    // M10a first slice: ksx session (the pipe client verbs)
    // -----------------------------------------------------------------

    #[test]
    fn session_verbs_parse_with_json_and_game() {
        let cli = Cli::try_parse_from(["ksx", "session", "status", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Session {
                command: SessionCommand::Status { json: true }
            }
        ));
        let cli =
            Cli::try_parse_from(["ksx", "session", "start", "--game", "Street Fighter"]).unwrap();
        match cli.command {
            Command::Session {
                command: SessionCommand::Start { game, json },
            } => {
                assert_eq!(game.as_deref(), Some("Street Fighter"));
                assert!(!json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
        for verb in ["stop", "reload"] {
            assert!(
                Cli::try_parse_from(["ksx", "session", verb, "--json"]).is_ok(),
                "{verb}"
            );
        }
        // A bare `ksx session` has no default verb: controlling a daemon is
        // always an explicit ask.
        assert!(Cli::try_parse_from(["ksx", "session"]).is_err());
    }

    /// The exit-code contract is what makes these verbs scriptable; it lives
    /// in `--help` where the script author looks.
    #[test]
    fn session_help_documents_the_pipe_and_exit_codes() {
        let mut cmd = Cli::command();
        let session = cmd.find_subcommand_mut("session").unwrap();
        let help = session.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            r"\\.\pipe\ksx-daemon",
            "same control surface as the tray menu",
            "same-user ACL",
            "0 = done",
            "2 = no daemon control channel",
            "predates `ksx session`",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
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

    // ---- `ksx config export|import` (JSON interop) -----------------------

    /// The pipeable default: everything, pretty, to stdout.
    #[test]
    fn config_export_defaults_to_the_whole_root_on_stdout() {
        let cli = Cli::try_parse_from(["ksx", "config", "export"]).unwrap();
        match cli.command {
            Command::Config {
                command:
                    ConfigCommand::Export {
                        what,
                        preset,
                        out,
                        compact,
                        json,
                    },
            } => {
                assert_eq!(what, None);
                assert_eq!(preset, None);
                assert_eq!(out, "-");
                assert!(!compact);
                assert!(!json);
                // No --what means no narrowing, exactly like `--what all`.
                assert!(ConfigPart::All.parts().is_empty());
                assert_eq!(ConfigPart::Presets.parts(), vec![ksx_config::Part::Presets]);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    #[test]
    fn config_export_flags_parse() {
        let cli = Cli::try_parse_from([
            "ksx",
            "config",
            "export",
            "--what",
            "presets",
            "--preset",
            "IPAC P1",
            "--out",
            "cabinet.json",
            "--compact",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Config {
                command:
                    ConfigCommand::Export {
                        what,
                        preset,
                        out,
                        compact,
                        json,
                    },
            } => {
                assert_eq!(what, Some(ConfigPart::Presets));
                assert_eq!(preset.as_deref(), Some("IPAC P1"));
                assert_eq!(out, "cabinet.json");
                assert!(compact && json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    /// Import is a DRY RUN unless `--yes` — the `install-drivers` / `winusb`
    /// consent shape, not a third one.
    #[test]
    fn config_import_is_a_report_until_yes() {
        let cli = Cli::try_parse_from(["ksx", "config", "import", "cabinet.json"]).unwrap();
        match cli.command {
            Command::Config {
                command:
                    ConfigCommand::Import {
                        path,
                        what,
                        dry_run,
                        yes,
                        force,
                        json,
                    },
            } => {
                assert_eq!(path, "cabinet.json");
                assert_eq!(what, None);
                assert!(!dry_run, "--dry-run is the explicit spelling...");
                assert!(!yes, "...and the absence of --yes is the default");
                assert!(!force && !json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }

    // -----------------------------------------------------------------
    // M7 general availability: ksx setup + ksx preset
    // -----------------------------------------------------------------

    #[test]
    fn setup_defaults_to_slot_one_and_takes_the_wizard_flags() {
        let cli = Cli::try_parse_from(["ksx", "setup"]).unwrap();
        match cli.command {
            Command::Setup {
                slot,
                preset,
                profile,
                step_secs,
                dry_run,
                json,
            } => {
                assert_eq!(slot, 1);
                assert_eq!(preset, None);
                assert_eq!(profile, None);
                assert_eq!(step_secs, setup::DEFAULT_STEP_SECS);
                assert!(!dry_run && !json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }

        let cli = Cli::try_parse_from([
            "ksx",
            "setup",
            "--slot",
            "3",
            "--preset",
            "Cabinet",
            "--profile",
            "MAME 4P",
            "--step-secs",
            "10",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Setup {
                slot,
                preset,
                profile,
                step_secs,
                dry_run,
                json,
            } => {
                assert_eq!(slot, 3);
                assert_eq!(preset.as_deref(), Some("Cabinet"));
                assert_eq!(profile.as_deref(), Some("MAME 4P"));
                assert_eq!(step_secs, 10);
                assert!(dry_run && json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }

        // Slots are 1..=MAX_SLOTS (slots past the fourth need a PlayStation
        // persona; XInput has four). The range itself is pinned to the
        // constant by `every_slot_flag_ranges_over_the_constant_and_says_so`.
        assert!(Cli::try_parse_from(["ksx", "setup", "--slot", "0"]).is_err());
        assert!(Cli::try_parse_from(["ksx", "setup", "--slot", &past_max_slot()]).is_err());
    }

    /// The wizard's `--help` has to carry the promises the design rests on:
    /// press-to-identify, position names, the skip affordance, and the fact
    /// that nothing is written until the review screen is confirmed.
    #[test]
    fn setup_help_states_press_to_identify_and_the_transaction() {
        let mut cmd = Cli::command();
        let setup = cmd.find_subcommand_mut("setup").unwrap();
        let help = setup.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "HOLD A KEY",
            "never to pick a device from a numbered list",
            "It asks for SOUTH, not \"A\"",
            "NOTHING IS WRITTEN UNTIL YOU SAY SO",
            "ALREADY TAKEN",
            "neither START nor BACK",
            "STOP EMULATION FIRST",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    /// `ksx slot assign` takes a slot NUMBER and a preset NAME, and both are
    /// required — a verb that guessed either would rewire a cabinet by
    /// accident.
    #[test]
    fn slot_assign_requires_a_slot_and_a_preset() {
        let cli = Cli::try_parse_from([
            "ksx", "slot", "assign", "--slot", "3", "--preset", "IPAC P3",
        ])
        .unwrap();
        let Command::Slot {
            command:
                SlotCommand::Assign {
                    slot,
                    preset,
                    profile,
                    reload,
                    json,
                },
        } = cli.command
        else {
            panic!("an assign");
        };
        assert_eq!(slot, 3);
        assert_eq!(preset, "IPAC P3");
        assert_eq!(profile, None, "config.toml unless a profile is named");
        assert!(!reload, "nothing is disturbed unless asked");
        assert!(!json);

        assert!(Cli::try_parse_from(["ksx", "slot", "assign", "--slot", "3"]).is_err());
        assert!(Cli::try_parse_from(["ksx", "slot", "assign", "--preset", "P"]).is_err());
        // 1..=MAX_SLOTS, enforced by clap before anything reads a file.
        assert!(
            Cli::try_parse_from(["ksx", "slot", "assign", "--slot", "0", "--preset", "P"]).is_err()
        );
        assert!(Cli::try_parse_from([
            "ksx",
            "slot",
            "assign",
            "--slot",
            &past_max_slot(),
            "--preset",
            "P"
        ])
        .is_err());
    }

    /// The pad bounce is in `--help`, because it is the one consequence a user
    /// must not discover by watching four controllers vanish.
    #[test]
    fn slot_assign_help_says_the_pads_replug() {
        let mut cmd = Cli::command();
        let slot = cmd.find_subcommand_mut("slot").unwrap();
        // The parent verb draws the line this whole surface is built on: this
        // one says which PRESET, and `ksx setup` is still what wires a DEVICE.
        let parent = slot.render_long_help().to_string();
        let parent = parent.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(parent.contains("ksx setup"), "{parent}");
        assert!(
            parent.contains("identifies the board by pressing it"),
            "{parent}"
        );

        let assign = slot.find_subcommand_mut("assign").unwrap();
        let help = assign.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "THE PADS REPLUG",
            "loses every comment",
            "must already exist",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    #[test]
    fn slot_list_reads_config_or_one_profile() {
        let cli = Cli::try_parse_from(["ksx", "slot", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Slot {
                command: SlotCommand::List {
                    profile: None,
                    json: false
                }
            }
        ));
        let cli =
            Cli::try_parse_from(["ksx", "slot", "list", "--profile", "Steam", "--json"]).unwrap();
        let Command::Slot {
            command: SlotCommand::List { profile, json },
        } = cli.command
        else {
            panic!("a list");
        };
        assert_eq!(profile.as_deref(), Some("Steam"));
        assert!(json);
    }

    /// The cabinet's `--help` has to carry the two facts that decide whether
    /// somebody reaches for it at all: what it will NOT do, and how it is
    /// driven when the panel produces no keystrokes.
    #[cfg(feature = "cabinet")]
    #[test]
    fn cabinet_help_states_the_operate_only_rule_and_both_input_paths() {
        let mut cmd = Cli::command();
        let cabinet = cmd.find_subcommand_mut("cabinet").unwrap();
        let help = cabinet.render_long_help().to_string();
        let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
        for needle in [
            "no mapper",
            "no macro editor",
            "ksx Studio does it",
            "produces no keystrokes",
            "virtual pads",
        ] {
            assert!(flat.contains(needle), "missing '{needle}' in:\n{help}");
        }
    }

    #[test]
    fn preset_list_takes_the_templates_switch() {
        let cli = Cli::try_parse_from(["ksx", "preset", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Preset {
                command: PresetCommand::List {
                    templates: false,
                    json: false
                }
            }
        ));
        let cli = Cli::try_parse_from(["ksx", "preset", "list", "--templates", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Preset {
                command: PresetCommand::List {
                    templates: true,
                    json: true
                }
            }
        ));
    }

    #[test]
    fn preset_new_requires_a_template_and_defaults_to_player_one() {
        // The template is the whole point of the verb: no guessing a default.
        assert!(Cli::try_parse_from(["ksx", "preset", "new", "P1"]).is_err());

        let cli = Cli::try_parse_from([
            "ksx",
            "preset",
            "new",
            "P1",
            "--from-template",
            "arcade-6button",
        ])
        .unwrap();
        match cli.command {
            Command::Preset {
                command:
                    PresetCommand::New {
                        name,
                        from_template,
                        player,
                        force,
                        dry_run,
                        json,
                    },
            } => {
                assert_eq!(name, "P1");
                assert_eq!(from_template, "arcade-6button");
                assert_eq!(player, 1);
                assert!(!force && !dry_run && !json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }

        let cli = Cli::try_parse_from([
            "ksx",
            "preset",
            "new",
            "P2",
            "--from-template",
            "arcade-4way",
            "--player",
            "4",
            "--force",
        ])
        .unwrap();
        match cli.command {
            Command::Preset {
                command: PresetCommand::New { player, force, .. },
            } => {
                assert_eq!(player, 4);
                assert!(force);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }

        // Player blocks are 1..=4; no template carries more.
        assert!(Cli::try_parse_from([
            "ksx",
            "preset",
            "new",
            "P5",
            "--from-template",
            "arcade-4way",
            "--player",
            "5",
        ])
        .is_err());
    }

    #[test]
    fn config_import_reads_stdin_and_takes_a_bare_document_hint() {
        let cli = Cli::try_parse_from([
            "ksx", "config", "import", "-", "--what", "config", "--yes", "--force", "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Config {
                command:
                    ConfigCommand::Import {
                        path,
                        what,
                        yes,
                        force,
                        json,
                        ..
                    },
            } => {
                assert_eq!(path, "-");
                assert_eq!(what, Some(ConfigPart::Config));
                assert!(yes && force && json);
            }
            _ => panic!("parsed to the wrong subcommand"),
        }
    }
}
