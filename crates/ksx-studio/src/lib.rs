//! ksx-studio — the optional Forma-powered web page: cabinet status + session
//! control (M10).
//!
//! One page, one screen (docs/research/padforge-ui-lessons.md — no tabs, no
//! nav): a SESSION panel (daemon state, profile dropdown, Start / Stop /
//! Reload as plain HTML forms) above the cabinet status — driver health
//! (ViGEmBus / Interception), the pads the bus is exposing, autostart
//! registration, the games.toml profile list. SSR only: forma-server 0.1.4
//! renders the embedded FMIR per request, the page auto-refreshes via meta
//! refresh (plus an HTTP `Refresh` header), and **zero JavaScript ships to
//! the client** — which also sidesteps forma's hardcoded CSP entirely
//! (whose `form-action 'self'` already covers the forms).
//!
//! Session state and the three POST routes go through [`ControlSource`] —
//! ksx-app implements it over the daemon's `\\.\pipe\ksx-daemon` control
//! channel, so every button maps to the same `DaemonCommand` the tray
//! enqueues (docs/CONTROL-SURFACE.md: no GUI-only code paths). When no
//! daemon answers the pipe, the controls render visibly disabled with the
//! reason and the way out ("start the daemon — tray or `ksx daemon`").
//! Action outcomes — failures included — come back as a `flash` query
//! parameter after a 303 redirect, rendered escaped; nothing fails silently.
//!
//! # Committed UI artifacts — Node is never required to build or run ksx
//!
//! `assets/` holds the **committed** output of the `studio-ui/` npm project
//! (FMIR module, manifest, CSS, service worker), embedded via `rust-embed`.
//! `cargo build` needs nothing but Rust. Node ≥ 18 is needed only to
//! REGENERATE the UI after editing `studio-ui/src/`:
//!
//! ```text
//! cd studio-ui
//! npm install
//! node build.mjs      # rebuilds crates/ksx-studio/assets/, then run the ksx gate
//! ```
//!
//! # Data injection
//!
//! Server-side FMIR slot injection — real prop injection, no JSON island, no
//! string templating. Scalars and lists are injected by slot name; `createShow`
//! booleans remain positional (the last shared-name slot kind). See `render.rs`
//! for the mechanism and the E7 dogfood history — first cycle closed when
//! `@getforma/compiler` 0.2.0 shipped the per-list naming this seam requested.
//!
//! # Boundaries (docs/ENHANCEMENTS.md E7, enforced)
//!
//! - **Localhost only.** [`serve`] refuses any non-loopback bind; there is no
//!   LAN option. That option arrives with the CSPRNG pairing token, not
//!   before.
//! - **Own tokio runtime, normal priority** — created inside [`serve`], never
//!   shared with (or visible to) anything session- or pipeline-related.
//! - This crate depends on **no other ksx crate**. Data arrives through the
//!   [`StatusSource`] and [`ControlSource`] traits; ksx-app implements both
//!   (collectors and pipe client respectively). Nothing here can touch
//!   capture, output, or a live session — a control implementation is a
//!   client of the daemon's pipe, never a second control loop.

mod control;
mod error;
mod render;
mod server;
mod snapshot;

pub use control::{ControlSource, SessionView};
pub use error::StudioError;
pub use server::serve;
pub use snapshot::{PadRow, ProfileRow, StatusSnapshot, StatusSource};
