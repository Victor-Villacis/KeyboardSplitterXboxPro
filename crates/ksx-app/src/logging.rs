//! Where ksx's log lines go — including the last one it ever writes.
//!
//! # The problem this exists for
//!
//! Until now every `tracing` line in the workspace went to **stderr and nowhere
//! else**. That is fine for `ksx devices` in a terminal and useless for the
//! thing ksx actually is: a daemon on an arcade cabinet, started by a scheduled
//! task, that releases its console the moment the tray icon appears
//! (`crate::console`). From that instant stderr is `NUL`. A daemon that died at
//! 3am — Interception slot exhaustion, a wedged consumer, a panic — left
//! **nothing behind at all**, and the three messages a user most needs
//! ("REBOOT REQUIRED", "event consumer stalled — forcing passthrough", the
//! Interception end-of-life notice) were written to a black hole.
//!
//! So: a daily-rolling file sink alongside stderr, per
//! `docs/research/design-architecture.md` §4.1, for **every** command — a
//! `ksx run` on the cabinet should leave a log too, not just the daemon.
//!
//! # Four properties, each of which is load-bearing
//!
//! - **stdout is never touched.** `--json` promises exactly one object on
//!   stdout, and one log line landing there corrupts it for every automated
//!   caller. Both sinks here are stderr and a file; nothing writes to stdout.
//! - **The writer is non-blocking, and the guard outlives the process.**
//!   `tracing-appender`'s classic footgun is `let _ = non_blocking(..)`, which
//!   drops the [`WorkerGuard`] immediately and silently stops logging forever.
//!   The guard goes into [`GUARD`], a `static` that is never dropped, so it
//!   cannot be lost by a refactor that changes what `main` holds.
//! - **It is bounded on disk.** [`KEEP_DAYS`] days, pruned when the appender
//!   opens and at every rollover. A cabinet must not fill its system drive with
//!   its own diagnostics.
//! - **A panic reaches the file.** [`install_panic_hook`] logs the panic
//!   *before* the unwind, which is the whole point: the one event that explains
//!   why the daemon is gone is the one the default hook writes to a stderr that
//!   does not exist.
//!
//! # Portable mode
//!
//! The directory comes from [`ksx_config::ConfigRoot::logs_dir`], so a portable
//! install (a `ksx.toml` next to the exe — arcade cabs love those) logs beside
//! the exe and an installed one logs to `%APPDATA%\ksx\logs\`. One rule, no
//! second discovery path to disagree with the first.
//!
//! # Two ksx processes, one file
//!
//! `ksx devices` run while a daemon is resident writes to the same file: the
//! appender opens it with `append`, and each event is one `write_all`, so the
//! lines interleave but do not corrupt each other. That is the right trade for
//! a cabinet — a single chronological file is what somebody debugging one
//! actually wants, and per-process files would leave the interesting one
//! unnamed.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context as _;
use ksx_config::ConfigRoot;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// Default log level when `RUST_LOG` says nothing.
pub const DEFAULT_LOG: &str = "info";

/// Filename parts: `ksx.<YYYY-MM-DD>.log`.
pub const LOG_PREFIX: &str = "ksx";
/// See [`LOG_PREFIX`].
pub const LOG_SUFFIX: &str = "log";

/// Days of history kept in the log directory.
///
/// Two weeks is chosen against the failure it has to survive: "the cabinet has
/// been doing something odd since about last weekend". Anything shorter loses
/// the evidence before the owner gets round to looking; anything longer is
/// unbounded growth in disguise. At ksx's line rate a day is kilobytes, so this
/// is a bound in principle rather than a squeeze.
pub const KEEP_DAYS: usize = 14;

/// How long the panic hook lets the writer thread drain before it hands control
/// back to the unwind. See [`install_panic_hook`].
const PANIC_DRAIN_MS: u64 = 100;

/// The non-blocking writer's guard.
///
/// A `static` on purpose. Dropping a [`WorkerGuard`] shuts the writer thread
/// down, and every real-world instance of "tracing-appender silently stopped
/// logging" is a guard that went out of scope. Nothing in this process can drop
/// this one.
static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// The file [`init`] opened, for anything that has to *tell* a user where the
/// log is (the startup banner, `crate::console::detach_notice`).
static ACTIVE_LOG: OnceLock<PathBuf> = OnceLock::new();

/// Where this process's log is going, so the caller can say so out loud.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogSink {
    /// stderr **and** this file.
    File(PathBuf),
    /// stderr only, for this reason. Not an error — `ksx --version` on a
    /// machine with no writable config root must still run.
    StderrOnly(String),
}

/// The file [`init`] is writing to, if it opened one.
pub fn active_path() -> Option<&'static Path> {
    ACTIVE_LOG.get().map(PathBuf::as_path)
}

/// Build the log filter: `RUST_LOG` if it parses, `info` otherwise.
///
/// Pure so the policy is testable without touching the process environment or
/// the global subscriber.
pub fn tracing_filter(rust_log: Option<&str>) -> tracing_subscriber::EnvFilter {
    use tracing_subscriber::EnvFilter;
    match rust_log {
        Some(spec) if !spec.trim().is_empty() => {
            EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG))
        }
        _ => EnvFilter::new(DEFAULT_LOG),
    }
}

/// Install the process-wide tracing subscriber: stderr, plus a daily-rolling
/// file under `root` when one can be opened.
///
/// Without a subscriber every `tracing::warn!`/`error!` in the workspace is a
/// silent no-op — including "REBOOT REQUIRED" (Interception slot exhaustion),
/// "event consumer stalled — forcing passthrough" (the watchdog), and the
/// Interception end-of-life / CI-policy warning. Those are the three messages a
/// user most needs to see, so this is not optional plumbing.
///
/// `root` is `None` when the config root could not be discovered (no
/// `%APPDATA%`, no exe directory) and in tests that must not touch the disk.
/// Logging degrades to stderr; it never fails the command.
pub fn init(root: Option<&ConfigRoot>) -> LogSink {
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let opened = match root {
        Some(root) => open_sink(root).map_err(|err| format!("{err:#}")),
        None => Err("no config root could be discovered (is %APPDATA% set?)".to_owned()),
    };

    let (file_layer, outcome) = match opened {
        Ok((appender, path)) => {
            // Lossy on purpose (the default): when the buffer is full the line
            // is dropped rather than blocking the caller. ksx logs from threads
            // that are translating keystrokes, and a logger that can stall the
            // input path is a worse bug than a missing diagnostic line.
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                // A log file is read with `type`/an editor, not a terminal
                // emulator: escape codes in it are noise at best.
                .with_ansi(false)
                .with_writer(writer);
            (Some(layer), Ok((guard, path)))
        }
        Err(why) => (None, Err(why)),
    };

    let installed = tracing_subscriber::registry()
        .with(tracing_filter(std::env::var("RUST_LOG").ok().as_deref()))
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        // `try_init` (not `init`): a second call — e.g. from a test binary that
        // already installed one — must not abort the program.
        .try_init()
        .is_ok();

    match (installed, outcome) {
        (true, Ok((guard, path))) => {
            // The guard must outlive everything. See GUARD.
            let _ = GUARD.set(guard);
            let _ = ACTIVE_LOG.set(path.clone());
            install_panic_hook();
            LogSink::File(path)
        }
        (true, Err(why)) => {
            // Still worth a panic hook: it reaches stderr, which is all there
            // is, and `--console` users are exactly who runs without a file.
            install_panic_hook();
            LogSink::StderrOnly(why)
        }
        // Somebody else owns the subscriber. Dropping `outcome` here shuts the
        // writer thread down, which is right: it is wired to nothing.
        (false, _) => LogSink::StderrOnly("a tracing subscriber was already installed".to_owned()),
    }
}

/// Say where the log is, on stderr, at startup.
///
/// `tracing::info!` rather than `println!` for two reasons: stdout belongs to
/// `--json`, and the line then lands in the log file as its own first entry —
/// so a file found later says which build wrote it and when it started.
pub fn announce(sink: &LogSink) {
    match sink {
        LogSink::File(path) => tracing::info!(
            "ksx {} — logging to {} (daily rotation, {KEEP_DAYS} days kept)",
            env!("CARGO_PKG_VERSION"),
            path.display()
        ),
        LogSink::StderrOnly(why) => {
            tracing::warn!("no log file ({why}); this session's output exists only on this console")
        }
    }
}

/// Open the rolling file sink under `root`, creating `<root>\logs\` if needed.
///
/// Returns the appender and the path it is writing to *right now*; the name
/// changes at the next UTC midnight, which is what daily rotation means.
pub fn open_sink(root: &ConfigRoot) -> anyhow::Result<(RollingFileAppender, PathBuf)> {
    let dir = root.logs_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating the log directory '{}'", dir.display()))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix(LOG_SUFFIX)
        // The disk bound. The appender prunes when it opens *and* at every
        // rollover, so a cabinet that runs ksx once a month is trimmed on the
        // way in rather than accumulating until someone notices.
        .max_log_files(KEEP_DAYS)
        .build(&dir)
        .map_err(|err| anyhow::anyhow!("opening the rolling log in '{}': {err}", dir.display()))?;
    Ok((appender, current_file(&dir)))
}

/// The file a [`Rotation::DAILY`] appender in `dir` is writing to at this
/// moment.
pub fn current_file(dir: &Path) -> PathBuf {
    dir.join(file_name(&utc_today()))
}

/// `ksx.<date>.log` — the name `tracing-appender` builds from our prefix and
/// suffix.
pub fn file_name(date: &str) -> String {
    format!("{LOG_PREFIX}.{date}.{LOG_SUFFIX}")
}

fn utc_today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    utc_date(secs)
}

/// `YYYY-MM-DD` in **UTC** for a Unix timestamp.
///
/// This is the exact string `tracing-appender` puts in a daily filename (it
/// stamps names from `OffsetDateTime::now_utc`), worked out here so ksx can
/// *name the file it is writing* without adding a date crate to a keyboard
/// driver front-end. The algorithm is Howard Hinnant's `civil_from_days`, the
/// standard integer form; `the_computed_path_is_the_file_the_appender_opens`
/// checks it against the appender rather than against my arithmetic.
pub fn utc_date(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    // Shift the epoch to 0000-03-01 so a leap day lands at the end of the year.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// Install a panic hook that logs the panic **before** the unwind starts.
///
/// This is the single most important line the file will ever hold. Without it,
/// a `ksx daemon` that panics after releasing its console writes its last words
/// with the default hook — to stderr, which by then is `NUL` — and the process
/// simply is not there any more, with no record of why.
///
/// The previous hook is chained, not replaced: whoever still has a console
/// keeps the standard message and `RUST_BACKTRACE` output.
pub fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>").to_owned();
        tracing::error!(
            "{}",
            panic_record(
                &name,
                info.location().map(std::string::ToString::to_string),
                &payload_of(info)
            )
        );
        previous(info);
        // The file itself is written unbuffered, but it is written by the
        // *writer thread*, and a panicking process can be torn down before that
        // thread is next scheduled. This is a drain window, not a flush: the
        // guard is deliberately never dropped (see GUARD), so there is nothing
        // to flush synchronously. 100 ms is thousands of lines' worth of
        // draining and is only ever paid by a process that is already in
        // trouble.
        //
        // Conditional on there being a file at all, and that is not an
        // optimisation: with stderr as the only sink the write already happened
        // synchronously on this thread, so the wait would buy nothing and would
        // slow down every caught panic in the process (`ksx run` treats a
        // panicking output thread as a recoverable teardown, and its tests
        // panic threads on purpose).
        if ACTIVE_LOG.get().is_some() {
            std::thread::sleep(Duration::from_millis(PANIC_DRAIN_MS));
        }
    }));
}

/// The line a panic writes to the log.
///
/// Pure so the wording is testable without panicking a process — and the
/// wording matters: `PANIC` is what somebody greps a cabinet's log for.
pub fn panic_record(thread: &str, location: Option<String>, payload: &str) -> String {
    let where_ = location.unwrap_or_else(|| "<unknown location>".to_owned());
    format!("PANIC in thread '{thread}' at {where_}: {payload}")
}

fn payload_of(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A directory that deletes itself. `ksx-config`'s equivalent is private to
    /// that crate, and this is eight lines.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let dir =
                std::env::temp_dir().join(format!("ksx-log-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // -----------------------------------------------------------------
    // Where the file lands
    // -----------------------------------------------------------------

    /// The log directory is the config root's, so a portable install (a
    /// `ksx.toml` next to the exe — how a cabinet is deployed) logs beside the
    /// exe instead of into the profile of whichever account the scheduled task
    /// happens to run as.
    #[test]
    fn the_log_directory_follows_the_config_root_including_portable_mode() {
        let installed = ConfigRoot::at(r"C:\cfg\ksx");
        assert_eq!(installed.logs_dir(), Path::new(r"C:\cfg\ksx").join("logs"));
        let portable = ConfigRoot::portable(r"D:\cab");
        assert_eq!(portable.logs_dir(), Path::new(r"D:\cab").join("logs"));
        assert!(portable.is_portable());
    }

    /// Opening the sink creates the directory (a fresh install has no `logs\`)
    /// and writes to the path we advertise. This is also the cross-check on
    /// [`utc_date`]: if our date arithmetic disagreed with the appender's by so
    /// much as a day, the advertised path would name a file that does not
    /// exist — and the startup banner would be pointing users at nothing.
    #[test]
    fn the_computed_path_is_the_file_the_appender_opens() {
        let tmp = TempDir::new("sink");
        let root = ConfigRoot::at(tmp.path());
        assert!(!root.logs_dir().exists(), "precondition: no logs dir yet");

        let (mut appender, path) = open_sink(&root).unwrap();
        assert!(
            root.logs_dir().is_dir(),
            "the log directory must be created"
        );
        assert_eq!(path.parent(), Some(root.logs_dir().as_path()));

        writeln!(appender, "hello from the sink test").unwrap();
        appender.flush().unwrap();

        let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("nothing at the advertised path {}: {err}", path.display())
        });
        assert!(text.contains("hello from the sink test"), "{text}");

        // ...and it is the only file there, i.e. the appender did not open some
        // *other* name that we then failed to notice.
        let names: Vec<String> = std::fs::read_dir(root.logs_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn the_file_name_is_the_documented_shape() {
        assert_eq!(file_name("2026-08-04"), "ksx.2026-08-04.log");
        assert!(file_name(&utc_date(0)).starts_with(LOG_PREFIX));
        assert!(file_name(&utc_date(0)).ends_with(LOG_SUFFIX));
    }

    #[test]
    fn utc_date_handles_epochs_leap_days_and_century_rules() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(86_399), "1970-01-01");
        assert_eq!(utc_date(86_400), "1970-01-02");
        // 2000 is a leap year (divisible by 400) — the rule most hand-rolled
        // calendars get wrong.
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(951_868_800), "2000-03-01");
        assert_eq!(utc_date(1_709_164_800), "2024-02-29");
        assert_eq!(utc_date(1_000_000_000), "2001-09-09");
        // Before the epoch: div_euclid, not `/`, or the date walks backwards.
        assert_eq!(utc_date(-1), "1969-12-31");
    }

    // -----------------------------------------------------------------
    // The disk bound
    // -----------------------------------------------------------------

    /// A cabinet must not fill its system drive with its own diagnostics.
    /// `max_log_files` prunes when the appender opens, so this is checked the
    /// way it will actually happen: a directory full of history from previous
    /// days, and a ksx that starts up.
    #[test]
    fn opening_the_sink_prunes_history_down_to_keep_days() {
        let tmp = TempDir::new("prune");
        let root = ConfigRoot::at(tmp.path());
        let dir = root.logs_dir();
        std::fs::create_dir_all(&dir).unwrap();

        // Twice the budget, in the appender's own naming (2025-01-01 onwards),
        // plus one file that is not ours and must survive untouched.
        for day in 1..=(KEEP_DAYS * 2) {
            let name = file_name(&utc_date(1_735_689_600 + (day as i64) * 86_400));
            std::fs::write(dir.join(name), b"old\n").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"not a log\n").unwrap();

        let (mut appender, path) = open_sink(&root).unwrap();
        writeln!(appender, "today").unwrap();
        appender.flush().unwrap();

        let logs: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(LOG_PREFIX) && n.ends_with(LOG_SUFFIX))
            .collect();
        assert_eq!(
            logs.len(),
            KEEP_DAYS,
            "the log directory must be bounded at {KEEP_DAYS} files, got {logs:?}"
        );
        assert!(
            logs.contains(&file_name(&utc_today())),
            "today's file must be one of the survivors: {logs:?}"
        );
        assert!(path.is_file());
        assert!(
            dir.join("notes.txt").is_file(),
            "pruning must only touch ksx's own log files"
        );
    }

    /// The bound has to be *told to the user*, not just enforced: "how far back
    /// does this go" is the first question anyone asks of a log directory.
    #[test]
    fn the_retention_bound_is_stated_where_a_user_will_read_it() {
        let notice = crate::console::detach_notice(Some(Path::new(r"C:\logs\ksx.log")));
        assert!(
            notice.contains(&format!("{KEEP_DAYS} days kept")),
            "{notice}"
        );
    }

    // -----------------------------------------------------------------
    // Filter policy
    // -----------------------------------------------------------------

    #[test]
    fn tracing_filter_defaults_to_info_and_honors_rust_log() {
        assert_eq!(tracing_filter(None).to_string(), DEFAULT_LOG);
        assert_eq!(tracing_filter(Some("")).to_string(), DEFAULT_LOG);
        assert_eq!(tracing_filter(Some("  ")).to_string(), DEFAULT_LOG);
        assert_eq!(
            tracing_filter(Some("ksx_capture=trace")).to_string(),
            "ksx_capture=trace"
        );
        // A malformed RUST_LOG must not silence the app.
        assert_eq!(tracing_filter(Some("=========")).to_string(), DEFAULT_LOG);
    }

    /// A subscriber must actually be installed: without one every
    /// `tracing::warn!`/`error!` in the workspace is a no-op, including
    /// "REBOOT REQUIRED" and the consumer-stall watchdog.
    ///
    /// `None` deliberately: this test must not write into the developer's real
    /// `%APPDATA%\ksx\logs\`.
    #[test]
    fn a_subscriber_is_installed_at_info() {
        use tracing::level_filters::LevelFilter;
        // Read the ambient RUST_LOG *before* installing: a developer or CI job
        // running the gate with `RUST_LOG=warn` set legitimately gets a lower
        // level, and that must not read as "no subscriber".
        let configured = std::env::var("RUST_LOG").ok();
        let sink = init(None);
        assert!(
            matches!(sink, LogSink::StderrOnly(_)),
            "no root means no file: {sink:?}"
        );
        assert_ne!(
            LevelFilter::current(),
            LevelFilter::OFF,
            "no tracing subscriber is active, so every warn!/error! is silently dropped"
        );
        if configured.is_none() {
            assert!(
                LevelFilter::current() >= LevelFilter::INFO,
                "with no RUST_LOG the default must be info, got {}",
                LevelFilter::current()
            );
        }
        // Announcing must not panic in either shape.
        announce(&sink);
        announce(&LogSink::File(PathBuf::from(r"C:\cfg\ksx\logs\ksx.log")));
    }

    // -----------------------------------------------------------------
    // The panic hook
    // -----------------------------------------------------------------

    #[test]
    fn a_panic_record_names_the_thread_the_place_and_the_message() {
        let line = panic_record(
            "ksx-session",
            Some("src/run/supervisor.rs:812:5".to_owned()),
            "the pad backend vanished",
        );
        assert!(line.starts_with("PANIC"), "{line}");
        assert!(line.contains("ksx-session"), "{line}");
        assert!(line.contains("src/run/supervisor.rs:812:5"), "{line}");
        assert!(line.contains("the pad backend vanished"), "{line}");
        // A panic with no location must still produce a usable line.
        let line = panic_record("main", None, "boom");
        assert!(line.contains("<unknown location>"), "{line}");
        assert!(line.contains("boom"), "{line}");
    }

    /// Environment variable that turns [`the_child_that_panics`] from a no-op
    /// into the real thing, and tells it where to log.
    const CHILD_ROOT: &str = "KSX_TEST_PANIC_LOG_ROOT";

    /// **The whole point of the panic hook**, end to end and for real: a
    /// process that installs logging at an injected root and then panics must
    /// leave the panic *in the file*.
    ///
    /// It has to be a child process. The subscriber and the panic hook are both
    /// process-global and installed once, and a panic that reaches the log has
    /// to be a panic that was not caught — neither is expressible inside a test
    /// that shares its process with 600 others. So this test re-runs **this
    /// test binary**, naming the ignored test below, with the log root injected
    /// through the environment, and then reads what it left behind.
    #[test]
    fn a_panic_reaches_the_log_file() {
        // Guard against recursion: inside the child, do nothing.
        if std::env::var(CHILD_ROOT).is_ok() {
            return;
        }
        let tmp = TempDir::new("panic");
        let exe = std::env::current_exe().expect("the test binary");
        let output = std::process::Command::new(exe)
            .args([
                "--exact",
                "logging::tests::the_child_that_panics",
                "--ignored",
                "--nocapture",
                // One thread, so nothing else in this binary runs alongside a
                // process that is deliberately dying.
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, tmp.path())
            .output()
            .expect("spawning the child test");

        assert!(
            !output.status.success(),
            "the child was supposed to panic; stdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );

        let dir = ConfigRoot::at(tmp.path()).logs_dir();
        let path = current_file(&dir);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "a panicking daemon left no log at {}: {err}\nchild stdout:\n{}",
                path.display(),
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert!(
            text.contains("PANIC"),
            "the panic must be greppable in the file:\n{text}"
        );
        assert!(
            text.contains("the cabinet daemon fell over"),
            "the panic *message* must be in the file, not just the fact of one:\n{text}"
        );
        assert!(
            text.contains("logging.rs"),
            "the panic's location must be in the file:\n{text}"
        );
        assert!(
            text.contains("ERROR"),
            "a panic is not an info-level event:\n{text}"
        );
    }

    /// The child half of [`a_panic_reaches_the_log_file`]. Ignored so it only
    /// ever runs when that test names it explicitly.
    #[test]
    #[ignore = "spawned as a child process by a_panic_reaches_the_log_file"]
    fn the_child_that_panics() {
        let Ok(dir) = std::env::var(CHILD_ROOT) else {
            // Run directly by a human with `--ignored`: do nothing rather than
            // panic in their terminal.
            return;
        };
        let sink = init(Some(&ConfigRoot::at(dir)));
        assert!(matches!(sink, LogSink::File(_)), "{sink:?}");
        announce(&sink);
        panic!("the cabinet daemon fell over");
    }
}
