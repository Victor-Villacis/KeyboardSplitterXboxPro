//! `ksx open`, and the tray item that is the same thing: ksx on screen as an
//! application window, or a sentence saying why not.
//!
//! docs/M9-DECISION.md cancelled the native config UI and made Studio the UI.
//! What that decision costs, and therefore what this file owes, is stated in
//! its §2: *"The thing that makes ksx feel like a web page is not HTML — it is
//! that you launch it by typing a URL, and that clicking a shortcut before the
//! daemon is up would hand you `ERR_CONNECTION_REFUSED`. Both are launcher
//! bugs."* This is the launcher.
//!
//! # The four things it does, in order
//!
//! 1. **Make sure a daemon is running.** [`ensure_daemon`] probes the control
//!    pipe, starts `ksx daemon` — the same verb `ksx autostart` registers —
//!    and waits, bounded, for the pipe to answer. A daemon that never answers
//!    is a WARNING and not the end: Studio's read side needs no daemon
//!    (M9-DECISION §4 item 7), so the window still opens, read-only, behind
//!    its own "No daemon" banner. Refusing to open at all would delete the
//!    recovery path.
//! 2. **Make sure Studio is serving.** Probe the port, start `ksx studio` if
//!    nothing answers, and **wait for it** before handing anyone a URL. This
//!    is the `ERR_CONNECTION_REFUSED` half, and it is the property this file
//!    had before the rest of M9 existed.
//! 3. **Resolve a browser through `App Paths`, never `ShellExecute` on a
//!    URL** (§4 item 2). `ksx_platform::app_paths` asks the registry where
//!    `msedge.exe` — then `chrome.exe` — actually is. A default-browser
//!    activation cannot be given flags, so it cannot produce the window
//!    below; and on a stripped image there may be no `http` association at
//!    all.
//! 4. **Open a chrome-less application window** — `--app=<url>` with a
//!    `--user-data-dir` **ksx owns**, under `%LOCALAPPDATA%\ksx`. Its own
//!    taskbar button, its own alt-tab entry, no address bar, no tabs, and
//!    none of the user's extensions, cookies or zoom state (§2).
//!
//! # What it still does not do
//!
//! - **No single instance.** A second `ksx open` starts a second window
//!   rather than focusing the first — M9-DECISION §4 item 5, not built here.
//! - **No WebView2 host.** That is Option B, priced and held in reserve (§3);
//!   this window is a browser we drive, and its right-click menu, Ctrl+P and
//!   F12 are all still live. The triggers that would flip that decision are
//!   in §7 and none of them has fired.
//! - **No port option.** [`PORT`] is spelled once here and matches `ksx
//!   studio`'s default. A Studio deliberately served on another port is not
//!   found by this launcher, which starts one of its own on [`PORT`].
//!
//! # Where the fallback goes
//!
//! With no Chromium registered under either name, or with no directory ksx
//! can call its own, the window degrades to the default-browser
//! `shell_open` this file did before — **and says which and why**, because a
//! surface that cannot do the thing it promised must say so rather than
//! quietly do something else (docs/CONTROL-SURFACE.md).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Studio's default port — the one `ksx studio` binds and the one this dials.
pub const PORT: u16 = 4460;

/// Where Studio will be, spelled once.
///
/// Returned to callers so a surface that cannot open a browser can still show
/// the address. On a 10-foot cabinet screen with a joystick and two buttons,
/// "type this on your phone" is frequently the *useful* outcome, and a UI that
/// only knows how to launch a local browser cannot offer it.
pub fn url() -> String {
    format!("http://127.0.0.1:{PORT}/")
}

// ---------------------------------------------------------------------------
// The decision — pure, and the whole of what is tested. Nothing below this
// heading opens a socket, reads a registry or starts a process.
// ---------------------------------------------------------------------------

/// A Chromium ksx knows how to drive, and the name a human would recognise.
pub struct Chromium {
    /// The `App Paths` key name — also the file name on disk.
    pub exe: &'static str,
    pub name: &'static str,
}

/// The browsers to try, **in this order**.
///
/// Edge first because it is the one that is *there*: Windows 11 ships it
/// in-box, so on a machine nobody has configured it is the only Chromium with
/// a registered path (M9-DECISION §2 verified it on this machine). Chrome
/// second — a machine that has both usually got Chrome on purpose, but only
/// after Edge was already present, so preferring it would make the launcher's
/// behaviour depend on a browser race nobody ran.
///
/// This is a fixed list, not a search: a browser ksx has never been run
/// against is not made safer by being launched with Chromium's flags.
pub const CHROMIUM: [Chromium; 2] = [
    Chromium {
        exe: "msedge.exe",
        name: "Microsoft Edge",
    },
    Chromium {
        exe: "chrome.exe",
        name: "Google Chrome",
    },
];

/// How ksx will put Studio on screen.
#[derive(Debug, PartialEq, Eq)]
pub enum Window {
    /// Our own Chromium invocation: a chrome-less application window.
    App {
        browser: &'static str,
        exe: PathBuf,
        argv: Vec<String>,
    },
    /// Whatever the user's `http` association opens — the pre-M9 behaviour,
    /// kept as the honest degradation. `why` is the sentence the user is owed.
    DefaultBrowser { url: String, why: String },
}

/// The ksx-owned browser profile directory, given the machine's
/// `%LOCALAPPDATA%`.
///
/// **Local, not roaming, and not the config root.** ksx's configuration lives
/// in `%APPDATA%\ksx` because it is small, hand-editable TOML a user might
/// legitimately sync between machines. A Chromium profile is neither: **63 MB
/// after one open**, measured here on 2026-08-08 by running exactly the argv
/// below, and it is cache and a code cache keyed to one installation — so it
/// grows, and none of it means anything on another machine. Beside
/// `config.toml` it would roam all of that and bury the handful of files a
/// human is meant to edit.
pub fn profile_dir(local_appdata: &Path) -> PathBuf {
    local_appdata.join("ksx").join("browser-profile")
}

/// The exact argument list that turns a browser into ksx's window.
///
/// Each flag, and what it is being *asked* for — Chromium's behaviour is
/// Chromium's to change, and §5 item 2 of the decision names that as a
/// dependency we do not control:
///
/// - `--app=<url>` — open this URL as an application window: no address bar,
///   no tab strip, its own taskbar button and alt-tab entry. This is the flag
///   the whole file exists to be able to pass, and the reason a default-browser
///   activation cannot substitute.
/// - `--user-data-dir=<ksx profile>` — use a profile ksx owns. Without it the
///   window would run inside the user's own browser profile and inherit its
///   extensions, its sign-in, its cookies and its per-origin zoom — the last
///   of which is how a stray Ctrl+scroll leaves ksx at 150 % forever, in the
///   user's browser, for every site on `127.0.0.1`.
/// - `--no-first-run` and `--no-default-browser-check` — a brand-new profile
///   is a brand-new browser installation as far as Chromium is concerned. Ask
///   it not to greet the user with a welcome flow or ask to be made the
///   default the first time ksx opens.
///
/// The URL comes from [`url`] rather than being spelled again, so the window
/// and the "type this on your phone" line can never name different addresses.
pub fn app_argv(url: &str, profile: &Path) -> Vec<String> {
    vec![
        format!("--app={url}"),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
    ]
}

/// Decide how to show `url`, given a profile directory (or the reason there is
/// none) and a way to look an executable up.
///
/// The lookup is injected — every test drives this function with a fake
/// resolver, and no test has ever started a browser.
///
/// **A missing profile directory means no application window**, not an
/// application window without `--user-data-dir`. Dropping the flag would put
/// ksx inside the user's own browser profile, which is precisely the thing the
/// flag is there to prevent; the honest degradation is the default browser,
/// said out loud.
pub fn choose(
    url: &str,
    profile: Result<PathBuf, String>,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
) -> Window {
    let profile = match profile {
        Ok(dir) => dir,
        Err(why) => {
            return Window::DefaultBrowser {
                url: url.to_owned(),
                why: format!("ksx has no browser profile directory of its own ({why})"),
            }
        }
    };
    match CHROMIUM
        .iter()
        .find_map(|browser| resolve(browser.exe).map(|exe| (browser, exe)))
    {
        Some((browser, exe)) => Window::App {
            browser: browser.name,
            argv: app_argv(url, &profile),
            exe,
        },
        None => Window::DefaultBrowser {
            url: url.to_owned(),
            why: format!(
                "no Chromium browser is registered on this machine (looked for {})",
                CHROMIUM
                    .iter()
                    .map(|b| b.exe)
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        },
    }
}

/// The refusal off Windows, for both entry points.
///
/// Nothing here has a portable form: `App Paths` is a Windows registry key and
/// `ksx_platform::process::shell_open` is `ShellExecuteW`. The verb exists on
/// every platform so the CLI's shape does not depend on the host; it declines
/// on the ones where it would only fail later, and names what to do instead.
#[cfg(not(windows))]
const NOT_WINDOWS: &str = "`ksx open` is Windows-only: it resolves a browser through the Windows \
     App Paths registry and starts the daemon that serves Studio. Run `ksx studio` and open its \
     URL yourself.";

// ---------------------------------------------------------------------------
// Doing it — probes, spawns, and the two entry points.
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod live {
    use super::*;

    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

    /// How long to wait for a freshly started Studio to answer.
    const READY_TIMEOUT: Duration = Duration::from_secs(8);
    /// How long to wait for a freshly started daemon's control pipe.
    ///
    /// Same shape and the same reasoning as `CONNECT_BUDGET` in
    /// `ksx-api/src/pipe.rs`: a deadline, a retry pause, and a worded failure
    /// — never an unbounded wait on a process that may have exited two
    /// seconds ago. Longer than that transport's two seconds because this one
    /// is waiting for a process to *start*, not for an instance to rotate.
    const DAEMON_TIMEOUT: Duration = Duration::from_secs(8);
    const PROBE_EVERY: Duration = Duration::from_millis(150);
    /// A connect attempt that hangs is a machine problem, not a busy server.
    const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

    /// Open ksx from the tray. Never blocks the caller.
    ///
    /// Everything below happens on a thread of its own, because "poll a port
    /// for eight seconds" is not something the daemon's control loop may do
    /// while a session is waiting to be reaped.
    ///
    /// No [`ensure_daemon`] step here, and that is not an omission: this code
    /// runs *inside* the daemon. The console may be gone by now (the daemon
    /// detaches once the tray icon exists), so the outcome — including which
    /// browser was used — goes to the log, which is the surface that always
    /// exists.
    pub fn open(out: &mut dyn Write) {
        let _ = writeln!(out, "opening ksx…");
        let _ =
            std::thread::Builder::new()
                .name("ksx-open".into())
                .spawn(|| match ensure_and_open() {
                    Ok(done) => tracing::info!("{done}"),
                    Err(why) => tracing::error!("could not open ksx: {why}"),
                });
    }

    /// `ksx open` — the whole launcher, on the caller's thread.
    ///
    /// Blocking on purpose: this process exists to put a window on screen and
    /// then exit, and a shortcut that returns before the window is up is the
    /// bug this file was written to fix.
    pub fn run() -> anyhow::Result<()> {
        let mut out = std::io::stdout();
        if let Err(why) = ensure_daemon(&mut out) {
            // Not fatal. Studio's read side needs no daemon, so the window is
            // still worth opening — and the banner it opens behind is the
            // documented recovery path (M9-DECISION §4 item 7), not a
            // degraded mode we are hiding.
            let _ = writeln!(out, "[WARN] {why}");
            let _ = writeln!(
                out,
                "       Studio will open read-only, behind its \"No daemon\" banner."
            );
        }
        match ensure_and_open() {
            Ok(done) => {
                let _ = writeln!(out, "{done}");
                Ok(())
            }
            Err(why) => Err(anyhow::anyhow!("{why}")),
        }
    }

    /// Studio, then a window. Shared by both entry points, which is what makes
    /// the tray item and the verb the same thing rather than two things that
    /// agree today.
    fn ensure_and_open() -> Result<String, String> {
        ensure_studio()?;
        show(choose(
            &url(),
            prepare_profile(),
            &ksx_platform::app_paths::resolve,
        ))
    }

    /// Start the window, and say what happened either way.
    fn show(window: Window) -> Result<String, String> {
        match window {
            Window::App { browser, exe, argv } => {
                ksx_platform::process::no_window(std::process::Command::new(&exe).args(&argv))
                    .spawn()
                    .map(|_| format!("opened ksx in {browser} ({})", exe.display()))
                    .map_err(|err| format!("could not start {browser} at {}: {err}", exe.display()))
            }
            Window::DefaultBrowser { url, why } => ksx_platform::process::shell_open(&url)
                .map(|()| format!("{why} — opened {url} in your default browser instead"))
                .map_err(|err| {
                    format!(
                        "{why}, and the default browser could not be opened either: {err} ({url})"
                    )
                }),
        }
    }

    /// The ksx-owned profile directory, created if it is not there.
    ///
    /// Created by us rather than left to Chromium so that a failure is ksx's
    /// sentence on ksx's console, instead of a browser error dialog about a
    /// path the user never chose.
    fn prepare_profile() -> Result<PathBuf, String> {
        let base = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| "%LOCALAPPDATA% is not set".to_owned())?;
        let dir = profile_dir(Path::new(&base));
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("{} could not be created: {err}", dir.display()))?;
        Ok(dir)
    }

    /// Bring a daemon up, and wait — bounded — for its control pipe.
    fn ensure_daemon(out: &mut dyn Write) -> Result<(), String> {
        if daemon_answering() {
            return Ok(());
        }
        let _ = writeln!(out, "starting the ksx daemon…");
        start_daemon()?;
        let deadline = Instant::now() + DAEMON_TIMEOUT;
        while !daemon_answering() {
            if Instant::now() >= deadline {
                return Err(format!(
                    "started `ksx daemon` but its control pipe did not answer within {}s — \
                     run `ksx daemon --console` to see why (a configuration it refuses exits 2)",
                    DAEMON_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(PROBE_EVERY);
        }
        Ok(())
    }

    /// Bring Studio up, and wait — bounded — for its port.
    ///
    /// **"It must never be possible to reach `ERR_CONNECTION_REFUSED` by
    /// clicking a ksx shortcut"** (M9-DECISION §4 item 1). A launcher that
    /// opens a browser at a port nothing is listening on has not opened
    /// Studio; it has produced an error page with ksx's name on it.
    fn ensure_studio() -> Result<(), String> {
        if studio_answering() {
            return Ok(());
        }
        start_studio()?;
        let deadline = Instant::now() + READY_TIMEOUT;
        while !studio_answering() {
            if Instant::now() >= deadline {
                return Err(format!(
                    "started `ksx studio` but nothing answered 127.0.0.1:{PORT} within {}s — \
                     run `ksx studio` in a console to see why",
                    READY_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(PROBE_EVERY);
        }
        Ok(())
    }

    /// Is something serving Studio's port right now?
    ///
    /// A TCP connect, not an HTTP request: neither the daemon nor this verb
    /// links an HTTP client and neither is about to grow one for a liveness
    /// probe. A listener on loopback that accepts is Studio for this purpose —
    /// and if it is not, the browser shows whatever it is, which is a truthful
    /// outcome rather than a refused connection.
    fn studio_answering() -> bool {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
        TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok()
    }

    /// Is a daemon answering the control pipe?
    ///
    /// One real `status` request through the same client `ksx session` uses —
    /// not a file-existence test — because the question is "will the pipe
    /// answer", and that is the only way to ask it. Only `NotRunning` counts
    /// as absent: a daemon that answers with a REFUSAL is a daemon, and a
    /// conversation that breaks mid-flight is not a problem a second daemon
    /// would fix (the pipe's first instance owns the name, so a second daemon
    /// exits rather than splitting the stream).
    fn daemon_answering() -> bool {
        use crate::daemon::pipe::{client, PIPE_NAME};

        let request = serde_json::to_value(ksx_api::Request::Status)
            .unwrap_or_else(|err| unreachable!("a control request is always serializable: {err}"));
        !matches!(
            client::request(PIPE_NAME, &request),
            Err(client::ClientError::NotRunning)
        )
    }

    /// Start `ksx daemon` — the same verb, spelled the same way, that
    /// `ksx autostart` registers with Task Scheduler
    /// ([`ksx_platform::autostart::TaskMode::verb`]). One spelling, so a
    /// machine that boots into ksx and a machine where someone clicked the
    /// shortcut are running the same command line.
    fn start_daemon() -> Result<(), String> {
        spawn_self(&[ksx_platform::autostart::TaskMode::Daemon.verb()])
    }

    /// Start `ksx studio` on the port this file dials.
    fn start_studio() -> Result<(), String> {
        spawn_self(&["studio", "--port", &PORT.to_string()])
    }

    /// Start one of OUR OWN verbs as a detached child, using OUR OWN
    /// executable.
    ///
    /// `current_exe`, never a `PATH` lookup: a launcher started from a build
    /// tree, a staging folder or an installer must start the binary it IS, not
    /// whichever ksx happens to be first on the path.
    ///
    /// `no_window`, and here it is load-bearing rather than tidy: both
    /// children are **long-lived**, so a console window would not flash — it
    /// would sit on the cabinet's game screen for as long as ksx is up, and
    /// closing it would kill what it belongs to. A parent that has already
    /// released its own console (the daemon) is exactly the condition under
    /// which Windows hands a console-subsystem child a fresh one.
    ///
    /// Nothing waits on the child: it outlives this process by design, which
    /// on Windows needs no detach ceremony — a child is not in its parent's
    /// job unless someone puts it there.
    fn spawn_self(args: &[&str]) -> Result<(), String> {
        let exe =
            std::env::current_exe().map_err(|err| format!("cannot find my own path: {err}"))?;
        ksx_platform::process::no_window(std::process::Command::new(&exe).args(args))
            .spawn()
            .map(|_| ())
            .map_err(|err| {
                format!(
                    "could not start `{} {}`: {err}",
                    exe.display(),
                    args.join(" ")
                )
            })
    }
}

#[cfg(windows)]
pub use live::{open, run};

/// The tray item, off Windows: it says what it cannot do (see [`NOT_WINDOWS`]).
#[cfg(not(windows))]
pub fn open(out: &mut dyn Write) {
    let _ = writeln!(out, "{NOT_WINDOWS}");
}

/// `ksx open`, off Windows: a refusal, not a half-launch.
#[cfg(not(windows))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("{NOT_WINDOWS} ({})", url())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolver that knows about a fixed set of browsers, and remembers what
    /// it was asked. No registry, no filesystem, no browser — ever.
    struct FakeAppPaths {
        installed: Vec<&'static str>,
        asked: std::cell::RefCell<Vec<String>>,
    }

    impl FakeAppPaths {
        fn with(installed: &[&'static str]) -> Self {
            Self {
                installed: installed.to_vec(),
                asked: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn resolver(&self) -> impl Fn(&str) -> Option<PathBuf> + '_ {
            move |exe| {
                self.asked.borrow_mut().push(exe.to_owned());
                self.installed
                    .contains(&exe)
                    .then(|| PathBuf::from(format!(r"C:\Program Files\{exe}")))
            }
        }
    }

    fn profile() -> PathBuf {
        profile_dir(Path::new(r"C:\Users\victor\AppData\Local"))
    }

    /// **The M9 launcher bug, as an assertion.** Before this pass the tray
    /// handed the URL to `shell_open` and nothing else: a tab in whatever
    /// browser the user had, inside the user's own profile.
    ///
    /// Every element below is load-bearing and fails against a plausible
    /// wrong version: no `--app=` is a browser tab, no `--user-data-dir` is
    /// the user's own profile with their extensions and their zoom state, and
    /// the URL must be the one Studio is actually serving.
    #[test]
    fn the_window_is_an_app_window_in_a_profile_ksx_owns() {
        let paths = FakeAppPaths::with(&["msedge.exe"]);
        let window = choose(&url(), Ok(profile()), &paths.resolver());
        let Window::App { browser, exe, argv } = window else {
            panic!("Edge is installed in this fixture; expected an app window");
        };
        assert_eq!(browser, "Microsoft Edge");
        assert_eq!(exe, PathBuf::from(r"C:\Program Files\msedge.exe"));
        assert_eq!(
            argv,
            vec![
                "--app=http://127.0.0.1:4460/".to_owned(),
                r"--user-data-dir=C:\Users\victor\AppData\Local\ksx\browser-profile".to_owned(),
                "--no-first-run".to_owned(),
                "--no-default-browser-check".to_owned(),
            ]
        );
    }

    /// The profile is ksx's, it is under `%LOCALAPPDATA%`, and it is NOT the
    /// config root. Catches a version that reuses `%APPDATA%\ksx` — which
    /// would roam a multi-hundred-megabyte browser cache and bury the handful
    /// of TOML files a human is meant to edit — and one that points
    /// `--user-data-dir` at the user's own Chrome profile.
    #[test]
    fn the_browser_profile_lives_under_local_appdata_and_belongs_to_ksx() {
        let local = Path::new(r"C:\Users\victor\AppData\Local");
        let dir = profile_dir(local);
        assert!(dir.starts_with(local), "{}", dir.display());
        assert!(dir.starts_with(local.join("ksx")), "{}", dir.display());
        assert_ne!(dir, local.join("ksx"), "the profile is not the ksx root");
        let text = dir.display().to_string();
        assert!(!text.contains("Roaming"), "{text}");
        assert!(!text.contains("Google"), "{text}");
    }

    /// Edge is asked for first and Chrome only if Edge is absent. Catches a
    /// version that iterates an unordered collection, or that prefers Chrome
    /// — on a stock Windows 11 the answer would then depend on which browser
    /// the user happened to install rather than on which one is guaranteed to
    /// be there.
    #[test]
    fn edge_is_preferred_and_chrome_is_the_fallback() {
        let both = FakeAppPaths::with(&["msedge.exe", "chrome.exe"]);
        match choose(&url(), Ok(profile()), &both.resolver()) {
            Window::App { browser, .. } => assert_eq!(browser, "Microsoft Edge"),
            other => panic!("expected an app window, got {other:?}"),
        }
        assert_eq!(both.asked.borrow().as_slice(), ["msedge.exe"]);

        let chrome_only = FakeAppPaths::with(&["chrome.exe"]);
        match choose(&url(), Ok(profile()), &chrome_only.resolver()) {
            Window::App { browser, exe, .. } => {
                assert_eq!(browser, "Google Chrome");
                assert_eq!(exe, PathBuf::from(r"C:\Program Files\chrome.exe"));
            }
            other => panic!("expected an app window, got {other:?}"),
        }
        // Both names are tried, in order, before giving up on the second.
        assert_eq!(
            chrome_only.asked.borrow().as_slice(),
            ["msedge.exe", "chrome.exe"]
        );
    }

    /// No Chromium at all: fall back to the default browser **and say so**,
    /// naming what was looked for. Catches the version that falls back
    /// silently — the user would have no way to tell an app window from a tab
    /// except by looking at it — and the version that gives up entirely,
    /// which would make a machine with only Firefox unable to open ksx.
    #[test]
    fn with_no_chromium_the_default_browser_is_used_and_the_user_is_told() {
        let none = FakeAppPaths::with(&[]);
        let Window::DefaultBrowser { url: target, why } =
            choose(&url(), Ok(profile()), &none.resolver())
        else {
            panic!("nothing is installed in this fixture; expected the fallback");
        };
        assert_eq!(target, url());
        assert!(why.contains("msedge.exe"), "{why}");
        assert!(why.contains("chrome.exe"), "{why}");
        assert_eq!(none.asked.borrow().as_slice(), ["msedge.exe", "chrome.exe"]);
    }

    /// **No profile directory means no app window.** Catches the version that
    /// keeps `--app=` and drops `--user-data-dir` when `%LOCALAPPDATA%` is
    /// unavailable: that window would run in the user's own browser profile,
    /// inheriting their extensions and leaving ksx's zoom state behind in it —
    /// exactly what the flag exists to prevent. The reason travels with the
    /// refusal, so the console says which of the two fallbacks happened.
    #[test]
    fn without_a_ksx_owned_profile_there_is_no_app_window() {
        let paths = FakeAppPaths::with(&["msedge.exe", "chrome.exe"]);
        let window = choose(
            &url(),
            Err("%LOCALAPPDATA% is not set".to_owned()),
            &paths.resolver(),
        );
        let Window::DefaultBrowser { why, .. } = window else {
            panic!("a browser without a ksx profile must not get an app window");
        };
        assert!(why.contains("%LOCALAPPDATA%"), "{why}");
        assert!(
            paths.asked.borrow().is_empty(),
            "a browser is not even looked for when it could not be used"
        );
    }

    /// The window's URL is the one this module hands out for typing on a
    /// phone. Catches a drift between the `--app=` target and [`url`] — two
    /// spellings of a port is how a launcher opens an empty window while
    /// telling the user a working address.
    #[test]
    fn the_window_opens_the_address_the_module_publishes() {
        assert_eq!(url(), format!("http://127.0.0.1:{PORT}/"));
        let argv = app_argv(&url(), Path::new(r"C:\p"));
        assert_eq!(argv[0], format!("--app={}", url()));
        assert!(argv[0].contains(&PORT.to_string()));
    }
}
