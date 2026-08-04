//! Process inspection and shell activation — the OS half of game launching.
//!
//! `ksx-games` owns the *policy* (when to launch, when a launcher counts as a
//! hand-off, when a session is over); this module owns the two Win32 calls that
//! policy needs and cannot be expressed with `std`:
//!
//! - [`snapshot`] — one `CreateToolhelp32Snapshot` pass, so "is `mame.exe`
//!   still running?" is answerable for a process ksx never spawned. Required
//!   for Steam/Epic/launcher hand-offs, where the thing we started is gone and
//!   the thing we care about was started by somebody else.
//! - [`shell_open`] — `ShellExecuteW("open", …)` for `steam://rungameid/NNN`
//!   and other protocol targets. `Command::new("steam://…")` cannot work:
//!   there is no such executable, the URL is resolved by the shell's protocol
//!   registration.
//!
//! Everything here is off the input hot path by construction — it is polled at
//! a few hertz from the supervisor thread.
//!
//! - [`launch`] — start an executable and keep a **process handle**, so
//!   liveness is answered without ever consulting a pid (pids are reused;
//!   handles are not).
//!
//! Off Windows every function degrades to a documented no-op/`Unsupported`
//! rather than failing to compile, so `ksx-games`' policy layer keeps its
//! cross-platform unit tests.
//!
//! # No-kill policy (deliberate, and enforced by a test)
//!
//! There is no `kill`, `terminate` or `stop` primitive in this module and there
//! will not be one. ksx starts the user's game as a convenience; it does not
//! own it. `TerminateProcess` gives a game no chance to flush a save, and the
//! one thing worse than a cabinet that will not stop cleanly is a cabinet that
//! eats a two-hour campaign because emulation wanted to shut down. When ksx
//! stops first, the game keeps running with a plain keyboard — which is exactly
//! what the legacy app did, and what a player expects.
//!
//! [`GameProcess`] therefore exposes only `is_alive` and `wait_timeout`.
//! `std::process::Child::kill` is reachable through the inner handle by
//! construction; `into_inner` does not exist, the field is private, and
//! `tests::no_kill_primitive_exists` reads this file to keep it that way.

use std::path::Path;
use std::time::Duration;

/// One live process, as the OS snapshot reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    /// Parent pid as recorded at snapshot time. Unreliable by nature (the
    /// parent may already be gone and the id reused), so ksx uses it only as a
    /// *tie-breaker*, never as the identity of a tracked game.
    pub parent_pid: u32,
    /// Image name only (`mame.exe`), never a full path — that is all
    /// `PROCESSENTRY32W` carries.
    pub name: String,
}

impl ProcessEntry {
    /// Windows process names are case-insensitive; profiles must not have to
    /// match the on-disk casing.
    pub fn name_matches(&self, wanted: &str) -> bool {
        self.name.eq_ignore_ascii_case(wanted)
    }
}

/// Enumerate every process visible to this token.
///
/// Returns an empty vector (never an error) when the snapshot cannot be taken:
/// a failed enumeration must read as "nothing matched yet", so a transient
/// failure cannot be mistaken for "the game exited". The caller's grace window
/// is what turns a *persistent* failure into a decision.
#[cfg(windows)]
pub fn snapshot() -> Vec<ProcessEntry> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    // SAFETY: TH32CS_SNAPPROCESS with pid 0 is the documented "all processes"
    // form; the returned handle is closed on every path below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() || snap == INVALID_HANDLE_VALUE {
        return out;
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `entry` is a correctly sized, zeroed PROCESSENTRY32W owned here.
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while ok {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        out.push(ProcessEntry {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..len]),
        });
        ok = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    // SAFETY: `snap` is a valid handle from CreateToolhelp32Snapshot.
    unsafe { CloseHandle(snap) };
    out
}

#[cfg(not(windows))]
pub fn snapshot() -> Vec<ProcessEntry> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Launching, with a handle
// ---------------------------------------------------------------------------

/// A process ksx started and still holds a handle to.
///
/// The handle — not the pid — is the identity. A pid can be recycled the
/// instant the process dies, so "is pid 4242 still there?" is a question that
/// can be answered `yes` about a completely different program; a handle stays
/// valid and signalled for as long as this value lives.
///
/// See the module docs for why there is no way to kill it.
pub struct GameProcess {
    child: std::process::Child,
    pid: u32,
    /// Cached once the process has been reaped, so repeated polls after exit
    /// stay cheap and consistent.
    exit_code: Option<i32>,
}

impl std::fmt::Debug for GameProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameProcess")
            .field("pid", &self.pid)
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

/// Why a process could not be started.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("the executable does not exist: {0}")]
    NotFound(String),
    #[error("could not start '{path}': {source}")]
    Spawn {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Start `exe` with `args`, in `working_dir` (defaulting to the exe's own
/// directory, as the legacy app did via `ProcessStartInfo.WorkingDirectory`).
///
/// The existence check up front is not redundant with the spawn error: it lets
/// the caller distinguish "this profile is misconfigured" (checkable before
/// anything is plugged, exit 2) from "the OS refused to start it" (exit 3).
pub fn launch(
    exe: &Path,
    args: &[String],
    working_dir: Option<&Path>,
) -> Result<GameProcess, LaunchError> {
    if !exe.is_file() {
        return Err(LaunchError::NotFound(exe.display().to_string()));
    }
    let mut command = std::process::Command::new(exe);
    command.args(args);
    match working_dir.or_else(|| exe.parent()) {
        Some(dir) if !dir.as_os_str().is_empty() => {
            command.current_dir(dir);
        }
        _ => {}
    }
    let child = command.spawn().map_err(|source| LaunchError::Spawn {
        path: exe.display().to_string(),
        source,
    })?;
    let pid = child.id();
    Ok(GameProcess {
        child,
        pid,
        exit_code: None,
    })
}

impl GameProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Exit code, once known. `None` while the process is still running.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Is the process still running? Non-blocking, and it reaps on the way past
    /// so no zombie is left behind.
    ///
    /// A failure to ask (the handle is gone, the OS said no) is reported as
    /// **not alive**: an unanswerable question about a process we started can
    /// only be resolved by giving up on it, and the alternative — treating it
    /// as alive forever — would hang the session on a cabinet nobody is
    /// watching.
    pub fn is_alive(&mut self) -> bool {
        if self.exit_code.is_some() {
            return false;
        }
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                self.exit_code = Some(status.code().unwrap_or(-1));
                false
            }
            Err(err) => {
                tracing::warn!(pid = self.pid, %err, "cannot poll the launched game; treating it as exited");
                self.exit_code = Some(-1);
                false
            }
        }
    }

    /// Block for at most `timeout`, then report whether the process exited.
    ///
    /// On Windows this is a real `WaitForSingleObject` on the process handle —
    /// no polling loop, so a game that exits 3 ms into a 60 s wait is noticed in
    /// 3 ms. Elsewhere it degrades to a bounded poll.
    pub fn wait_timeout(&mut self, timeout: Duration) -> bool {
        if self.exit_code.is_some() {
            return true;
        }
        self.wait_for_signal(timeout);
        !self.is_alive()
    }

    #[cfg(windows)]
    fn wait_for_signal(&self, timeout: Duration) {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let handle = self.child.as_raw_handle() as HANDLE;
        // SAFETY: `handle` is the live process handle owned by `self.child`,
        // which outlives this call; WaitForSingleObject only reads it.
        unsafe { WaitForSingleObject(handle, ms) };
    }

    #[cfg(not(windows))]
    fn wait_for_signal(&self, timeout: Duration) {
        // No handle-wait primitive in std; the policy layer polls anyway and
        // this path exists only so the tests build off Windows.
        std::thread::sleep(timeout.min(Duration::from_millis(50)));
    }
}

/// Why a shell activation could not be performed.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("ShellExecute failed for '{target}' (code {code}): {hint}")]
    Failed {
        target: String,
        code: isize,
        hint: &'static str,
    },
    #[error("shell activation is Windows-only; cannot open '{0}'")]
    Unsupported(String),
}

/// `ShellExecuteW("open", target)` — the `UseShellExecute = true` the legacy
/// app used, and the only way to start a `steam://rungameid/NNN` target.
///
/// Returns as soon as the shell has *accepted* the request. For a protocol URL
/// that is essentially immediate and there is no process to wait on, which is
/// exactly why a profile that uses one must also name its `process_name`.
#[cfg(windows)]
pub fn shell_open(target: &str) -> Result<(), ShellError> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call; a null HWND
    // and null parameter/directory pointers are the documented "no parent
    // window, no extra arguments" form.
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if rc > 32 {
        return Ok(());
    }
    Err(ShellError::Failed {
        target: target.to_owned(),
        code: rc,
        hint: shell_hint(rc),
    })
}

#[cfg(not(windows))]
pub fn shell_open(target: &str) -> Result<(), ShellError> {
    Err(ShellError::Unsupported(target.to_owned()))
}

/// The handful of `ShellExecute` failure codes worth spelling out — everything
/// else gets the generic line. Pure, so it is tested off Windows too.
pub fn shell_hint(code: isize) -> &'static str {
    match code {
        2 => "the file was not found",
        3 => "the path was not found",
        5 => "access denied",
        8 => "not enough memory",
        31 => {
            "no application is registered for this file type or URL scheme \
               (is Steam installed and has it registered steam://?)"
        }
        _ => "see ShellExecute's documented return values",
    }
}

/// Open a folder in Explorer. Used by the daemon's "Open config folder".
pub fn open_folder(path: &Path) -> Result<(), ShellError> {
    shell_open(&path.display().to_string())
}

/// Is this process running with an elevated token?
///
/// `None` means the question could not be answered (never assume "yes" — the
/// caller prints installation advice, and telling a non-admin user they are
/// admin wastes their time with a UAC-less failure).
#[cfg(windows)]
pub fn is_elevated() -> Option<bool> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    #[repr(C)]
    struct TokenElevationRaw {
        token_is_elevated: u32,
    }

    let mut token = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing;
    // `token` is closed below on the success path.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut info = TokenElevationRaw {
        token_is_elevated: 0,
    };
    let mut returned = 0u32;
    // SAFETY: TokenElevation's out-parameter is a TOKEN_ELEVATION, which is
    // layout-identical to the single-u32 struct above.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut info as *mut TokenElevationRaw).cast(),
            std::mem::size_of::<TokenElevationRaw>() as u32,
            &mut returned,
        )
    } != 0;
    // SAFETY: `token` came from OpenProcessToken and is not used again.
    unsafe { CloseHandle(token) };
    ok.then_some(info.token_is_elevated != 0)
}

#[cfg(not(windows))]
pub fn is_elevated() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matching_ignores_case() {
        let entry = ProcessEntry {
            pid: 1,
            parent_pid: 0,
            name: "MAME.exe".into(),
        };
        assert!(entry.name_matches("mame.exe"));
        assert!(entry.name_matches("MAME.EXE"));
        assert!(!entry.name_matches("mame"));
    }

    #[test]
    fn shell_hint_explains_the_missing_protocol_handler() {
        assert!(shell_hint(31).contains("steam://"));
        assert!(shell_hint(2).contains("not found"));
        assert!(!shell_hint(1234).is_empty());
    }

    /// A snapshot must never *invent* a process, and on Windows it must at
    /// least find this test binary — the launcher hand-off logic is built on
    /// the assumption that an empty result means "not found", not "broken".
    #[test]
    fn snapshot_is_self_consistent() {
        let procs = snapshot();
        if cfg!(windows) {
            let me = std::process::id();
            assert!(
                procs.iter().any(|p| p.pid == me),
                "the snapshot must contain this very process"
            );
            assert!(procs.iter().all(|p| !p.name.is_empty()));
        } else {
            assert!(
                procs.is_empty(),
                "non-Windows snapshot is a documented stub"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_open_is_unsupported_off_windows() {
        assert!(matches!(
            shell_open("steam://rungameid/1"),
            Err(ShellError::Unsupported(_))
        ));
    }

    /// A missing exe is diagnosable *before* the spawn, which is what lets
    /// `ksx run --game` exit 2 without ever plugging a pad.
    #[test]
    fn launching_a_missing_executable_is_not_found_not_a_spawn_error() {
        let missing = std::env::temp_dir().join("ksx-no-such-game-9c1f.exe");
        assert!(matches!(
            launch(&missing, &[], None),
            Err(LaunchError::NotFound(_))
        ));
    }

    /// The handle really is a handle: a process that exits is observed to exit,
    /// with its code, and repeated polls stay consistent.
    #[cfg(windows)]
    #[test]
    fn a_launched_process_is_tracked_to_exit_with_its_code() {
        let cmd = std::path::PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| {
            format!(
                "{}\\System32\\cmd.exe",
                std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
            )
        }));
        let mut proc = launch(&cmd, &["/c".into(), "exit 7".into()], None).expect("cmd.exe /c");
        assert!(proc.pid() != 0);
        assert!(
            proc.wait_timeout(Duration::from_secs(10)),
            "cmd /c exit must finish well inside the timeout"
        );
        assert!(!proc.is_alive());
        assert_eq!(proc.exit_code(), Some(7));
        // Idempotent: asking again after exit must not change the answer.
        assert!(proc.wait_timeout(Duration::from_millis(1)));
        assert_eq!(proc.exit_code(), Some(7));
    }

    /// A long-running process is reported alive, and `wait_timeout` honours its
    /// timeout instead of blocking to completion.
    #[cfg(windows)]
    #[test]
    fn a_running_process_is_alive_and_the_timeout_is_respected() {
        let cmd = std::path::PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| {
            format!(
                "{}\\System32\\cmd.exe",
                std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
            )
        }));
        // `timeout /t` needs a console; `ping -n` is the portable sleep.
        let mut proc = launch(
            &cmd,
            &["/c".into(), "ping -n 6 127.0.0.1 >NUL".into()],
            None,
        )
        .expect("cmd.exe /c ping");
        assert!(proc.is_alive());
        let started = std::time::Instant::now();
        assert!(
            !proc.wait_timeout(Duration::from_millis(150)),
            "a 5-second sleep must not have finished in 150 ms"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "wait_timeout blocked past its timeout: {:?}",
            started.elapsed()
        );
        // Let it go; ksx never kills a game, and neither does this test — the
        // child is reaped when cmd finishes on its own.
        assert!(proc.wait_timeout(Duration::from_secs(20)));
    }

    /// The no-kill policy is a property of the source, not of good intentions:
    /// nothing in this module may terminate a process the user is playing.
    #[test]
    fn no_kill_primitive_exists() {
        let source = include_str!("process.rs");
        // Skip the doc comment and this test, which necessarily name the APIs.
        let body = source
            .split("// ---------------------------------------------------------------------------\n// Launching, with a handle")
            .nth(1)
            .expect("the launching section exists");
        let body = body.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["TerminateProcess", ".kill(", "fn kill", "ExitProcess"] {
            assert!(
                !body.contains(forbidden),
                "ksx never kills the user's game, but '{forbidden}' appears in process.rs"
            );
        }
    }
}
