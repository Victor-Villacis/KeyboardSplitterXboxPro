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

    /// `ksx pads` — plug N test pads, run the pattern, unplug.
    ///
    /// Defaulted-refused deliberately: test pads COMPETE for the four XInput
    /// slots, so this is only ever legal while emulation is stopped, and the
    /// CLI is where that judgement (and the Ctrl+C that ends it) lives today.
    fn pads(&self, _count: u8, _persona: &str) -> Result<String, Refusal> {
        Err(Refusal::not_here(
            "the pad test",
            "run `ksx pads --count 4` with emulation stopped",
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
}

/// `ksx device scan`, presentation-shaped: one row per PHYSICAL board, plus
/// the `[[device]]` entries this config already holds.
///
/// The two lists are separate on purpose and neither is derivable from the
/// other. A board can be plugged in and unconfigured (the thing to pick), and
/// an entry can be configured with its board unplugged (the thing that needs
/// saying out loud, because it looks identical to a broken config from the
/// slot's end).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
            ("ksx autostart", Nothing.autostart().unwrap_err()),
            ("ksx doctor", Nothing.doctor().unwrap_err()),
            ("ksx winusb status", Nothing.winusb().unwrap_err()),
            ("ksx pads", Nothing.pads(4, "xbox360").unwrap_err()),
            ("ksx winusb claim", Nothing.winusb_claim("ID").unwrap_err()),
            (
                "ksx winusb release",
                Nothing.winusb_release("ID").unwrap_err(),
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
}
