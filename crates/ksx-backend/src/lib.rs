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

pub mod console;
#[cfg(windows)]
pub mod ctrl_c;
pub mod logging;
pub mod setup;
