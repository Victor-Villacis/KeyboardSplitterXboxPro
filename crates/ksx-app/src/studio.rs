//! `ksx studio` — the localhost control room (feature `studio`).
//!
//! Two providers, both thin, and neither of them Studio's:
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
        // The third provider (v17): the MACHINE verbs the `/devices` picker
        // needs. Daemon-free by construction — it walks the USB tree and the
        // config store directly — which is why the picker keeps working behind
        // the "No daemon" banner that disables every session control.
        Box::new(LocalMachine),
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
