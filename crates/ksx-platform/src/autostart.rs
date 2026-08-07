//! `ksx autostart` — start-at-logon via a **per-user** Task Scheduler task.
//!
//! # Why Task Scheduler and not `HKCU\...\Run`
//!
//! The cabinet's whole point is that a cold boot reaches a playable frontend
//! with nobody at a keyboard. `Run` keys give no control over working
//! directory, no restart-on-failure, no "only after the network/desktop is
//! ready", and no way to inspect what is registered other than reading the
//! registry. A logon-triggered task gives all of that and can be examined,
//! exported and deleted with one documented tool.
//!
//! # Why `schtasks.exe` XML and not the COM API
//!
//! Auditability. `--dry-run` can print the *exact* XML and the *exact* command
//! line that would be applied, a human can diff it against
//! `schtasks /Query /XML`, and nothing about the registration is hidden behind
//! an in-process COM call. The COM `ITaskService` route would need a large
//! amount of `windows` crate surface to express the same document and would
//! still produce this XML underneath.
//!
//! # Never requires administrator
//!
//! The task is registered for the *invoking* user with `LogonType`
//! `InteractiveToken` and `RunLevel` `LeastPrivilege`. Registering a task for
//! yourself needs no elevation; asking for `HighestAvailable`, another user, or
//! a system account does — so this module never does. Neither `ksx daemon` nor
//! `ksx run` needs admin (Interception and ViGEmBus are already installed), and
//! a cabinet that silently runs elevated at logon is a much worse thing to own
//! than one that does not.
//!
//! Everything here except [`apply`], [`remove`] and [`query`] is pure and
//! tested off-Windows.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Default task path. The leading folder keeps it out of the Task Scheduler
/// Library root, where it would be lost among vendor updaters.
pub const DEFAULT_TASK_NAME: &str = "ksx\\autostart";

/// Which ksx entry point the logon task starts.
///
/// `Daemon` is the default, deliberately. A registered `ksx run` captures the
/// assigned keyboards unconditionally at every logon — a hostile default on a
/// machine that is also a desktop PC: log in to pay a bill and the panel's
/// keyboard is a gamepad. `ksx daemon` puts an icon in the tray and captures
/// nothing until a session is started from the tray or the frontend wrapper.
/// `Run` stays available (`--mode run`) for the kiosk shape where
/// logon-straight-into-the-game is the point.
///
/// Changing the default from `run` to `daemon` breaks nobody: no cabinet has
/// ever run the M5 gate, so there are no deployed registrations to preserve
/// (the gate runbook is what found this gap — docs/GATES.md, GATE 1 Phase B).
/// Inspection still parses both shapes, so a task registered as `run …`
/// before this change is reported correctly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskMode {
    /// `ksx daemon` — tray icon at logon; sessions start on demand.
    #[default]
    Daemon,
    /// `ksx run` — capture keyboards and start a session at logon (kiosk).
    Run,
}

impl TaskMode {
    /// The ksx subcommand this mode registers.
    pub fn verb(self) -> &'static str {
        match self {
            TaskMode::Daemon => "daemon",
            TaskMode::Run => "run",
        }
    }

    /// Inverse of [`TaskMode::verb`], for reading a registration back.
    pub fn from_verb(verb: &str) -> Option<Self> {
        match verb {
            "daemon" => Some(TaskMode::Daemon),
            "run" => Some(TaskMode::Run),
            _ => None,
        }
    }

    /// One-line human description for status output.
    pub fn describe(self) -> &'static str {
        match self {
            TaskMode::Daemon => "daemon (tray icon at logon; sessions start on demand)",
            TaskMode::Run => "run (captures keyboards and starts a session at logon)",
        }
    }
}

/// What the registered task should run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    pub task_name: String,
    /// Absolute path to `ksx.exe`.
    pub exe: PathBuf,
    /// Which subcommand runs at logon — see [`TaskMode`].
    pub mode: TaskMode,
    /// Appends `--game <TITLE>` after the mode's verb: the daemon uses the
    /// profile for every session it starts; `run` starts it immediately.
    pub game: Option<String>,
    /// Extra flags appended verbatim (e.g. `--latency`).
    pub extra_args: Vec<String>,
    /// `DOMAIN\user` or `COMPUTER\user`, the logon trigger's principal.
    pub user_id: String,
    /// Seconds to wait after logon before starting. A frontend that races the
    /// shell loses; 10 s is the legacy cabinet's habit, and it is configurable.
    pub delay_secs: u32,
}

impl TaskSpec {
    /// The `Arguments` element — everything after `ksx.exe`.
    ///
    /// A title with spaces is quoted; a title containing a quote, or a
    /// to-be-quoted title ending in a backslash, is refused upstream (see
    /// [`validate`]) rather than escaped, because Task Scheduler hands this
    /// string to `CommandLineToArgvW` and a half-escaped title is a
    /// silently-wrong autostart nobody would notice until a cold boot.
    pub fn arguments(&self) -> String {
        let mut parts = vec![self.mode.verb().to_owned()];
        if let Some(game) = &self.game {
            parts.push("--game".to_owned());
            parts.push(if game.contains(' ') {
                format!("\"{game}\"")
            } else {
                game.clone()
            });
        }
        parts.extend(self.extra_args.iter().cloned());
        parts.join(" ")
    }

    /// What the task will actually do, as one pasteable line.
    pub fn command_line(&self) -> String {
        format!("\"{}\" {}", self.exe.display(), self.arguments())
    }

    pub fn working_dir(&self) -> PathBuf {
        self.exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Reasons a spec cannot be registered. All are exit-code-2 refusals.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutostartError {
    #[error("cannot resolve the path to ksx.exe: {0}")]
    NoExePath(String),
    #[error(
        "the game title {0:?} contains a double quote; Task Scheduler would mis-split the \
         command line. Rename the profile in games.toml"
    )]
    UnquotableTitle(String),
    #[error(
        "the game title {0:?} needs quoting (it contains a space) but ends with a backslash; \
         CommandLineToArgvW reads backslashes before a closing quote as escapes and would \
         mis-split the command line. Rename the profile in games.toml"
    )]
    TrailingBackslashTitle(String),
    #[error("cannot determine the current user (USERNAME is not set)")]
    NoUser,
    #[error("schtasks.exe could not be run: {0}")]
    SchtasksUnavailable(String),
    #[error("schtasks.exe failed (exit {code}): {output}")]
    SchtasksFailed { code: i32, output: String },
    #[error("`ksx autostart` is Windows-only (it registers a Task Scheduler task)")]
    Unsupported,
}

/// Reject anything that would produce a subtly-wrong command line.
pub fn validate(spec: &TaskSpec) -> Result<(), AutostartError> {
    if let Some(game) = &spec.game {
        if game.contains('"') {
            return Err(AutostartError::UnquotableTitle(game.clone()));
        }
        // A quoted title ending in a backslash is the other CommandLineToArgvW
        // trap: `--game "Foo Bar\"` reads `\"` as an escaped quote, so the
        // quoted region never closes (odd run) or the backslashes are halved
        // (even run) — either way a silently-wrong argv at logon. A title that
        // needs no quoting keeps its trailing backslash verbatim, so only the
        // quoted shape is refused.
        if game.contains(' ') && game.ends_with('\\') {
            return Err(AutostartError::TrailingBackslashTitle(game.clone()));
        }
    }
    if spec.exe.as_os_str().is_empty() {
        return Err(AutostartError::NoExePath("empty path".into()));
    }
    Ok(())
}

/// `DOMAIN\user` for the current process, from the environment.
pub fn current_user_id() -> Result<String, AutostartError> {
    let user = std::env::var("USERNAME")
        .ok()
        .filter(|u| !u.is_empty())
        .ok_or(AutostartError::NoUser)?;
    let domain = std::env::var("USERDOMAIN")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|d| !d.is_empty());
    Ok(match domain {
        Some(domain) => format!("{domain}\\{user}"),
        None => user,
    })
}

/// Build the default spec for this installation.
pub fn spec_for_current_exe(
    mode: TaskMode,
    game: Option<String>,
    extra_args: Vec<String>,
    delay_secs: u32,
    task_name: Option<String>,
) -> Result<TaskSpec, AutostartError> {
    let exe = std::env::current_exe().map_err(|e| AutostartError::NoExePath(e.to_string()))?;
    let spec = TaskSpec {
        task_name: task_name.unwrap_or_else(|| DEFAULT_TASK_NAME.to_owned()),
        exe,
        mode,
        game,
        extra_args,
        user_id: current_user_id()?,
        delay_secs,
    };
    validate(&spec)?;
    Ok(spec)
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

/// Render the Task Scheduler 1.2 document `schtasks /Create /XML` consumes.
///
/// Deliberate settings, each one a cabinet lesson:
/// - `DisallowStartIfOnBatteries`/`StopIfGoingOnBatteries` **false** — a task
///   that refuses to run on a UPS-backed cabinet is a support call.
/// - `ExecutionTimeLimit PT0S` — no time limit; a session lasts as long as the
///   session lasts.
/// - `MultipleInstancesPolicy IgnoreNew` — a second logon must never plug a
///   second set of pads (8 virtual pads > 4 XInput slots; see the playbook).
/// - `RunLevel LeastPrivilege` — see the module docs: never elevated.
/// - `Delay` after logon so the shell, the frontend and USB enumeration are
///   settled before ksx claims keyboards.
pub fn render_xml(spec: &TaskSpec) -> String {
    let delay = format!("PT{}S", spec.delay_secs);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>ksx — split keyboards into virtual Xbox 360 controllers at logon.
Managed by `ksx autostart`; remove with `ksx autostart --disable`.</Description>
    <URI>\{task}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>{delay}</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{args}</Arguments>
      <WorkingDirectory>{cwd}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        task = xml_escape(&spec.task_name),
        user = xml_escape(&spec.user_id),
        delay = delay,
        exe = xml_escape(&spec.exe.display().to_string()),
        args = xml_escape(&spec.arguments()),
        cwd = xml_escape(&spec.working_dir().display().to_string()),
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Task Scheduler wants the `/XML` file as UTF-16 (the document even declares
/// `encoding="UTF-16"`), so the bytes must actually be UTF-16LE with a BOM.
/// Writing UTF-8 here is the classic "schtasks says the file is invalid".
pub fn to_utf16le_bom(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + text.len() * 2);
    out.extend_from_slice(&[0xFF, 0xFE]);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Decode whatever `schtasks` wrote: UTF-16LE (with or without BOM) or UTF-8.
pub fn decode_console_output(bytes: &[u8]) -> String {
    let (body, forced_utf16) = match bytes {
        [0xFF, 0xFE, rest @ ..] => (rest, true),
        _ => (bytes, false),
    };
    // schtasks /XML emits UTF-16 without a BOM on some builds; ASCII text in
    // UTF-16LE is "X\0X\0…", which UTF-8 would never produce in bulk.
    let looks_utf16 = forced_utf16
        || (body.len() >= 8
            && body
                .iter()
                .skip(1)
                .step_by(2)
                .take(16)
                .filter(|b| **b == 0)
                .count()
                >= 6);
    if looks_utf16 && body.len() >= 2 {
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(body).into_owned()
}

// ---------------------------------------------------------------------------
// schtasks argv
// ---------------------------------------------------------------------------

/// `/F` makes creation idempotent: registering over an existing task replaces
/// it instead of prompting, so `--enable` twice is the same as once.
pub fn create_argv(task_name: &str, xml_path: &Path) -> Vec<String> {
    vec![
        "/Create".into(),
        "/TN".into(),
        task_name.to_owned(),
        "/XML".into(),
        xml_path.display().to_string(),
        "/F".into(),
    ]
}

pub fn delete_argv(task_name: &str) -> Vec<String> {
    vec![
        "/Delete".into(),
        "/TN".into(),
        task_name.to_owned(),
        "/F".into(),
    ]
}

pub fn query_argv(task_name: &str) -> Vec<String> {
    vec![
        "/Query".into(),
        "/TN".into(),
        task_name.to_owned(),
        "/XML".into(),
        "ONE".into(),
    ]
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// What is registered right now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RegisteredTask {
    pub command: Option<String>,
    pub arguments: Option<String>,
    pub working_directory: Option<String>,
    pub user_id: Option<String>,
    pub enabled: Option<bool>,
}

impl RegisteredTask {
    /// The full command line as a user would read it.
    pub fn command_line(&self) -> String {
        match (&self.command, &self.arguments) {
            (Some(cmd), Some(args)) if !args.is_empty() => format!("\"{cmd}\" {args}"),
            (Some(cmd), _) => format!("\"{cmd}\""),
            (None, _) => "<no Exec action>".to_owned(),
        }
    }

    /// Which entry point the registered task starts, read back from its
    /// arguments. Both shapes must parse — tasks registered before the daemon
    /// became the default say `run …` and must still be reported correctly.
    /// `None` means the arguments were not written by `ksx autostart` at all.
    pub fn mode(&self) -> Option<TaskMode> {
        TaskMode::from_verb(self.arguments.as_deref()?.split_whitespace().next()?)
    }

    /// Which `--game` profile the registered task points at, if any.
    pub fn game(&self) -> Option<String> {
        let args = self.arguments.as_deref()?;
        let rest = args.split("--game").nth(1)?.trim_start();
        let title = match rest.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next()?.to_owned(),
            None => rest.split_whitespace().next()?.to_owned(),
        };
        (!title.is_empty()).then_some(title)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Status {
    NotRegistered,
    Registered(RegisteredTask),
}

impl Status {
    pub fn is_registered(&self) -> bool {
        matches!(self, Status::Registered(_))
    }
}

/// Does the registered task still point at a working ksx?
///
/// The failure this exists to catch is silent and slow: someone moves or
/// reinstalls ksx, the scheduled task keeps the old path, and the next cold
/// boot runs nothing at all. Nobody is watching the cabinet's console at logon,
/// so without an explicit check the first symptom is "the arcade machine came
/// up to a desktop" — weeks later, with no clue why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Staleness {
    /// The task points at an executable that exists and is this one.
    Current,
    /// The registered path does not exist any more. The task is dead.
    MissingExe { registered: String },
    /// The registered path exists but is a *different* ksx than the one being
    /// asked. Not necessarily wrong (two installs, a portable copy), so it is
    /// reported rather than treated as broken.
    DifferentExe { registered: String, current: String },
    /// No `<Command>` in the task at all — something else edited it.
    NoCommand,
}

impl Staleness {
    pub fn is_broken(&self) -> bool {
        matches!(self, Staleness::MissingExe { .. } | Staleness::NoCommand)
    }

    pub fn code(&self) -> &'static str {
        match self {
            Staleness::Current => "current",
            Staleness::MissingExe { .. } => "missing-exe",
            Staleness::DifferentExe { .. } => "different-exe",
            Staleness::NoCommand => "no-command",
        }
    }

    pub fn message(&self) -> Option<String> {
        match self {
            Staleness::Current => None,
            Staleness::MissingExe { registered } => Some(format!(
                "STALE: the registered task runs '{registered}', which no longer exists. This \
                 cabinet will boot to nothing. Re-run `ksx autostart --enable` from the ksx you \
                 want to keep."
            )),
            Staleness::DifferentExe {
                registered,
                current,
            } => Some(format!(
                "the registered task runs '{registered}', but this is '{current}'. If you meant \
                 to replace it, re-run `ksx autostart --enable` from here."
            )),
            Staleness::NoCommand => Some(
                "the registered task has no Exec action — it was edited outside ksx. \
                 Re-run `ksx autostart --enable` to rewrite it."
                    .to_owned(),
            ),
        }
    }
}

/// Pure staleness check. `exists` is injected so this is tested without
/// touching a filesystem or a scheduler.
pub fn check_staleness(
    task: &RegisteredTask,
    current_exe: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Staleness {
    let Some(registered) = task
        .command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    else {
        return Staleness::NoCommand;
    };
    let registered_path = Path::new(registered);
    if !exists(registered_path) {
        return Staleness::MissingExe {
            registered: registered.to_owned(),
        };
    }
    // Windows paths are case-insensitive, and `/` vs `\` is not a difference.
    let normalize = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    if normalize(registered_path) == normalize(current_exe) {
        Staleness::Current
    } else {
        Staleness::DifferentExe {
            registered: registered.to_owned(),
            current: current_exe.display().to_string(),
        }
    }
}

/// Pull the fields we care about out of a task XML document.
///
/// A deliberately small hand-rolled reader rather than a new XML dependency:
/// the document is machine-generated by Task Scheduler, always has these exact
/// elements, and the failure mode of a missing element is "report unknown",
/// not "corrupt a config".
pub fn parse_registered(xml: &str) -> RegisteredTask {
    RegisteredTask {
        command: element(xml, "Command"),
        arguments: element(xml, "Arguments"),
        working_directory: element(xml, "WorkingDirectory"),
        user_id: element(xml, "UserId"),
        enabled: element(xml, "Enabled").map(|v| v.eq_ignore_ascii_case("true")),
    }
}

fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml_unescape(xml[start..end].trim()))
}

// ---------------------------------------------------------------------------
// The plan (what --dry-run prints)
// ---------------------------------------------------------------------------

/// Everything `--dry-run` shows and `--enable` would do.
pub struct EnablePlan {
    pub spec: TaskSpec,
    pub xml: String,
    /// The argv passed to `schtasks.exe`, with the XML path filled in.
    pub argv: Vec<String>,
    pub xml_path: PathBuf,
}

/// Build the plan without touching the filesystem or Task Scheduler.
/// A per-invocation suffix for the temp XML name, so the path an attacker
/// would have to pre-create is not one they can predict. Not a secret and not
/// a substitute for `create_new` in [`apply`] — the two together are what make
/// the drop box safe. Address of a heap allocation + the clock + the pid, which
/// is plenty for "unguessable before the process starts".
fn unpredictable_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0);
    let boxed = Box::new(0u8);
    let addr = (&*boxed as *const u8) as usize as u64;
    format!(
        "{:016x}",
        nanos.rotate_left(17) ^ addr ^ (std::process::id() as u64)
    )
}

pub fn enable_plan(spec: TaskSpec) -> Result<EnablePlan, AutostartError> {
    validate(&spec)?;
    let xml = render_xml(&spec);
    let xml_path = std::env::temp_dir().join(format!(
        "ksx-autostart-{}-{}.xml",
        spec.task_name.replace(['\\', '/', ':'], "-"),
        unpredictable_suffix()
    ));
    let argv = create_argv(&spec.task_name, &xml_path);
    Ok(EnablePlan {
        spec,
        xml,
        argv,
        xml_path,
    })
}

impl EnablePlan {
    pub fn render_human(&self, dry_run: bool) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "task name:    {}", self.spec.task_name);
        let _ = writeln!(out, "runs at:      logon of {}", self.spec.user_id);
        let _ = writeln!(out, "after:        {} s delay", self.spec.delay_secs);
        let _ = writeln!(out, "mode:         {}", self.spec.mode.describe());
        let _ = writeln!(out, "runs:         {}", self.spec.command_line());
        let _ = writeln!(out, "working dir:  {}", self.spec.working_dir().display());
        let _ = writeln!(out, "elevation:    none (LeastPrivilege, per-user task)");
        let _ = writeln!(
            out,
            "\ncommand:      schtasks {}",
            crate::installer::quote_argv(&self.argv)
        );
        if dry_run {
            let _ = writeln!(
                out,
                "\n---- XML that would be written to {} (UTF-16LE) ----\n{}\
                 ---- end XML ----\n\ndry run: nothing was registered.",
                self.xml_path.display(),
                self.xml
            );
        }
        out
    }

    pub fn to_json(&self, dry_run: bool) -> serde_json::Value {
        serde_json::json!({
            "action": if dry_run { "dry-run" } else { "enable" },
            "task_name": self.spec.task_name,
            "user_id": self.spec.user_id,
            "delay_secs": self.spec.delay_secs,
            "exe": self.spec.exe.display().to_string(),
            "mode": self.spec.mode,
            "arguments": self.spec.arguments(),
            "command_line": self.spec.command_line(),
            "working_directory": self.spec.working_dir().display().to_string(),
            "elevated": false,
            "schtasks_argv": self.argv,
            "xml_path": self.xml_path.display().to_string(),
            "xml": self.xml,
        })
    }
}

// ---------------------------------------------------------------------------
// Live operations (Windows)
// ---------------------------------------------------------------------------

/// Absolute path to the system `schtasks.exe`.
///
/// Never a bare `"schtasks.exe"`: `CreateProcess` resolves a bare name against
/// the current directory and `%PATH%` before `System32`, so `cd`-ing into a
/// folder holding a hostile `schtasks.exe` would hijack the call. That is only
/// a same-privilege trick when `ksx autostart` runs unelevated — but people run
/// whole sessions from an admin terminal, and then it is a straight escalation.
/// `%SystemRoot%` is not user-writable (it is one of the installer's protected
/// roots), so an absolute path removes the question entirely.
#[cfg(windows)]
pub fn schtasks_exe() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    Path::new(&root).join("System32").join("schtasks.exe")
}

/// Run `schtasks` with `argv`, returning its decoded stdout.
///
/// `no_window`, and this is the call that proved why: `ksx autostart --status`
/// is one of the read-only collectors behind `StatusSource::snapshot`, which
/// the cabinet window re-runs **every two seconds**. Inside a daemon that has
/// released its console, each of those spawns opened, drew and destroyed a
/// console window of its own — the "ghost window that flashes but never fully
/// loads" over the cabinet UI. Its output is captured and parsed here; nobody
/// was ever meant to read it on screen.
#[cfg(windows)]
fn schtasks(argv: &[String]) -> Result<String, AutostartError> {
    let out = crate::process::no_window(std::process::Command::new(schtasks_exe()).args(argv))
        .output()
        .map_err(|e| AutostartError::SchtasksUnavailable(e.to_string()))?;
    let stdout = decode_console_output(&out.stdout);
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = decode_console_output(&out.stderr);
    Err(AutostartError::SchtasksFailed {
        code: out.status.code().unwrap_or(-1),
        output: if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        }
        .trim()
        .to_owned(),
    })
}

#[cfg(not(windows))]
fn schtasks(_argv: &[String]) -> Result<String, AutostartError> {
    Err(AutostartError::Unsupported)
}

/// Register (or replace) the task. Idempotent thanks to `/F`.
#[cfg(windows)]
pub fn apply(plan: &EnablePlan) -> Result<(), AutostartError> {
    // `%TEMP%` is user-writable, and `schtasks` re-opens this file by path
    // after we close it — the same check-then-use shape `crate::sealed` exists
    // to kill. It matters because people run `ksx autostart --enable` from an
    // admin terminal even though the task itself needs no elevation: anything
    // that can win the race there gets an arbitrary task definition registered
    // by an elevated `schtasks`, and a plain `fs::write` onto a pre-planted
    // symlink is an arbitrary-file-overwrite as administrator on top of that.
    //
    // `create_new(true)` is the fix: it fails rather than following or
    // truncating anything that already exists, and the name it wants is
    // unpredictable (see `unpredictable_suffix`). A collision means someone got
    // there first, which is a refusal, not something to retry through.
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.xml_path)
        .map_err(|e| {
            AutostartError::SchtasksUnavailable(format!(
                "could not create {} exclusively: {e}. Refusing to write the task definition \
                 through a file this process did not create.",
                plan.xml_path.display()
            ))
        })?;
    file.write_all(&to_utf16le_bom(&plan.xml))
        .and_then(|()| file.flush())
        .map_err(|e| AutostartError::SchtasksUnavailable(e.to_string()))?;
    drop(file);
    let result = schtasks(&plan.argv);
    // The temp XML carries a user name and a path; do not leave it lying around
    // whether or not registration worked.
    let _ = std::fs::remove_file(&plan.xml_path);
    result.map(|_| ())
}

#[cfg(not(windows))]
pub fn apply(_plan: &EnablePlan) -> Result<(), AutostartError> {
    Err(AutostartError::Unsupported)
}

/// Read the current registration.
pub fn query(task_name: &str) -> Result<Status, AutostartError> {
    match schtasks(&query_argv(task_name)) {
        Ok(xml) => Ok(Status::Registered(parse_registered(&xml))),
        // schtasks returns a non-zero exit for "the system cannot find the file
        // specified" — that is the not-registered answer, not a failure.
        Err(AutostartError::SchtasksFailed { output, .. }) if is_not_found(&output) => {
            Ok(Status::NotRegistered)
        }
        Err(err) => Err(err),
    }
}

/// Remove the task. Removing one that is not there is success — `--disable`
/// must be safe to run twice.
pub fn remove(task_name: &str) -> Result<bool, AutostartError> {
    match schtasks(&delete_argv(task_name)) {
        Ok(_) => Ok(true),
        Err(AutostartError::SchtasksFailed { output, .. }) if is_not_found(&output) => Ok(false),
        Err(err) => Err(err),
    }
}

/// Recognise schtasks' "no such task" wording across locales as best we can:
/// the English text, plus the `ERROR:` + `cannot find` shape.
pub fn is_not_found(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("cannot find the file specified")
        || lower.contains("does not exist")
        || lower.contains("the system cannot find")
        || (lower.contains("error:") && lower.contains("not exist"))
}

/// Human `--status` report.
pub fn render_status(task_name: &str, status: &Status) -> String {
    match status {
        Status::NotRegistered => format!(
            "autostart: NOT registered (no scheduled task '{task_name}')\n\
             enable it with `ksx autostart --enable` (add --game \"Title\" to give sessions a \
             profile; --mode run to start one at logon)"
        ),
        Status::Registered(task) => {
            use std::fmt::Write as _;
            let mut out = format!("autostart: registered as '{task_name}'\n");
            let _ = writeln!(out, "  runs:        {}", task.command_line());
            let _ = writeln!(
                out,
                "  mode:        {}",
                match task.mode() {
                    Some(mode) => mode.describe(),
                    None => "unknown (the arguments were not written by ksx)",
                }
            );
            if let Some(game) = task.game() {
                let _ = writeln!(out, "  game:        {game}");
            } else {
                let _ = writeln!(out, "  game:        (none)");
            }
            if let Some(user) = &task.user_id {
                let _ = writeln!(out, "  as user:     {user}");
            }
            if let Some(dir) = &task.working_directory {
                let _ = writeln!(out, "  working dir: {dir}");
            }
            let _ = writeln!(
                out,
                "  enabled:     {}",
                match task.enabled {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "unknown",
                }
            );
            let _ = write!(out, "remove it with `ksx autostart --disable`");
            out
        }
    }
}

pub fn status_json(task_name: &str, status: &Status) -> serde_json::Value {
    match status {
        Status::NotRegistered => serde_json::json!({
            "action": "status",
            "task_name": task_name,
            "registered": false,
        }),
        Status::Registered(task) => serde_json::json!({
            "action": "status",
            "task_name": task_name,
            "registered": true,
            "command": task.command,
            "arguments": task.arguments,
            "command_line": task.command_line(),
            "mode": task.mode(),
            "game": task.game(),
            "user_id": task.user_id,
            "working_directory": task.working_directory,
            "enabled": task.enabled,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(mode: TaskMode, game: Option<&str>) -> TaskSpec {
        TaskSpec {
            task_name: DEFAULT_TASK_NAME.to_owned(),
            exe: PathBuf::from(r"C:\Program Files\ksx\ksx.exe"),
            mode,
            game: game.map(str::to_owned),
            extra_args: Vec::new(),
            user_id: "CAB\\victor".to_owned(),
            delay_secs: 10,
        }
    }

    const BOTH_MODES: [TaskMode; 2] = [TaskMode::Daemon, TaskMode::Run];

    /// A bare program name is resolved against the current directory before
    /// `System32`, so `ksx autostart` run from an admin terminal sitting in a
    /// writable folder would execute that folder's `schtasks.exe` as
    /// administrator. The absolute path is the whole defence; assert it.
    #[cfg(windows)]
    #[test]
    fn schtasks_is_invoked_by_absolute_system32_path_not_by_bare_name() {
        let exe = schtasks_exe();
        assert!(
            exe.is_absolute(),
            "a relative schtasks path is a current-directory hijack: {}",
            exe.display()
        );
        let text = exe.display().to_string().to_ascii_lowercase();
        assert!(text.ends_with(r"\system32\schtasks.exe"), "{text}");
        assert!(
            exe.is_file(),
            "the resolved path must actually exist on a real Windows box: {}",
            exe.display()
        );
    }

    /// All four mode×game combinations, and the quoting rule is identical in
    /// both modes — the mode only ever changes the leading verb.
    #[test]
    fn arguments_compose_the_mode_verb_with_the_game_for_all_four_combinations() {
        assert_eq!(spec(TaskMode::Daemon, None).arguments(), "daemon");
        assert_eq!(
            spec(TaskMode::Daemon, Some("Steam")).arguments(),
            "daemon --game Steam"
        );
        assert_eq!(spec(TaskMode::Run, None).arguments(), "run");
        assert_eq!(
            spec(TaskMode::Run, Some("Steam")).arguments(),
            "run --game Steam"
        );
        // A title with spaces is quoted the same way in both modes.
        assert_eq!(
            spec(TaskMode::Daemon, Some("Street Fighter")).arguments(),
            "daemon --game \"Street Fighter\""
        );
        assert_eq!(
            spec(TaskMode::Run, Some("Street Fighter")).arguments(),
            "run --game \"Street Fighter\""
        );
    }

    #[test]
    fn command_line_is_the_quoted_exe_plus_arguments_in_every_combination() {
        for (mode, game, want) in [
            (TaskMode::Daemon, None, "daemon"),
            (TaskMode::Daemon, Some("Steam"), "daemon --game Steam"),
            (TaskMode::Run, None, "run"),
            (TaskMode::Run, Some("Steam"), "run --game Steam"),
        ] {
            assert_eq!(
                spec(mode, game).command_line(),
                format!(r#""C:\Program Files\ksx\ksx.exe" {want}"#)
            );
        }
    }

    /// The daemon is the default mode: a registered `ksx run` captures the
    /// keyboards unconditionally at every logon, which is hostile on a machine
    /// that is also a desktop PC. Nothing deployed depended on the old default
    /// (no cabinet has run the M5 gate), so the default could change safely.
    #[test]
    fn the_default_mode_is_the_daemon_not_a_session() {
        assert_eq!(TaskMode::default(), TaskMode::Daemon);
    }

    /// A title with a quote in it cannot be represented safely; refuse loudly
    /// rather than register an autostart that silently starts the wrong thing.
    /// The refusal is mode-independent.
    #[test]
    fn a_title_containing_a_quote_is_refused_in_both_modes() {
        for mode in BOTH_MODES {
            let bad = spec(mode, Some(r#"Rock"n Roll"#));
            assert_eq!(
                validate(&bad),
                Err(AutostartError::UnquotableTitle(r#"Rock"n Roll"#.to_owned())),
                "{mode:?}"
            );
            assert!(enable_plan(bad).is_err(), "{mode:?}");
        }
    }

    /// The other `CommandLineToArgvW` trap: backslashes immediately before a
    /// closing quote are escapes, so `--game "Foo Bar\"` never closes its
    /// quoted region (odd run) or halves the backslashes (even run). A quoted
    /// title ending in a backslash must be refused, in both modes; an unquoted
    /// one is representable and stays allowed.
    #[test]
    fn a_quoted_title_ending_in_a_backslash_is_refused_in_both_modes() {
        for mode in BOTH_MODES {
            for title in [r"Street Fighter\", r"Street Fighter\\"] {
                let bad = spec(mode, Some(title));
                assert_eq!(
                    validate(&bad),
                    Err(AutostartError::TrailingBackslashTitle(title.to_owned())),
                    "{mode:?} {title}"
                );
                assert!(enable_plan(bad).is_err(), "{mode:?} {title}");
            }
        }
        // No space → never quoted → the trailing backslash survives verbatim.
        let unquoted = spec(TaskMode::Daemon, Some(r"Foo\"));
        assert_eq!(validate(&unquoted), Ok(()));
        assert_eq!(unquoted.arguments(), r"daemon --game Foo\");
    }

    #[test]
    fn extra_args_are_appended_after_the_game() {
        let mut s = spec(TaskMode::Daemon, Some("Steam"));
        s.extra_args = vec!["--latency".into()];
        assert_eq!(s.arguments(), "daemon --game Steam --latency");
    }

    #[test]
    fn xml_is_a_per_user_least_privilege_logon_task() {
        let xml = render_xml(&spec(TaskMode::Daemon, Some("Steam")));
        assert!(xml.contains("<LogonTrigger>"), "{xml}");
        assert!(xml.contains("<UserId>CAB\\victor</UserId>"), "{xml}");
        assert!(
            xml.contains("<LogonType>InteractiveToken</LogonType>"),
            "{xml}"
        );
        assert!(
            xml.contains("<RunLevel>LeastPrivilege</RunLevel>"),
            "the task must NEVER ask for elevation: {xml}"
        );
        assert!(
            !xml.contains("HighestAvailable"),
            "an elevated autostart is exactly what this module refuses to create"
        );
        assert!(xml.contains("<Delay>PT10S</Delay>"), "{xml}");
        assert!(
            xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"),
            "two concurrent sessions would try to plug 8 pads into 4 XInput slots"
        );
        assert!(
            xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"),
            "a play session must not be killed by a default 3-day limit"
        );
        assert!(
            xml.contains("<DisallowStartIfOnBatteries>false"),
            "a UPS-backed cabinet must still autostart"
        );
        assert!(
            xml.contains(r"<Command>C:\Program Files\ksx\ksx.exe</Command>"),
            "{xml}"
        );
        assert!(
            xml.contains("<Arguments>daemon --game Steam</Arguments>"),
            "{xml}"
        );
        assert!(
            xml.contains(r"<WorkingDirectory>C:\Program Files\ksx</WorkingDirectory>"),
            "{xml}"
        );
    }

    #[test]
    fn xml_escapes_titles_that_would_break_the_document() {
        let mut s = spec(TaskMode::Daemon, Some("Tom & Jerry <2>"));
        s.user_id = "CAB\\a&b".into();
        let xml = render_xml(&s);
        assert!(xml.contains("Tom &amp; Jerry &lt;2&gt;"), "{xml}");
        assert!(xml.contains("<UserId>CAB\\a&amp;b</UserId>"), "{xml}");
        // ...and it round-trips back out through the status parser.
        let parsed = parse_registered(&xml);
        assert_eq!(
            parsed.arguments.as_deref(),
            Some("daemon --game \"Tom & Jerry <2>\"")
        );
    }

    /// The inspection round trip, all four combinations: what `render_xml`
    /// writes, `parse_registered` reads back with the right mode and game.
    #[test]
    fn registered_mode_and_game_round_trip_through_the_xml_in_both_modes() {
        for mode in BOTH_MODES {
            for game in [None, Some("Street Fighter")] {
                let task = parse_registered(&render_xml(&spec(mode, game)));
                assert_eq!(task.mode(), Some(mode), "{mode:?} {game:?}");
                assert_eq!(task.game().as_deref(), game, "{mode:?} {game:?}");
            }
        }
    }

    /// Registrations that predate the daemon default say `run …`; they must
    /// still be inspected correctly, not misread or reported as unknown.
    #[test]
    fn a_pre_daemon_registration_of_run_is_still_reported_as_run() {
        let task = RegisteredTask {
            arguments: Some("run --game \"Street Fighter\"".into()),
            ..RegisteredTask::default()
        };
        assert_eq!(task.mode(), Some(TaskMode::Run));
        assert_eq!(task.game().as_deref(), Some("Street Fighter"));
    }

    /// Arguments ksx never wrote parse to no mode, and the status output says
    /// so instead of guessing.
    #[test]
    fn arguments_not_written_by_ksx_have_no_mode() {
        for arguments in [Some("frobnicate --game X"), Some(""), None] {
            let task = RegisteredTask {
                arguments: arguments.map(str::to_owned),
                ..RegisteredTask::default()
            };
            assert_eq!(task.mode(), None, "{arguments:?}");
        }
        let text = render_status(
            DEFAULT_TASK_NAME,
            &Status::Registered(RegisteredTask {
                arguments: Some("frobnicate".into()),
                ..RegisteredTask::default()
            }),
        );
        assert!(text.contains("unknown"), "{text}");
    }

    #[test]
    fn schtasks_argv_is_idempotent_and_targets_the_named_task() {
        let argv = create_argv("ksx\\autostart", Path::new(r"C:\Temp\t.xml"));
        assert_eq!(argv[0], "/Create");
        assert!(
            argv.contains(&"/F".to_owned()),
            "must overwrite, not prompt"
        );
        assert!(argv.contains(&"ksx\\autostart".to_owned()));
        assert!(argv.contains(&r"C:\Temp\t.xml".to_owned()));
        assert!(!argv.iter().any(|a| a == "/RU" || a == "/RP"), "{argv:?}");

        assert_eq!(
            delete_argv("ksx\\autostart"),
            vec!["/Delete", "/TN", "ksx\\autostart", "/F"]
        );
        assert_eq!(
            query_argv("ksx\\autostart"),
            vec!["/Query", "/TN", "ksx\\autostart", "/XML", "ONE"]
        );
    }

    /// A real `schtasks /Query /XML ONE` document, trimmed.
    const QUERY_OUTPUT: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>CAB\victor</UserId>
    </LogonTrigger>
  </Triggers>
  <Actions Context="Author">
    <Exec>
      <Command>C:\Program Files\ksx\ksx.exe</Command>
      <Arguments>run --game "Street Fighter"</Arguments>
      <WorkingDirectory>C:\Program Files\ksx</WorkingDirectory>
    </Exec>
  </Actions>
</Task>"#;

    #[test]
    fn status_parsing_reports_what_the_task_points_at() {
        let task = parse_registered(QUERY_OUTPUT);
        assert_eq!(
            task.command.as_deref(),
            Some(r"C:\Program Files\ksx\ksx.exe")
        );
        assert_eq!(task.mode(), Some(TaskMode::Run));
        assert_eq!(task.game().as_deref(), Some("Street Fighter"));
        assert_eq!(task.user_id.as_deref(), Some("CAB\\victor"));
        assert_eq!(task.enabled, Some(true));

        // The mode is surfaced in both the human text and the JSON.
        let json = status_json(DEFAULT_TASK_NAME, &Status::Registered(task.clone()));
        assert_eq!(json.pointer("/mode"), Some(&serde_json::json!("run")));
        let text = render_status(DEFAULT_TASK_NAME, &Status::Registered(task));
        assert!(text.contains("Street Fighter"), "{text}");
        assert!(
            text.contains("run (captures keyboards"),
            "the mode must be in the human status: {text}"
        );
        assert!(text.contains("--disable"), "{text}");
    }

    #[test]
    fn a_daemon_registration_is_surfaced_in_status_and_json() {
        let task = parse_registered(&render_xml(&spec(TaskMode::Daemon, Some("Steam"))));
        let json = status_json(DEFAULT_TASK_NAME, &Status::Registered(task.clone()));
        assert_eq!(json.pointer("/mode"), Some(&serde_json::json!("daemon")));
        assert_eq!(json.pointer("/game"), Some(&serde_json::json!("Steam")));
        let text = render_status(DEFAULT_TASK_NAME, &Status::Registered(task));
        assert!(text.contains("daemon (tray icon at logon"), "{text}");
        assert!(text.contains("Steam"), "{text}");
    }

    #[test]
    fn an_unquoted_game_title_is_still_read_back_in_both_modes() {
        for verb in ["run", "daemon"] {
            let task = RegisteredTask {
                arguments: Some(format!("{verb} --game Steam --latency")),
                ..RegisteredTask::default()
            };
            assert_eq!(task.game().as_deref(), Some("Steam"), "{verb}");

            let plain = RegisteredTask {
                arguments: Some(verb.to_owned()),
                ..RegisteredTask::default()
            };
            assert_eq!(plain.game(), None, "{verb}");
        }
    }

    /// The silent killer: ksx moved, the task did not, and the cabinet boots to
    /// a desktop with no error anyone will ever see.
    #[test]
    fn a_registered_task_pointing_at_a_moved_exe_is_reported_as_stale() {
        let task = RegisteredTask {
            command: Some(r"C:\old\ksx.exe".into()),
            ..RegisteredTask::default()
        };
        let stale = check_staleness(&task, Path::new(r"C:\new\ksx.exe"), |_| false);
        assert_eq!(
            stale,
            Staleness::MissingExe {
                registered: r"C:\old\ksx.exe".to_owned()
            }
        );
        assert!(stale.is_broken());
        let message = stale.message().unwrap();
        assert!(message.contains("STALE"), "{message}");
        assert!(message.contains("boot to nothing"), "{message}");
        assert!(message.contains("--enable"), "{message}");
    }

    #[test]
    fn a_task_pointing_at_a_different_but_present_ksx_is_reported_not_condemned() {
        let task = RegisteredTask {
            command: Some(r"C:\other\ksx.exe".into()),
            ..RegisteredTask::default()
        };
        let stale = check_staleness(&task, Path::new(r"C:\here\ksx.exe"), |_| true);
        assert!(matches!(stale, Staleness::DifferentExe { .. }));
        assert!(
            !stale.is_broken(),
            "two installs is a legitimate setup, not a fault"
        );
    }

    #[test]
    fn the_same_exe_in_different_spelling_is_not_stale() {
        for registered in [
            r"C:\Program Files\ksx\ksx.exe",
            r"c:\program files\ksx\ksx.exe",
            "C:/Program Files/ksx/ksx.exe",
        ] {
            let task = RegisteredTask {
                command: Some(registered.into()),
                ..RegisteredTask::default()
            };
            assert_eq!(
                check_staleness(&task, Path::new(r"C:\Program Files\ksx\ksx.exe"), |_| true),
                Staleness::Current,
                "{registered}"
            );
        }
    }

    #[test]
    fn a_task_with_no_exec_action_is_broken() {
        let stale = check_staleness(&RegisteredTask::default(), Path::new("x"), |_| true);
        assert_eq!(stale, Staleness::NoCommand);
        assert!(stale.is_broken());
    }

    #[test]
    fn not_registered_status_tells_you_how_to_enable_it() {
        let text = render_status(DEFAULT_TASK_NAME, &Status::NotRegistered);
        assert!(text.contains("NOT registered"), "{text}");
        assert!(text.contains("--enable"), "{text}");
        assert!(!Status::NotRegistered.is_registered());
        let v = status_json(DEFAULT_TASK_NAME, &Status::NotRegistered);
        assert_eq!(v.pointer("/registered"), Some(&serde_json::json!(false)));
    }

    #[test]
    fn schtasks_not_found_output_is_recognised_as_not_registered() {
        for message in [
            "ERROR: The system cannot find the file specified.",
            "ERROR: The specified task name \"ksx\\autostart\" does not exist in the system.",
        ] {
            assert!(is_not_found(message), "{message}");
        }
        assert!(!is_not_found("ERROR: Access is denied."));
    }

    #[test]
    fn utf16_round_trip_matches_what_schtasks_expects() {
        let bytes = to_utf16le_bom("<Task/>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "BOM is mandatory for /XML");
        assert_eq!(decode_console_output(&bytes), "<Task/>");
        // ...and plain UTF-8 output still decodes.
        assert_eq!(
            decode_console_output(b"SUCCESS: task created"),
            "SUCCESS: task created"
        );
        // ...as does BOM-less UTF-16, which some schtasks builds emit.
        let bomless: Vec<u8> = "<?xml version=\"1.0\"?>"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode_console_output(&bomless), "<?xml version=\"1.0\"?>");
    }

    #[test]
    fn dry_run_prints_the_exact_xml_and_command() {
        let plan = enable_plan(spec(TaskMode::Daemon, Some("Steam"))).unwrap();
        let text = plan.render_human(true);
        assert!(text.contains("schtasks /Create"), "{text}");
        assert!(text.contains("<LogonTrigger>"), "{text}");
        assert!(text.contains("nothing was registered"), "{text}");
        assert!(text.contains("LeastPrivilege"), "{text}");
        assert!(
            text.contains("mode:         daemon (tray icon at logon"),
            "{text}"
        );

        let v = plan.to_json(true);
        assert_eq!(v.pointer("/action"), Some(&serde_json::json!("dry-run")));
        assert_eq!(v.pointer("/elevated"), Some(&serde_json::json!(false)));
        assert_eq!(v.pointer("/mode"), Some(&serde_json::json!("daemon")));
        assert_eq!(
            v.pointer("/arguments"),
            Some(&serde_json::json!("daemon --game Steam"))
        );
        assert!(v
            .pointer("/xml")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("<Task"));
    }

    #[test]
    fn the_temp_xml_path_never_contains_a_path_separator_from_the_task_name() {
        let plan = enable_plan(spec(TaskMode::Daemon, None)).unwrap();
        let name = plan.xml_path.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("ksx-autostart-ksx-autostart-") && name.ends_with(".xml"),
            "{name}"
        );
        assert!(!name.contains('\\') && !name.contains('/'), "{name}");
    }

    /// The temp XML lands in a user-writable `%TEMP%` and is then re-opened by
    /// `schtasks`, possibly elevated. Two invocations must not agree on the
    /// name, or an attacker can pre-create it and win the race every time.
    #[test]
    fn two_plans_never_pick_the_same_temp_xml_name() {
        let a = enable_plan(spec(TaskMode::Daemon, None)).unwrap().xml_path;
        let b = enable_plan(spec(TaskMode::Daemon, None)).unwrap().xml_path;
        assert_ne!(a, b, "a predictable drop-box name is the whole attack");
    }

    /// ...and if someone gets there anyway, `apply` refuses instead of writing
    /// through whatever is sitting at that name (a symlink to a file we would
    /// then truncate with an administrator token, in the worst case).
    #[cfg(windows)]
    #[test]
    fn apply_refuses_a_pre_planted_temp_file_instead_of_overwriting_it() {
        let plan = enable_plan(spec(TaskMode::Daemon, None)).unwrap();
        std::fs::write(&plan.xml_path, b"planted").unwrap();

        let err = apply(&plan).expect_err("a pre-existing drop box must be refused");
        assert!(
            matches!(err, AutostartError::SchtasksUnavailable(ref m) if m.contains("exclusively")),
            "{err}"
        );
        // The planted bytes are untouched: nothing was written through it.
        assert_eq!(std::fs::read(&plan.xml_path).unwrap(), b"planted");
        let _ = std::fs::remove_file(&plan.xml_path);
    }
}
