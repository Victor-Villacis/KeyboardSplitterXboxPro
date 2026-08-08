//! **ksx-api — one typed Rust surface every ksx front end consumes.**
//!
//! Studio's server, `ksx session`, a future native shell and the E5 MCP server
//! all speak to the daemon through the traits in this crate. There is no
//! second description of a verb anywhere: the JSON that goes on the control
//! pipe is derived from [`wire`]'s types, and the daemon reads it back through
//! the same ones.
//!
//! # Why this crate exists (docs/M9-DECISION.md §6)
//!
//! M9 was specified as an egui window that would host the supervisor
//! in-process "with no serialization tax, mapping 1:1 to `DaemonCommand`".
//! That window is cancelled; the *property* it was after is not. It lives here
//! instead, as a trait with two implementations:
//!
//! - [`PipeTransport`] — one `\\.\pipe\ksx-daemon` line per call. What Studio
//!   and `ksx session` use, and what any surface in a SEPARATE process uses.
//! - a hosted supervisor — the daemon's own dispatch, called directly, no line
//!   and no parse. What a UI running INSIDE the daemon process would use.
//!
//! Both satisfy [`VerbSink`]; [`Client`] turns either into the
//! [`ControlSource`] a surface is written against. So a surface is written
//! once and can be hosted either way, and the in-process ideal stays available
//! for free if a native shell is ever built — with no second copy of any verb.
//!
//! # The boundaries this crate keeps
//!
//! - **No transport of its own beyond the pipe, and no async.** No axum, no
//!   forma, no tokio, no HTTP types — not even behind a feature. If this crate
//!   ever grows a dependency that can open a socket, the M9 decision has been
//!   undone by accident.
//! - **Exactly the tray's reach.** Every write verb is one `DaemonCommand` or
//!   one call into the single mapping writer. Nothing here can touch capture,
//!   output, or a live session (docs/ARCHITECTURE.md rules 1–5).
//! - **The read side never needs a daemon.** [`StatusSource`] is satisfiable
//!   from the config store and the platform alone, which is what keeps the
//!   read-only mapper alive behind the "No daemon" banner.
//! - **A surface that cannot act must SAY so, per click.** Every defaulted
//!   method answers with a worded [`Refusal`] naming the command that works —
//!   never a silent no-op.
//!
//! # Layout
//!
//! | module | what |
//! |---|---|
//! | [`wire`] | the protocol: one type per request and per response, and the ONE place the JSON shapes are derived from |
//! | [`control`] | the write side — [`ControlSource`] and its view types |
//! | [`status`] | the read side — [`StatusSource`] and its snapshots |
//! | [`machine`] | the local machine verbs (devices, pads, presets, autostart, doctor, WinUSB) |
//! | [`client`] | [`VerbSink`] → [`ControlSource`], for either transport |
//! | [`pipe`] | the named-pipe transport |

pub mod client;
pub mod control;
pub mod live;
pub mod machine;
pub mod pipe;
pub mod refusal;
pub mod status;
pub mod wire;

/// **How many slots exist**, re-exported so a surface can size a list against
/// it without naming `ksx-core`.
///
/// This is part of the wire contract already — [`wire`] validates
/// `1..=MAX_SLOTS` on the way in, and three of this crate's doc comments point
/// at it — but every front end had to reach past `ksx-api` to read the number,
/// so none of them did. That is how the cabinet's slot list came to be sized
/// for four after the ceiling moved to sixteen: nothing in the surface's own
/// dependency list could tell it how many rows there might be.
pub use ksx_core::MAX_SLOTS;

pub use client::{Client, VerbSink};
pub use control::{
    map_request, multi_key_refusal, with_key, without_key, BindConflict, BindOutcome, BindRequest,
    ControlSource, LearnView, MacroOutcome, MacroWrite, SessionView, SlotOutcome,
};
pub use live::{KeyHit, LiveFeed, LiveFrame, NoFeed, PadFeedback, SlotLive};
pub use machine::{
    setup_states, setup_steps, AdviceRow, AutostartView, BoardRow, ConfigExport, ConfiguredDevice,
    DevicePickSpec, DevicePickView, DeviceRemoveSpec, DeviceRemoveView, DeviceScanView,
    DevicesView, DoctorRow, DoctorView, ExportRequest, ImportReport, ImportRequest, ImportWrite,
    KeyboardRow, MachineSource, NewPreset, NewProfile, PadsSpawnSpec, PadsView, PresetRow,
    PresetsView, ProfileDetail, ProfilesView, PrunePlanView, SetupDeviceRow, SetupSlotRow,
    SetupStep, SetupView, SpawnOffer, SpawnOption, TemplateRow, UsbRow, VirtualPadRow, WinusbView,
    CAVEAT_NOT_A_KEYBOARD, CLAIM_LEAD, NO_BOARDS_LINE, RELEASE_LEAD, UNREAD_BOARDS_LINE,
    UNREAD_CONFIGURED_LINE,
};
pub use pipe::{PipeTransport, TransportError, NO_CHANNEL};
pub use refusal::{codes, Refusal, Refused};
pub use status::{
    MacroSnapshot, MacroStepView, MacroView, MapperSlot, MapperSnapshot, PadRow, ProfileRow,
    StatusSnapshot, StatusSource,
};
pub use wire::{
    macro_body, ActionResponse, BackupView, BackupsRequest, BackupsResponse, ClearAllRequest,
    ConflictView, FlashView, HealthView, LastSessionView, LearnResponse, MacroResponse,
    MacroWriteKind, MapMacroRequest, MapRequest, MapResponse, MovedFromView, Request, Response,
    RestoreMode, RestoreRequest, RestoreResponse, SlotAssignRequest, SlotAssignResponse,
    StatusResponse, PIPE_NAME, RESTORE_MODES,
};
