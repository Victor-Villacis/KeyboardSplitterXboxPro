//! The MACHINE verbs: the operations docs/CONTROL-SURFACE.md enumerates that
//! are neither a `DaemonCommand` nor a preset write — devices, pads, presets,
//! autostart, doctor, WinUSB.
//!
//! They are here for the same reason [`crate::ControlSource`] is: a native
//! shell, the MCP shim and Studio must reach them through ONE typed surface,
//! or each grows its own JSON and the standing rule ("every front-door action
//! maps to an existing backend verb") becomes a thing reviewers enforce by
//! reading.
//!
//! **Status, stated plainly: this is the typed surface, and nothing implements
//! it yet.** Every method is defaulted, and every default is a WORDED refusal
//! naming the command that does work — never a silent no-op, never an empty
//! view that looks like real data. That is exactly what CONTROL-SURFACE's own
//! column says about these rows today ("M9: same verb in-process. M10: api"):
//! the CLI is the front door, and a surface asking through this trait is told
//! so, per call, with the line to type.
//!
//! The reads (devices, presets, autostart, doctor, WinUSB status) are
//! daemon-free and safe from anywhere, so ksx-app can implement them the day a
//! surface consumes them — the shapes below are what it would fill in. The
//! mutating and elevated ones are a different question and not merely
//! unimplemented: a pad test COMPETES for the four XInput slots, and `winusb
//! claim`/`release` can leave a panel that no longer types. Those keep the
//! CLI's dry-run-first consent shape, and the refusal says which command
//! carries it.
//!
//! Writing the trait before the implementations is deliberate, and it is the
//! cheap half: it fixes the vocabulary (one `DevicesView`, not one per
//! surface), and it means the first consumer adds a provider instead of
//! inventing a second JSON shape on its way to the same collectors.

use serde::{Deserialize, Serialize};

use crate::refusal::Refusal;

/// The machine ksx is installed on, as a surface can see it.
pub trait MachineSource: Send + Sync {
    /// `ksx devices` — the keyboards Interception sees and the USB interfaces
    /// ksx can reason about. Read-only and safe mid-session.
    fn devices(&self) -> Result<DevicesView, Refusal> {
        Err(Refusal::not_here("listing devices", "run `ksx devices`"))
    }

    /// `ksx device scan` — the SAME enumeration [`Self::devices`] returns,
    /// grouped into the physical boards a person picks from, with the
    /// `[[device]]` entries that are configured against them.
    ///
    /// It exists beside `devices` for the reason `crates/ksx-app/src/
    /// device_scan.rs` opens with: one interface per row is the right shape
    /// for diagnosing a backend and the wrong shape for CHOOSING. On the
    /// reference cabinet `ksx devices` prints 29 interfaces, three of which are
    /// one I-PAC. A picker that offered those three would be asking the user to
    /// know which `MI_` number carries the keys.
    ///
    /// Read-only and safe mid-session, exactly like `devices`: it enumerates
    /// and describes. Nothing is opened, claimed or written.
    fn device_scan(&self) -> Result<DeviceScanView, Refusal> {
        Err(Refusal::not_here(
            "grouping the devices into boards",
            "run `ksx device scan`",
        ))
    }

    /// `ksx device pick` — write the `[[device]]` entry for one board.
    ///
    /// Not a claim, and the distinction is load-bearing
    /// (`docs/DEVICE-IDENTITY.md` §7): this writes four lines of TOML and
    /// stops. Taking the board off the Windows keyboard stack is
    /// [`Self::winusb_claim`], which needs elevation and a separate `--yes`.
    /// A surface that renders "picked" as "claimed" produces a user who
    /// discovers otherwise mid-game.
    fn device_pick(&self, _spec: &DevicePickSpec) -> Result<DevicePickView, Refusal> {
        Err(Refusal::not_here(
            "writing a [[device]] entry",
            "run `ksx device pick <ID>`",
        ))
    }

    /// `ksx device remove` — delete one `[[device]]` entry.
    ///
    /// The narrowest of the three removals ksx has, and the three must never be
    /// confused: `ksx pads --prune` drops stale VIRTUAL PADS off the ViGEm bus,
    /// `ksx winusb release` puts a claimed board BACK on the keyboard stack,
    /// and this forgets a config entry. Deleting the entry releases nothing —
    /// which is why [`DeviceRemoveView::still_claimed`] exists.
    fn device_remove(&self, _spec: &DeviceRemoveSpec) -> Result<DeviceRemoveView, Refusal> {
        Err(Refusal::not_here(
            "deleting a [[device]] entry",
            "run `ksx device remove <ALIAS>`",
        ))
    }

    /// `ksx preset list [--templates]` — the presets on disk and the templates
    /// a new one can be seeded from.
    fn presets(&self) -> Result<PresetsView, Refusal> {
        Err(Refusal::not_here(
            "listing presets",
            "run `ksx preset list`",
        ))
    }

    /// The `[[game]]` profiles in games.toml, **with the launch preflight
    /// already run**.
    ///
    /// That last clause is the whole reason this is not just
    /// [`StatusSnapshot::profiles`](crate::status::StatusSnapshot::profiles),
    /// which carries a title and a composed line and nothing a surface can
    /// branch on. `ksx_games::preflight` has always been able to say "that
    /// .exe is not there" — it just ran at LAUNCH time, so a profile pointing
    /// at a moved emulator looked perfectly healthy in every list until the
    /// moment someone tried to play. Running the same check on the read side
    /// costs one `is_file()` per row and turns a mystery into a row with the
    /// wrong path printed on it.
    fn profiles(&self) -> Result<ProfilesView, Refusal> {
        Err(Refusal::not_here(
            "listing games.toml profiles",
            "run `ksx config export --what games`",
        ))
    }

    /// Add a `[[game]]` profile to games.toml.
    ///
    /// The remedy names Studio rather than a CLI verb because there is no CLI
    /// verb: `ksx setup` writes INTO an existing profile and refuses when the
    /// title is absent, and `ksx config import` replaces the whole file. This
    /// is the surface docs/SURFACES.md §3 gives "Edit config, profiles" to,
    /// and until a `ksx games new` exists the honest remedy is to say so.
    fn profile_new(&self, _spec: &NewProfile) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "creating a games.toml profile",
            "run `ksx studio` and use its Profiles page",
        ))
    }

    /// `ksx preset new <NAME> --from-template <ID>` — one atomic write that
    /// turns an in-box template into an ordinary, editable preset file.
    fn preset_new(&self, _spec: &NewPreset) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "creating a preset",
            "run `ksx preset new \"<NAME>\" --from-template <ID>`",
        ))
    }

    /// `ksx autostart --status` — the logon-task registration.
    fn autostart(&self) -> Result<AutostartView, Refusal> {
        Err(Refusal::not_here(
            "reading the autostart registration",
            "run `ksx autostart --status`",
        ))
    }

    /// `ksx doctor` — driver health plus advice, with the stable codes.
    fn doctor(&self) -> Result<DoctorView, Refusal> {
        Err(Refusal::not_here("the driver report", "run `ksx doctor`"))
    }

    /// `ksx winusb status` — read-only; nothing is opened or claimed.
    fn winusb(&self) -> Result<WinusbView, Refusal> {
        Err(Refusal::not_here(
            "the WinUSB survey",
            "run `ksx winusb status`",
        ))
    }

    /// `ksx pads` / `ksx pads --prune`, as a page can read them: what is on
    /// the bus, what would be removed, and what a spawn may legally offer.
    ///
    /// Read-only. Everything a surface needs to decide *before* a click —
    /// including the XInput ceiling stated in words — arrives here, so no
    /// surface has to know that four is the number (docs/SURFACES.md §1).
    ///
    /// **`session_running` is an input, not something this re-reads.** Both
    /// pad verbs refuse while emulation is live, so the answer appears twice
    /// on one screen: in the header pill and in the spawn panel's refusal. A
    /// second daemon round-trip inside this call would make those two reads
    /// separate points in time, and a session that starts between them paints
    /// "idle" beside "a session is running". One read, one render.
    fn pads_view(&self, _session_running: bool) -> Result<PadsView, Refusal> {
        Err(Refusal::not_here(
            "the virtual-pad report",
            "run `ksx pads --prune` (a dry run) or `ksx doctor`",
        ))
    }

    /// `ksx pads` — plug N test pads, run the pattern, unplug.
    ///
    /// Defaulted-refused, and the default is still the honest answer for any
    /// surface that has not thought about the two things the CLI carried
    /// implicitly: test pads COMPETE for the four XInput slots, so this is
    /// only ever legal while emulation is stopped, and the Ctrl+C that ends a
    /// console run does not exist on a web page. An implementation owes both —
    /// a refusal while a session is live, and a bounded hold that unplugs
    /// itself — which is why the spec carries [`PadsSpawnSpec::hold_secs`]
    /// rather than leaving "how does this end" to the caller.
    fn pads(&self, _spec: &PadsSpawnSpec) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "the pad test",
            "run `ksx pads --count 4` with emulation stopped",
        ))
    }

    /// `ksx pads --prune` — clear pads that outlived whatever made them.
    ///
    /// `confirm` is the CLI's `--yes`, and it means the same thing: without it
    /// this is a DRY RUN that only says what it would do. The dry-run-first
    /// consent shape is not the surface's to choose — a verb that restarts a
    /// bus device and drops every pad on it keeps it wherever it is called
    /// from.
    ///
    /// Restarting a bus devnode needs an administrator token and ksx never
    /// self-elevates, so an implementation running unelevated must REFUSE in
    /// words with the command to run instead — never silently do nothing.
    /// [`PadsView::elevated`] is how a page says that before the click.
    fn pads_prune(&self, _confirm: bool) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "clearing the bus",
            "run `ksx pads --prune` (a dry run) and then `--yes` from an elevated prompt",
        ))
    }

    /// `ksx winusb claim` — takes a keyboard OUT of the keyboard stack.
    ///
    /// Defaulted-refused deliberately, and this one is not a scheduling
    /// concern: the worst case is a panel that no longer types and a user who
    /// cannot type the command to undo it. It is a dry run by default, needs
    /// `--yes` AND elevation, and CONTROL-SURFACE keeps it local on purpose.
    fn winusb_claim(&self, _device: &str) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "claiming a panel for WinUSB",
            "run `ksx winusb claim <ID>` (a dry run) and then `--yes` from an elevated prompt",
        ))
    }

    /// `ksx winusb release` — puts it back. Same consent shape as the claim.
    fn winusb_release(&self, _device: &str) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "releasing a panel back to the keyboard stack",
            "run `ksx winusb release <ID>` (a dry run) and then `--yes` from an elevated prompt",
        ))
    }

    /// The first-run state: what the config root already holds, and which of
    /// the onboarding steps is next.
    ///
    /// The STEPS are part of the answer on purpose. "Which step comes next" is
    /// a decision about configuration, and docs/SURFACES.md §1 puts decisions
    /// in the backend — a surface that worked it out from the rows would be a
    /// second, silently diverging copy of the rule the moment a fifth step
    /// exists.
    fn setup_state(&self) -> Result<SetupView, Refusal> {
        Err(Refusal::not_here(
            "reading the first-run state",
            "run `ksx setup`",
        ))
    }

    /// `ksx config export` — the config root as ONE interop JSON document,
    /// **in memory**.
    ///
    /// Deliberately not a path. The CLI's export writes to stdout or to a file
    /// because that is what a shell can consume; a surface that offers the user
    /// a DOWNLOAD needs the bytes, and giving it a path instead would put the
    /// filesystem back in front of a person who asked for a file (which is the
    /// whole reason `/setup` exists). Same `ksx_config::interop` machinery
    /// underneath either way — there is no second serializer.
    fn config_export(&self, _request: &ExportRequest) -> Result<ConfigExport, Refusal> {
        Err(Refusal::not_here(
            "exporting the configuration",
            "run `ksx config export`",
        ))
    }

    /// `ksx config import` — a document in, **dry run unless
    /// [`ImportRequest::apply`]**.
    ///
    /// The consent shape is load-bearing and is the CLI's, unchanged
    /// (`ksx-app/src/config_io.rs`): a first call reports exactly which files
    /// it would create and which it would overwrite, every overwrite leaves a
    /// timestamped `.bak`, and validation faults refuse the write unless
    /// [`ImportRequest::force`] says otherwise. A surface may not skip the dry
    /// run — replacing a cabinet's whole configuration is not a thing to do on
    /// one click.
    fn config_import(&self, _request: &ImportRequest) -> Result<ImportReport, Refusal> {
        Err(Refusal::not_here(
            "importing a configuration",
            "run `ksx config import`",
        ))
    }

    /// Get ksx Studio on screen — start it if nothing is listening, wait for
    /// the port to answer, then hand the URL to the shell.
    ///
    /// Returns the URL, so a surface that cannot open a browser (a cabinet on a
    /// 10-foot screen with no pointer, a phone across the room) can still
    /// *display* it. That is the whole reason this returns a string rather than
    /// `()`: on a cabinet the useful outcome is often "type this on your
    /// phone", not "a window appeared".
    ///
    /// The implementation must never open a browser before the port answers —
    /// `docs/M9-DECISION.md` §4: *it must never be possible to reach
    /// `ERR_CONNECTION_REFUSED` by clicking a ksx shortcut.* A menu item that
    /// opens a browser at a dead port has not opened Studio; it has produced an
    /// error page with ksx's name on it.
    fn open_studio(&self) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "opening ksx Studio",
            "run `ksx studio` and browse to the address it prints",
        ))
    }
}

/// `ksx devices`, presentation-shaped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesView {
    pub generated_at: String,
    /// Keyboards the Interception driver sees, in slot order.
    pub keyboards: Vec<KeyboardRow>,
    /// Was the Interception driver available at all? `false` is the EXPECTED
    /// end state after M6, not a failure.
    pub interception_available: bool,
    /// HID-class USB interfaces, claimable or not.
    pub usb: Vec<UsbRow>,
    /// Was USB enumeration possible?
    pub usb_available: bool,
    /// Anything the report has to say out loud (both backends missing, an
    /// enumeration that failed) — rendered, never swallowed.
    pub notes: Vec<String>,
}

/// One keyboard the capture layer can see.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardRow {
    /// Interception's slot number — the id `[[device]]` entries name.
    pub slot: u16,
    /// The hardware id, as config matching spells it.
    pub hardware_id: String,
    /// The `[[device]]` alias bound to it, if any.
    pub alias: Option<String>,
    /// `interception` | `winusb` — which backend config asks for.
    pub backend: String,
    /// Composed human line (make/model where known, the duplicate-id warning
    /// where it applies).
    pub detail: String,
}

/// One USB interface `ksx winusb status` reasons about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbRow {
    /// The instance id, uppercased — the string a `[[device]] id` holds.
    pub instance_id: String,
    pub description: String,
    /// `claimed` | `claimable` | `not-a-keyboard` | `foreign-driver`.
    pub state: String,
    /// The one sentence that says what that means for ksx.
    pub verdict: String,
    /// The `[[device]]` alias bound to this id, if any.
    pub alias: Option<String>,
    /// A `[[device]]` entry selects `backend = "winusb"` for it.
    pub selected: bool,
    /// Selected AND rebound — `ksx run` will capture it.
    pub ready: bool,
    /// The board's name, when ksx recognises the VID/PID
    /// (`ksx_core::vendors`). `None` is the normal answer and not a failure —
    /// most devices are not in the table, and [`Self::description`] carries
    /// what the device says about itself.
    ///
    /// Display only, always: `docs/DEVICE-IDENTITY.md` §6 is explicit that no
    /// capture, claim or refusal path may branch on a vendor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// The physical board this interface belongs to.
    ///
    /// One I-PAC exposes three interfaces (`MI_00`/`01`/`02`); they are one
    /// device to a human and three devnodes to Windows. Grouping by this is
    /// what lets a picker say "I-PAC 4X — 3 interfaces, keyboard on MI_00"
    /// instead of listing three cryptic paths and asking the user to guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    /// Does this interface declare the HID **boot keyboard** protocol?
    ///
    /// The difference between "ksx could claim this" and "this is a keyboard",
    /// and a picker needs both. `state == "claimable"` is deliberately
    /// generous — it means "it is HID", because a rebound interface stops
    /// describing itself as a keyboard and plenty of NKRO firmware reports
    /// protocol 0, so guessing harder there would produce confident wrong
    /// answers. The real proof is the report descriptor at claim time.
    ///
    /// The cost of that generosity shows up the moment you render a MENU: on
    /// the reference cabinet a mouse, an LED controller, a fan controller and
    /// a USB audio device all have HID interfaces and all read as claimable.
    /// This flag is the honest positive signal — set, it is very probably a
    /// keyboard; unset, it might still be one, and only claiming proves it.
    #[serde(default)]
    pub boot_keyboard: bool,
    /// The `[[device]] id` `ksx device pick` **would** write for this interface
    /// — the weakest rung that still names it alone, chosen against everything
    /// else connected in the same pass.
    ///
    /// Computed in the backend, exactly once, by the same
    /// `DeviceSelector::strongest_for` call the writer makes
    /// (`docs/SURFACES.md` §1: a surface renders a result, it does not re-derive
    /// one). Two surfaces cannot therefore print two different answers to "what
    /// would you write for this board?", and no surface can print one the writer
    /// would not have written.
    ///
    /// `docs/DEVICE-IDENTITY.md` §5 promises this out loud — *"`ksx device scan`
    /// prints the stronger selector it would write and leaves the decision with
    /// them"* — and so do two user-facing strings, `DeviceSelector::explain` and
    /// `ResolveError::Missing`. It was a promise nothing kept until this field
    /// existed.
    ///
    /// `None` only when no `usb:` selector could name the interface at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

/// `ksx device scan`, presentation-shaped: one row per PHYSICAL board, plus
/// the `[[device]]` entries this config already holds.
///
/// The two lists are separate on purpose and neither is derivable from the
/// other. A board can be plugged in and unconfigured (the thing to pick), and
/// an entry can be configured with its board unplugged (the thing that needs
/// saying out loud, because it looks identical to a broken config from the
/// slot's end).
///
/// # Every derived field on this view is decided by [`Self::read`]
///
/// The partition into pickable/unpickable boards, the counts, the summary
/// lines and the two "…and that emptiness is a real reading" flags are NOT
/// independently settable. They come out of [`Self::read`], and a surface's
/// only job is to render them.
///
/// That is `docs/SURFACES.md` §1 applied to the thing that keeps going wrong
/// here: Studio's SSR seam is Rust and its poller is TypeScript, so any
/// decision a surface takes gets taken TWICE, in two languages, and the two
/// drift silently. The `/devices` page shipped with exactly that — one copy
/// consulted [`Self::usb_available`] and the other did not, so a machine whose
/// enumeration FAILED was told "no keyboard-capable board found".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceScanView {
    pub generated_at: String,
    /// Was USB enumeration possible? `false` means [`Self::boards`] is empty
    /// because nothing could be READ, not because nothing is plugged in.
    pub usb_available: bool,
    /// Every board, in enumeration order (stable on one machine), probable
    /// keyboards first.
    pub boards: Vec<BoardRow>,
    /// Every `[[device]]` entry in `config.toml`, resolved against the live
    /// enumeration.
    pub configured: Vec<ConfiguredDevice>,
    /// Anything the report has to say out loud — rendered, never swallowed.
    pub notes: Vec<String>,
    /// How many of [`Self::boards`] ksx could actually capture
    /// ([`BoardRow::pickable`]).
    #[serde(default)]
    pub pickable_boards: usize,
    /// How many of [`Self::boards`] have no keyboard interface.
    #[serde(default)]
    pub other_boards: usize,
    /// The line above the configured list.
    #[serde(default)]
    pub configured_summary: String,
    /// The line above the pickable-board list — and the one place an empty
    /// list is NOT the same sentence as an empty machine.
    #[serde(default)]
    pub boards_summary: String,
    /// The line above the "other USB interfaces" list; empty when there are
    /// none.
    #[serde(default)]
    pub other_summary: String,
    /// **Is [`Self::boards`] empty *because this machine has no keyboard-capable
    /// board*?**
    ///
    /// The single flag that licenses a surface to say "there is nothing here".
    /// `false` whenever the list is empty because nothing could be READ — a
    /// refused or failed read is not an assertion of absence, and on a cabinet
    /// with four boards plugged in the difference is the whole answer.
    #[serde(default)]
    pub no_pickable_board_found: bool,
    /// **Is [`Self::configured`] empty *because `config.toml` names no
    /// device*?** Same contract as [`Self::no_pickable_board_found`]: `false`
    /// when the config was never read.
    #[serde(default)]
    pub no_configured_device: bool,
}

/// A view of a machine nothing has been read from — NOT an empty machine.
///
/// `Default` is this deliberately, and it is hand-written rather than derived
/// so it cannot degrade into a confident wrong answer. Every path that fails to
/// read (a `MachineSource` refusal, a panicked collector, a surface that has no
/// enumerator) reaches for `DeviceScanView::default()`, and every one of them
/// must produce a view whose sentences say "not read" and whose
/// `no_*` flags are `false`.
impl Default for DeviceScanView {
    fn default() -> Self {
        Self::unreadable()
    }
}

impl DeviceScanView {
    /// Compose the view from what was actually read.
    ///
    /// The ONLY constructor for a view that describes a real machine, so that
    /// the partition, the counts, the summary lines and the per-entry health
    /// verdicts are decided once — here — rather than once per surface and
    /// once per language. Test fixtures build through it too, which is why no
    /// fixture can hand a page a summary line that disagrees with the list
    /// beneath it.
    pub fn read(
        generated_at: String,
        usb_available: bool,
        mut boards: Vec<BoardRow>,
        mut configured: Vec<ConfiguredDevice>,
        notes: Vec<String>,
    ) -> Self {
        for board in &mut boards {
            board.pickable = board.keyboard.is_some();
            board.caveat = if board.looks_like_a_keyboard {
                String::new()
            } else {
                CAVEAT_NOT_A_KEYBOARD.to_owned()
            };
            let (lead, command) = command_cell(
                board.claim_command.as_deref(),
                board.release_command.as_deref(),
            );
            board.command_lead = lead;
            board.command = command;
        }
        for device in &mut configured {
            let (line, level) = device_health(device);
            device.health_line = line;
            device.health_level = level.to_owned();
            let (lead, command) = command_cell(
                device.claim_command.as_deref(),
                device.release_command.as_deref(),
            );
            device.command_lead = lead;
            device.command = command;
        }
        let pickable_boards = boards.iter().filter(|b| b.pickable).count();
        let other_boards = boards.len() - pickable_boards;
        Self {
            configured_summary: configured_summary_line(configured.len()),
            boards_summary: boards_summary_line(pickable_boards, usb_available),
            other_summary: other_summary_line(other_boards),
            // The two conclusions, drawn once. `usb_available` gates the first
            // for the reason its own doc gives; the second is unconditional
            // here because reaching `read` at all means the config WAS read.
            no_pickable_board_found: usb_available && pickable_boards == 0,
            no_configured_device: configured.is_empty(),
            pickable_boards,
            other_boards,
            generated_at,
            usb_available,
            boards,
            configured,
            notes,
        }
    }

    /// The view for a machine that could not be read: empty lists, and every
    /// line saying why the list is empty.
    ///
    /// Both `no_*` flags are `false`, so nothing downstream is licensed to
    /// draw an empty machine.
    pub fn unreadable() -> Self {
        Self {
            generated_at: String::new(),
            usb_available: false,
            boards: Vec::new(),
            configured: Vec::new(),
            notes: Vec::new(),
            pickable_boards: 0,
            other_boards: 0,
            configured_summary: UNREAD_CONFIGURED_LINE.to_owned(),
            boards_summary: UNREAD_BOARDS_LINE.to_owned(),
            other_summary: String::new(),
            no_pickable_board_found: false,
            no_configured_device: false,
        }
    }
}

/// What the boards line says when the enumeration did not answer.
///
/// Kept as a named constant because it is the sentence this page exists to get
/// right, and because two tests assert it appears while the empty-machine
/// sentence does not.
pub const UNREAD_BOARDS_LINE: &str =
    "USB enumeration failed — this list is empty because nothing could be READ, not because \
     nothing is plugged in";

/// The same distinction for the configured list. A refusal reaches this page
/// before `config.toml` is opened, so "no [[device]] entries" would be a claim
/// about a file nobody looked at.
pub const UNREAD_CONFIGURED_LINE: &str =
    "config.toml was not read — this list is empty because nothing could be READ, not because no \
     device is configured";

/// The empty-machine sentence. Named so the tests that must prove it is ABSENT
/// under a failed read cannot drift away from the string the page emits.
pub const NO_BOARDS_LINE: &str = "no keyboard-capable board found";

/// The lead that must accompany `ksx winusb claim` wherever it is shown.
///
/// It says ELEVATED because both commands need an admin shell, and a copyable
/// command handed over without that word produces one "access denied" and no
/// explanation. It says the claim STOPS THE BOARD TYPING because that is the
/// consequence, and it is the one people do not expect.
pub const CLAIM_LEAD: &str =
    "To move it to the WinUSB backend it must be claimed — it then stops typing to Windows, which \
     is a separate, consented step. Run this in an ELEVATED shell:";

/// The same for `ksx winusb release`.
pub const RELEASE_LEAD: &str =
    "It is claimed, so Windows sees no keyboard on it. To put it back on the keyboard driver, run \
     this in an ELEVATED shell:";

/// What a board that is merely HID has to be told about itself.
///
/// `state == "claimable"` is deliberately generous — it means "it is HID" — so
/// on the reference cabinet a mouse, an LED controller and a fan controller all
/// read as claimable. Without this sentence "ksx could claim it" reads as a
/// recommendation.
pub const CAVEAT_NOT_A_KEYBOARD: &str =
    "NOT declared as a keyboard. This is an HID interface and may be something else entirely — on \
     a real cabinet a mouse, an LED controller and a fan controller all read as claimable.";

/// `(lead, command)` for the elevated command a row shows, or two empty
/// strings. Release wins when both are somehow present: it is the one that
/// describes the board's current state.
fn command_cell(claim: Option<&str>, release: Option<&str>) -> (String, String) {
    match (release, claim) {
        (Some(cmd), _) => (RELEASE_LEAD.to_owned(), cmd.to_owned()),
        (None, Some(cmd)) => (CLAIM_LEAD.to_owned(), cmd.to_owned()),
        (None, None) => (String::new(), String::new()),
    }
}

fn configured_summary_line(count: usize) -> String {
    match count {
        0 => "no [[device]] entries in config.toml".to_owned(),
        1 => "1 [[device]] entry in config.toml:".to_owned(),
        n => format!("{n} [[device]] entries in config.toml:"),
    }
}

fn boards_summary_line(count: usize, usb_available: bool) -> String {
    if !usb_available {
        return UNREAD_BOARDS_LINE.to_owned();
    }
    match count {
        0 => NO_BOARDS_LINE.to_owned(),
        1 => "1 keyboard-capable board:".to_owned(),
        n => format!("{n} keyboard-capable boards:"),
    }
}

fn other_summary_line(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 board has no keyboard interface — ksx cannot capture it:".to_owned(),
        n => format!("{n} boards have no keyboard interface — ksx cannot capture them:"),
    }
}

/// `(sentence, level)` for one configured entry — including the one
/// combination that is a real fault.
///
/// `backend = "winusb"` on a board that is NOT bound to `winusb.sys` is the
/// only config shape a session refuses to start on, and it refuses **only for
/// the slots that name the alias**: `run/plan.rs` builds `captureable` from
/// slots' keyboards, `run/resolve.rs` promotes an id into `plan.winusb` only if
/// it is already in `captureable`, and `capture.rs` raises `NotRebound` only
/// for ids in `plan.winusb`. An entry no slot references never enters the plan,
/// so the session starts cleanly — telling that user "ksx run will refuse"
/// sends them to debug a session that works.
///
/// It is decided here because it is a verdict about what `ksx run` does, and a
/// verdict minted in a view file is a verdict every other surface has to mint
/// again.
fn device_health(device: &ConfiguredDevice) -> (String, &'static str) {
    if !device.present {
        // Nothing is connected, so there is no driver binding to describe. A
        // stale claim pill here would be a claim about a board that is absent.
        return (String::new(), "none");
    }
    if device.claimed {
        return ("claimed — bound to winusb.sys".to_owned(), "ok");
    }
    if device.backend != "winusb" {
        return ("on the Windows keyboard stack".to_owned(), "idle");
    }
    if device.used_by.is_empty() {
        (
            "backend is winusb but the board is NOT claimed. No [[slot]] names this alias, so a \
             session still starts — the first slot that names it will refuse."
                .to_owned(),
            "idle",
        )
    } else {
        (
            "backend is winusb but the board is NOT claimed — every slot naming it refuses to \
             start"
                .to_owned(),
            "warn",
        )
    }
}

/// One physical board: every interface that shares a composite parent
/// (`crates/ksx-app/src/device_scan.rs` `Board`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardRow {
    /// The vendor table's name, else the device's own product string, else the
    /// parent path. Never empty.
    pub name: String,
    /// Every devnode this one board wears.
    pub interfaces: Vec<UsbRow>,
    /// The instance id of the interface a keyboard would be captured through,
    /// when the board has one. `None` = ksx cannot capture this board, and it
    /// cannot be picked either.
    pub keyboard: Option<String>,
    /// That interface's one-sentence verdict, straight from the enumerator.
    pub keyboard_verdict: String,
    /// Does anything here DECLARE itself a keyboard (HID boot protocol), or is
    /// it merely HID? A mouse, an LED controller and a fan controller are all
    /// claimable; only this separates them from a panel.
    pub looks_like_a_keyboard: bool,
    /// Is the keyboard interface bound to `winusb.sys` right now — i.e. off the
    /// Windows keyboard stack?
    pub claimed: bool,
    /// The `[[device]]` alias bound to this board, if any.
    pub alias: Option<String>,
    /// `ksx winusb claim <ID>` — present only while the board is claimABLE.
    /// A command, never an action: the claim needs elevation, so a surface
    /// that cannot elevate SHOWS this rather than running it.
    pub claim_command: Option<String>,
    /// `ksx winusb release <ID> --yes` — present only while it is claimed.
    pub release_command: Option<String>,
    /// Can this board be picked at all — i.e. does it have a keyboard
    /// interface ([`Self::keyboard`] is `Some`)?
    ///
    /// The partition every picker needs, taken once by
    /// [`DeviceScanView::read`] rather than re-derived per surface. A pick form
    /// on a board ksx cannot capture is an offer that always refuses, so the
    /// two groups are drawn differently — and before this field existed the
    /// filter was written three times in Rust and twice more in TypeScript.
    #[serde(default)]
    pub pickable: bool,
    /// The elevated command this board's row should SHOW (never run), and the
    /// sentence that has to go with it. Both empty when there is neither.
    /// Chosen from [`Self::claim_command`] / [`Self::release_command`] by
    /// [`DeviceScanView::read`].
    #[serde(default)]
    pub command_lead: String,
    #[serde(default)]
    pub command: String,
    /// The honest caveat when nothing on the board DECLARES itself a keyboard;
    /// empty when [`Self::looks_like_a_keyboard`]. See [`CAVEAT_NOT_A_KEYBOARD`].
    #[serde(default)]
    pub caveat: String,
}

/// One `[[device]]` entry, resolved against the machine as it is right now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredDevice {
    /// The name `[[slot]]` entries use.
    pub alias: String,
    /// The `id` as the FILE spells it.
    pub id: String,
    /// `winusb` | `interception`.
    pub backend: String,
    /// One word for the rung: `model` | `serial` | `port` | `hardware-id`.
    pub rung: String,
    /// Does this id still name the same board after a replug into another
    /// socket? `false` is the PORT-PINNED case.
    pub survives_replug: bool,
    /// What the selector pins down and what it costs
    /// (`DeviceSelector::explain`).
    pub means: String,
    /// The whole PORT-PINNED paragraph, verbatim from the writer that decided
    /// it — including the half that is easy to miss, that a `port=` value names
    /// THIS PC's USB topology and does not travel to another cabinet. `None`
    /// when the id survives a replug.
    ///
    /// Carried rather than re-worded per surface: `ksx device pick` prints it
    /// at write time, and this is the same sentence standing on the entry
    /// afterwards, because the cost is a property of the entry and not of the
    /// moment it was written.
    pub port_pinned_warning: Option<String>,
    /// Does the id resolve to exactly one connected interface right now?
    pub present: bool,
    /// The board's name, when it is present.
    pub board: Option<String>,
    /// The interface it resolved to, when it is present.
    pub instance_id: Option<String>,
    /// Is that interface bound to `winusb.sys`?
    pub claimed: bool,
    pub claim_command: Option<String>,
    pub release_command: Option<String>,
    /// Every slot that names this alias, in `config.toml` AND in every
    /// games.toml profile — the list `remove` refuses on without `--force`.
    pub used_by: Vec<String>,
    /// One sentence about how this entry stands right now: claimed, on the
    /// keyboard stack, or the one combination a session refuses to start on.
    /// Empty when the board is not connected — there is then no driver binding
    /// to describe, and a stale pill would be a claim about an absent board.
    ///
    /// Decided by [`DeviceScanView::read`], never by a surface: it is a verdict
    /// about `ksx run`'s behaviour, and `run` is not something a view file can
    /// see. See `device_health` for which of `run`'s three gates it reads.
    #[serde(default)]
    pub health_line: String,
    /// Severity of [`Self::health_line`]: `ok` | `warn` | `idle` | `none`.
    /// A surface maps this to its own vocabulary of pills and colours; the
    /// judgement itself is not a surface's to make.
    #[serde(default)]
    pub health_level: String,
    /// The elevated command this entry's row should SHOW (never run), and the
    /// sentence that has to go with it. Both empty when there is neither.
    #[serde(default)]
    pub command_lead: String,
    #[serde(default)]
    pub command: String,
}

/// One `ksx device pick`, as any surface spells it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePickSpec {
    /// An instance path, an existing `[[device]]` alias, or any unique part of
    /// an instance path.
    pub query: String,
    /// The name `[[slot]]` entries will use. `None`/empty derives one from the
    /// board, and re-picking a configured board keeps the name it already has.
    pub alias: Option<String>,
}

/// What `ksx device pick` wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePickView {
    pub alias: String,
    /// The `id` value written: the weakest rung that is still unique.
    pub id: String,
    pub backend: String,
    pub board: String,
    pub instance_id: String,
    /// The alias of the entry this replaced; `None` = a new entry.
    pub replaced: Option<String>,
    /// Was the interface ALREADY bound to `winusb.sys`? Nothing here ever
    /// claims one.
    pub claimed: bool,
    /// Did the writer have to fall back to the socket to tell this board from
    /// its twin? The entry then carries
    /// [`ConfiguredDevice::port_pinned_warning`].
    pub port_pinned: bool,
    /// `ksx winusb claim <ID>`, when claiming is the sensible next step.
    pub next_step: Option<String>,
    pub backup: Option<String>,
    /// ONE sentence — a flash is one line and capped, so the structured facts
    /// above are what a page renders.
    pub summary: String,
}

/// One `ksx device remove`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRemoveSpec {
    pub alias: String,
    /// Remove it even though slots still name it.
    pub force: bool,
}

/// What `ksx device remove` deleted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRemoveView {
    pub alias: String,
    pub id: String,
    /// The interface still bound to `winusb.sys`. Deleting config does not
    /// release a board, and this is how the user finds out before the panel is
    /// dark rather than after.
    pub still_claimed: Option<String>,
    pub release_command: Option<String>,
    /// Slots that now name a device which does not exist (`--force` only).
    pub breaks: Vec<String>,
    pub backup: Option<String>,
    /// ONE sentence, for the flash.
    pub summary: String,
}

/// `ksx winusb status`, presentation-shaped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinusbView {
    pub generated_at: String,
    pub interfaces: Vec<UsbRow>,
    /// How many keyboards can still type right now — the number that makes
    /// "claim the last one" a refusal rather than a lockout.
    pub typing_keyboards: usize,
    pub notes: Vec<String>,
}

/// `ksx preset list`, plus the templates a new preset can be seeded from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetsView {
    pub config_root: String,
    pub presets: Vec<PresetRow>,
    pub templates: Vec<TemplateRow>,
}

/// One preset on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetRow {
    pub name: String,
    /// Bound controls (the inert `"None"` placeholders not counted).
    pub bound: usize,
    /// `[macros.<name>]` tables it defines.
    pub macros: usize,
    /// A built-in that must not be overwritten (`default`, `empty`).
    pub protected: bool,
    /// Where it came from — a path, or the built-in's name.
    pub source: String,
}

/// The games.toml profiles, preflighted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesView {
    pub generated_at: String,
    /// The config root these were read from.
    pub config_root: String,
    /// games.toml's own path — the file a broken row has to be fixed in, and
    /// therefore the one string a "this is wrong" row cannot leave out.
    pub games_path: String,
    pub profiles: Vec<ProfileDetail>,
    /// Anything the read has to say out loud (an unreadable file, a profile
    /// with no slots) — rendered, never swallowed.
    pub notes: Vec<String>,
}

/// One `[[game]]` profile, as a surface can see it — including whether the
/// program it names is actually on this disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDetail {
    pub title: String,
    /// The `path` key verbatim: an executable, or a `steam://`-style URL.
    pub path: String,
    pub arguments: String,
    /// How many `[[game.slot]]` entries it hands out.
    pub slots: usize,
    /// The presets those slots name, de-duplicated, in slot order.
    pub presets: Vec<String>,
    /// `ok` | `broken` | `launcher`.
    ///
    /// `launcher` is NOT a weaker `ok`, and collapsing the two would be a lie
    /// in the direction that matters: a `steam://` URL is unverifiable by
    /// construction — only the shell knows whether `rungameid/9999` names a
    /// real game — so ksx cannot promise it works and must not imply it has
    /// checked (`ksx_games::preflight`'s own words).
    pub state: String,
    /// The one sentence that says what that means for a launch.
    pub verdict: String,
    /// The path that does not resolve. `Some` **only** when
    /// `state == "broken"`, because a row that says "broken" without printing
    /// the string it is complaining about sends the user back to the file to
    /// guess which of two paths moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken_path: Option<String>,
}

/// A new `[[game]]` profile, as a surface asks for one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewProfile {
    pub title: String,
    /// The program to launch: a full path to an .exe, or a launcher URL.
    pub path: String,
    /// Command line for it, split the way `CommandLineToArgvW` would.
    #[serde(default)]
    pub arguments: String,
    /// How many `[[game.slot]]` entries to seed, `1..=ksx_core::MAX_SLOTS`.
    ///
    /// Seeding is not a convenience. A profile with no slots hands out no
    /// pads, and `ksx run --game` refuses it — so "create" that produced an
    /// empty shell would answer "I can't make a profile" with a profile that
    /// cannot be used.
    pub slots: u8,
    /// The preset every seeded slot starts on. The device stays unset —
    /// wiring a board to a slot is `ksx setup`'s job and the /devices page's,
    /// not something a create form should guess.
    pub preset: String,
}

/// A new preset, seeded from an in-box template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPreset {
    pub name: String,
    /// The template id ([`TemplateRow::id`]).
    pub template: String,
    /// Which player block to instantiate, `1..=`[`TemplateRow::players`].
    pub player: u8,
    /// Overwrite an existing preset of that name. A timestamped backup is
    /// taken first — same consent shape as `ksx preset new --force`.
    #[serde(default)]
    pub force: bool,
}

/// One preset template (`ksx-core/src/templates.rs`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRow {
    pub id: String,
    pub label: String,
    /// The panel note that travels with it — what this layout assumes.
    pub detail: String,
    /// Player blocks it can instantiate (P1–P4), when it has them.
    pub players: Vec<u8>,
}

/// `ksx pads`, presentation-shaped: the bus, its children, and both verbs'
/// preconditions in one read.
///
/// Composed sentences on purpose, like [`DevicesView`]'s `detail` and
/// [`StatusSnapshot`](crate::StatusSnapshot)'s driver lines: the wording of
/// "four XInput slots, two already taken" is a fact about ksx's own limits,
/// and a surface that re-derives it is a surface that will disagree with the
/// CLI about it (docs/SURFACES.md §1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadsView {
    pub generated_at: String,
    /// One line: how many pads the bus is carrying.
    pub summary: String,
    /// The ViGEmBus devnode the children hang off — also the argument to the
    /// prune command. `None` when the bus has no present devnode.
    pub bus_instance_id: Option<String>,
    /// [`Self::bus_instance_id`] as a sentence, including the case where there
    /// is not one. A surface renders this; it does not compose it.
    #[serde(default)]
    pub bus_line: String,
    pub pads: Vec<VirtualPadRow>,
    /// Known splitter processes alive right now, as `name (pid N)` lines.
    /// Heuristic by nature — a third-party ViGEm feeder is invisible to it.
    pub owners: Vec<String>,
    /// [`Self::owners`] as a sentence — and, when the list is empty, the
    /// hedge the heuristic needs ("no KNOWN splitter", never "no owner").
    #[serde(default)]
    pub owners_line: String,
    /// Is emulation live? Both verbs refuse while it is: a spawn would fight
    /// the pads a player is holding, and a prune would yank them.
    pub session_running: bool,
    /// How many XInput slots Windows has (`ksx_core::MAX_XINPUT_SLOTS`).
    pub xinput_ceiling: u8,
    /// How many of those slots hold a pad right now — **any** pad on this
    /// machine, real or virtual, read from XInput itself
    /// (`ksx_platform::xinput::slots_in_use`).
    ///
    /// `None` means the reading FAILED. It is not zero, and a surface must
    /// not render it as zero: counting only ksx's own virtual pads is how a
    /// cabinet with two real wired Xbox pads gets told four more will be
    /// readable when the true answer is two.
    #[serde(default)]
    pub xinput_in_use: Option<u8>,
    /// **The ceiling, said out loud for this machine right now.** The reason
    /// this is a backend string and not a page's `if`: `ksx pads --count 8
    /// --persona xbox360` plugs eight pads and four of them are invisible to
    /// every game (open task #16), and a surface must be able to say that
    /// BEFORE the button is pressed without knowing why four is the number.
    pub xinput_line: String,
    /// Does THIS process hold an administrator token? `None` = unanswerable.
    /// A prune restarts a bus device, which needs one; a page shows this
    /// before the click rather than after the refusal.
    pub elevated: Option<bool>,
    /// [`Self::elevated`] as a sentence, including the "this prune WILL be
    /// refused" consequence. Not neutral formatting — which is exactly why it
    /// is composed once here instead of twice, in two languages, on two
    /// surfaces that then disagree.
    #[serde(default)]
    pub elevation_line: String,
    /// The destructive panel's lead: what a prune removes, and that it takes
    /// every pad at once. Also not neutral formatting.
    #[serde(default)]
    pub confirm_line: String,
    /// The failed-read banner's HEADING — "ksx could not read the ViGEm bus."
    /// — set by [`Self::unreadable`] and empty on a read that answered.
    ///
    /// A field because the sentence used to exist twice: composed here, and
    /// hardcoded again as an `<h2>` in `PadsIsland.ts`. Two copies of one
    /// sentence in two languages is the drift docs/SURFACES.md §1a names, and
    /// a heading is not an exception to it — reword this constructor and a
    /// page with its own copy would keep announcing the old wording forever.
    #[serde(default)]
    pub unreadable_heading: String,
    pub prune: PrunePlanView,
    pub spawn: SpawnOffer,
}

impl PadsView {
    /// The view for a read that **failed** — which is not the same view as an
    /// empty bus, and must never render as one.
    ///
    /// This is the project's signature bug in miniature: a session once
    /// reported success while the arcade panel was dead, because a board fell
    /// back to a driver nobody had asked for. "I could not read this" and
    /// "there is nothing here" are different sentences and a user acts on
    /// them differently — the first sends them to `ksx doctor`, the second
    /// tells them the machine is fine.
    ///
    /// So every composed sentence here says the reading failed, and the spawn
    /// offer is REFUSED rather than served empty: an empty `<select>` beside a
    /// live Spawn button is a page inviting a click it cannot describe.
    pub fn unreadable(reason: &str) -> Self {
        let failed = "ksx could not read the ViGEm bus.";
        PadsView {
            generated_at: String::new(),
            summary: format!("{failed} Nothing below is a reading of this machine."),
            bus_instance_id: None,
            bus_line: format!("{failed} This is not a claim that the bus has no devnode."),
            pads: Vec::new(),
            owners: Vec::new(),
            owners_line: format!("{failed} Nothing is known about who holds the pads."),
            session_running: false,
            // A fact about Windows, not a reading of this machine — it stays
            // true whether or not the bus could be reached.
            xinput_ceiling: ksx_core::MAX_XINPUT_SLOTS,
            xinput_in_use: None,
            xinput_line: format!(
                "{failed} It cannot say how many XInput slots are free, or how many more pads a \
                 game would see."
            ),
            elevated: None,
            elevation_line: String::new(),
            confirm_line: String::new(),
            unreadable_heading: failed.to_owned(),
            prune: PrunePlanView {
                kind: "unreadable".to_owned(),
                count: 0,
                command: None,
                detail: format!("{failed} It cannot say what a prune would remove."),
            },
            spawn: SpawnOffer {
                counts: Vec::new(),
                personas: Vec::new(),
                holds: Vec::new(),
                note: String::new(),
                refused: Some(format!("{failed} {reason}")),
            },
        }
    }
}

/// One pad hanging off the bus.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualPadRow {
    /// Full device instance id — what a prune removes, named exactly.
    pub instance_id: String,
    /// The id the persona guess was made from, so an `unknown pad` row is
    /// still actionable.
    pub hardware_id: String,
    /// What the bus says it is: `Xbox 360 pad`, `PlayStation (DS4) pad`, or
    /// `unknown pad`.
    pub persona: String,
    /// Does this pad hold one of the four XInput slots?
    pub xinput: bool,
}

/// What `ksx pads --prune` would do, as a page renders it.
///
/// The decision itself is the CLI's pure `plan_prune`; this is that plan on
/// the wire, so no surface re-decides "is a bus restart allowed right now".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrunePlanView {
    /// `nothing` | `no-bus` | `session-running` | `restart`. Stable strings —
    /// a surface keys its panels off these, not off the prose.
    #[serde(default)]
    pub kind: String,
    /// How many pads it would remove.
    pub count: usize,
    /// The command a user could run by hand instead, when one exists. A
    /// driver operation nobody can reproduce without ksx is one nobody can
    /// undo without ksx either.
    pub command: Option<String>,
    /// The paragraph the CLI prints. Too long for a flash (Studio caps those
    /// at 300 chars), which is exactly why it is a field and not a message.
    pub detail: String,
}

/// What a spawn may offer right now, every option already labelled with what
/// it will actually do.
///
/// The labels are the load-bearing part. "8" in a dropdown is a lie on a
/// machine with four XInput slots; "8 pads — only 4 readable, 4 invisible to
/// games" is the same click with the consequence attached, and putting that
/// sentence here rather than in a page is what stops the second surface from
/// disagreeing with the first.
///
/// **The first option in each list is the default**, because that is what a
/// `<select>` does with no further help — so the ordering here IS the default,
/// and there is no second field to keep in step with it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnOffer {
    pub counts: Vec<SpawnOption>,
    pub personas: Vec<SpawnOption>,
    pub holds: Vec<SpawnOption>,
    /// One sentence saying what a spawn will do, in advance — including the
    /// fact that it unplugs itself when the hold expires.
    pub note: String,
    /// Why a spawn must not be offered at all right now (`None` = offer it).
    pub refused: Option<String>,
}

/// One `<option>`: the value that goes on the wire, and what it means.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnOption {
    pub value: String,
    pub label: String,
}

/// `ksx pads --count N --persona P` as a typed spec.
///
/// `hold_secs` is not optional and not a detail: a console run ends at Ctrl+C
/// and a web click has no Ctrl+C, so every caller states when the pads go
/// away. See [`MachineSource::pads`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadsSpawnSpec {
    pub count: u8,
    /// A [`ksx_core::Persona`] name — `xbox360`, `playstation`, … Parsed by
    /// the implementation with the same lenient parser the CLI uses, so a
    /// surface never has to carry the alias table.
    pub persona: String,
    /// Seconds to hold the pads up before they unplug themselves.
    pub hold_secs: u64,
}

/// `ksx autostart --status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartView {
    pub registered: bool,
    /// The one line the status panel prints.
    pub line: String,
    /// What it would start: `daemon` / `run` / an unrecognized command line.
    pub mode: Option<String>,
    /// The games.toml profile it is pointed at, if any.
    pub profile: Option<String>,
}

/// `ksx doctor`, presentation-shaped: the rows, plus the advice with its
/// stable codes and severities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorView {
    pub generated_at: String,
    /// `(subject, line)` — ViGEmBus, Interception, the CI policy, the pads.
    pub rows: Vec<DoctorRow>,
    pub advice: Vec<AdviceRow>,
    /// The worst severity present: `ok` | `info` | `warning` | `critical`.
    pub worst: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorRow {
    pub subject: String,
    pub line: String,
}

/// One piece of advice, with the code the exit status derives from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdviceRow {
    /// The stable code (`vigem-missing`, `interception-inactive`, …).
    pub code: String,
    /// `info` | `warning` | `critical`.
    pub severity: String,
    pub message: String,
    /// The command that fixes it, when one exists — the same field, and the
    /// same obligation, as [`Refusal::remedy`](crate::Refusal::remedy).
    pub remedy: Option<String>,
}

// ---------------------------------------------------------------------------
// First run, and the config in/out that has no path in it
// ---------------------------------------------------------------------------

/// Stable [`SetupStep::id`] values, in the order a first run performs them.
///
/// Named constants because a surface routes on them (the board step is a link
/// to the devices screen, the prove step is a button that starts the learner),
/// and a typo in a match arm would silently render a step nobody can act on.
pub mod setup_steps {
    /// Find the board and give it a name — one `[[device]]` entry.
    pub const BOARD: &str = "board";
    /// Point a slot at a preset — one `[[slot]]`, or one `[[game.slot]]`.
    pub const SLOT: &str = "slot";
    /// Press a button and watch ksx name it.
    pub const PROVE: &str = "prove";
}

/// Stable [`SetupStep::state`] values.
pub mod setup_states {
    /// Already true of this machine — nothing to do.
    pub const DONE: &str = "done";
    /// The next thing to do.
    pub const NOW: &str = "now";
    /// Real, but an earlier step has to happen first.
    pub const LATER: &str = "later";
}

/// The first run, as a surface reads it: what is already configured, and what
/// the next step is.
///
/// The `now` step is the backend's decision and every implementation owes the
/// same invariant — exactly one step is next, always. It is held where a real
/// implementation can be driven against it: `ksx-app`'s
/// `onboard::exactly_one_step_is_next_for_every_state_of_the_machine`, which
/// runs `plan_steps` over every combination of the three counts. A hand-built
/// `SetupView` asserted against here would only test the `vec!` literal beside
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupView {
    pub generated_at: String,
    /// **Support detail, never an interface.** A person setting a cabinet up
    /// does not operate on a directory — they import, export, and follow the
    /// steps. This is here so a bug report can quote it, and it belongs in
    /// small print.
    pub config_root: String,
    /// A `config.toml` (or `config.json`) is really on disk. `false` is the
    /// first-run state, and it is not an error.
    pub config_exists: bool,
    /// `[[device]]` entries — the boards this cabinet has been told about.
    pub devices: Vec<SetupDeviceRow>,
    /// Every wired slot, from `config.toml` and from every games.toml profile.
    pub slots: Vec<SetupSlotRow>,
    /// Preset names on disk.
    pub presets: Vec<String>,
    /// games.toml profile titles.
    pub profiles: Vec<String>,
    /// The onboarding checklist, ordered, with exactly one step in
    /// [`setup_states::NOW`] while anything is left to do.
    pub steps: Vec<SetupStep>,
    /// Anything the read has to say out loud (a games.toml that would not
    /// parse) — rendered, never swallowed.
    pub notes: Vec<String>,
    /// **The highest slot number this build accepts** — `ksx_core::MAX_SLOTS`,
    /// carried so a surface can offer the slots the daemon would actually take.
    ///
    /// Here rather than in the view code for the reason docs/SURFACES.md §1
    /// gives: "which slots exist" is a decision about configuration, and a
    /// surface that answers it from a constant of its own is a second copy of
    /// the rule. Task #17 lifted this number from an arbitrary 8 and had to
    /// chase three frozen `1..=8` literals to do it; a menu built from this
    /// field cannot become the fourth.
    #[serde(default = "default_max_slots")]
    pub max_slots: u8,
}

/// The ceiling a document that predates the field gets — the real one, never
/// zero. A missing field means "an older ksx wrote this", not "no slots".
fn default_max_slots() -> u8 {
    ksx_core::MAX_SLOTS
}

impl Default for SetupView {
    /// Hand-written for one field: `max_slots` defaults to the ceiling this
    /// build really has. A derived `Default` would hand every caller a zero,
    /// and a zero renders as a slot menu with nothing in it.
    fn default() -> Self {
        Self {
            generated_at: String::new(),
            config_root: String::new(),
            config_exists: false,
            devices: Vec::new(),
            slots: Vec::new(),
            presets: Vec::new(),
            profiles: Vec::new(),
            steps: Vec::new(),
            notes: Vec::new(),
            max_slots: default_max_slots(),
        }
    }
}

/// One `[[device]]` entry, as the setup page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupDeviceRow {
    /// The name the user gave it — what slots refer to.
    pub alias: String,
    /// The selector it resolves through. Long and machine-shaped: small print.
    pub id: String,
    /// `interception` | `winusb`.
    pub backend: String,
}

/// One wired slot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupSlotRow {
    pub number: u8,
    /// The `[[device]]` alias (or the raw selector, in a games.toml profile),
    /// or `(any)` when the slot takes whatever is plugged in.
    pub device: String,
    pub preset: String,
    pub persona: String,
    /// `config.toml`, or the games.toml profile title this slot lives in.
    pub source: String,
}

/// One step of the first run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    /// One of [`setup_steps`] — what a surface routes on.
    pub id: String,
    pub title: String,
    /// What is true about this step RIGHT NOW, in one sentence.
    pub detail: String,
    /// One of [`setup_states`].
    pub state: String,
}

/// What to export. Empty `what` is the whole root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRequest {
    /// `config` | `games` | `presets`; empty = all three.
    #[serde(default)]
    pub what: Vec<String>,
}

/// One exported document, held in memory rather than written anywhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigExport {
    /// A suggested name for whatever the surface does with the bytes.
    pub filename: String,
    /// The document — interop JSON (TOML stays canonical on disk).
    pub document: String,
    pub bytes: usize,
    /// Which parts it carries: `config`, `games`, `presets`.
    pub parts: Vec<String>,
    /// How many presets travelled with it.
    pub presets: usize,
    /// Anything the read had to say (unknown keys in a file, a shadowed
    /// `.json`) — said out loud, never swallowed.
    pub warnings: Vec<String>,
}

/// One import attempt. `apply` false — the default — is a DRY RUN.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRequest {
    /// The document itself. A bare (un-enveloped) document needs `what` to
    /// name what it is, exactly as the CLI does — importing the wrong file
    /// over the wrong file is the failure this verb exists to prevent.
    pub document: String,
    #[serde(default)]
    pub what: Vec<String>,
    /// **Write.** Absent or false reports what would happen and changes
    /// nothing.
    #[serde(default)]
    pub apply: bool,
    /// Write even though validation found faults (advisories never block).
    #[serde(default)]
    pub force: bool,
}

/// What an import did, or would do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// The verb ended the way it was asked to. A DRY RUN with a clean plan is
    /// `true`; a write refused by validation faults and a write that stopped
    /// half way are both `false`.
    ///
    /// A separate field from [`applied`](Self::applied) so no surface has to
    /// re-derive "did this go wrong" from two other booleans and get it
    /// subtly different from the next surface.
    pub ok: bool,
    /// Files were really written. `false` is a dry-run report — or a refusal.
    pub applied: bool,
    /// ONE sentence, short enough to survive a redirect's `?flash=` — the
    /// no-JavaScript path has nowhere else to put it.
    pub summary: String,
    /// The write that stopped a half-finished import. The paths in
    /// [`written`](Self::written) ARE on disk; the rest are not.
    pub error: Option<String>,
    pub parts: Vec<String>,
    /// Every file the import touches, whether or not it was applied.
    pub writes: Vec<ImportWrite>,
    /// Validation faults in the configuration this import would produce.
    /// Non-empty and `force` unset means nothing was written.
    pub faults: Vec<String>,
    /// Validation advisories — never blocking, always shown.
    pub advisories: Vec<String>,
    pub warnings: Vec<String>,
    /// Paths really written (empty on a dry run).
    pub written: Vec<String>,
    /// The `.bak-YYYYMMDD-HHMMSS` copies taken first — the road home.
    pub backups: Vec<String>,
}

/// One file an import touches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWrite {
    /// `config` | `games` | `presets`.
    pub part: String,
    pub path: String,
    /// `create` | `overwrite`.
    pub action: String,
    /// `toml` | `json` — the format the file KEEPS, not the document's.
    pub format: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A failed read must not render as an empty bus.**
    ///
    /// Fails against the version this replaced, which handed surfaces a
    /// `PadsView::default()` when the provider refused: every composed
    /// sentence came out either blank or, worse, an assertion of absence —
    /// the devnode line read "none present — ViGEmBus is not installed" about
    /// a bus ksx had never managed to look at, and the spawn offer arrived
    /// un-refused with three empty dropdowns beside a live button.
    #[test]
    fn an_unreadable_view_says_so_everywhere_and_asserts_nothing() {
        let view = PadsView::unreadable("the pipe did not answer");
        let default = PadsView::default();

        for (what, line) in [
            ("summary", &view.summary),
            ("bus_line", &view.bus_line),
            ("owners_line", &view.owners_line),
            ("xinput_line", &view.xinput_line),
            ("prune.detail", &view.prune.detail),
            // The banner's heading is composed HERE, once — the page renders
            // it and owns no copy of the sentence.
            ("unreadable_heading", &view.unreadable_heading),
        ] {
            assert!(
                line.contains("could not read"),
                "{what} must say the read failed, got: {line}"
            );
            assert_ne!(
                line,
                &"".to_string(),
                "{what} must not be blank — a blank line says nothing at all"
            );
            assert!(
                !line.contains("not installed") && !line.contains("no known splitter"),
                "{what} asserts an absence about a machine ksx never read: {line}"
            );
        }

        // The occupancy is UNKNOWN, not free.
        assert_eq!(view.xinput_in_use, None);
        // A verb must not be offered when the page cannot describe it.
        assert!(
            view.spawn
                .refused
                .is_some_and(|r| r.contains("the pipe did not answer")),
            "a spawn offer with empty menus and no refusal is a live button with no meaning"
        );
        assert!(view.spawn.counts.is_empty());
        // …and the prune's kind is its own state, so no panel keyed on
        // "nothing" (the empty-bus kind) can render for a failed read.
        assert_eq!(view.prune.kind, "unreadable");
        assert_ne!(view.prune.kind, default.prune.kind);
    }

    /// A surface that implements nothing still SAYS something, per call, with
    /// the command that works — the CONTROL-SURFACE invariant, as a default.
    #[test]
    fn every_unimplemented_machine_verb_names_the_command_that_works() {
        struct Nothing;
        impl MachineSource for Nothing {}

        let checks: Vec<(&str, Refusal)> = vec![
            ("ksx devices", Nothing.devices().unwrap_err()),
            ("ksx device scan", Nothing.device_scan().unwrap_err()),
            (
                "ksx device pick",
                Nothing.device_pick(&DevicePickSpec::default()).unwrap_err(),
            ),
            (
                "ksx device remove",
                Nothing
                    .device_remove(&DeviceRemoveSpec::default())
                    .unwrap_err(),
            ),
            ("ksx preset list", Nothing.presets().unwrap_err()),
            (
                "ksx preset new",
                Nothing.preset_new(&NewPreset::default()).unwrap_err(),
            ),
            ("ksx config export", Nothing.profiles().unwrap_err()),
            // The one row whose remedy names a SURFACE rather than a verb,
            // because no CLI verb creates a profile yet. It still names
            // something a person can run, which is the invariant.
            (
                "ksx studio",
                Nothing.profile_new(&NewProfile::default()).unwrap_err(),
            ),
            ("ksx autostart", Nothing.autostart().unwrap_err()),
            ("ksx doctor", Nothing.doctor().unwrap_err()),
            ("ksx winusb status", Nothing.winusb().unwrap_err()),
            (
                "ksx pads",
                Nothing
                    .pads(&PadsSpawnSpec {
                        count: 4,
                        persona: "xbox360".into(),
                        hold_secs: 10,
                    })
                    .unwrap_err(),
            ),
            ("ksx pads --prune", Nothing.pads_view(false).unwrap_err()),
            ("ksx pads --prune", Nothing.pads_prune(false).unwrap_err()),
            ("ksx winusb claim", Nothing.winusb_claim("ID").unwrap_err()),
            (
                "ksx winusb release",
                Nothing.winusb_release("ID").unwrap_err(),
            ),
            ("ksx setup", Nothing.setup_state().unwrap_err()),
            (
                "ksx config export",
                Nothing
                    .config_export(&ExportRequest::default())
                    .unwrap_err(),
            ),
            (
                "ksx config import",
                Nothing
                    .config_import(&ImportRequest::default())
                    .unwrap_err(),
            ),
        ];
        for (command, refusal) in checks {
            assert_eq!(refusal.code, crate::refusal::codes::NOT_HERE);
            let remedy = refusal.remedy.as_deref().unwrap_or_default();
            assert!(
                remedy.contains(command),
                "the refusal for {command} must name it: {refusal}"
            );
        }
    }

    fn scan(usb_available: bool, boards: Vec<BoardRow>) -> DeviceScanView {
        DeviceScanView::read(
            "t".to_owned(),
            usb_available,
            boards,
            Vec::new(),
            Vec::new(),
        )
    }

    fn keyboard_board() -> BoardRow {
        BoardRow {
            name: "Ultimarc I-PAC 4X".to_owned(),
            keyboard: Some("USB\\VID_D209&PID_0430&MI_00\\X".to_owned()),
            ..BoardRow::default()
        }
    }

    /// **A refused read must not render as an assertion of absence.**
    ///
    /// FAILS against the shipped `/devices` seam, which computed
    /// `no_pickable_board_found` as `pickable == 0` and never consulted
    /// `usb_available` — so a machine whose USB enumeration DIED was told "no
    /// keyboard-capable board found" while the banner above it said nothing
    /// could be read. On the reference cabinet that sentence is wrong about
    /// four boards at once.
    #[test]
    fn a_failed_enumeration_is_never_an_empty_machine() {
        let blind = scan(false, Vec::new());
        assert!(
            !blind.no_pickable_board_found,
            "an unreadable machine licensed a surface to say the machine is empty"
        );
        assert_ne!(blind.boards_summary, NO_BOARDS_LINE);
        assert!(blind.boards_summary.contains("nothing could be READ"));

        // …and the ordinary empty machine still says its own, different
        // sentence. Without this half the bug could be "fixed" by never
        // answering the question at all.
        let empty = scan(true, Vec::new());
        assert!(empty.no_pickable_board_found);
        assert_eq!(empty.boards_summary, NO_BOARDS_LINE);

        // The two states must not be spelled the same way anywhere.
        assert_ne!(blind.boards_summary, empty.boards_summary);
    }

    /// The same distinction for the configured list, which is read from a file
    /// a refusal never gets as far as opening.
    #[test]
    fn an_unread_config_is_never_a_config_with_no_devices() {
        let unread = DeviceScanView::unreadable();
        assert!(!unread.no_configured_device);
        assert!(unread.configured_summary.contains("nothing could be READ"));

        let empty = scan(true, Vec::new());
        assert!(empty.no_configured_device);
        assert_ne!(unread.configured_summary, empty.configured_summary);
    }

    /// `Default` is the UNREADABLE view, not an empty machine. Every failure
    /// path in every surface reaches for it, so if it ever degrades into "there
    /// is nothing here" the bug comes back everywhere at once.
    #[test]
    fn the_default_view_claims_nothing_about_the_machine() {
        let default = DeviceScanView::default();
        assert_eq!(default, DeviceScanView::unreadable());
        assert!(!default.no_pickable_board_found);
        assert!(!default.no_configured_device);
    }

    /// The partition and the counts are the view's, not a surface's — and they
    /// agree with the rows, because the same call sets both.
    #[test]
    fn the_partition_and_the_counts_come_from_the_rows() {
        let view = scan(
            true,
            vec![
                keyboard_board(),
                BoardRow {
                    name: "NZXT fan controller".to_owned(),
                    keyboard: None,
                    ..BoardRow::default()
                },
            ],
        );
        assert_eq!(view.pickable_boards, 1);
        assert_eq!(view.other_boards, 1);
        assert!(view.boards[0].pickable);
        assert!(!view.boards[1].pickable);
        assert!(view.boards_summary.starts_with("1 keyboard-capable board"));
        assert!(view.other_summary.starts_with("1 board has no keyboard"));
        assert!(!view.no_pickable_board_found);
    }

    fn loose_winusb_entry(used_by: Vec<String>) -> ConfiguredDevice {
        ConfiguredDevice {
            alias: "panel".to_owned(),
            backend: "winusb".to_owned(),
            present: true,
            claimed: false,
            used_by,
            ..ConfiguredDevice::default()
        }
    }

    /// **A verdict about `ksx run` must match what `ksx run` does.**
    ///
    /// FAILS against the shipped page, which showed "ksx run will refuse" for
    /// every present-unclaimed-winusb entry regardless of whether any slot
    /// named it. `run/plan.rs` builds `captureable` from slots' keyboards and
    /// `capture.rs` only raises `NotRebound` for ids that reached
    /// `plan.winusb` through it, so an entry no slot references starts cleanly
    /// — and the user was being sent to debug a working session.
    #[test]
    fn an_unreferenced_winusb_entry_is_not_told_the_session_refuses() {
        let orphan = DeviceScanView::read(
            "t".to_owned(),
            true,
            Vec::new(),
            vec![loose_winusb_entry(Vec::new())],
            Vec::new(),
        );
        let line = &orphan.configured[0].health_line;
        assert!(
            !line.contains("refuses to start"),
            "no slot names this alias, so nothing refuses: {line}"
        );
        assert!(
            line.contains("NOT claimed"),
            "the fault is still named: {line}"
        );
        assert_eq!(orphan.configured[0].health_level, "idle");

        // The same entry WITH a slot naming it is the real fault, and it must
        // still be called one — otherwise this fix is just silence.
        let named = DeviceScanView::read(
            "t".to_owned(),
            true,
            Vec::new(),
            vec![loose_winusb_entry(vec!["slot 1 (keyboard)".to_owned()])],
            Vec::new(),
        );
        let line = &named.configured[0].health_line;
        assert!(line.contains("refuses to start"), "{line}");
        assert_eq!(named.configured[0].health_level, "warn");
    }

    /// An absent board gets no claim verdict at all: there is no binding to
    /// describe, and "on the Windows keyboard stack" about a board that is not
    /// plugged in is the same class of confident wrong answer.
    #[test]
    fn an_absent_board_gets_no_claim_verdict() {
        let view = DeviceScanView::read(
            "t".to_owned(),
            true,
            Vec::new(),
            vec![ConfiguredDevice {
                alias: "panel".to_owned(),
                backend: "winusb".to_owned(),
                present: false,
                used_by: vec!["slot 1 (keyboard)".to_owned()],
                ..ConfiguredDevice::default()
            }],
            Vec::new(),
        );
        assert_eq!(view.configured[0].health_line, "");
        assert_eq!(view.configured[0].health_level, "none");
    }

    /// An elevated command never travels without the word ELEVATED.
    ///
    /// The pair used to be assembled separately in `render_devices.rs` and in
    /// `DevicesIsland.ts`, which is two chances to ship the command with the
    /// wrong lead or no lead at all — and a `ksx winusb claim` line pasted
    /// without it produces one "access denied" and no explanation.
    #[test]
    fn a_shown_elevated_command_always_carries_its_lead() {
        let claimable = BoardRow {
            claim_command: Some("ksx winusb claim ID".to_owned()),
            ..keyboard_board()
        };
        let claimed = BoardRow {
            release_command: Some("ksx winusb release ID --yes".to_owned()),
            ..keyboard_board()
        };
        let neither = keyboard_board();
        let view = scan(true, vec![claimable, claimed, neither]);

        for board in &view.boards {
            assert_eq!(
                board.command.is_empty(),
                board.command_lead.is_empty(),
                "a command and its lead must appear and vanish together: {board:?}"
            );
            if !board.command.is_empty() {
                assert!(
                    board.command_lead.contains("ELEVATED"),
                    "{}",
                    board.command_lead
                );
            }
        }
        assert!(view.boards[0].command_lead.starts_with("To move it"));
        assert!(view.boards[1].command_lead.starts_with("It is claimed"));
        assert!(view.boards[2].command.is_empty());
    }

    /// A board that is merely HID gets the caveat; one that declares itself a
    /// keyboard does not. Without it "ksx could claim it" reads as a
    /// recommendation to claim a fan controller.
    #[test]
    fn only_a_board_that_does_not_declare_itself_a_keyboard_gets_the_caveat() {
        let view = scan(
            true,
            vec![
                BoardRow {
                    looks_like_a_keyboard: true,
                    ..keyboard_board()
                },
                BoardRow {
                    looks_like_a_keyboard: false,
                    ..keyboard_board()
                },
            ],
        );
        assert_eq!(view.boards[0].caveat, "");
        assert_eq!(view.boards[1].caveat, CAVEAT_NOT_A_KEYBOARD);
    }

    /// The derived fields are not settable past `read`: a fixture that builds
    /// the struct literally and then lies about the summary cannot exist,
    /// because `read` overwrites every one of them.
    #[test]
    fn read_overwrites_any_derived_field_it_is_handed() {
        let lying = BoardRow {
            keyboard: None,
            pickable: true,
            ..BoardRow::default()
        };
        let view = scan(true, vec![lying]);
        assert!(!view.boards[0].pickable);
        assert_eq!(view.pickable_boards, 0);
    }

    /// The import consent shape, as a TYPE rather than as a review note: an
    /// [`ImportRequest`] a surface builds — or deserializes from a form that
    /// simply omitted the box — writes nothing at all. `apply` has to be said.
    #[test]
    fn an_import_request_is_a_dry_run_until_someone_says_so() {
        assert!(!ImportRequest::default().apply);
        assert!(!ImportRequest::default().force);
        let off_the_wire: ImportRequest =
            serde_json::from_str(r#"{"document":"{}"}"#).expect("minimal request");
        assert!(
            !off_the_wire.apply && !off_the_wire.force,
            "a document with no consent fields must not write: {off_the_wire:?}"
        );
    }

    /// The slot ceiling a surface renders is THIS build's, never zero and never
    /// a number the view code picked.
    ///
    /// Two ways it can go wrong and both are covered: a derived `Default`
    /// (`max_slots: 0`, a slot menu with nothing in it) and a plain
    /// `#[serde(default)]` on the field, which turns a document written by an
    /// older ksx into the same zero. Either regression fails here; both were
    /// live in the version that hardcoded the menu at eight.
    ///
    /// (The old test in this slot built a `SetupView` with one `now` step and
    /// then asserted it had one `now` step — it tested the `vec!` literal three
    /// lines above it. The invariant it claimed to hold is really held in
    /// ksx-app, against `plan_steps`; see [`SetupView`].)
    #[test]
    fn the_slot_ceiling_a_surface_renders_is_this_builds_max_slots() {
        assert_eq!(SetupView::default().max_slots, ksx_core::MAX_SLOTS);
        // Compile-time, because a runtime assert on a constant is vacuous
        // (clippy agrees): if this build's ceiling were zero, every menu
        // derived from it is meaningless before any test runs.
        const _: () = assert!(ksx_core::MAX_SLOTS > 0);

        // A document from a ksx that predates the field.
        let older: SetupView = serde_json::from_str(
            r#"{"generated_at":"","config_root":"","config_exists":false,
                 "devices":[],"slots":[],"presets":[],"profiles":[],"steps":[],"notes":[]}"#,
        )
        .expect("an older document must still deserialize");
        assert_eq!(
            older.max_slots,
            ksx_core::MAX_SLOTS,
            "a missing ceiling means 'an older ksx wrote this', not 'no slots'"
        );

        // …and it survives the round trip a surface actually makes.
        let wire = serde_json::to_string(&SetupView::default()).unwrap();
        let back: SetupView = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.max_slots, ksx_core::MAX_SLOTS);
    }
}
