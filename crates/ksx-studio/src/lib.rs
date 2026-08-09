//! ksx-studio — the optional Forma-powered local application for first run,
//! status, mapping, checks, pads, devices, profiles and configuration (M10).
//!
//! Eight focused routes expose those tasks: `/start`, `/`, `/map`, `/check`,
//! `/pads`, `/devices`, `/profiles` and `/setup`. Each route owns one screen;
//! the navigation connects them without turning the product back into a
//! multi-panel dashboard.
//!
//! Rendering model — SSR first paint + one live island per route:
//! forma-server 0.2.0 renders the embedded FMIR per request (a complete page,
//! no JS required), and each screen's Forma island seeds its signals from
//! server props BEFORE adoption (the islands protocol; dogfood ledger #5).
//! Route clients then use the same-origin APIs and streams to update those
//! signals in place. With JavaScript disabled the server-rendered screens
//! remain usable; the status route also carries a `<noscript>` meta refresh.
//! Client bundles load under Forma's strict nonce'd CSP (`connect-src 'self'`
//! covers live data, `form-action 'self'` the forms).
//!
//! `/setup` (M10) is the configuration screen: what this machine
//! holds, and the two verbs a person performs on a configuration — **Export**
//! (`GET /setup/export.json`, the whole root as one JSON document) and
//! **Import** (`POST /setup/import`, dry run unless the write box is ticked).
//! Neither takes a filesystem path: `ksx_api::MachineSource::{config_export,
//! config_import}` work in memory, so no screen has to put a directory in front
//! of someone who asked for their configuration. Under them is the first run —
//! a checklist the BACKEND decides (`ksx-app::onboard::plan_steps`) with one
//! backend verb per step: the board step links to the devices screen, `POST
//! /setup/slot` is one [`ControlSource::assign_slot`], and `POST /setup/prove`
//! is the daemon's own learner, read back per render so it works with scripting
//! switched off.
//!
//! Session state and the three POST routes go through [`ControlSource`] —
//! ksx-backend implements it over the daemon's `\\.\pipe\ksx-daemon` control
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
//! `cargo build` needs nothing but Rust. Node ≥ 20.19 is needed only to
//! REGENERATE the UI after editing `studio-ui/src/`:
//!
//! ```text
//! cd studio-ui
//! npm ci
//! node build.mjs      # rebuilds crates/ksx-studio/assets/, then run the ksx gate
//! ```
//!
//! # Data injection
//!
//! The same per-request route data is emitted twice, deliberately: server-side
//! FMIR slot injection for the SSR first paint (scalars, lists AND
//! `createShow` booleans, all by slot name since compiler 0.3.1), and a
//! typed JSON payload in the `__ksx-payload` script block for client
//! hydration — the same shape each route's live endpoint returns, pinned by
//! parity tests. forma-ir additionally emits its own
//! `data-forma-props` from the island's slot_ids, carrying the rendered slot
//! values. See `render.rs` for the mechanism, the rationale, and the E7
//! dogfood history — cycle one closed when `@getforma/compiler` 0.2.0
//! shipped per-list naming, cycle two when 0.3.1 shipped named shows,
//! island slot_ids and island-file signal extraction.
//!
//! # Boundaries (docs/ENHANCEMENTS.md E7, enforced)
//!
//! - **Localhost only.** [`serve`] refuses any non-loopback bind; there is no
//!   LAN option. That option arrives with the CSPRNG pairing token, not
//!   before.
//! - **Own tokio runtime, normal priority** — created inside [`serve`], never
//!   shared with (or visible to) anything session- or pipeline-related.
//! - This crate depends on **exactly one other ksx crate: `ksx-api`**, the
//!   typed control API every ksx front end consumes (docs/M9-DECISION.md §6).
//!   Data arrives through its [`StatusSource`] and [`ControlSource`] traits;
//!   ksx-backend supplies the implementations (collectors and pipe client
//!   respectively). Nothing here can touch capture, output, or a live session
//!   — a control implementation is a client of the daemon's pipe, never a
//!   second control loop. `ksx-api` links no axum, no forma and no tokio, so
//!   the default build is unaffected by this crate existing.
//!
//!   The traits used to live HERE, which was wrong in one specific way: a
//!   contract cannot be owned by the surface that happens to have been written
//!   first. `ksx session` performs the same verbs with no HTTP anywhere, and a
//!   native shell would too.

// The mapper's scalar-slot object is one `serde_json::json!` literal per page
// (render_map.rs `scalar_slots`), and that macro recurses once per field. The
// object is deliberately flat and deliberately long — every state on the page
// is a named scalar — so it outgrew the default 128.
#![recursion_limit = "256"]

mod control;
mod error;
mod guard;
mod live;
mod render;
mod render_check;
mod render_devices;
mod render_map;
mod render_pads;
mod render_profiles;
mod render_setup;
mod render_start;
mod server;
mod snapshot;

pub use control::{
    BindConflict, BindOutcome, BindRequest, ControlSource, LearnView, MacroOutcome, MacroWrite,
    RestoreMode, SessionView,
};
pub use error::StudioError;
pub use server::serve;
pub use snapshot::{
    CheckPayload, DevicesPayload, MacroSnapshot, MacroStepView, MacroView, MapPayload, MapperSlot,
    MapperSnapshot, PadRow, PadsPayload, ProfileRow, ProfilesPayload, SetupFlags, SetupLines,
    SetupPayload, SetupRows, SetupSnapshot, StatusSnapshot, StatusSource,
};
