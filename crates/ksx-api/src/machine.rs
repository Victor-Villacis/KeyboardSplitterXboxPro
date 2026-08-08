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

    /// **Can a virtual controller be created right now?** — the one slice of
    /// [`Self::doctor`] a first run has to know before it offers to Play.
    ///
    /// Its own method, and not a field on [`DoctorView`], for two reasons that
    /// both matter at the poll rate `/start` runs at: the full report walks the
    /// Interception class filters, probes HIDMaestro, reads the CI policy and
    /// takes a process snapshot, none of which this question needs; and
    /// [`DoctorView`] is prose plus advice rows, while a page deciding what to
    /// say before a button needs something it can branch on.
    ///
    /// Read-only, exactly like the rest of this trait's reads: it queries the
    /// registry and the service manager and installs nothing. `SURFACES.md` §3
    /// keeps installing off the browser surface entirely, and this method is
    /// what lets a page obey that rule while still telling the truth.
    fn pad_bus(&self) -> Result<PadBusView, Refusal> {
        Err(Refusal::not_here(
            "the controller-driver state",
            "run `ksx doctor`",
        ))
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
    /// **Every input devnode, on every transport ksx models** — HID-class USB
    /// interfaces and Bluetooth device nodes in one list.
    ///
    /// The field keeps its wire name because it has always been the device
    /// list; what changed is that it is no longer only USB. [`UsbRow::transport`]
    /// says which, and nothing downstream may infer a transport from anything
    /// else. Before the Bluetooth pass existed this list came from
    /// `nusb::list_devices()` alone, so a Bluetooth keyboard simply did not
    /// appear — the user was told, in effect, that their keyboard did not exist.
    pub usb: Vec<UsbRow>,
    /// Was USB enumeration possible?
    pub usb_available: bool,
    /// Was Bluetooth enumeration possible?
    ///
    /// A separate flag from [`Self::usb_available`] because they are separate
    /// reads and either can fail alone. Collapsing them would let a failed
    /// Bluetooth walk hide behind a successful USB one and print a list that
    /// silently omits half the machine — "I could not read this" and "there is
    /// nothing here" are different sentences.
    #[serde(default)]
    pub bluetooth_available: bool,
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

/// One devnode in the unified device list — a USB interface or a Bluetooth
/// device node.
///
/// Named `UsbRow` from when this list was USB-only. [`Self::transport`] is the
/// field that says what a row actually is, and every consumer reads it rather
/// than assuming: a Bluetooth row is not a defective USB row, it is a device on
/// a different transport with a different, permanent answer about backends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbRow {
    /// The instance id, uppercased — the string a `[[device]] id` holds.
    pub instance_id: String,
    pub description: String,
    /// `usb` | `bluetooth` — [`ksx_core::Transport::code`].
    ///
    /// The one field that must never be inferred from another. A row's
    /// enumerator prefix, its vendor id and its description are all things a
    /// surface could squint at; the collector already knows, so it says.
    #[serde(default)]
    pub transport: String,
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
    /// **Which backends can reach this row**, as `ksx_core::Reach::eligibility`
    /// decided — never as a surface guessed.
    ///
    /// The rule is not derivable from anything else on this row, which is
    /// exactly why it is carried: a Bluetooth keyboard is Interception-eligible
    /// (it is a keyboard on the Windows input stack, so splitting it into
    /// virtual pads works today) and can NEVER be WinUSB-claimed (WinUSB binds
    /// a USB interface through an INF hardware id and there is no USB interface
    /// here). Someone reading `state == "claimable"` alone would get both
    /// halves wrong.
    #[serde(default)]
    pub interception_eligible: bool,
    #[serde(default)]
    pub winusb_eligible: bool,
    /// The one line a row shows about backends — composed in `ksx-core`, so the
    /// CLI report, Studio's Rust seam and Studio's TypeScript island cannot
    /// word it three ways. Never empty for a collected row.
    #[serde(default)]
    pub backends: String,
    /// **Can this device deliver a keystroke right now?**
    ///
    /// Not the same question as "is it present". A paired-but-disconnected
    /// Bluetooth keyboard is present all day and cannot type a character; a
    /// claimed USB interface is present and deliberately off the keyboard
    /// stack. Both are listed, and both say so.
    #[serde(default)]
    pub can_type: bool,
    /// Why not, when not. Empty when it can.
    #[serde(default)]
    pub cannot_type_reason: String,
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
    /// Was USB enumeration possible? `false` means [`Self::boards`] is missing
    /// its USB half because nothing could be READ, not because nothing is
    /// plugged in.
    pub usb_available: bool,
    /// The same question for the Bluetooth pass, tracked separately because it
    /// is a separate read and either can fail alone.
    #[serde(default)]
    pub bluetooth_available: bool,
    /// Every device, in enumeration order (stable on one machine), probable
    /// keyboards first — **both transports in one list**, which is the whole
    /// point of this shape. [`BoardRow::transport`] says which each is.
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
        bluetooth_available: bool,
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
            // The board's transport and backend eligibility are the KEYBOARD
            // interface's, because that is the devnode a pick would name. A
            // board with no keyboard falls back to its first interface, so a
            // "no backend" row still says which transport it is on — half the
            // reason a Bluetooth speaker shows up in a keyboard list at all is
            // to answer "why is my device not here".
            let source = board
                .keyboard
                .as_deref()
                .and_then(|kb| {
                    board
                        .interfaces
                        .iter()
                        .find(|i| i.instance_id.eq_ignore_ascii_case(kb))
                })
                .or_else(|| board.interfaces.first());
            if let Some(row) = source {
                board.transport = row.transport.clone();
                board.interception_eligible = row.interception_eligible;
                board.winusb_eligible = row.winusb_eligible;
                board.backends = row.backends.clone();
                board.can_type = row.can_type;
                board.cannot_type_reason = row.cannot_type_reason.clone();
                // The selector of the SAME interface, so "which board did I
                // pick" and "which board will be captured" cannot answer
                // differently. A board with no keyboard interface keeps `None`
                // even though its first devnode has one: it is not pickable,
                // and handing a surface a selector for it would be offering a
                // choice the backend then refuses.
                board.selector = board.keyboard.as_ref().and(row.selector.clone());
            }
            board.alias_hint = board
                .alias
                .clone()
                .unwrap_or_else(|| board.name.clone())
                .trim()
                .to_owned();
            board.transport_label = transport_label(&board.transport);
            // A CLAIMED board also cannot type to Windows — that is what a
            // claim does — and it must not wear this alarm: the row already
            // says `claimed`, the backends line already says why, and warning
            // about the one state that is working exactly as asked is how a
            // page teaches people to ignore its warnings.
            board.cannot_type_line =
                if board.claimed || board.can_type || board.cannot_type_reason.is_empty() {
                    String::new()
                } else {
                    format!("{} {CANNOT_TYPE_TAIL}", board.cannot_type_reason)
                };
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
            boards_summary: boards_summary_line(
                pickable_boards,
                usb_available,
                bluetooth_available,
            ),
            other_summary: other_summary_line(other_boards),
            // The two conclusions, drawn once. BOTH enumerations gate the
            // first: a machine whose Bluetooth walk failed has not been fully
            // read, so "no keyboard-capable board found" would be an assertion
            // about half a machine. The second is unconditional here because
            // reaching `read` at all means the config WAS read.
            no_pickable_board_found: usb_available && bluetooth_available && pickable_boards == 0,
            no_configured_device: configured.is_empty(),
            pickable_boards,
            other_boards,
            generated_at,
            usb_available,
            bluetooth_available,
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
            bluetooth_available: false,
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

/// The same sentence for the Bluetooth half, and the reason the two halves are
/// tracked apart.
///
/// A machine whose USB walk worked and whose Bluetooth walk failed has a list
/// that is *right about USB and silent about Bluetooth*, which reads exactly
/// like a machine with no Bluetooth devices. Saying which half is missing is
/// the difference between "your keyboard is not paired" and "ksx could not
/// look".
pub const UNREAD_BLUETOOTH_LINE: &str =
    "Bluetooth enumeration failed — any Bluetooth device is MISSING from this list, and its \
     absence here is not evidence that it is unpaired";

/// The USB half failed and the Bluetooth half answered.
pub const UNREAD_USB_LINE: &str =
    "USB enumeration failed — any USB board is MISSING from this list, and its absence here is \
     not evidence that it is unplugged";

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

/// The line above the pickable list, and the one place an empty list is NOT
/// the same sentence as an empty machine.
///
/// Two transports means two ways to be half-blind, and a partial read must not
/// read as a complete one. When one pass failed the count is still printed —
/// it is true of the half that answered — and the missing half is named beside
/// it, because "no Bluetooth device here" and "ksx could not look at Bluetooth"
/// send a user to two different places.
fn boards_summary_line(count: usize, usb_available: bool, bluetooth_available: bool) -> String {
    match (usb_available, bluetooth_available) {
        (false, false) => return UNREAD_BOARDS_LINE.to_owned(),
        (true, false) => return format!("{} — {UNREAD_BLUETOOTH_LINE}", found_line(count)),
        (false, true) => return format!("{} — {UNREAD_USB_LINE}", found_line(count)),
        (true, true) => {}
    }
    match count {
        0 => NO_BOARDS_LINE.to_owned(),
        1 => "1 keyboard-capable board:".to_owned(),
        n => format!("{n} keyboard-capable boards:"),
    }
}

/// What a partial read may still assert: the count of what it DID see.
fn found_line(count: usize) -> String {
    match count {
        1 => "1 keyboard-capable device found so far".to_owned(),
        n => format!("{n} keyboard-capable devices found so far"),
    }
}

/// `usb` → `USB`, `bluetooth` → `Bluetooth`.
///
/// An unknown or empty code renders as `?` rather than defaulting to USB: a
/// collector that forgot to set a transport is a bug, and it must LOOK like one
/// instead of quietly mislabelling a Bluetooth device as USB — which is the
/// label that decides whether the WinUSB advice on the same row is true.
fn transport_label(code: &str) -> String {
    match code {
        c if c == crate::Transport::Usb.code() => crate::Transport::Usb.label().to_owned(),
        c if c == crate::Transport::Bluetooth.code() => {
            crate::Transport::Bluetooth.label().to_owned()
        }
        _ => "?".to_owned(),
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
    // `backend = "winusb"` on a device that is not on the USB transport is not
    // a claim someone forgot to perform. It is a claim that can never be
    // performed, so the advice "run `ksx winusb claim`" — correct for every
    // other unclaimed entry — would send this user to a command that refuses
    // forever. The entry has to be changed, not the machine.
    if device.transport == ksx_core::Transport::Bluetooth.code() {
        return (
            format!(
                "backend is winusb and this device is on Bluetooth — {}. Change the entry to \
                 backend = \"interception\", which CAN capture it.",
                ksx_core::transport::WINUSB_NEEDS_A_USB_INTERFACE
            ),
            "warn",
        );
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
    /// `usb` | `bluetooth` — this device's transport, taken from the interface
    /// a pick would name (its keyboard, else its first devnode).
    ///
    /// Filled by [`DeviceScanView::read`]. The transport column exists because
    /// the answer to "which backends can I use" is decided by it and by nothing
    /// else on the row.
    #[serde(default)]
    pub transport: String,
    /// Which backends can reach this device — see [`UsbRow::interception_eligible`].
    /// Filled by [`DeviceScanView::read`].
    #[serde(default)]
    pub interception_eligible: bool,
    #[serde(default)]
    pub winusb_eligible: bool,
    /// The composed backend line. Filled by [`DeviceScanView::read`].
    #[serde(default)]
    pub backends: String,
    /// Can it deliver a keystroke right now? Filled by [`DeviceScanView::read`].
    #[serde(default)]
    pub can_type: bool,
    /// Why not, when not. Filled by [`DeviceScanView::read`].
    #[serde(default)]
    pub cannot_type_reason: String,
    /// The transport as a HUMAN reads it (`USB`, `Bluetooth`), not as a
    /// consumer keys on it.
    ///
    /// Served rather than mapped per surface. The mapping is three lines, which
    /// is exactly the size of thing that gets written in Rust and again in
    /// TypeScript and then disagrees — and a device labelled two ways in two
    /// places is the specific confusion this column exists to remove.
    #[serde(default)]
    pub transport_label: String,
    /// The whole "it cannot type" caveat, or empty when there is nothing to
    /// say. Filled by [`DeviceScanView::read`] — see [`CANNOT_TYPE_TAIL`].
    #[serde(default)]
    pub cannot_type_line: String,
    /// The [`ksx_core::DeviceSelector`] spelling of the interface a pick would
    /// name — `usb:d209:0430:00`. `None` on a board with no keyboard interface,
    /// and on one whose devnode the enumerator could not describe.
    ///
    /// **This is what a surface sends back when a user picks a board**, and it
    /// exists so that no surface has to derive it from
    /// [`Self::interfaces`]. `docs/FIRST-RUN.md` §6 forbids ever asking a user
    /// for a device path; §5 forbids ever showing one as the identifier. A
    /// surface that had to pick the keyboard interface out of the list itself
    /// would be re-taking the decision [`Self::keyboard`] already records — and
    /// it would take it in TypeScript as well as in Rust.
    #[serde(default)]
    pub selector: Option<String>,
    /// The alias a pick would write for this board: the one it already has,
    /// else its name.
    ///
    /// One derivation, served. `device_edit::plan_pick` decides it the same way
    /// for an absent `--alias`, and re-picking a configured board deliberately
    /// keeps the name the user gave it, because renaming orphans every
    /// `[[slot]]` that refers to it.
    #[serde(default)]
    pub alias_hint: String,
}

/// What has to follow the reason a device cannot type.
///
/// The reason alone (`not connected (paired but absent?)`) is a fact about a
/// devnode. This is what it MEANS to the person reading a device list before
/// they claim their panel, and it is the whole point of listing an unusable
/// keyboard rather than hiding it.
pub const CANNOT_TYPE_TAIL: &str =
    "— it is present and CANNOT type right now, so it does not count as the spare keyboard a \
     claim needs";

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
    /// `usb` | `bluetooth` for the device this entry resolved to; empty while
    /// it resolves to nothing.
    ///
    /// Carried because the entry's `backend` can be *permanently* wrong rather
    /// than merely not-yet-right: `backend = "winusb"` on a Bluetooth device is
    /// not a claim someone forgot to perform, it is a claim that can never be
    /// performed. [`device_health`] tells those two apart, and it needs this to
    /// do it.
    #[serde(default)]
    pub transport: String,
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
    /// **This template binds nothing** (`empty` — every control listed, no
    /// keys on any of them).
    ///
    /// Served rather than left to a surface to notice, because it is the one
    /// option that produces something that cannot play: `StagedSetup::commit`
    /// refuses a controller with no bindings, so a menu that offered this
    /// silently would be offering the choice that dead-ends, with nothing on
    /// screen to say so.
    #[serde(default)]
    pub blank: bool,
}

impl TemplateRow {
    /// **The roster, from the one registry that holds it** —
    /// [`ksx_core::templates::TEMPLATES`], the same slice
    /// `ksx preset list --templates` prints.
    ///
    /// One composition, two consumers: the preset list (seed a new FILE) and
    /// the staged setup (dress a controller that is not a file yet). They ask
    /// the same question — "what layouts does this build know" — and a second
    /// mapping from `Template` to a row is a second chance to describe a panel
    /// differently on two screens.
    pub fn roster() -> Vec<Self> {
        ksx_core::templates::TEMPLATES
            .iter()
            .map(|t| Self {
                id: t.id.to_owned(),
                label: t.summary.to_owned(),
                detail: t.panel.to_owned(),
                players: (1..=t.players).collect(),
                // Asked of the template rather than matched on its id: "does
                // this bind anything" is a property of the rows, and a second
                // blank template would otherwise be offered as a working one.
                blank: t
                    .instantiate("probe", 1)
                    .map(|preset| preset.binds_nothing())
                    .unwrap_or(true),
            })
            .collect()
    }
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
    /// every game, and a surface must be able to say that BEFORE the button is
    /// pressed without knowing why four is the number. (The console says the
    /// same thing before its own plug, from the same constant — `ksx-app`'s
    /// `pads::ceiling_warning`.)
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
// The pad bus, as the one question a first run has to ask about it
// ---------------------------------------------------------------------------

/// Stable [`PadBusView::code`] values — `ksx doctor`'s own ViGEmBus advice
/// codes, spelled once so a match arm cannot drift from the producer.
///
/// They are `ksx_platform::advice`'s, not a second vocabulary: the whole point
/// of this view is that `/start` and `ksx doctor` reach the same verdict from
/// the same function. `HEALTHY` is the one addition, because "doctor said
/// nothing" needs a name on the wire.
pub mod pad_bus_codes {
    /// Doctor has nothing to say: registered, file present, service running.
    pub const HEALTHY: &str = "";
    /// No `ViGEmBus` service key at all.
    pub const MISSING: &str = "vigembus-missing";
    /// Service key registered, `ViGEmBus.sys` gone — a broken install.
    pub const FILE_MISSING: &str = "vigembus-file-missing";
    /// Installed, service not running. Usually a machine that has not rebooted.
    pub const NOT_RUNNING: &str = "vigembus-not-running";
    /// Installed, and Windows would not say what the service is doing.
    pub const STATE_UNKNOWN: &str = "vigembus-state-unknown";
    /// The read itself failed. **Not** a state of the driver (`SURFACES.md`
    /// §1b) — a state of our knowledge.
    pub const UNREADABLE: &str = "vigembus-unreadable";
}

/// **Can ksx create a virtual controller on this machine right now?**
///
/// One fact, because it is the one fact a first run cannot discover for
/// itself: every persona ksx can plug goes out through ViGEmBus, so a machine
/// without that driver stages perfectly, saves perfectly, and then plugs
/// nothing. `docs/FIRST-RUN.md` §6 forbids exactly that shape — "a screen
/// reports success while nothing works" — which is why this is stated *before*
/// the button rather than diagnosed after it.
///
/// **Three states, not two.** `blocked` and `unknown` are separate fields and
/// never both set: "the driver is not installed" and "I could not find out" are
/// different sentences, and a user acts on them differently (`SURFACES.md`
/// §1b). The default value of this type is the `unknown` one, so a payload
/// nobody filled in cannot read as a healthy bus.
///
/// **The verdict is not re-derived here.** [`Self::code`] is a `ksx doctor`
/// advice code, produced by `ksx_platform::advice::vigembus_advice` — the same
/// function `ksx doctor` prints from. What this type adds is the *wording* for
/// somebody who has never opened a terminal, which is the audience `FIRST-RUN`
/// is written about and not the audience `ksx doctor` is written for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadBusView {
    /// The driver state was read at all. `false` means every judgement below
    /// is "unknown", never "absent".
    pub readable: bool,
    /// One of [`pad_bus_codes`].
    pub code: String,
    /// **Known** to be unable to plug a pad.
    pub blocked: bool,
    /// Could not be determined — the read failed, the service state was
    /// unreadable, or this build does not recognise the code it got back.
    pub unknown: bool,
    /// The driver file's version, when one could be read.
    pub version: Option<String>,
    /// What is true, in a first-run user's words.
    pub line: String,
    /// What to do about it, **no-terminal route first**. Empty when there is
    /// nothing to do. `FIRST-RUN.md` §6: "the only way out of a mistake is a
    /// shell command" is on the list of things that must never happen, so the
    /// shell command is named second and never alone.
    pub remedy: String,
}

/// The honest zero value: nothing has been read, so nothing is known.
///
/// Hand-written rather than derived because a derived `Default` would be
/// `readable: false` with an empty `line` and `unknown: false` — a view that
/// claims nothing is wrong while having looked at nothing, which is the exact
/// failure this type exists to prevent.
impl Default for PadBusView {
    fn default() -> Self {
        Self::unreadable("the controller-driver state has not been read")
    }
}

impl PadBusView {
    /// The read failed or never happened.
    pub fn unreadable(reason: impl std::fmt::Display) -> Self {
        Self {
            readable: false,
            code: pad_bus_codes::UNREADABLE.to_owned(),
            blocked: false,
            unknown: true,
            version: None,
            line: format!(
                "Whether the ViGEmBus controller driver is installed could not be determined \
                 ({reason}). That is not the same as it being missing — nothing here knows \
                 either way."
            ),
            remedy: NO_BUS_READ_REMEDY.to_owned(),
        }
    }

    /// Build the view from `ksx doctor`'s verdict.
    ///
    /// `code` is the first advice code `vigembus_advice` returned, or
    /// [`pad_bus_codes::HEALTHY`] when it returned none. An unrecognised code
    /// lands in [`Self::unknown`] with the code quoted: a producer that grew a
    /// state this build has never heard of has said something, and rendering
    /// that as "all fine" would be inventing an answer.
    pub fn from_doctor(code: &str, version: Option<String>) -> Self {
        let base = Self {
            readable: true,
            code: code.to_owned(),
            blocked: false,
            unknown: false,
            version: version.clone(),
            line: String::new(),
            remedy: String::new(),
        };
        match code {
            pad_bus_codes::HEALTHY => Self {
                line: format!(
                    "The ViGEmBus controller driver{} is installed and running, so ksx can \
                     create virtual controllers on this machine.",
                    match &version {
                        Some(v) => format!(" (v{v})"),
                        None => String::new(),
                    }
                ),
                ..base
            },
            pad_bus_codes::MISSING => Self {
                blocked: true,
                line: "The ViGEmBus controller driver is NOT installed. It is the driver that \
                       makes a virtual controller exist, so until it is there ksx can stage and \
                       save a setup but cannot plug a single pad — and no game will see one."
                    .to_owned(),
                remedy: INSTALL_BUS_REMEDY.to_owned(),
                ..base
            },
            pad_bus_codes::FILE_MISSING => Self {
                blocked: true,
                line: "The ViGEmBus controller driver is registered with Windows but its driver \
                       file is gone — a broken install. ksx cannot plug a pad through it."
                    .to_owned(),
                remedy: INSTALL_BUS_REMEDY.to_owned(),
                ..base
            },
            pad_bus_codes::NOT_RUNNING => Self {
                blocked: true,
                line: "The ViGEmBus controller driver is installed but its service is not \
                       running, so nothing can be plugged through it yet. A machine that has \
                       just installed the driver and not restarted looks exactly like this."
                    .to_owned(),
                remedy: "Restart Windows. If it is still stopped afterwards, run the ksx \
                         installer again and leave \"Install the ViGEmBus controller driver\" \
                         ticked."
                    .to_owned(),
                ..base
            },
            pad_bus_codes::STATE_UNKNOWN => Self {
                unknown: true,
                line: "The ViGEmBus controller driver is installed, but Windows would not say \
                       whether its service is running — so whether a pad can be plugged is not \
                       known from here."
                    .to_owned(),
                remedy: NO_BUS_READ_REMEDY.to_owned(),
                ..base
            },
            other => Self {
                unknown: true,
                line: format!(
                    "The controller driver reported a state this build does not recognise \
                     (\"{other}\"), so whether a pad can be plugged is not known from here."
                ),
                remedy: NO_BUS_READ_REMEDY.to_owned(),
                ..base
            },
        }
    }

    /// Nothing is wrong and nothing needs saying before the button.
    pub fn silent(&self) -> bool {
        !self.blocked && !self.unknown
    }
}

/// The remedy for every state where the bus is known to be unusable.
///
/// The installer comes first and the command second, on purpose: the installer
/// is elevated already, it is the one moment the user has consented to an
/// administrator token, and it is a route that needs no terminal. The command
/// is named anyway because someone who has one should not have to guess it.
pub const INSTALL_BUS_REMEDY: &str =
    "Run the ksx installer again and leave \"Install the ViGEmBus controller driver\" ticked — \
     the driver is bundled with ksx and nothing is downloaded. From a terminal opened as \
     administrator, `ksx install-drivers --yes` does the same thing.";

/// The remedy when the answer itself is unknown. It cannot promise a fix,
/// because it does not know there is anything to fix.
pub const NO_BUS_READ_REMEDY: &str =
    "Nothing needs doing in advance. If Play starts and no controller appears in your game, the \
     Start menu's \"ksx (advanced) > driver check\" prints what this machine really has.";

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

    /// Both enumerations answered — the ordinary machine.
    fn scan(available: bool, boards: Vec<BoardRow>) -> DeviceScanView {
        DeviceScanView::read(
            "t".to_owned(),
            available,
            available,
            boards,
            Vec::new(),
            Vec::new(),
        )
    }

    fn configured(devices: Vec<ConfiguredDevice>) -> DeviceScanView {
        DeviceScanView::read("t".to_owned(), true, true, Vec::new(), devices, Vec::new())
    }

    fn keyboard_board() -> BoardRow {
        BoardRow {
            name: "Ultimarc I-PAC 4X".to_owned(),
            keyboard: Some("USB\\VID_D209&PID_0430&MI_00\\X".to_owned()),
            ..BoardRow::default()
        }
    }

    /// **A surface picks a board by SELECTOR, and it never derives one.**
    ///
    /// `docs/FIRST-RUN.md` §5 makes the human name the identifier on screen and
    /// §6 forbids ever asking anyone for a device path, so what a picked row
    /// sends back has to be the id ksx would write — and it has to arrive
    /// already chosen, because the choice is *which of a board's devnodes
    /// carries the keys* and [`BoardRow::keyboard`] is where that was decided.
    ///
    /// Breaks against a surface that reached into `interfaces` for itself: the
    /// obvious implementation takes `interfaces[0].selector`, which on the
    /// reference I-PAC is the AUX devnode — a selector that resolves, captures
    /// nothing, and looks correct on screen. It also breaks against filling the
    /// field on an unpickable board, which offers a choice `add_slot` then
    /// refuses.
    #[test]
    fn a_board_carries_the_selector_and_the_alias_a_pick_would_use() {
        let kb = "USB\\VID_D209&PID_0430&MI_00\\X";
        let aux = "USB\\VID_D209&PID_0430&MI_01\\X";
        let iface = |id: &str, selector: &str| UsbRow {
            instance_id: id.to_owned(),
            selector: Some(selector.to_owned()),
            ..UsbRow::default()
        };
        // The AUX devnode is FIRST, which is the ordering that catches a
        // surface reaching for `interfaces[0]`.
        let board = BoardRow {
            interfaces: vec![
                iface(aux, "usb:d209:0430:01"),
                iface(kb, "usb:d209:0430:00"),
            ],
            ..keyboard_board()
        };
        let no_keyboard = BoardRow {
            name: "NZXT fan controller".to_owned(),
            keyboard: None,
            interfaces: vec![iface("USB\\VID_1E71&PID_300E&MI_01\\X", "usb:1e71:300e:01")],
            ..BoardRow::default()
        };

        let view = scan(true, vec![board, no_keyboard]);
        assert_eq!(view.boards[0].selector.as_deref(), Some("usb:d209:0430:00"));
        assert_eq!(
            view.boards[1].selector, None,
            "a board that cannot be picked must not be handed a selector to pick it with"
        );

        // The alias is the one `plan_pick` would write: the board's name when
        // nothing is configured, and the name the USER gave it when something
        // is — renaming orphans every [[slot]] that refers to the old alias.
        assert_eq!(view.boards[0].alias_hint, "Ultimarc I-PAC 4X");
        let configured = scan(
            true,
            vec![BoardRow {
                alias: Some("panel".to_owned()),
                ..keyboard_board()
            }],
        );
        assert_eq!(configured.boards[0].alias_hint, "panel");
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

    /// **Half a read is not a whole one.** USB answered and Bluetooth did not:
    /// the count of what WAS found is still true and still printed, but the
    /// page may not conclude "nothing here" and must name the missing half.
    ///
    /// FAILS against the single `usb_available` flag this view shipped with:
    /// with Bluetooth enumeration dead and USB fine, `no_pickable_board_found`
    /// was `true` on a machine whose Bluetooth keyboard was sitting right there
    /// — the same class of confident wrong answer the USB flag was added to
    /// stop, one transport later.
    #[test]
    fn one_failed_transport_never_licenses_the_empty_machine_sentence() {
        let usb_only = DeviceScanView::read(
            "t".to_owned(),
            true,
            false,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert!(
            !usb_only.no_pickable_board_found,
            "the Bluetooth half was never read, so the machine was not read"
        );
        assert_ne!(usb_only.boards_summary, NO_BOARDS_LINE);
        assert!(
            usb_only
                .boards_summary
                .contains("Bluetooth enumeration failed"),
            "the missing half must be named: {}",
            usb_only.boards_summary
        );
        assert!(
            usb_only.boards_summary.contains("not evidence"),
            "absence from a list nobody could read is not evidence: {}",
            usb_only.boards_summary
        );

        let bt_only = DeviceScanView::read(
            "t".to_owned(),
            false,
            true,
            vec![keyboard_board()],
            Vec::new(),
            Vec::new(),
        );
        assert!(!bt_only.no_pickable_board_found);
        assert!(
            bt_only.boards_summary.contains("USB enumeration failed"),
            "{}",
            bt_only.boards_summary
        );
        assert!(
            bt_only.boards_summary.contains("1 keyboard-capable device"),
            "what WAS found is still true and still printed: {}",
            bt_only.boards_summary
        );
        // The three failure shapes must not be spelled the same way.
        assert_ne!(usb_only.boards_summary, bt_only.boards_summary);
        assert_ne!(usb_only.boards_summary, UNREAD_BOARDS_LINE);
    }

    /// **The permanent case, told apart from the not-yet case.**
    ///
    /// `backend = "winusb"` on an unclaimed USB board is a claim someone has
    /// not performed yet, and the advice is to perform it. The same line on a
    /// Bluetooth device is a claim that can NEVER be performed, and repeating
    /// "run `ksx winusb claim`" would send that user to a command that refuses
    /// forever.
    ///
    /// FAILS against the shipped `device_health`, which had one sentence for
    /// both and no idea what transport the entry resolved to.
    #[test]
    fn a_winusb_entry_on_bluetooth_is_told_no_claim_can_ever_fix_it() {
        let bt = configured(vec![ConfiguredDevice {
            transport: "bluetooth".to_owned(),
            ..loose_winusb_entry(vec!["slot 1 (keyboard)".to_owned()])
        }]);
        let line = &bt.configured[0].health_line;
        assert!(line.contains("Bluetooth"), "{line}");
        assert!(
            line.contains("no USB interface to bind"),
            "the transport fact, not a vague 'unsupported': {line}"
        );
        assert!(
            line.contains("interception"),
            "and the backend that CAN capture it: {line}"
        );
        assert!(
            !line.contains("ksx winusb claim"),
            "never advise a command that refuses forever: {line}"
        );
        assert_eq!(bt.configured[0].health_level, "warn");

        // The USB entry beside it keeps the recoverable wording.
        let usb = configured(vec![ConfiguredDevice {
            transport: "usb".to_owned(),
            ..loose_winusb_entry(vec!["slot 1 (keyboard)".to_owned()])
        }]);
        assert!(usb.configured[0].health_line.contains("refuses to start"));
        assert_ne!(usb.configured[0].health_line, *line);
    }

    /// A board's transport and backend eligibility come from the interface a
    /// pick would NAME — its keyboard — not from whichever devnode enumerated
    /// first. On an I-PAC that is the difference between describing MI_00 and
    /// describing the vendor collection on MI_02.
    #[test]
    fn a_boards_transport_is_the_one_its_keyboard_interface_is_on() {
        let vendor_iface = UsbRow {
            instance_id: "USB\\VID_D209&PID_0430&MI_02\\X".to_owned(),
            transport: "usb".to_owned(),
            backends: "wrong row".to_owned(),
            ..UsbRow::default()
        };
        let keyboard_iface = UsbRow {
            instance_id: "USB\\VID_D209&PID_0430&MI_00\\X".to_owned(),
            transport: "usb".to_owned(),
            interception_eligible: true,
            winusb_eligible: true,
            backends: "the keyboard's line".to_owned(),
            can_type: true,
            ..UsbRow::default()
        };
        let view = scan(
            true,
            vec![BoardRow {
                // Deliberately NOT first in the list.
                interfaces: vec![vendor_iface, keyboard_iface],
                ..keyboard_board()
            }],
        );
        assert_eq!(view.boards[0].backends, "the keyboard's line");
        assert!(view.boards[0].winusb_eligible);
        assert!(view.boards[0].can_type);
    }

    /// A device with no keyboard interface still gets a transport, because
    /// half the reason it is listed at all is to answer "why is my device not
    /// in the picker" — and for a Bluetooth device the answer is a permanent
    /// one a user needs to read.
    #[test]
    fn a_device_with_no_keyboard_still_reports_its_transport() {
        let view = scan(
            true,
            vec![BoardRow {
                name: "ULT TOWER 9".to_owned(),
                keyboard: None,
                interfaces: vec![UsbRow {
                    transport: "bluetooth".to_owned(),
                    backends: "Bluetooth · no backend".to_owned(),
                    ..UsbRow::default()
                }],
                ..BoardRow::default()
            }],
        );
        assert!(!view.boards[0].pickable);
        assert_eq!(view.boards[0].transport, "bluetooth");
        assert!(!view.boards[0].backends.is_empty());
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
        let orphan = configured(vec![loose_winusb_entry(Vec::new())]);
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
        let named = configured(vec![loose_winusb_entry(vec![
            "slot 1 (keyboard)".to_owned()
        ])]);
        let line = &named.configured[0].health_line;
        assert!(line.contains("refuses to start"), "{line}");
        assert_eq!(named.configured[0].health_level, "warn");
    }

    /// An absent board gets no claim verdict at all: there is no binding to
    /// describe, and "on the Windows keyboard stack" about a board that is not
    /// plugged in is the same class of confident wrong answer.
    #[test]
    fn an_absent_board_gets_no_claim_verdict() {
        let view = configured(vec![ConfiguredDevice {
            alias: "panel".to_owned(),
            backend: "winusb".to_owned(),
            present: false,
            used_by: vec!["slot 1 (keyboard)".to_owned()],
            ..ConfiguredDevice::default()
        }]);
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

    /// **A default `PadBusView` must never read as a healthy bus.**
    ///
    /// Fails against a `#[derive(Default)]` on that struct — which is the
    /// obvious thing to write and was the first thing written here. The
    /// derived value is `blocked: false, unknown: false, line: ""`, so
    /// `silent()` is true and a page renders nothing: a payload nobody filled
    /// in would look exactly like a machine with a working driver, on the one
    /// screen whose whole job is to say otherwise before the button.
    #[test]
    fn an_uncollected_pad_bus_is_unknown_and_not_silent() {
        let view = PadBusView::default();
        assert!(!view.readable);
        assert!(view.unknown, "an unread bus is unknown, never fine");
        assert!(!view.blocked, "unknown is not the same as blocked");
        assert!(!view.silent(), "a page must have something to say here");
        assert_eq!(view.code, pad_bus_codes::UNREADABLE);
        assert!(!view.line.trim().is_empty());
    }

    /// The three states stay three, and only one of them licenses silence.
    #[test]
    fn every_doctor_code_lands_in_exactly_one_state() {
        for (code, blocked, unknown) in [
            (pad_bus_codes::HEALTHY, false, false),
            (pad_bus_codes::MISSING, true, false),
            (pad_bus_codes::FILE_MISSING, true, false),
            (pad_bus_codes::NOT_RUNNING, true, false),
            (pad_bus_codes::STATE_UNKNOWN, false, true),
            // A code from a future ksx-platform. Unknown, never silent.
            ("vigembus-something-new", false, true),
        ] {
            let view = PadBusView::from_doctor(code, None);
            assert_eq!(view.blocked, blocked, "blocked for {code:?}");
            assert_eq!(view.unknown, unknown, "unknown for {code:?}");
            assert!(
                !(view.blocked && view.unknown),
                "{code:?} claims both at once"
            );
            assert!(view.readable, "{code:?} came from a read that answered");
            assert!(!view.line.trim().is_empty(), "{code:?} says nothing");
            assert_eq!(
                view.silent(),
                code == pad_bus_codes::HEALTHY,
                "only a healthy bus is silent ({code:?})"
            );
            // Anything that is not silent owes the user a next step, and
            // FIRST-RUN.md §6 forbids that step being a shell command ALONE.
            if !view.silent() {
                assert!(!view.remedy.trim().is_empty(), "{code:?} has no remedy");
                assert!(
                    view.remedy.contains("installer") || view.remedy.contains("Start menu"),
                    "{code:?}'s remedy needs a route with no terminal in it: {}",
                    view.remedy
                );
            }
        }
    }

    /// The version is evidence, so it is printed only when it was read.
    #[test]
    fn a_healthy_bus_names_its_version_only_when_there_is_one() {
        let known = PadBusView::from_doctor(pad_bus_codes::HEALTHY, Some("1.22.0.0".into()));
        assert!(known.line.contains("v1.22.0.0"), "{}", known.line);
        let unknown = PadBusView::from_doctor(pad_bus_codes::HEALTHY, None);
        assert!(!unknown.line.contains("(v"), "{}", unknown.line);
    }
}
