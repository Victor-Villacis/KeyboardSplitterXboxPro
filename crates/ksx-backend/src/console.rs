//! Whether `ksx daemon` keeps the console window it was started from.
//!
//! # The problem
//!
//! `ksx.exe` is a console subsystem binary, and it has to stay one: the same
//! executable is `ksx devices`, `ksx doctor`, `ksx run --dry-run`,
//! `ksx import-legacy`. Marking it `#![windows_subsystem = "windows"]` would
//! silence every one of those — a CLI whose output goes nowhere. So the daemon
//! inherits a console window, and that window is a liability:
//!
//! - closing it kills the process, taking the tray icon and the session with it,
//!   and a black terminal beside a tray icon is *exactly* what a person clicks
//!   the X on;
//! - a Task Scheduler autostart of `ksx daemon` puts that window on the
//!   cabinet's game screen at every logon. (Note `ksx autostart --enable`
//!   registers `ksx run`, not the daemon, so this applies to a hand-registered
//!   daemon task — see the autostart module.)
//!
//! # The fix, and what used to be its cost
//!
//! `ksx daemon` (tray mode) calls `FreeConsole` once the icon is on screen:
//! after that there is no window to close and nothing to see.
//!
//! That used to cost the logs. The console is where tracing went, so from the
//! moment of detaching every `tracing::warn!`/`error!` in the process — REBOOT
//! REQUIRED, the capture watchdog, the Interception end-of-life notice, and a
//! panic message — was written to nothing, and a daemon that died left no trace
//! anywhere. [`crate::logging`] closed that: the same lines also go to a daily
//! rotating file under the config root (`docs/research/design-architecture.md`
//! §4.1), and a panic hook puts a dying daemon's last words there too. So
//! [`detach_notice`] now *names the file* instead of apologising — and still
//! says the old thing, loudly, in the one case where it is true again: no log
//! file could be opened.
//!
//! Two escape hatches, both keeping the console:
//!
//! - `--console` — debugging, and the only way to watch a daemon session live;
//! - `--headless` — **stdin is its control surface**. Detaching would close the
//!   one channel that mode has, so it is never detached, whatever else is asked.

/// What to do with the console this process inherited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleMode {
    /// Keep it. The `&'static str` is why, for `--help`-grade honesty in logs.
    Keep(&'static str),
    /// Let it go once the tray icon exists.
    Detach,
}

impl ConsoleMode {
    pub fn detaches(self) -> bool {
        matches!(self, ConsoleMode::Detach)
    }
}

/// The policy, as a pure function of the two flags.
///
/// `headless` wins over everything: its control surface *is* stdin, so a
/// detached headless daemon would be a daemon nobody can talk to — the same
/// failure the control loop treats as a reason to shut down.
pub fn mode(headless: bool, console: bool) -> ConsoleMode {
    if headless {
        ConsoleMode::Keep("--headless takes its commands on stdin")
    } else if console {
        ConsoleMode::Keep("--console was given")
    } else {
        ConsoleMode::Detach
    }
}

/// The fixed part of what is printed to the console immediately before it is
/// taken away. Use [`detach_notice`] to print it — the one fact this constant
/// cannot hold is the runtime path of the log file.
///
/// Says the things somebody staring at a vanishing window needs: how to get it
/// back, and where their logs are now. Written before the flush + detach so it
/// is guaranteed to be on screen.
pub const DETACH_NOTICE: &str = "\
ksx daemon: releasing this console. The tray icon is now the whole interface.
  * closing this window can no longer stop ksx (that is the point);
  * log output does NOT stop here: every line keeps going to the rotating log
    file below, and a panic hook puts a dying daemon's last words in it too;
  * the tray tooltip reports the RUNNING session's health, so a mid-session
    REBOOT REQUIRED or watchdog trip shows up while it is happening, not only
    after the session ends;
  * to watch a session live, start it as `ksx daemon --console`;
  * to drive it from a script instead, use `ksx daemon --headless`.";

/// The notice as it is actually printed: [`DETACH_NOTICE`] plus the one fact a
/// constant cannot carry — which file the logs are in.
///
/// `log` is `None` when [`crate::logging`] could not open one (a read-only
/// config root, no `%APPDATA%`). That is the *only* case where the pre-file-log
/// warning is still true, and then it is printed in full: a user about to lose
/// their console has to know when they are losing everything with it.
pub fn detach_notice(log: Option<&std::path::Path>) -> String {
    match log {
        Some(path) => format!(
            "{DETACH_NOTICE}\n  * this daemon's log: {} (daily rotation, {} days kept).",
            path.display(),
            crate::logging::KEEP_DAYS
        ),
        None => format!(
            // ASCII only, like the constant: the console this is printed to may
            // be a legacy code page, and a mojibake warning is a lost warning.
            "{DETACH_NOTICE}\n  * [!] NO LOG FILE could be opened, so log output really does stop \
             here, a panic message included. Start it as `ksx daemon --console`, or redirect: \
             `ksx daemon >> ksx.log 2>&1` is not detached from its file and keeps logging."
        ),
    }
}

/// Flush stdout/stderr, then detach from the console.
///
/// Returns `false` if there was nothing to detach from (already detached, or
/// started without a console — e.g. from a scheduled task with no window),
/// which is not an error: the desired end state is the same either way.
///
/// After this returns, the standard handles are pointed at `NUL` rather than
/// left dangling. A freed console's handles are invalid, and ksx writes to
/// stdout/stderr from several threads that would otherwise start failing —
/// including the supervisor, whose writes are `?`-propagated and would turn a
/// healthy session into an I/O error. `NUL` makes them succeed and go nowhere,
/// which is exactly what "detached" should mean.
#[cfg(windows)]
pub fn detach() -> bool {
    use std::io::Write as _;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        FreeConsole, SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    // Anything already buffered belongs to the console that is about to go.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    // SAFETY: no arguments; returns 0 when this process had no console.
    let freed = unsafe { FreeConsole() } != 0;

    let nul: Vec<u16> = "NUL\0".encode_utf16().collect();
    // SAFETY: NUL-terminated path, no security attributes, no template — the
    // documented form for opening the null device.
    let handle = unsafe {
        CreateFileW(
            nul.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    // Only when a console actually went away. If `FreeConsole` returned false
    // there was nothing to free — the daemon was started with its output
    // redirected (`ksx daemon >> ksx.log 2>&1`, a scheduled task with no
    // window, a pipe) — and those handles are perfectly good files. Pointing
    // them at NUL there would silently truncate a cabinet's log to whatever was
    // written before this call, which is the opposite of what a detached
    // daemon needs. The motivation for NUL is a *freed console's* dangling
    // handles, so the fix is scoped to exactly that case.
    if freed && !handle.is_null() && handle != INVALID_HANDLE_VALUE {
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `handle` is a live file handle; SetStdHandle only stores it.
            unsafe { SetStdHandle(id, handle) };
        }
    }
    freed
}

#[cfg(not(windows))]
pub fn detach() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The whole policy table, including the combination that matters most:
    /// `--headless` must keep its console even though nothing asked it to.
    #[test]
    fn headless_always_keeps_the_console_and_plain_daemon_never_does() {
        assert_eq!(mode(false, false), ConsoleMode::Detach);
        assert!(mode(false, false).detaches());

        for console in [false, true] {
            let m = mode(true, console);
            assert!(
                !m.detaches(),
                "--headless (console={console}) must keep stdin: {m:?}"
            );
            match m {
                ConsoleMode::Keep(why) => assert!(why.contains("stdin"), "{why}"),
                ConsoleMode::Detach => unreachable!(),
            }
        }

        match mode(false, true) {
            ConsoleMode::Keep(why) => assert!(why.contains("--console"), "{why}"),
            ConsoleMode::Detach => panic!("--console must keep the console"),
        }
    }

    /// The notice is the only place the way back is written down, and the only
    /// warning a user gets about what happens to their output. Both are
    /// load-bearing.
    #[test]
    fn the_notice_states_both_ways_to_keep_the_console() {
        assert!(
            DETACH_NOTICE.contains("ksx daemon --console"),
            "{DETACH_NOTICE}"
        );
        assert!(DETACH_NOTICE.contains("--headless"), "{DETACH_NOTICE}");
        assert!(
            DETACH_NOTICE.contains("closing this window can no longer stop ksx"),
            "{DETACH_NOTICE}"
        );
        // It has to be printable before the console dies: no trailing newline
        // dependency, no ANSI, nothing that needs a terminal to render.
        assert!(
            DETACH_NOTICE.is_ascii(),
            "the console may be a legacy code page"
        );
        assert!(detach_notice(None).is_ascii(), "{}", detach_notice(None));
    }

    /// **The notice must not promise the opposite of what ksx now does.** For
    /// the whole of M5–M6 it said logging stopped at the detach and that health
    /// was visible only after a session ended; both are now false, and a notice
    /// that tells a user their logs are gone is a user who never looks for the
    /// file that has the answer in it.
    #[test]
    fn the_notice_points_at_the_log_file_and_the_live_tooltip() {
        let notice = detach_notice(Some(Path::new(r"C:\cfg\ksx\logs\ksx.2026-08-04.log")));
        assert!(notice.contains("log output does NOT stop here"), "{notice}");
        assert!(
            notice.contains(r"C:\cfg\ksx\logs\ksx.2026-08-04.log"),
            "the user must be told where to look: {notice}"
        );
        assert!(
            notice.contains("14 days kept"),
            "the retention bound is stated, not implied: {notice}"
        );
        assert!(
            notice.contains("panic"),
            "the panic hook is the reason the file is worth mentioning: {notice}"
        );
        assert!(
            notice.contains("RUNNING session's health"),
            "the mid-session gap is closed, so the notice must stop claiming it: {notice}"
        );
        for gone in [
            "log output stops here",
            "leaves no trace",
            "visible nowhere",
            "only AFTER a session ends",
        ] {
            assert!(
                !notice.contains(gone),
                "the pre-M6.5 claim '{gone}' must be gone:\n{notice}"
            );
        }
    }

    /// ...and when there really is no log file, the old warning is the truth
    /// again and has to be printed in full.
    #[test]
    fn with_no_log_file_the_notice_says_output_really_does_stop() {
        let notice = detach_notice(None);
        assert!(notice.contains("NO LOG FILE"), "{notice}");
        assert!(notice.contains("really does stop"), "{notice}");
        assert!(
            notice.contains("ksx daemon >> ksx.log 2>&1"),
            "the redirect escape hatch matters most when there is no file: {notice}"
        );
    }
}
