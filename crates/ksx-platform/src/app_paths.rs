//! `App Paths` — where Windows itself keeps the full path of an application
//! that is known by bare name.
//!
//! One caller today: `ksx open` has to find a Chromium to host Studio's
//! window, and docs/M9-DECISION.md §4 item 2 is specific about how — **"launch
//! by App Paths, never `ShellExecute` on a URL"**. The difference is not
//! stylistic:
//!
//! - `ShellExecute("http://…")` asks for *the default browser*, which is a
//!   per-user association a stripped or hardened image may not have at all,
//!   and which cannot be given command-line flags. There is no way to ask it
//!   for a chrome-less window.
//! - `App Paths` names an executable we can then run with the flags that make
//!   the window an application window, in a profile ksx owns.
//!
//! It is also how the Run box resolves `msedge`: the key exists precisely so a
//! program can be found without knowing which of `Program Files`,
//! `Program Files (x86)` or `%LOCALAPPDATA%` this machine's installer chose.
//!
//! # What this module will and will not claim
//!
//! [`resolve`] answers `Some` only when the registry names a path **and a file
//! is there**. An uninstall that leaves the key behind is common enough that
//! trusting the string alone would hand `Command::new` a path that cannot
//! spawn, and the caller would report "could not start Edge" about an Edge
//! that is not installed. `None` therefore means "nothing runnable under that
//! name", which is the only thing a caller can act on.

use std::path::PathBuf;

/// The key every `App Paths` entry hangs off, under HKCU and HKLM alike.
pub const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";

/// The subkey one executable's entry lives at.
pub fn subkey(exe: &str) -> String {
    format!("{APP_PATHS}\\{exe}")
}

/// The default value of an `App Paths` key, as a path we could spawn.
///
/// Two things are done to the raw string and no more:
///
/// - **trimmed**, because whitespace around a registry string is invisible in
///   `regedit` and fatal to `CreateProcess`;
/// - **unquoted**, because the convention allows `"C:\…\msedge.exe"` and
///   `std::process::Command` does its own quoting — a program path that still
///   carries its quotes is looked up literally, quotes and all, and is never
///   found.
///
/// Nothing else: no expansion, no case folding, no guessing at a directory.
/// An empty or all-quotes value is `None` rather than a path to nowhere.
pub fn clean(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches('"').trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Where Windows says `exe` lives — `None` if it says nothing, or names a file
/// that is not there.
///
/// HKCU is consulted before HKLM. A browser installed by a user who is not an
/// administrator writes **only** HKCU, so an HKLM-only lookup would report it
/// absent; and where both exist, the copy that user installed for themselves
/// is the one to prefer.
///
/// Off Windows there is no such registry and this is `None` — which makes
/// every caller take its "no browser here" path instead of failing to compile.
#[cfg(windows)]
pub fn resolve(exe: &str) -> Option<PathBuf> {
    use crate::win::registry;

    let key = subkey(exe);
    [registry::HKEY_CURRENT_USER, registry::HKEY_LOCAL_MACHINE]
        .into_iter()
        // The DEFAULT value (empty name) is where the full path lives.
        .filter_map(|root| registry::read_string_under(root, &key, ""))
        .filter_map(|raw| clean(&raw))
        .find(|path| path.is_file())
}

#[cfg(not(windows))]
pub fn resolve(_exe: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key path is a literal Windows expects character for character —
    /// "App Paths" with its space, under `CurrentVersion`. A typo here reads
    /// as "no browser installed" on every machine in the world, which is the
    /// hardest possible failure to notice from inside the program.
    #[test]
    fn the_subkey_is_the_one_windows_publishes() {
        assert_eq!(
            subkey("msedge.exe"),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\msedge.exe"
        );
    }

    /// Catches the version that hands the raw registry string to
    /// `Command::new`: `"C:\…\msedge.exe"` **with** its quotes is a filename
    /// no filesystem has, so a perfectly installed browser fails to start with
    /// "the system cannot find the file specified".
    #[test]
    fn a_quoted_or_padded_value_becomes_a_path_that_can_actually_spawn() {
        assert_eq!(
            clean(r#""C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe""#),
            Some(PathBuf::from(
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
            ))
        );
        assert_eq!(
            clean("  C:\\Chrome\\chrome.exe  "),
            Some(PathBuf::from(r"C:\Chrome\chrome.exe"))
        );
    }

    /// An empty default value is a key with nothing in it, not a program at
    /// the filesystem root. Catches the version that returns `PathBuf::new()`
    /// and lets an empty program name reach a spawn.
    #[test]
    fn an_empty_value_is_nothing_rather_than_a_path_to_nowhere() {
        assert_eq!(clean(""), None);
        assert_eq!(clean("   "), None);
        assert_eq!(clean("\"\""), None);
    }

    /// Off Windows there is no registry to ask, and the answer must be a
    /// truthful "nothing" rather than a compile error or a panic — that is
    /// what lets the launcher's decision logic be tested on any host.
    #[cfg(not(windows))]
    #[test]
    fn there_are_no_app_paths_off_windows() {
        assert_eq!(resolve("msedge.exe"), None);
    }

    /// Self-consistency, true on a machine with every browser and on one with
    /// none: whatever `resolve` returns must be a file that is there. It
    /// deliberately does NOT assert that this machine has Edge — that is a
    /// fact about a machine, not about this code.
    #[cfg(windows)]
    #[test]
    fn whatever_resolves_is_a_file_that_exists() {
        for exe in ["msedge.exe", "chrome.exe", "ksx-no-such-browser.exe"] {
            if let Some(path) = resolve(exe) {
                assert!(
                    path.is_file(),
                    "resolve({exe}) returned {} which is not a file",
                    path.display()
                );
            }
        }
        // A name nobody registers must be absent on every machine.
        assert_eq!(resolve("ksx-no-such-browser.exe"), None);
    }
}
