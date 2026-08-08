//! `ksx studio` — the localhost control room (feature `studio`).
//!
//! Three providers, all thin, and none of them Studio's:
//!
//! - [`ksx_api::StatusSource`] from the EXISTING collectors, which live in
//!   [`crate::sources`] because the cabinet reads the same facts and a
//!   contract cannot be owned by whichever surface was written first. Fresh
//!   point-in-time snapshot per page load, and satisfiable with NO daemon
//!   running, which is what keeps the read-only mapper alive behind the "No
//!   daemon" banner.
//! - [`ksx_api::ControlSource`] as [`ksx_api::Client`] over
//!   [`ksx_api::PipeTransport`]: the session panel's state and its Start /
//!   Stop / Reload buttons are each one pipe request, which enqueues the same
//!   `DaemonCommand` a tray click would (docs/CONTROL-SURFACE.md — no GUI-only
//!   code paths). No daemon on the pipe → the panel says so and the controls
//!   render disabled; this process never becomes a daemon itself.
//! - [`ksx_api::MachineSource`] as [`crate::sources::LocalMachine`]: the reads
//!   and writes that are neither a snapshot nor a `DaemonCommand` — the
//!   device scan and the two `[[device]]` writes behind `/devices`, the
//!   preflighted games.toml profile list, the presets with their in-box
//!   templates, and the two creates behind `/profiles`. Daemon-free by
//!   construction (it is the config store, the USB tree and the filesystem),
//!   which is what lets a first-run cabinet pick its board and make its first
//!   profile and preset before anything is running.
//!
//! **The control implementation used to live here**, as ~250 lines that built
//! each request with `serde_json::json!` and read each answer with
//! `response["field"]`. It is `ksx-api`'s now (docs/M9-DECISION.md §6), for
//! the reason that layer exists: `ksx session` dials the same pipe with no
//! HTTP anywhere, the cabinet window does too, and a hand-written request at
//! each caller is how a field gets dropped between two descriptions of one
//! message.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::sources::{configured_profile, CollectorSource, LocalMachine};

pub fn run(port: u16) -> anyhow::Result<()> {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    println!("ksx Studio: http://{bind}/  (localhost only; Ctrl+C or close the window to stop)");
    println!("Session controls talk to a running `ksx daemon` over its control pipe.");
    ksx_studio::serve(
        bind,
        Box::new(CollectorSource),
        Box::new(control_source()),
        // The third provider: the MACHINE verbs — `/devices`, `/profiles` and
        // `/pads`, plus the config in and out and the first-run state `/setup`
        // reads. Daemon-free by construction — it walks the USB tree, the
        // config store and the ViGEm bus directly — which is why none of it is
        // on `ControlSource`: every one of these pages keeps working behind the
        // "No daemon" banner that disables the session controls. (The two pad
        // WRITES do ask the pipe whether a session is live before acting, but
        // an unreachable daemon answers "no session" there, so /pads never
        // goes dark with the pipe dead.)
        Box::new(LocalMachine),
        // The FOURTH provider: the LIVE INPUT FEED, over the daemon's second
        // pipe (`ksx_api::LIVE_PIPE_NAME`). Not on `ControlSource`, and this
        // is the reason — the other three are questions with an answer, served
        // over a channel that is one line out, one line in, per connection. A
        // stream held open on that channel would hold the daemon's single pipe
        // thread for as long as a browser tab lived, and nothing else would
        // ever be answered again (`ksx_api::LiveSource`).
        //
        // With no daemon running this refuses in words, per open, and the page
        // says so — it never hangs and never renders a dead grid.
        std::sync::Arc::new(ksx_api::PipeLiveSource::new()),
    )?;
    Ok(())
}

/// The daemon control surface: the typed api client, over the pipe.
///
/// The one thing supplied on top of the transport is the OFFLINE PROFILE — the
/// games.toml title the "No daemon" banner has to name, read from the config
/// because the pipe is the thing that just failed to answer. Everything else
/// about talking to a daemon is `ksx-api`'s and is identical for every surface.
fn control_source() -> ksx_api::Client<ksx_api::PipeTransport> {
    ksx_api::Client::new(ksx_api::PipeTransport::new()).with_offline_profile(configured_profile)
}
