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
//! `ksx-backend`'s `cabinet` feature is the only thing that names this crate, and
//! `cargo tree` proves it — the same rule, and the same proof, as `studio`
//! (docs/ENHANCEMENTS.md E7 rule A).

pub mod app;
pub mod demo;
pub mod list;
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

/// The window icon — title bar, taskbar button, alt-tab card.
///
/// A raw straight-alpha RGBA8 blob, not a PNG, and that is a dependency
/// decision rather than a lazy one: this crate's dependency list *is* its
/// boundary (see the manifest), and a PNG would pull a decoder in behind it
/// to produce the very buffer `egui::IconData` wants uncompressed anyway.
/// `tools/icongen` writes the blob beside the rest of the brand rasters, so
/// it cannot drift from the `.ico` the exe and the installer wear.
///
/// 256 px because that is the largest size Windows asks for (alt-tab on a
/// scaled 4K desktop); winit downsamples for the 16 and 32 px surfaces.
const ICON_PX: u32 = 256;
const ICON_RGBA: &[u8] = include_bytes!("../../../assets/brand/dist/ksx-256.rgba");

/// A regenerated blob of a different size would otherwise fail at runtime,
/// as a winit error on one machine's first launch. Fail at compile time.
const _: () = assert!(
    ICON_RGBA.len() == (ICON_PX as usize) * (ICON_PX as usize) * 4,
    "ksx-256.rgba is not 256×256×RGBA — re-run tools/icongen, or fix ICON_PX"
);

/// Open the window on THIS thread and pump it until it is closed.
///
/// Blocks. Returns `Ok` when the user closed the window — which, hosted inside
/// the daemon, must mean nothing more than that.
pub fn run(cabinet: Cabinet) -> eframe::Result<()> {
    launch(cabinet, false)
}

/// **Everything this window does is logged at `ksx_cabinet::*`.**
///
/// A cabinet surface has no console — the daemon released its own before this
/// window ever exists — so the rotating log file is the only place it can
/// speak. It had nothing to say for the whole of M9, and the cost was exact:
/// an evening spent on a "ghost window that flashes but never fully loads,
/// over and behind the cabinet UI" with no way to tell whether the flash was
/// eframe creating and destroying a window, a message-only window being
/// registered twice, or something else entirely. (It was none of those: it was
/// a *console* window, conjured by the `schtasks` spawn behind the two-second
/// status refresh, in a daemon with no console to inherit. See
/// `ksx_platform::process::no_window`.)
///
/// The rule this module now keeps: **every edge of the window's life emits one
/// line** — requested, event loop entered, viewport created, first frame,
/// focus gained/lost, close requested, event loop returned, and every error
/// path. Enough that the next ghost is named by reading a file rather than by
/// guessing at eframe's internals.
const _LOGGING_CONTRACT: () = ();

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

/// The window's `NativeOptions`, as a function so a test can read them.
///
/// # `run_and_return` is load-bearing, and it is NOT left to a default
///
/// eframe branches on this field in exactly one place that matters. With
/// `run_and_return: true` a closed window calls `event_loop.exit()`, the loop
/// returns, and [`run_on_any_thread`] hands control back to its caller. With
/// `false` the same code path calls **`std::process::exit(0)`**
/// (`eframe::native::run::WinitAppWrapper::handle_event_result`) — which,
/// inside `ksx daemon`, means shutting a status panel kills the daemon: pads
/// unplugged, WinUSB claim released, tray icon gone, mid-game.
///
/// `true` is eframe's default today, so ksx has always taken the safe branch —
/// but it took it by inheriting `..Default::default()`, which is not a decision
/// anybody made and not one any reader can see. A field whose two values are
/// "the window closes" and "the process dies" is spelled out, commented, and
/// asserted (`tests::the_window_may_never_take_the_process_with_it`).
///
/// It is also why [`run_on_any_thread`] must keep using ONE host thread for the
/// daemon's whole life: `run_and_return: true` routes through eframe's
/// thread-local event-loop cache, and winit allows one event loop per process.
fn native_options(any_thread: bool) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_icon(egui::IconData {
                rgba: ICON_RGBA.to_vec(),
                width: ICON_PX,
                height: ICON_PX,
            })
            .with_inner_size(DEFAULT_SIZE)
            .with_min_inner_size(MIN_SIZE),
        // Never `std::process::exit`. See above.
        run_and_return: true,
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
    options
}

fn launch(cabinet: Cabinet, any_thread: bool) -> eframe::Result<()> {
    let started = std::time::Instant::now();
    tracing::info!(
        any_thread,
        title = TITLE,
        size = ?DEFAULT_SIZE,
        thread = std::thread::current().name().unwrap_or("<unnamed>"),
        "cabinet window: entering the event loop"
    );
    let result = eframe::run_native(
        TITLE,
        native_options(any_thread),
        Box::new(|cc| {
            // The first thing that happens INSIDE winit: if this line is
            // missing from a log, the window never got as far as a GL context
            // and the error below says why.
            tracing::info!(
                renderer = if cc.gl.is_some() { "glow" } else { "wgpu" },
                pixels_per_point = cc.egui_ctx.pixels_per_point(),
                "cabinet window: viewport created, building the app"
            );
            Ok(Box::new(app::App::new(&cc.egui_ctx, cabinet)))
        }),
    );
    // The event loop RETURNED. Inside the daemon this must mean "the user shut
    // a panel" and nothing else, so it is recorded as such — with how long the
    // window was up, which is the one number that separates "they closed it"
    // from "it never opened".
    match &result {
        Ok(()) => tracing::info!(
            open_for_ms = started.elapsed().as_millis(),
            "cabinet window: event loop returned cleanly; the daemon is unaffected"
        ),
        Err(err) => tracing::error!(
            %err,
            open_for_ms = started.elapsed().as_millis(),
            "cabinet window: event loop returned an error"
        ),
    }
    result
}

#[cfg(test)]
mod tests {
    /// **Closing the window must never be able to end the process.**
    ///
    /// eframe's `handle_event_result` calls `std::process::exit(0)` on the
    /// `run_and_return: false` branch. Hosted inside `ksx daemon` that turns
    /// "shut the status panel" into "unplug four pads, release the WinUSB
    /// claim and drop the tray icon, mid-game" — which is exactly the outcome
    /// `lib.rs`'s own module docs promise cannot happen.
    ///
    /// The value is eframe's default, so this has never fired. That is the
    /// reason to assert it: a default is not a decision, it can change in a
    /// patch release, and the failure would show up as an unreproducible
    /// "the daemon quit by itself" on somebody's cabinet.
    #[test]
    fn the_window_may_never_take_the_process_with_it() {
        for any_thread in [false, true] {
            assert!(
                super::native_options(any_thread).run_and_return,
                "run_and_return=false makes eframe call std::process::exit(0) \
                 when the cabinet window is closed"
            );
        }
    }

    /// The window is identified by one title everywhere — alt-tab, the
    /// taskbar, the eframe app id and the log lines above.
    #[test]
    fn the_window_carries_the_icon_and_the_size_the_cabinet_expects() {
        let options = super::native_options(false);
        assert_eq!(options.viewport.title.as_deref(), Some(super::TITLE));
        assert!(options.viewport.icon.is_some(), "the taskbar face");
        assert_eq!(
            options.viewport.inner_size,
            Some(egui::vec2(super::DEFAULT_SIZE[0], super::DEFAULT_SIZE[1]))
        );
    }
}
