//! The control contract — **now `ksx-api`'s** (docs/M9-DECISION.md §6).
//!
//! `ControlSource` and its view types were defined here while Studio was the
//! only surface that had them. They are not Studio's any more: `ksx session`,
//! the E5 MCP shim and any future native shell perform the same verbs, and a
//! contract that lives inside the web crate cannot be consumed by a build that
//! excludes it (`--features studio` is off by default — E7 rule A).
//!
//! So the types moved to `ksx-api`, which links no axum, no forma and no
//! tokio, and this module is the re-export that keeps every `crate::control::…`
//! path in `render.rs`, `render_map.rs` and `server.rs` meaning what it always
//! meant. Nothing here has behaviour; the seam is the point.

pub use ksx_api::control::*;
/// The three restore destinations. `RESTORE_MODES` is the wire spelling a form
/// post is validated against before it has a [`RestoreMode`]; the enum is what
/// the verb takes, so "restore" can never travel as an unchecked string.
pub use ksx_api::wire::{RestoreMode, RESTORE_MODES};
