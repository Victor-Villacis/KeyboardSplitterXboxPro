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
//! a system account does — so this module never does. `ksx run` does not need
//! admin either (Interception and ViGEmBus are already installed by then), and
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

/// What the registered task should run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    pub task_name: String,
    /// Absolute path to `ksx.exe`.
    pub exe: PathBuf,
    /// `ksx run --game <TITLE>` when set, plain `ksx run` otherwise.
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
    /// A title with spaces is quoted; a title containing a quote is refused
    /// upstream (see [`validate`]) rather than escaped, because Task Scheduler
    /// hands this string to `CommandLineToArgvW` and a half-escaped title is a
    /// silently-wrong autostart nobody would notice until a cold boot.
    pub fn arguments(&self) -> String {
        let mut parts = vec!["run".to_owned()];
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
    game: Option<String>,
    extra_args: Vec<String>,
    delay_secs: u32,
    task_name: Option<String>,
) -> Result<TaskSpec, AutostartError> {
    let exe = std::env::current_exe().map_err(|e| AutostartError::NoExePath(e.to_string()))?;
    let spec = TaskSpec {
        task_name: task_name.unwrap_or_else(|| DEFAULT_TASK_NAME.to_owned()),
        exe,
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
pub fn enable_plan(spec: TaskSpec) -> Result<EnablePlan, AutostartError> {
    validate(&spec)?;
    let xml = render_xml(&spec);
    let xml_path = std::env::temp_dir().join(format!(
        "ksx-autostart-{}.xml",
        spec.task_name.replace(['\\', '/', ':'], "-")
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

/// Run `schtasks` with `argv`, returning its decoded stdout.
#[cfg(windows)]
fn schtasks(argv: &[String]) -> Result<String, AutostartError> {
    let out = std::process::Command::new("schtasks.exe")
        .args(argv)
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
    std::fs::write(&plan.xml_path, to_utf16le_bom(&plan.xml))
        .map_err(|e| AutostartError::SchtasksUnavailable(e.to_string()))?;
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
             enable it with `ksx autostart --enable` (add --game \"Title\" to start a profile)"
        ),
        Status::Registered(task) => {
            use std::fmt::Write as _;
            let mut out = format!("autostart: registered as '{task_name}'\n");
            let _ = writeln!(out, "  runs:        {}", task.command_line());
            if let Some(game) = task.game() {
                let _ = writeln!(out, "  game:        {game}");
            } else {
                let _ = writeln!(out, "  game:        (none — plain `ksx run`)");
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

    fn spec(game: Option<&str>) -> TaskSpec {
        TaskSpec {
            task_name: DEFAULT_TASK_NAME.to_owned(),
            exe: PathBuf::from(r"C:\Program Files\ksx\ksx.exe"),
            game: game.map(str::to_owned),
            extra_args: Vec::new(),
            user_id: "CAB\\victor".to_owned(),
            delay_secs: 10,
        }
    }

    #[test]
    fn arguments_quote_a_title_with_spaces_and_omit_game_when_absent() {
        assert_eq!(spec(None).arguments(), "run");
        assert_eq!(spec(Some("Steam")).arguments(), "run --game Steam");
        assert_eq!(
            spec(Some("Street Fighter")).arguments(),
            "run --game \"Street Fighter\""
        );
    }

    /// A title with a quote in it cannot be represented safely; refuse loudly
    /// rather than register an autostart that silently starts the wrong thing.
    #[test]
    fn a_title_containing_a_quote_is_refused() {
        let bad = spec(Some(r#"Rock"n Roll"#));
        assert_eq!(
            validate(&bad),
            Err(AutostartError::UnquotableTitle(r#"Rock"n Roll"#.to_owned()))
        );
        assert!(enable_plan(bad).is_err());
    }

    #[test]
    fn extra_args_are_appended_after_the_game() {
        let mut s = spec(Some("Steam"));
        s.extra_args = vec!["--latency".into()];
        assert_eq!(s.arguments(), "run --game Steam --latency");
    }

    #[test]
    fn xml_is_a_per_user_least_privilege_logon_task() {
        let xml = render_xml(&spec(Some("Steam")));
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
            xml.contains("<Arguments>run --game Steam</Arguments>"),
            "{xml}"
        );
        assert!(
            xml.contains(r"<WorkingDirectory>C:\Program Files\ksx</WorkingDirectory>"),
            "{xml}"
        );
    }

    #[test]
    fn xml_escapes_titles_that_would_break_the_document() {
        let mut s = spec(Some("Tom & Jerry <2>"));
        s.user_id = "CAB\\a&b".into();
        let xml = render_xml(&s);
        assert!(xml.contains("Tom &amp; Jerry &lt;2&gt;"), "{xml}");
        assert!(xml.contains("<UserId>CAB\\a&amp;b</UserId>"), "{xml}");
        // ...and it round-trips back out through the status parser.
        let parsed = parse_registered(&xml);
        assert_eq!(
            parsed.arguments.as_deref(),
            Some("run --game \"Tom & Jerry <2>\"")
        );
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
        assert_eq!(task.game().as_deref(), Some("Street Fighter"));
        assert_eq!(task.user_id.as_deref(), Some("CAB\\victor"));
        assert_eq!(task.enabled, Some(true));
        let text = render_status(DEFAULT_TASK_NAME, &Status::Registered(task));
        assert!(text.contains("Street Fighter"), "{text}");
        assert!(text.contains("--disable"), "{text}");
    }

    #[test]
    fn an_unquoted_game_title_is_still_read_back() {
        let task = RegisteredTask {
            arguments: Some("run --game Steam --latency".into()),
            ..RegisteredTask::default()
        };
        assert_eq!(task.game().as_deref(), Some("Steam"));

        let plain = RegisteredTask {
            arguments: Some("run".into()),
            ..RegisteredTask::default()
        };
        assert_eq!(plain.game(), None);
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
        let plan = enable_plan(spec(Some("Steam"))).unwrap();
        let text = plan.render_human(true);
        assert!(text.contains("schtasks /Create"), "{text}");
        assert!(text.contains("<LogonTrigger>"), "{text}");
        assert!(text.contains("nothing was registered"), "{text}");
        assert!(text.contains("LeastPrivilege"), "{text}");

        let v = plan.to_json(true);
        assert_eq!(v.pointer("/action"), Some(&serde_json::json!("dry-run")));
        assert_eq!(v.pointer("/elevated"), Some(&serde_json::json!(false)));
        assert_eq!(
            v.pointer("/arguments"),
            Some(&serde_json::json!("run --game Steam"))
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
        let plan = enable_plan(spec(None)).unwrap();
        let name = plan.xml_path.file_name().unwrap().to_string_lossy();
        assert_eq!(name, "ksx-autostart-ksx-autostart.xml", "{name}");
    }
}
