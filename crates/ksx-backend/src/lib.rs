//! ksx — the backend. Every verb's typed spec, pure plan and I/O.
//!
//! # Why this is its own crate
//!
//! `docs/SURFACES.md` §1 states the one rule this project follows: **the
//! backend owns state; every surface is a view**. Until this crate existed
//! that rule was on the honour system, because the backend and one of its
//! three surfaces — the CLI — were the same crate. `ksx-app` was
//! simultaneously the argument parser, the daemon, the session supervisor, all
//! the business logic, the Windows plumbing and the composition root: 50,665
//! lines across 49 files, 27 of them touching Win32. Changing one verb meant
//! reading a great deal of code that had nothing to do with it.
//!
//! The crate graph was never the problem — `ksx-core` depends on nothing,
//! `ksx-studio` and `ksx-cabinet` reach the backend only through `ksx-api`
//! traits and neither depends on `ksx-app`. The *packaging* was. So this is a
//! move, not a redesign: the modules below arrived here byte-for-byte, with
//! their tests, and `ksx-app` kept `main.rs` — clap definitions, verb dispatch,
//! and nothing else.
//!
//! # What that buys, concretely
//!
//! - A verb's code is reachable without the CLI. `ksx-app` is now the only
//!   crate that knows clap exists, so "which surface holds this logic?" has a
//!   compiler-checked answer instead of a review-checklist one.
//! - The four-way feature matrix means something here too. `studio` and
//!   `cabinet` are independent opt-ins forwarded from `ksx-app`; CI compiles
//!   both crates in all four combinations, because the unit tests that used to
//!   sit behind those gates in `ksx-app` now sit behind them here.
//!
//! # Orientation
//!
//! By verb, not by layer. `mapping` is the mapper's write half; `map` is its
//! CLI-shaped entry point; `device_edit` / `device_scan` are the device
//! picker's write and read halves; `run/` is the session supervisor (`plan.rs`
//! builds a plan, `resolve.rs` turns config spellings into live devnodes);
//! `daemon/` is the resident tray process and its control pipe; `sources.rs`
//! is where surfaces get their data.

pub mod autostart;
#[cfg(feature = "cabinet")]
pub mod cabinet;
#[cfg(windows)]
pub mod capture;
pub mod config_io;
pub mod console;
#[cfg(windows)]
pub mod ctrl_c;
pub mod daemon;
pub mod device_edit;
pub mod device_scan;
pub mod devices;
pub mod doctor;
pub mod feed;
pub mod import_legacy;
pub mod install;
pub mod logging;
pub mod macro_cli;
pub mod macro_trace;
pub mod map;
pub mod mapping;
pub mod monitor;
// The first-run state and the path-free config in/out, for the surfaces that
// have a screen. Gated with `sources` because that is the only caller: the CLI
// reaches this machinery through `config_io` directly.
#[cfg(any(feature = "studio", feature = "cabinet"))]
pub mod onboard;
pub mod pads;
pub mod play;
pub mod preset_cli;
pub mod preset_edit;
// Gated exactly like `sources` below: this is the write half of games.toml and
// Studio's Profiles page is its only caller today. The gate comes off the day a
// `ksx games new` CLI verb exists — which is where it belongs per
// docs/SURFACES.md §2.
#[cfg(any(feature = "studio", feature = "cabinet"))]
pub mod profile_edit;
pub mod run;
pub mod session;
pub mod setup;
pub mod slot_cli;
pub mod slots;
// Gated because it reaches `cabinet` and `studio_launch`, which are themselves
// behind those features — so this is not a tidiness gate, it is what the module
// can actually name.
#[cfg(any(feature = "studio", feature = "cabinet"))]
pub mod sources;
// The staged setup's two exits — save it, or play it without saving
// (docs/FIRST-RUN.md §2). Not feature-gated: `ksx_core::StagedSetup` lives in
// the daemon for the length of a visit, so every build that can run a daemon
// needs the paths that turn one into a config write or a run plan.
pub mod stage;
#[cfg(feature = "studio")]
pub mod studio;
#[cfg(feature = "studio")]
pub mod studio_launch;
pub mod winusb;
