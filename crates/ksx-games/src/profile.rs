//! A `[[game]]` profile turned into something launchable.
//!
//! Two shapes come out of `games.toml`, and they behave completely differently
//! once started:
//!
//! - **An executable.** ksx spawns it and holds a process handle. Liveness is a
//!   fact, not a guess.
//! - **A protocol URL** (`steam://rungameid/…`, `com.epicgames.launcher://…`).
//!   There is nothing to spawn — the shell hands the URL to an already-running
//!   launcher, which returns immediately. ksx never gets a handle to anything,
//!   so the only way to know when the game ends is to watch for a process by
//!   name. That is what `process_name` is for, and why a protocol profile
//!   without one cannot detect its own exit.
//!
//! Everything in this module is pure: no spawning, no filesystem beyond the
//! one existence check that `preflight` is *for*.

use std::path::{Path, PathBuf};

use ksx_config::GameEntry;

/// Protocol schemes ksx recognises as "hand this to the shell, then track by
/// name". Anything with a `scheme:` prefix that is not a Windows drive letter
/// is treated the same way; this list only drives the friendlier messages.
pub const KNOWN_LAUNCHER_SCHEMES: &[(&str, &str)] = &[
    ("steam:", "Steam"),
    ("com.epicgames.launcher:", "the Epic Games Launcher"),
    ("uplay:", "Ubisoft Connect"),
    ("origin:", "EA/Origin"),
    ("gog:", "GOG Galaxy"),
    ("battlenet:", "Battle.net"),
    ("minecraft:", "the Minecraft launcher"),
];

/// What ksx will actually do to start the game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    /// Spawned directly; ksx holds the process handle.
    Executable {
        exe: PathBuf,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
    },
    /// Handed to the shell (`ShellExecuteW`). Returns immediately; there is no
    /// process to hold.
    Protocol { url: String, launcher: &'static str },
}

impl LaunchTarget {
    pub fn is_protocol(&self) -> bool {
        matches!(self, LaunchTarget::Protocol { .. })
    }

    /// One line for the plan/`--dry-run` output.
    pub fn describe(&self) -> String {
        match self {
            LaunchTarget::Executable { exe, args, .. } if args.is_empty() => {
                format!("run {}", exe.display())
            }
            LaunchTarget::Executable { exe, args, .. } => {
                format!("run {} {}", exe.display(), args.join(" "))
            }
            LaunchTarget::Protocol { url, launcher } => {
                format!("open {url} (handled by {launcher})")
            }
        }
    }
}

/// A profile, resolved into a launch plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub title: String,
    pub target: LaunchTarget,
    /// The image name to watch for once the launched thing is gone
    /// (`mame.exe`). Optional for executables, load-bearing for protocol URLs.
    pub process_name: Option<String>,
    /// Per-profile override for "how long may the launched program live and
    /// still be a launcher" (`launcher_grace_ms`). `None` = the default.
    pub launcher_grace_ms: Option<u64>,
}

impl LaunchSpec {
    pub fn from_entry(entry: &GameEntry) -> Self {
        let raw = entry.path.trim();
        let target = match launcher_for(raw) {
            Some(launcher) => LaunchTarget::Protocol {
                url: raw.to_owned(),
                launcher,
            },
            None => LaunchTarget::Executable {
                exe: PathBuf::from(raw),
                args: split_args(&entry.arguments),
                working_dir: None,
            },
        };
        Self {
            title: entry.title.clone(),
            target,
            process_name: entry
                .process_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            launcher_grace_ms: entry.launcher_grace_ms,
        }
    }

    /// The image name this profile's own executable would have — the obvious
    /// `process_name` candidate to suggest when there is none.
    ///
    /// `Some("steam.exe")` for `C:\Program Files (x86)\Steam\steam.exe`, `None`
    /// for a protocol URL (where the launcher's image name is not the game's,
    /// and guessing would be worse than saying nothing).
    pub fn exe_file_name(&self) -> Option<String> {
        self.exe()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
    }

    /// The executable path, when there is one.
    pub fn exe(&self) -> Option<&Path> {
        match &self.target {
            LaunchTarget::Executable { exe, .. } => Some(exe),
            LaunchTarget::Protocol { .. } => None,
        }
    }
}

/// `steam://…` yes, `C:\games\x.exe` no.
///
/// The drive-letter exclusion is the whole subtlety: `C:` *is* a scheme by
/// URL grammar, and treating `C:\Program Files\Steam\steam.exe` as a protocol
/// would hand a perfectly good executable to `ShellExecute` and lose the
/// process handle with it.
pub fn launcher_for(path: &str) -> Option<&'static str> {
    let scheme_end = path.find(':')? + 1;
    if scheme_end <= 2 {
        return None; // "C:" — a drive, not a scheme
    }
    let scheme = &path[..scheme_end];
    for (known, label) in KNOWN_LAUNCHER_SCHEMES {
        if scheme.eq_ignore_ascii_case(known) {
            return Some(label);
        }
    }
    // An unknown but well-formed scheme is still a protocol: ksx should not
    // have to ship a list of every storefront that will ever exist.
    let name = &path[..scheme_end - 1];
    if !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && path[scheme_end..].starts_with('/')
    {
        return Some("the registered protocol handler");
    }
    None
}

/// Split a legacy `Arguments` string into argv the way `CommandLineToArgvW`
/// does for the parts that matter: double quotes group, `\"` is a literal
/// quote, whitespace separates.
///
/// Not a full reimplementation of the backslash-run rules — those only differ
/// inside runs of backslashes immediately before a quote, which no games.toml
/// in the wild contains. Anything stranger belongs in a launcher script, and
/// the profile can point at that instead.
pub fn split_args(arguments: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut started = false;
    let mut chars = arguments.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'"') => {
                chars.next();
                current.push('"');
                started = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// Why a profile cannot be launched. Both are found **before** anything is
/// plugged, so both map to exit code 2.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PreflightError {
    #[error(
        "game profile '{title}' has an empty `path`; set it to the game's .exe or a \
         launcher URL in games.toml"
    )]
    NoPath { title: String },
    #[error(
        "game profile '{title}' points at '{path}', which does not exist. Fix `path` in \
         games.toml, or run without --game to use the config's own [[slot]] layout"
    )]
    ExeMissing { title: String, path: String },
    #[error("game profile '{title}' points at '{path}', which is a directory, not a program")]
    NotAFile { title: String, path: String },
}

/// Check what can be checked before a single pad is plugged.
///
/// A protocol URL is unverifiable by construction (only the shell knows whether
/// `steam://rungameid/9999` names a real game), so it always passes here and
/// fails — if it fails — at activation time, which is a runtime failure.
pub fn preflight(spec: &LaunchSpec) -> Result<(), PreflightError> {
    match &spec.target {
        LaunchTarget::Protocol { .. } => Ok(()),
        LaunchTarget::Executable { exe, .. } => {
            if exe.as_os_str().is_empty() {
                return Err(PreflightError::NoPath {
                    title: spec.title.clone(),
                });
            }
            if exe.is_dir() {
                return Err(PreflightError::NotAFile {
                    title: spec.title.clone(),
                    path: exe.display().to_string(),
                });
            }
            if !exe.is_file() {
                return Err(PreflightError::ExeMissing {
                    title: spec.title.clone(),
                    path: exe.display().to_string(),
                });
            }
            Ok(())
        }
    }
}

/// The warning a profile with no `process_name` gets — once, loudly, naming the
/// exact file and the exact line to add.
///
/// Deliberately **not** a refusal (an approved judgement call): the session is
/// perfectly usable, the pads work, and every emergency escape still ends it.
/// Refusing to run a Steam profile because ksx cannot detect its exit would
/// turn a cosmetic gap into a cabinet that will not start.
///
/// # Two profiles, two causes, two texts
///
/// This give-up has two entirely different origins and they must not share a
/// sentence:
///
/// - a **protocol URL** (`steam://…`): nothing was ever spawned, the shell
///   handed the URL to a launcher that returned at once, and ksx never had a
///   handle to anything;
/// - an **executable** (`C:\Program Files (x86)\Steam\steam.exe`): ksx *did*
///   spawn it and *did* hold a handle — the program simply exited quickly,
///   because it handed the request to an already-running client.
///
/// Printing the first for the second is the bug this function was split to
/// fix: the cabinet's `path = "C:\Program Files (x86)\Steam\steam.exe"` profile
/// was told "profile 'Steam' starts a URL", which is false, and sent its owner
/// looking for a URL that does not exist in their file.
pub fn missing_process_name_warning(spec: &LaunchSpec, games_toml: &Path) -> String {
    match &spec.target {
        LaunchTarget::Protocol { launcher, .. } => {
            protocol_no_process_name(spec, games_toml, launcher)
        }
        LaunchTarget::Executable { .. } => launcher_exited_no_process_name(spec, games_toml),
    }
}

/// A `steam://`-style profile: no handle ever existed.
fn protocol_no_process_name(spec: &LaunchSpec, games_toml: &Path, launcher: &str) -> String {
    format!(
        "[WARN] profile '{title}' starts a URL, so {launcher} returns immediately and ksx \
         never gets a handle to the game. Without `process_name` it cannot tell when you \
         quit, so emulation will keep running until you press an emergency escape \
         (LeftCtrl x5, or Ctrl+Alt+Del to stop).\n\
         {fix}",
        title = spec.title,
        fix = add_process_name_block(spec, games_toml, "YourGame.exe"),
    )
}

/// An `.exe` profile whose program handed off and exited.
///
/// The suggested value is the profile's **own** image name, because that is
/// nearly always right for this shape: a `steam.exe` that exits handed off to
/// the `steam.exe` that is already running, and watching for `steam.exe` ends
/// the session when Steam closes. (For a profile that launches a specific game
/// through a store client, the game's own image name is better still — the text
/// says so.)
fn launcher_exited_no_process_name(spec: &LaunchSpec, games_toml: &Path) -> String {
    let exe = spec
        .exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "the program".to_owned());
    let suggestion = spec
        .exe_file_name()
        .unwrap_or_else(|| "YourGame.exe".into());
    format!(
        "[WARN] profile '{title}': the program ksx started ({exe}) exited almost immediately \
         and handed off to something else — typically a launcher passing the request to a \
         copy of itself that was already running. The game is still starting; ksx just has \
         no handle on it any more.\n\
         [WARN] Without `process_name`, ksx cannot tell when the game closes, so emulation \
         will keep running until you press an emergency escape (LeftCtrl x5, or \
         Ctrl+Alt+Del to stop). This is not an error and nothing has been stopped.\n\
         {fix}",
        title = spec.title,
        fix = add_process_name_block(spec, games_toml, &suggestion),
    )
}

/// The shared "here is the exact file and the exact line" block.
///
/// One implementation so the two texts above can differ in diagnosis without
/// drifting in the fix — the fix genuinely is the same key in the same file.
fn add_process_name_block(spec: &LaunchSpec, games_toml: &Path, suggestion: &str) -> String {
    format!(
        "[WARN] To fix it, add the game's image name to {path}:\n\
         [WARN]\n\
         [WARN]     [[game]]\n\
         [WARN]     title = \"{title}\"\n\
         [WARN]     process_name = \"{suggestion}\"   # <- add this line\n\
         [WARN]\n\
         [WARN] Find the name in Task Manager > Details while the game is running.",
        title = spec.title,
        path = games_toml.display(),
    )
}

/// The warning for the *other* give-up: `process_name` was set, ksx hunted for
/// it for the whole grace period, and it never appeared.
///
/// This is a different problem from a missing `process_name` and needs
/// different advice — telling somebody to add a key they already added is how a
/// diagnostic becomes noise. The overwhelmingly likely causes are a misspelled
/// image name and a launcher that never actually started the game.
///
/// Like [`missing_process_name_warning`], the *opening clause* branches on what
/// was actually launched: a protocol profile has no "program ksx started" to
/// speak of, and saying it did is the same copy-paste error in a different
/// place.
pub fn handoff_timed_out_warning(spec: &LaunchSpec, games_toml: &Path, grace_ms: u64) -> String {
    let wanted = spec.process_name.as_deref().unwrap_or("<unset>");
    let what_happened = match &spec.target {
        LaunchTarget::Protocol { url, launcher } => format!(
            "ksx handed {url} to {launcher}, which returned immediately (as it always does)"
        ),
        LaunchTarget::Executable { exe, .. } => format!(
            "the program ksx started ({}) exited quickly, so it was a launcher",
            exe.display()
        ),
    };
    format!(
        "[WARN] profile '{title}': {what_happened}, and no process named '{wanted}' appeared \
         within {secs} s. ksx has stopped looking, so it cannot tell when you quit — \
         emulation will keep running until you press an emergency escape (LeftCtrl x5, or \
         Ctrl+Alt+Del to stop).\n\
         [WARN] The pads still work; this only affects automatic shutdown. Usually one of:\n\
         [WARN]   - `process_name` in {path} does not match the real image name\n\
         [WARN]     (check Task Manager > Details while the game is running), or\n\
         [WARN]   - the launcher never started the game (an update, a login prompt, a crash), \
         or\n\
         [WARN]   - the game takes longer than {secs} s to appear: raise `handoff` patience by \
         starting it once by hand to see how long it really needs.",
        title = spec.title,
        secs = grace_ms / 1_000,
        path = games_toml.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, args: &str, process_name: Option<&str>) -> GameEntry {
        GameEntry {
            title: "Test".into(),
            notes: String::new(),
            path: path.into(),
            arguments: args.into(),
            process_name: process_name.map(str::to_owned),
            launcher_grace_ms: None,
            block_keyboards: true,
            block_mice: false,
            slots: Vec::new(),
        }
    }

    /// The cabinet's exact profile shape.
    fn steam_exe_entry() -> GameEntry {
        GameEntry {
            title: "Steam".into(),
            path: r"C:\Program Files (x86)\Steam\steam.exe".into(),
            ..entry("", "", None)
        }
    }

    #[test]
    fn an_exe_profile_becomes_a_spawnable_target() {
        let spec = LaunchSpec::from_entry(&entry(
            r"C:\Program Files (x86)\Steam\steam.exe",
            "-applaunch 400",
            Some("portal2.exe"),
        ));
        assert_eq!(
            spec.target,
            LaunchTarget::Executable {
                exe: PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe"),
                args: vec!["-applaunch".into(), "400".into()],
                working_dir: None,
            }
        );
        assert_eq!(spec.process_name.as_deref(), Some("portal2.exe"));
        assert!(!spec.target.is_protocol());
    }

    /// The bug this exclusion prevents: a Windows path is not a URL.
    #[test]
    fn a_drive_letter_is_never_mistaken_for_a_protocol() {
        for path in [
            r"C:\games\x.exe",
            r"c:/games/x.exe",
            r"D:\Program Files\Steam\steam.exe",
        ] {
            assert_eq!(launcher_for(path), None, "{path}");
            assert!(!LaunchSpec::from_entry(&entry(path, "", None))
                .target
                .is_protocol());
        }
    }

    #[test]
    fn launcher_urls_are_recognised_by_scheme() {
        assert_eq!(launcher_for("steam://rungameid/620"), Some("Steam"));
        assert_eq!(launcher_for("STEAM://rungameid/620"), Some("Steam"));
        assert_eq!(
            launcher_for("com.epicgames.launcher://apps/abc?action=launch"),
            Some("the Epic Games Launcher")
        );
        assert_eq!(
            launcher_for("someshop://play/1"),
            Some("the registered protocol handler"),
            "an unknown storefront is still a protocol"
        );
        assert_eq!(launcher_for("notaurl"), None);
        assert_eq!(launcher_for(""), None);
        // A scheme with no `//` is not something to shell out to blindly.
        assert_eq!(launcher_for("mailto:a@b.c"), None);
    }

    #[test]
    fn argument_splitting_survives_quoted_paths() {
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("   "), Vec::<String>::new());
        assert_eq!(split_args("-a -b"), vec!["-a", "-b"]);
        assert_eq!(
            split_args(r#"-rompath "C:\My Roms" -nowindow"#),
            vec!["-rompath", r"C:\My Roms", "-nowindow"]
        );
        assert_eq!(
            split_args(r#"-title "Say \"hi\"""#),
            vec!["-title", r#"Say "hi""#]
        );
        // An empty quoted argument is a real argument.
        assert_eq!(split_args(r#"-x "" -y"#), vec!["-x", "", "-y"]);
    }

    #[test]
    fn preflight_catches_a_missing_exe_before_anything_is_plugged() {
        let missing = std::env::temp_dir().join("ksx-no-such-game-71fe.exe");
        let spec = LaunchSpec::from_entry(&entry(&missing.display().to_string(), "", None));
        let err = preflight(&spec).unwrap_err();
        assert!(matches!(err, PreflightError::ExeMissing { .. }));
        assert!(err.to_string().contains("games.toml"), "{err}");

        let empty = LaunchSpec::from_entry(&entry("", "", None));
        assert!(matches!(
            preflight(&empty),
            Err(PreflightError::NoPath { .. })
        ));

        let dir = LaunchSpec::from_entry(&entry(
            &std::env::temp_dir().display().to_string(),
            "",
            None,
        ));
        assert!(matches!(
            preflight(&dir),
            Err(PreflightError::NotAFile { .. })
        ));
    }

    /// A `steam://` URL cannot be checked ahead of time, and pretending
    /// otherwise would refuse every working Steam profile.
    #[test]
    fn a_protocol_url_always_passes_preflight() {
        let spec = LaunchSpec::from_entry(&entry("steam://rungameid/620", "", None));
        assert!(preflight(&spec).is_ok());
    }

    #[test]
    fn the_missing_process_name_warning_names_the_file_and_the_key() {
        let spec = LaunchSpec::from_entry(&entry("steam://rungameid/620", "", None));
        let text = missing_process_name_warning(&spec, Path::new(r"C:\cfg\ksx\games.toml"));
        assert!(text.contains(r"C:\cfg\ksx\games.toml"), "{text}");
        assert!(text.contains("process_name = "), "{text}");
        assert!(text.contains("title = \"Test\""), "{text}");
        assert!(text.contains("LeftCtrl x5"), "the way out must be stated");
        assert!(text.contains("Steam"), "{text}");
    }

    #[test]
    fn a_blank_process_name_is_the_same_as_none() {
        let spec = LaunchSpec::from_entry(&entry("steam://x/1", "", Some("   ")));
        assert_eq!(spec.process_name, None);
    }

    /// **The cabinet's wrong-diagnosis bug.** With
    /// `path = "C:\Program Files (x86)\Steam\steam.exe"` and no `process_name`,
    /// ksx printed *"profile 'Steam' starts a URL, so Steam returns
    /// immediately"*. It is not a URL — the exe branch was reusing the protocol
    /// branch's sentence, which sends the user looking for a URL their file does
    /// not contain.
    #[test]
    fn an_exe_profile_is_never_told_that_it_starts_a_url() {
        let spec = LaunchSpec::from_entry(&steam_exe_entry());
        let text = missing_process_name_warning(&spec, Path::new(r"C:\cfg\ksx\games.toml"));

        assert!(
            !text.contains("starts a URL"),
            "an .exe profile does not start a URL:\n{text}"
        );
        // What it must say instead: the launcher exited and handed off...
        assert!(text.contains("handed off"), "{text}");
        assert!(
            text.contains(r"C:\Program Files (x86)\Steam\steam.exe"),
            "the program that exited must be named:\n{text}"
        );
        // ...that this is not a failure...
        assert!(text.contains("nothing has been stopped"), "{text}");
        // ...that exit detection is what is lost...
        assert!(text.contains("cannot tell when the game closes"), "{text}");
        // ...and the exact file plus the exact line to add, with the right value
        // already filled in.
        assert!(text.contains(r"C:\cfg\ksx\games.toml"), "{text}");
        assert!(
            text.contains("process_name = \"steam.exe\"   # <- add this line"),
            "the suggested value must be this profile's own image name:\n{text}"
        );
        assert!(text.contains("title = \"Steam\""), "{text}");
        assert!(text.contains("LeftCtrl x5"), "the way out must be stated");
    }

    /// ...and the protocol text is unchanged, because for a `steam://` profile
    /// it was correct all along.
    #[test]
    fn a_protocol_profile_still_gets_the_url_wording() {
        let spec = LaunchSpec::from_entry(&entry("steam://rungameid/620", "", None));
        let text = missing_process_name_warning(&spec, Path::new(r"C:\cfg\ksx\games.toml"));
        assert!(text.contains("starts a URL"), "{text}");
        assert!(text.contains("Steam returns immediately"), "{text}");
        // No exe to name, so no exe name is guessed at.
        assert!(text.contains("process_name = \"YourGame.exe\""), "{text}");
    }

    /// The same copy-paste, found by audit in the *other* give-up message: a
    /// protocol profile has no "program ksx started" to have exited.
    #[test]
    fn the_handoff_timeout_text_matches_what_was_actually_launched() {
        let url = LaunchSpec::from_entry(&entry("steam://rungameid/620", "", Some("portal2.exe")));
        let text = handoff_timed_out_warning(&url, Path::new(r"C:\cfg\ksx\games.toml"), 60_000);
        assert!(
            !text.contains("the program ksx started"),
            "nothing was started for a URL profile:\n{text}"
        );
        assert!(text.contains("steam://rungameid/620"), "{text}");
        assert!(text.contains("portal2.exe"), "{text}");

        let exe = LaunchSpec::from_entry(&GameEntry {
            process_name: Some("portla2.exe".into()),
            ..steam_exe_entry()
        });
        let text = handoff_timed_out_warning(&exe, Path::new(r"C:\cfg\ksx\games.toml"), 60_000);
        assert!(text.contains("the program ksx started"), "{text}");
        assert!(
            text.contains(r"C:\Program Files (x86)\Steam\steam.exe"),
            "{text}"
        );
        assert!(text.contains("portla2.exe"), "{text}");
        assert!(
            !text.contains("# <- add this line"),
            "process_name is already set; do not tell the user to add it:\n{text}"
        );
    }

    /// Every message in this module names what actually happened. Asserted as a
    /// sweep, because the failure mode here is a *reused sentence*, and the way
    /// that gets caught is by checking every message against every shape rather
    /// than one at a time.
    #[test]
    fn no_message_claims_a_url_for_an_exe_or_an_exe_for_a_url() {
        let toml = Path::new(r"C:\cfg\ksx\games.toml");
        let exe = LaunchSpec::from_entry(&steam_exe_entry());
        let url = LaunchSpec::from_entry(&entry("steam://rungameid/620", "", None));

        for text in [
            missing_process_name_warning(&exe, toml),
            handoff_timed_out_warning(&exe, toml, 60_000),
        ] {
            assert!(!text.contains("starts a URL"), "exe profile: {text}");
            assert!(!text.contains("steam://"), "exe profile: {text}");
        }
        for text in [
            missing_process_name_warning(&url, toml),
            handoff_timed_out_warning(&url, toml, 60_000),
        ] {
            assert!(
                !text.contains("the program ksx started"),
                "url profile: {text}"
            );
            assert!(!text.contains(".exe) exited"), "url profile: {text}");
        }
    }

    /// The suggested `process_name` comes from the profile's own path, so the
    /// advice is a line the user can paste rather than a placeholder.
    #[test]
    fn the_exe_file_name_is_what_gets_suggested() {
        assert_eq!(
            LaunchSpec::from_entry(&steam_exe_entry()).exe_file_name(),
            Some("steam.exe".to_owned())
        );
        // A protocol URL has no exe, and the launcher's own name is not the
        // game's — better to say nothing than to suggest "steam.exe" for a
        // profile whose game is Portal 2.
        assert_eq!(
            LaunchSpec::from_entry(&entry("steam://rungameid/620", "", None)).exe_file_name(),
            None
        );
    }

    /// `launcher_grace_ms` reaches the launch spec, and its absence means "the
    /// default", not "zero".
    #[test]
    fn the_launcher_grace_is_carried_from_the_profile() {
        let spec = LaunchSpec::from_entry(&entry(r"C:\g\x.exe", "", None));
        assert_eq!(spec.launcher_grace_ms, None);
        let spec = LaunchSpec::from_entry(&GameEntry {
            launcher_grace_ms: Some(30_000),
            ..entry(r"C:\g\x.exe", "", None)
        });
        assert_eq!(spec.launcher_grace_ms, Some(30_000));
    }
}
