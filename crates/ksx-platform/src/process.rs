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
//! Off Windows every function degrades to a documented no-op/`Unsupported`
//! rather than failing to compile, so `ksx-games`' policy layer keeps its
//! cross-platform unit tests.

use std::path::Path;

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
}
