//! ksx-studio — the optional Forma-powered web page: cabinet status + session
//! control (M10).
//!
//! One page, one screen (docs/research/padforge-ui-lessons.md — no tabs, no
//! nav): a SESSION panel (daemon state, profile dropdown, Start / Stop /
//! Reload as plain HTML forms) above the cabinet status — driver health
//! (ViGEmBus / Interception), the pads the bus is exposing, autostart
//! registration, the games.toml profile list.
//!
//! v4 rendering model — SSR first paint + one live island:
//! forma-server 0.1.4 renders the embedded FMIR per request (complete page,
//! no JS required), and the whole screen is a Forma ISLAND whose client
//! runtime seeds its signals from server props BEFORE adoption (the
//! islands protocol; dogfood ledger #5) and then polls `GET /api/status`
//! every 2 s, rewriting the same signals in place — pills, state line, pad
//! tiles and profiles update without a reload. With JavaScript disabled the
//! page IS the v3 experience: full SSR plus a `<noscript>` meta refresh
//! every 5 s. The client bundle loads under forma's strict nonce'd CSP
//! (`connect-src 'self'` covers the poller, `form-action 'self'` the
//! forms).
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
//! The same per-request data is emitted twice, deliberately: server-side
//! FMIR slot injection for the SSR first paint (scalars and lists by slot
//! name; `createShow` booleans positional — the last shared-name slot kind),
//! and a `StatusPayload` JSON as island props (`__forma_islands` script
//! block) for client hydration — the identical shape `GET /api/status`
//! serves the poller, pinned by a parity test. See `render.rs` for the
//! mechanism, the rationale, and the E7 dogfood history — first cycle
//! closed when `@getforma/compiler` 0.2.0 shipped the per-list naming this
//! seam requested.
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

// The mapper's scalar-slot object is one `serde_json::json!` literal per page
// (render_map.rs `scalar_slots`), and that macro recurses once per field. The
// object is deliberately flat and deliberately long — every state on the page
// is a named scalar — so it outgrew the default 128.
#![recursion_limit = "256"]

mod control;
mod error;
mod render;
mod render_map;
mod server;
mod snapshot;

pub use control::{
    BindConflict, BindOutcome, BindRequest, ControlSource, LearnView, MacroOutcome, MacroWrite,
    SessionView,
};
pub use error::StudioError;
pub use server::serve;
pub use snapshot::{
    MacroSnapshot, MacroStepView, MacroView, MapPayload, MapperSlot, MapperSnapshot, PadRow,
    ProfileRow, StatusSnapshot, StatusSource,
};
