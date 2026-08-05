//! ksx-studio — the optional Forma-powered web status page (M10 skeleton).
//!
//! One page: the cabinet status screen — driver health (ViGEmBus /
//! Interception), the pads the bus is exposing, autostart registration, the
//! games.toml profile list, and whether a ksx daemon process is alive. SSR
//! only: forma-server 0.1.4 renders the embedded FMIR per request, the page
//! auto-refreshes every 2 seconds via meta refresh (plus an HTTP `Refresh`
//! header), and **zero JavaScript ships to the client** — which also
//! sidesteps forma's hardcoded CSP entirely. Snapshots are point-in-time,
//! re-read on each request; there is no daemon IPC yet
//! (docs/CONTROL-SURFACE.md documents that gap) and the page footer says so.
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
//! string templating. See `render.rs` for the mechanism, its one wrinkle
//! (positional resolution of `list:array` slots) and the upstream feature
//! request that wrinkle generates per the E7 dogfood loop.
//!
//! # Boundaries (docs/ENHANCEMENTS.md E7, enforced)
//!
//! - **Localhost only.** [`serve`] refuses any non-loopback bind; there is no
//!   LAN option. That option arrives with the CSPRNG pairing token, not
//!   before.
//! - **Own tokio runtime, normal priority** — created inside [`serve`], never
//!   shared with (or visible to) anything session- or pipeline-related.
//! - This crate depends on **no other ksx crate**. Data arrives through the
//!   [`StatusSource`] trait; ksx-app implements it from the existing
//!   collectors. Nothing here can touch capture, output, or a live session.

mod error;
mod render;
mod server;
mod snapshot;

pub use error::StudioError;
pub use server::serve;
pub use snapshot::{PadRow, ProfileRow, StatusSnapshot, StatusSource};
