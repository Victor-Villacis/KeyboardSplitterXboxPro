//! Customer-facing, console-free hand-off to `ksx.exe open`.
//!
//! `ksx.exe` remains a console-subsystem binary because its CLI is a real
//! development and recovery surface. Windows would therefore create a console
//! when a shortcut starts it directly. This GUI-subsystem binary resolves the
//! installed sibling executable, runs it with `CREATE_NO_WINDOW`, and waits
//! for `ksx open` to report that the window was handed off. A failed hand-off
//! therefore becomes a visible dialog instead of a shortcut that appears to do
//! nothing. The CLI's behavior remains unchanged everywhere else.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// WinBase.h `CREATE_NO_WINDOW`: do not allocate a console for the console-
/// subsystem child when this GUI-subsystem process starts it.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, PartialEq, Eq)]
struct LaunchPlan {
    executable: PathBuf,
    working_dir: PathBuf,
}

impl LaunchPlan {
    fn beside(launcher: &Path) -> Result<Self, LaunchError> {
        let working_dir = launcher
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| LaunchError::NoInstallDirectory(launcher.to_path_buf()))?;
        Ok(Self {
            executable: working_dir.join("ksx.exe"),
            working_dir: working_dir.to_path_buf(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .arg("open")
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}

#[derive(Debug)]
enum LaunchError {
    CurrentExecutable(std::io::Error),
    NoInstallDirectory(PathBuf),
    Spawn {
        executable: PathBuf,
        source: std::io::Error,
    },
    ChildExit {
        executable: PathBuf,
        code: Option<i32>,
    },
}

impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(error) => write!(
                formatter,
                "Windows could not locate the ksx launcher: {error}"
            ),
            Self::NoInstallDirectory(path) => write!(
                formatter,
                "The ksx launcher has no install directory: {}",
                path.display()
            ),
            Self::Spawn { executable, source } => write!(
                formatter,
                "Windows could not start {}: {source}",
                executable.display()
            ),
            Self::ChildExit { executable, code } => match code {
                Some(code) => write!(
                    formatter,
                    "{} stopped before ksx opened (exit code {code})",
                    executable.display()
                ),
                None => write!(
                    formatter,
                    "{} stopped before ksx opened",
                    executable.display()
                ),
            },
        }
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentExecutable(error) => Some(error),
            Self::Spawn { source, .. } => Some(source),
            Self::NoInstallDirectory(_) | Self::ChildExit { .. } => None,
        }
    }
}

fn launch() -> Result<(), LaunchError> {
    let launcher = std::env::current_exe().map_err(LaunchError::CurrentExecutable)?;
    let plan = LaunchPlan::beside(&launcher)?;
    spawn(&plan)
}

fn spawn(plan: &LaunchPlan) -> Result<(), LaunchError> {
    let mut command = plan.command();
    hide_console(&mut command);
    let status = command.status().map_err(|source| LaunchError::Spawn {
        executable: plan.executable.clone(),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchError::ChildExit {
            executable: plan.executable.clone(),
            code: status.code(),
        })
    }
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}

fn failure_message(error: &LaunchError) -> String {
    format!(
        "ksx could not start.\r\n\r\n{error}\r\n\r\n\
         Reinstall ksx and try again. If the problem continues, include this \
         message when reporting it."
    )
}

#[cfg(windows)]
fn show_error(message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    };

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    let title = wide(OsStr::new("ksx could not start"));
    let body = wide(OsStr::new(message));
    // SAFETY: both strings are live, NUL-terminated UTF-16 buffers for the
    // duration of this modal call; a null owner is documented for MessageBoxW.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = launch() {
        show_error(&failure_message(&error));
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    // The installer and customer shortcut are Windows-only. Keeping a small
    // non-Windows main lets workspace metadata/checks remain portable.
    eprintln!("ksx-launcher is available only on Windows");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_plan_uses_only_the_installed_sibling_and_open_verb() {
        let launcher = Path::new(r"C:\Program Files\ksx\ksx-launcher.exe");
        let plan = LaunchPlan::beside(launcher).expect("launcher has a parent");
        assert_eq!(
            plan.executable,
            PathBuf::from(r"C:\Program Files\ksx\ksx.exe")
        );
        assert_eq!(plan.working_dir, PathBuf::from(r"C:\Program Files\ksx"));

        let command = plan.command();
        assert_eq!(command.get_program(), plan.executable.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("open")]
        );
        assert_eq!(command.get_current_dir(), Some(plan.working_dir.as_path()));
    }

    #[test]
    fn launch_plan_refuses_a_path_without_an_install_directory() {
        let error = LaunchPlan::beside(Path::new("ksx-launcher.exe"))
            .expect_err("a bare filename has no sibling directory");
        assert!(matches!(error, LaunchError::NoInstallDirectory(_)));
    }

    #[test]
    fn spawn_failure_dialog_names_the_program_and_a_recovery() {
        let error = LaunchError::Spawn {
            executable: PathBuf::from(r"C:\Program Files\ksx\ksx.exe"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
        };
        let message = failure_message(&error);
        assert!(message.contains("ksx could not start"));
        assert!(message.contains(r"C:\Program Files\ksx\ksx.exe"));
        assert!(message.contains("file not found"));
        assert!(message.contains("Reinstall ksx"));
    }

    #[test]
    fn child_failure_dialog_reports_that_the_product_never_opened() {
        let error = LaunchError::ChildExit {
            executable: PathBuf::from(r"C:\Program Files\ksx\ksx.exe"),
            code: Some(1),
        };
        let message = failure_message(&error);
        assert!(message.contains("stopped before ksx opened"));
        assert!(message.contains("exit code 1"));
        assert!(message.contains("Reinstall ksx"));
    }

    #[cfg(windows)]
    #[test]
    fn no_window_flag_is_the_win32_contract_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }
}
