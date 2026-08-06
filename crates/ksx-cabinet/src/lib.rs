//! **ksx cabinet — the 10-foot OPERATE surface.**
//!
//! One rule decides what is in this crate and what is not:
//!
//! > **The cabinet OPERATES — choose among things that exist. Studio AUTHORS —
//! > create, edit, name, delete.**
//!
//! and one test decides whether a thing belongs on it:
//!
//! > *Can it be done with a joystick, two buttons, no text entry, in ten
//! > seconds, by someone standing at the cab with a game about to start?*
//!
//! So there is a button check, a "is this working", a start/stop, a game
//! picker and a per-slot preset picker — and there is **no mapper, no macro
//! editor and no preset file management here, ever.** Those are authoring, they
//! need a keyboard and a pointer, and Studio has 23,000 lines that already do
//! them properly.
//!
//! # What this crate is allowed to touch
//!
//! `ksx-api` and egui. That is the whole list. There is no pipe client here, no
//! config store, no capture, no output and no `DaemonCommand` — the window is
//! handed [`Cabinet`]'s four trait objects and draws them, so "exactly the
//! tray's reach" is a property of the dependency graph rather than of anyone's
//! discipline (docs/CONTROL-SURFACE.md, "Invariants a GUI must not break").
//!
//! # Where it runs
//!
//! Two hosts, one window:
//!
//! - **inside `ksx daemon`**, as a fourth thread beside the tray, the control
//!   loop and the session. [`run_on_any_thread`] is what makes that legal on
//!   Windows. Closing the window does NOT quit the daemon — Quit stays the
//!   tray's, because a cabinet whose emulation dies when somebody shuts a
//!   status panel is a cabinet nobody trusts;
//! - **as its own process** (`ksx cabinet`), talking to a daemon over the
//!   control pipe, which is the recovery path when the in-daemon window cannot
//!   be created — the same reason `ksx studio` stays a separate process.
//!
//! # It is NOT in the default build
//!
//! `ksx-app`'s `cabinet` feature is the only thing that names this crate, and
//! `cargo tree` proves it — the same rule, and the same proof, as `studio`
//! (docs/ENHANCEMENTS.md E7 rule A).

pub mod app;
pub mod demo;
pub mod nav;
#[cfg(windows)]
pub mod pad;
pub mod screens;
pub mod theme;

pub use app::{Cabinet, Slow};
pub use nav::{Focus, Nav, Screen};

/// The window title. Also what alt-tab and the taskbar call it.
pub const TITLE: &str = "ksx cabinet";

/// Default window size: a 16:9 panel with room for two columns of button
/// check at [`theme::fs::LIGHT`], and a minimum below which the two columns
/// stop being two columns.
const DEFAULT_SIZE: [f32; 2] = [1280.0, 800.0];
const MIN_SIZE: [f32; 2] = [900.0, 600.0];

/// Open the window on THIS thread and pump it until it is closed.
///
/// Blocks. Returns `Ok` when the user closed the window — which, hosted inside
/// the daemon, must mean nothing more than that.
pub fn run(cabinet: Cabinet) -> eframe::Result<()> {
    launch(cabinet, false)
}

/// The same window, on a thread that is not the process's main one.
///
/// winit refuses to build an event loop off the main thread by default,
/// because on macOS and X11 that genuinely does not work. On Windows it does —
/// a message pump belongs to whichever thread created the window, which is the
/// same fact `daemon/tray.rs` relies on — and `with_any_thread` is winit's own
/// documented opt-in. This is what lets the cabinet be a **fourth thread
/// inside `ksx daemon`** rather than a second process, and therefore what lets
/// it hold an in-process `ControlSource` with no serialization at all.
///
/// On any other platform this is [`run`] and will fail the same way winit
/// always fails there; the daemon is Windows-only regardless.
pub fn run_on_any_thread(cabinet: Cabinet) -> eframe::Result<()> {
    launch(cabinet, true)
}

fn launch(cabinet: Cabinet, any_thread: bool) -> eframe::Result<()> {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size(DEFAULT_SIZE)
            .with_min_inner_size(MIN_SIZE),
        ..Default::default()
    };
    if any_thread {
        #[cfg(windows)]
        {
            options.event_loop_builder = Some(Box::new(|builder| {
                use winit::platform::windows::EventLoopBuilderExtWindows as _;
                builder.with_any_thread(true);
            }));
        }
    }
    let _ = any_thread;
    eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(&cc.egui_ctx, cabinet)))),
    )
}
