//! `ksx device pick` and `ksx device remove` — the write half of the device
//! picker.
//!
//! [`crate::device_scan`] is the read half: it groups interfaces into physical
//! boards and prints what ksx *would* write. This is what writes it, and it is
//! a separate verb precisely so that looking is never a commitment.
//!
//! # One writer, like every other write
//!
//! Same shape as [`crate::slots`]: a typed spec in, a pure plan out, a
//! timestamped backup taken before the write, and the store's atomic save doing
//! the I/O. [`plan_pick`] and [`plan_remove`] take a `Survey` and a slice of
//! [`DeviceFacts`] rather than reaching for hardware, so every refusal below is
//! exercised in CI against a synthetic device tree, on any platform.
//!
//! # Picking is not claiming (`docs/DEVICE-IDENTITY.md` §7)
//!
//! A WinUSB claim takes a keyboard *off the Windows input stack*. On a machine
//! whose only keyboard is that board, an automatic claim is a lockout — and a
//! lockout the user cannot type their way out of. So `pick` writes config,
//! prints `ksx winusb claim <ID>` as an explicit next step, and stops.
//!
//! The corollary that is easy to get backwards: `backend = "winusb"` describes
//! what an interface **is**, never what it could become. Setting it because an
//! interface merely *could* be claimed turns a working Interception keyboard
//! into a config that refuses to start, in one command that looked like a menu
//! choice.
//!
//! # Removing an entry does not release a board
//!
//! `remove` deletes four lines of TOML. The driver binding is a property of the
//! machine, not of ksx's config, so a claimed board stays claimed — Windows
//! still sees no keyboard on it, and now there is no config entry left to
//! explain why. That warning is the whole reason `remove` looks at the device
//! tree at all.

use std::fmt;
use std::path::PathBuf;

use ksx_config::{Backend, ConfigError, ConfigFile, DeviceEntry, GamesFile, Store};
use ksx_core::{DeviceFacts, DeviceRef, DeviceSelector, Match};
use ksx_platform::winusb::{Candidate, ClaimState, Refusal, Survey};

// ---------------------------------------------------------------------------
// pick
// ---------------------------------------------------------------------------

/// The cost of a `port=` rung, in one paragraph, said once.
///
/// It lives here rather than inline in [`PickOutcome::message`] because it is
/// not a property of the MOMENT the entry was written — it is a property of the
/// entry. `ksx device pick` prints it at write time; a surface listing
/// configured devices has to keep printing it, or the entry looks like every
/// other one right up until the board moves socket.
///
/// The second half is the half people miss. A `port=` value names THIS PC's USB
/// topology, so the entry does not travel — which matters the moment configs
/// get shared or a cabinet gets rebuilt, and `pick` is the only place that
/// knows enough to say it (by the time `run` sees a missing board it cannot
/// tell a move from an unplug).
pub const PORT_PINNED_WARNING: &str =
    "PORT-PINNED — nothing weaker than the socket separates this board from its twin. Move \
     it to another USB port and this entry stops matching. It is also specific to THIS \
     machine: the port names this PC's USB topology, so do not copy this config to another \
     cabinet — run `ksx device pick` there instead.";

/// One `ksx device pick`, as any surface spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickSpec {
    /// An instance path, an existing `[[device]]` alias, or any unique part of
    /// an instance path — see [`resolve`].
    pub query: String,
    /// The name `[[slot]]` entries will use. `None` derives one from the board.
    pub alias: Option<String>,
}

/// Everything `pick` decided, before a byte is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickPlan {
    /// The chosen interface in canonical (uppercase) form — the argument
    /// `ksx winusb claim` and `ksx winusb release` take.
    pub instance_id: String,
    /// What to call the board on screen.
    pub name: String,
    pub alias: String,
    /// The `id` value that will be written: the weakest rung that is still
    /// unique, verified against the live enumeration.
    pub selector: DeviceSelector,
    pub backend: Backend,
    /// The alias of the entry this replaces, when the board is already
    /// configured. `None` means a new entry.
    pub replaces: Option<String>,
    /// Is the interface bound to `winusb.sys` right now? Decides
    /// [`Self::backend`], and decides whether the claim command is the next
    /// step or noise.
    pub claimed: bool,
}

impl PickPlan {
    pub fn entry(&self) -> DeviceEntry {
        DeviceEntry {
            id: DeviceRef::from_selector(self.selector.clone()),
            alias: self.alias.clone(),
            backend: self.backend,
        }
    }
}

/// Why a pick was refused. Nothing is written for any of these.
#[derive(Debug, thiserror::Error)]
pub enum PickError {
    /// Straight from `ksx winusb claim`'s own resolver — one resolver, one
    /// vocabulary, one set of refusal codes.
    #[error("{0}")]
    Device(#[from] Refusal),
    #[error("the alias \"{alias}\" already names {id}")]
    AliasTaken { alias: String, id: String },
    #[error("\"{alias}\" cannot be an alias: {problem}")]
    BadAlias {
        alias: String,
        problem: &'static str,
    },
    #[error("{instance_id} is not in the USB enumeration, so ksx cannot describe it")]
    NotEnumerable { instance_id: String },
    #[error(
        "no selector ksx can write names {instance_id} and nothing else — `{selector}` \
         matches {} connected interface(s)",
        hits.len()
    )]
    NotUnique {
        instance_id: String,
        selector: String,
        hits: Vec<String>,
    },
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl PickError {
    /// The stable refusal word, shared with `--json` and with `ksx winusb`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Device(refusal) => refusal.code(),
            Self::AliasTaken { .. } => "alias-taken",
            Self::BadAlias { .. } => "bad-alias",
            Self::NotEnumerable { .. } => "not-enumerable",
            Self::NotUnique { .. } => "ambiguous-selector",
            Self::Config(_) => "config-error",
        }
    }

    /// What to do about it. A refusal with no way forward is just an error
    /// message.
    pub fn advice(&self) -> String {
        match self {
            Self::Device(refusal) => refusal.advice(),
            Self::AliasTaken { alias, .. } => format!(
                "aliases are how [[slot]] entries name a board, so two boards cannot share one. \
                 Choose another with --alias, or forget the old board first:\n  ksx device remove \
                 \"{alias}\""
            ),
            Self::BadAlias { .. } => {
                "an alias is the short name a [[slot]] refers to — \"panel\", \"P1\". A value \
                 containing a backslash is read as a literal device path everywhere else in the \
                 config, so it can never resolve back to this entry."
                    .to_owned()
            }
            Self::NotEnumerable { .. } => {
                "run `ksx devices`. The USB enumeration ksx builds selectors from did not return \
                 this interface, so there is nothing to write an id from — writing the raw \
                 instance path instead would pin the entry to one USB socket \
                 (docs/DEVICE-IDENTITY.md §1)."
                    .to_owned()
            }
            Self::NotUnique { hits, .. } => format!(
                "these interfaces all answer to it:\n  {}\n\nNo rung of the selector separates \
                 them — they report the same vendor, product, interface and instance — so any id \
                 ksx wrote would name whichever board Windows enumerated first. Unplug one and \
                 pick the other, or leave them on the Interception backend \
                 (docs/DEVICE-IDENTITY.md §8).",
                hits.join("\n  ")
            ),
            Self::Config(_) => String::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "refused": true,
            "code": self.code(),
            "message": self.to_string(),
            "advice": self.advice(),
        })
    }
}

/// The board a QUERY names.
///
/// The instance-path/substring half is `ksx winusb claim`'s resolver, called
/// directly. Two verbs that take the same argument and disagree about which
/// board it means would be worse than one verb.
///
/// The only thing layered on top is an existing `[[device]]` alias, and that is
/// a translation rather than a second resolver: the alias is replaced by the
/// concrete interface its own selector resolves to, and *that* string goes
/// through `Survey::resolve` like any other. It is what makes `pick` the verb
/// that upgrades a legacy entry in place — `ksx device scan` prints the
/// stronger selector but never rewrites the file itself
/// (`docs/DEVICE-IDENTITY.md` §5).
pub fn resolve<'a>(
    survey: &'a Survey,
    connected: &[DeviceFacts],
    config: &ConfigFile,
    query: &str,
) -> Result<&'a Candidate, Refusal> {
    let query = query.trim();
    let Some(entry) = config
        .devices
        .iter()
        .find(|d| d.alias.eq_ignore_ascii_case(query))
    else {
        return survey.resolve(query);
    };
    let unknown = || Refusal::UnknownDevice {
        requested: query.to_owned(),
        known: candidate_ids(survey),
    };
    let selector = entry.id.selector().clone();
    match selector.match_against(connected) {
        Match::One(facts) => survey.resolve(facts.id.as_str()),
        Match::None => Err(unknown()),
        Match::Ambiguous(hits) => Err(Refusal::Ambiguous {
            requested: query.to_owned(),
            matches: hits.iter().map(|f| f.id.as_str().to_owned()).collect(),
        }),
    }
}

/// Decide what `pick` would write, or refuse.
///
/// Every check that can refuse runs **before** the caller is allowed anywhere
/// near the store, so a refusal never leaves a stray backup behind — the same
/// ordering [`crate::slots::assign`] uses.
pub fn plan_pick(
    survey: &Survey,
    connected: &[DeviceFacts],
    config: &ConfigFile,
    spec: &PickSpec,
) -> Result<PickPlan, PickError> {
    let candidate = resolve(survey, connected, config, &spec.query)?;

    // An interface with no keyboard behind it can never deliver a keystroke, so
    // an entry naming it is a slot that silently never fires. `ksx device scan`
    // already refuses to offer these; the writer has to agree.
    match candidate.state {
        ClaimState::NotAKeyboard | ClaimState::ForeignDriver => {
            return Err(Refusal::NotAKeyboard {
                instance_id: candidate.interface.instance_id.to_uppercase(),
            }
            .into())
        }
        ClaimState::Claimed | ClaimState::Claimable => {}
    }

    // Uppercase deliberately, like `ksx winusb status`: this is the string a
    // user pastes into the claim command, and the enumerator canonicalizes
    // every id it produces the same way.
    let instance_id = candidate.interface.instance_id.to_uppercase();
    let facts = connected
        .iter()
        .find(|f| f.id.as_str().eq_ignore_ascii_case(&instance_id))
        .ok_or_else(|| PickError::NotEnumerable {
            instance_id: instance_id.clone(),
        })?;

    // The weakest rung that survives a replug — and then the check
    // `docs/DEVICE-IDENTITY.md` §8 spells out, because `strongest_for` can hand
    // back a PORT selector that is still ambiguous: twins whose instance tails
    // are identical get the same `port=` value, and the port rung is the last
    // one there is. An id that names two boards would drive whichever one
    // Windows enumerated first, which is the exact failure this whole design
    // exists to prevent.
    let selector = DeviceSelector::strongest_for(facts, connected);
    match selector.match_against(connected) {
        Match::One(hit) if hit.id == facts.id => {}
        other => {
            return Err(PickError::NotUnique {
                instance_id,
                selector: selector.to_string(),
                hits: hits_of(&other),
            })
        }
    }

    // Rule (b), §7: `winusb` is a statement of fact about the binding, never an
    // intention. Setting it for a claimable interface would flip a working
    // Interception keyboard into a config `ksx run` refuses to start on.
    let claimed = candidate.state == ClaimState::Claimed;
    let backend = if claimed {
        Backend::Winusb
    } else {
        Backend::Interception
    };

    let replaces = config
        .devices
        .iter()
        .find(|d| names_the_same_board(d, facts, connected))
        .map(|d| d.alias.clone());
    let name = board_name(candidate, facts);
    let alias = match (&spec.alias, &replaces) {
        (Some(wanted), _) => wanted.trim().to_owned(),
        // Re-picking a board keeps the name the user gave it: renaming it would
        // break every [[slot]] that refers to it by name.
        (None, Some(existing)) => existing.clone(),
        (None, None) => name.clone(),
    };
    if alias.is_empty() {
        return Err(PickError::BadAlias {
            alias,
            problem: "it is empty",
        });
    }
    if alias.contains('\\') {
        return Err(PickError::BadAlias {
            alias,
            problem: "it contains a backslash, which makes it a device path",
        });
    }
    if let Some(clash) = config.devices.iter().find(|d| {
        d.alias.eq_ignore_ascii_case(&alias) && replaces.as_deref() != Some(d.alias.as_str())
    }) {
        return Err(PickError::AliasTaken {
            alias,
            id: clash.id.raw().to_owned(),
        });
    }

    Ok(PickPlan {
        instance_id,
        name,
        alias,
        selector,
        backend,
        replaces,
        claimed,
    })
}

/// What landed on disk.
#[derive(Clone, Debug)]
pub struct PickOutcome {
    pub path: PathBuf,
    /// The timestamped copy taken before the write; `None` when there was no
    /// config file yet.
    pub backup: Option<PathBuf>,
    pub plan: PickPlan,
}

/// Write the entry `plan` describes.
pub fn apply_pick(store: &Store, plan: &PickPlan) -> Result<PickOutcome, PickError> {
    let mut config = store.load_config()?.value;
    let entry = plan.entry();
    match plan
        .replaces
        .as_deref()
        .and_then(|alias| config.devices.iter_mut().find(|d| d.alias == alias))
    {
        Some(existing) => *existing = entry,
        None => config.devices.push(entry),
    }
    let path = store.root().config_path();
    let backup = store.backup(&path)?;
    let written = store.save_config(&config)?;
    Ok(PickOutcome {
        path: written,
        backup,
        plan: plan.clone(),
    })
}

impl PickOutcome {
    /// The report a surface prints. It always names the claim as a separate
    /// act — a user who reads this and believes their board was claimed will
    /// discover otherwise mid-game.
    pub fn message(&self) -> String {
        use std::fmt::Write as _;
        let plan = &self.plan;
        let mut out = String::new();

        let verb = match &plan.replaces {
            Some(_) => "updated",
            None => "wrote",
        };
        let _ = writeln!(out, "{verb} [[device]] \"{}\"", plan.alias);
        let _ = writeln!(out, "  id      = {}", plan.selector);
        let _ = writeln!(
            out,
            "  backend = {}",
            match plan.backend {
                Backend::Winusb => "winusb",
                Backend::Interception => "interception",
            }
        );
        let _ = writeln!(out, "  board   : {} ({})", plan.name, plan.instance_id);
        let _ = writeln!(out, "  means   : {}", plan.selector.explain());
        if !plan.selector.survives_replug() {
            // The trade has to be stated at the moment it is made: a user with
            // two identical encoders needs to know WHICH of their boards is now
            // pinned to a socket. A `port=` rung means the port is what
            // SEPARATES this board from its twin — which is why it appears
            // exactly when the serials collide (measured: two I-PAC 4X boards
            // both answer "4"). The wording is [`PORT_PINNED_WARNING`], shared
            // with every surface that lists a configured device.
            let _ = writeln!(out, "  [!] {PORT_PINNED_WARNING}");
        }
        if let Some(backup) = &self.backup {
            let _ = writeln!(out, "  backup  : {}", backup.display());
        }

        if plan.claimed {
            let _ = writeln!(
                out,
                "\nThis interface is already bound to winusb.sys, which is why the entry says \
                 backend = \"winusb\". Nothing was claimed and nothing was released."
            );
        } else {
            let _ = writeln!(
                out,
                "\nNothing was claimed. `ksx device pick` only writes config; it never takes a \
                 board off the Windows keyboard stack. The board still types to Windows and runs \
                 on the Interception backend exactly as configured.\n\nTo move it to the WinUSB \
                 backend instead — it will stop typing to Windows, and that is a separate, \
                 consented step:\n  ksx winusb claim {}",
                plan.instance_id
            );
        }
        let _ = writeln!(
            out,
            "\nWire a slot to it with `ksx setup`, which identifies the board by press."
        );
        out
    }

    pub fn to_json(&self) -> serde_json::Value {
        let plan = &self.plan;
        serde_json::json!({
            "written": self.path.display().to_string(),
            "backup": self.backup.as_ref().map(|p| p.display().to_string()),
            "device": {
                "id": plan.selector.to_string(),
                "alias": plan.alias,
                "backend": match plan.backend {
                    Backend::Winusb => "winusb",
                    Backend::Interception => "interception",
                },
            },
            "instance_id": plan.instance_id,
            "board": plan.name,
            "rung": plan.selector.rung(),
            "survives_replug": plan.selector.survives_replug(),
            "replaced": plan.replaces,
            "claimed": plan.claimed,
            // Never run by ksx. Printed so a script and a human get the same
            // next step, and so neither is surprised by one it did not ask for.
            "next_step": (!plan.claimed).then(|| format!("ksx winusb claim {}", plan.instance_id)),
        })
    }
}

// ---------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------

/// One `ksx device remove`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveSpec {
    pub alias: String,
    /// Remove it even though slots still name it.
    pub force: bool,
}

/// One place a slot still names the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRef {
    pub slot: u8,
    /// `None` = `config.toml`'s `[[slot]]` list; `Some` = a games.toml profile.
    pub profile: Option<String>,
    /// `keyboard` or `mouse`.
    pub field: &'static str,
}

impl fmt::Display for SlotRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "slot {} ({})", self.slot, self.field)?;
        match &self.profile {
            Some(profile) => write!(f, " in profile \"{profile}\""),
            None => Ok(()),
        }
    }
}

/// Everything `remove` decided, before a byte is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovePlan {
    /// The alias as the FILE spells it.
    pub alias: String,
    /// The `id` the entry held, for the record.
    pub id: String,
    pub backend: Backend,
    /// Slots that name it. Non-empty here means this is a `--force` removal and
    /// those slots are about to stop resolving.
    pub breaks: Vec<SlotRef>,
    /// The interface still bound to `winusb.sys`, if the board is claimed.
    /// Deleting config does not release it, and this is how the user finds out
    /// before the panel is dark rather than after.
    pub still_claimed: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("no [[device]] is called \"{alias}\"{}", list_or_none(known))]
    UnknownAlias { alias: String, known: Vec<String> },
    #[error(
        "\"{alias}\" is still used by {}",
        breaks.iter().map(SlotRef::to_string).collect::<Vec<_>>().join(", ")
    )]
    InUse { alias: String, breaks: Vec<SlotRef> },
    #[error("{0}")]
    Config(#[from] ConfigError),
}

impl RemoveError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownAlias { .. } => "unknown-device-alias",
            Self::InUse { .. } => "device-in-use",
            Self::Config(_) => "config-error",
        }
    }

    pub fn advice(&self) -> String {
        match self {
            Self::UnknownAlias { .. } => {
                "`ksx device scan` shows which boards are configured, and the alias is the name in \
                 the parentheses."
                    .to_owned()
            }
            Self::InUse { breaks, .. } => format!(
                "those slots would name a device that no longer exists, and `ksx run` refuses to \
                 start on an unknown device alias — the panel would be dead at the next boot, \
                 long after whoever typed this walked away. Repoint them first (`ksx setup`), or \
                 re-run with --force and fix them after:\n  {}",
                breaks
                    .iter()
                    .map(SlotRef::to_string)
                    .collect::<Vec<_>>()
                    .join("\n  ")
            ),
            Self::Config(_) => String::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "refused": true,
            "code": self.code(),
            "message": self.to_string(),
            "advice": self.advice(),
        })
    }
}

/// `" — aliases in config.toml: a, b"`, or an honest note when there are none.
fn list_or_none(known: &[String]) -> String {
    if known.is_empty() {
        " — there are no [[device]] entries at all".to_owned()
    } else {
        format!(" — aliases in config.toml: {}", known.join(", "))
    }
}

/// Decide what `remove` would delete, or refuse.
pub fn plan_remove(
    survey: &Survey,
    connected: &[DeviceFacts],
    config: &ConfigFile,
    games: &GamesFile,
    spec: &RemoveSpec,
) -> Result<RemovePlan, RemoveError> {
    let wanted = spec.alias.trim();
    let Some(entry) = config
        .devices
        .iter()
        .find(|d| d.alias.eq_ignore_ascii_case(wanted))
    else {
        return Err(RemoveError::UnknownAlias {
            alias: wanted.to_owned(),
            known: config.devices.iter().map(|d| d.alias.clone()).collect(),
        });
    };

    let breaks = references_to(config, games, &entry.alias);
    if !breaks.is_empty() && !spec.force {
        return Err(RemoveError::InUse {
            alias: entry.alias.clone(),
            breaks,
        });
    }

    Ok(RemovePlan {
        alias: entry.alias.clone(),
        id: entry.id.raw().to_owned(),
        backend: entry.backend,
        breaks,
        still_claimed: claimed_interface(survey, connected, entry),
    })
}

/// Every slot that names `alias`, in `config.toml` and in every games.toml
/// profile.
///
/// Both files are searched because both resolve through the SAME `[[device]]`
/// table — `ConfigFile::game_slot_spec` says so explicitly — so a profile slot
/// naming a deleted alias fails exactly as hard as a `[[slot]]` one, and
/// listing only half of them would send someone off to fix a cabinet that is
/// still broken.
///
/// Matching is byte-exact, like `ConfigFile::resolve_device`: a slot that says
/// `"Panel"` when the entry says `"panel"` is already an unresolved reference
/// and deleting this entry is not what broke it.
///
/// Public because it is the ONE answer to "what breaks if this goes away", and
/// a surface that offers a Remove button has to show it BEFORE the click —
/// `plan_remove` refuses on this list, and a page that re-derived its own
/// version would be a second implementation of the refusal
/// (`docs/SURFACES.md` §1). [`crate::device_scan::view`] renders it per entry.
pub fn references_to(config: &ConfigFile, games: &GamesFile, alias: &str) -> Vec<SlotRef> {
    let mut out = Vec::new();
    let mut push =
        |slot: u8, profile: Option<&str>, field: &'static str, value: &Option<String>| {
            if value.as_deref() == Some(alias) {
                out.push(SlotRef {
                    slot,
                    profile: profile.map(str::to_owned),
                    field,
                });
            }
        };
    for slot in &config.slots {
        push(slot.number, None, "keyboard", &slot.keyboard);
        push(slot.number, None, "mouse", &slot.mouse);
    }
    for game in &games.games {
        for slot in &game.slots {
            push(slot.number, Some(&game.title), "keyboard", &slot.keyboard);
            push(slot.number, Some(&game.title), "mouse", &slot.mouse);
        }
    }
    out
}

/// The interface this entry names, if it is bound to `winusb.sys` right now.
fn claimed_interface(
    survey: &Survey,
    connected: &[DeviceFacts],
    entry: &DeviceEntry,
) -> Option<String> {
    let selector = entry.id.selector().clone();
    let Match::One(facts) = selector.match_against(connected) else {
        return None;
    };
    survey
        .candidates
        .iter()
        .find(|c| {
            c.state == ClaimState::Claimed
                && c.interface
                    .instance_id
                    .eq_ignore_ascii_case(facts.id.as_str())
        })
        .map(|c| c.interface.instance_id.to_uppercase())
}

/// What landed on disk.
#[derive(Clone, Debug)]
pub struct RemoveOutcome {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    pub plan: RemovePlan,
}

pub fn apply_remove(store: &Store, plan: &RemovePlan) -> Result<RemoveOutcome, RemoveError> {
    let mut config = store.load_config()?.value;
    config.devices.retain(|d| d.alias != plan.alias);
    let path = store.root().config_path();
    let backup = store.backup(&path)?;
    let written = store.save_config(&config)?;
    Ok(RemoveOutcome {
        path: written,
        backup,
        plan: plan.clone(),
    })
}

impl RemoveOutcome {
    pub fn message(&self) -> String {
        use std::fmt::Write as _;
        let plan = &self.plan;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "removed [[device]] \"{}\" (id = {})",
            plan.alias, plan.id
        );
        if let Some(backup) = &self.backup {
            let _ = writeln!(out, "  backup  : {}", backup.display());
        }

        if let Some(instance_id) = &plan.still_claimed {
            // A warning, not a refusal: forgetting a board and releasing it are
            // two different intentions, and refusing here would stop someone
            // tidying a config while the cabinet is mid-session. But the board
            // stays off the Windows keyboard stack, and there is now no entry
            // in any file explaining why the panel does not type.
            let _ = writeln!(
                out,
                "\n[!] THE BOARD IS STILL CLAIMED. Deleting the config entry did not release \
                 it:\n    {instance_id} is still bound to winusb.sys, so Windows still sees no \
                 keyboard there — and there is no longer a [[device]] entry saying why. Put it \
                 back on the keyboard driver with:\n      ksx winusb release {instance_id} --yes"
            );
        }
        if !plan.breaks.is_empty() {
            let _ = writeln!(
                out,
                "\n[!] --force: these slots now name a device that does not exist, and `ksx run` \
                 will refuse to start until they are repointed (`ksx setup`):\n    {}",
                plan.breaks
                    .iter()
                    .map(SlotRef::to_string)
                    .collect::<Vec<_>>()
                    .join("\n    ")
            );
        }
        out
    }

    pub fn to_json(&self) -> serde_json::Value {
        let plan = &self.plan;
        serde_json::json!({
            "written": self.path.display().to_string(),
            "backup": self.backup.as_ref().map(|p| p.display().to_string()),
            "removed": {
                "alias": plan.alias,
                "id": plan.id,
                "backend": match plan.backend {
                    Backend::Winusb => "winusb",
                    Backend::Interception => "interception",
                },
            },
            "broken_slots": plan.breaks.iter().map(SlotRef::to_string).collect::<Vec<_>>(),
            "still_claimed": plan.still_claimed,
            "release_command": plan
                .still_claimed
                .as_ref()
                .map(|id| format!("ksx winusb release {id} --yes")),
        })
    }
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

fn candidate_ids(survey: &Survey) -> Vec<String> {
    survey
        .candidates
        .iter()
        .map(|c| c.interface.instance_id.to_uppercase())
        .collect()
}

fn hits_of(found: &Match<'_>) -> Vec<String> {
    match found {
        Match::One(hit) => vec![hit.id.as_str().to_owned()],
        Match::None => Vec::new(),
        Match::Ambiguous(hits) => hits.iter().map(|f| f.id.as_str().to_owned()).collect(),
    }
}

/// Does this entry already name the board `facts` describes?
///
/// Asked through the selector rather than by comparing id strings, so a legacy
/// entry holding a raw instance path is recognised as the same board and gets
/// UPGRADED in place instead of gaining a second, competing entry.
fn names_the_same_board(
    entry: &DeviceEntry,
    facts: &DeviceFacts,
    connected: &[DeviceFacts],
) -> bool {
    // Already parsed at load: `DeviceEntry.id` is a `DeviceRef`, so there is
    // no failure arm left to handle here.
    let selector = entry.id.selector();
    matches!(selector.match_against(connected), Match::One(hit) if hit.id == facts.id)
}

/// Best available name for a board, in the order a human would prefer it.
fn board_name(candidate: &Candidate, facts: &DeviceFacts) -> String {
    if let Some(vendor) = ksx_core::vendors::name_for(facts.vendor_id, facts.product_id) {
        return vendor.to_owned();
    }
    let description = candidate.interface.description();
    if !description.is_empty() {
        return description;
    }
    format!("usb-{:04x}-{:04x}", facts.vendor_id, facts.product_id)
}

// ---------------------------------------------------------------------------
// the commands
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub(crate) fn connected_facts() -> Vec<DeviceFacts> {
    match ksx_capture::usb_candidates() {
        Ok(found) => found.iter().map(ksx_capture::UsbCandidate::facts).collect(),
        Err(err) => {
            tracing::warn!("USB enumeration failed: {err}");
            Vec::new()
        }
    }
}

#[cfg(windows)]
pub(crate) fn store() -> Result<Store, ConfigError> {
    Ok(Store::new(ksx_config::ConfigRoot::discover()?))
}

#[cfg(windows)]
pub fn pick(spec: PickSpec, json: bool) -> anyhow::Result<()> {
    let survey = ksx_platform::winusb::survey();
    let connected = connected_facts();
    let store = store()?;
    let config = store.load_config()?.value;

    let plan = match plan_pick(&survey, &connected, &config, &spec) {
        Ok(plan) => plan,
        Err(err) => refuse(&err.to_string(), &err.advice(), &err.to_json(), json),
    };
    let outcome = match apply_pick(&store, &plan) {
        Ok(outcome) => outcome,
        Err(err) => refuse(&err.to_string(), &err.advice(), &err.to_json(), json),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome.to_json())?);
    } else {
        print!("{}", outcome.message());
    }
    Ok(())
}

#[cfg(windows)]
pub fn remove(spec: RemoveSpec, json: bool) -> anyhow::Result<()> {
    let survey = ksx_platform::winusb::survey();
    let connected = connected_facts();
    let store = store()?;
    let config = store.load_config()?.value;
    let games = store.load_games()?.value;

    let plan = match plan_remove(&survey, &connected, &config, &games, &spec) {
        Ok(plan) => plan,
        Err(err) => refuse(&err.to_string(), &err.advice(), &err.to_json(), json),
    };
    let outcome = match apply_remove(&store, &plan) {
        Ok(outcome) => outcome,
        Err(err) => refuse(&err.to_string(), &err.advice(), &err.to_json(), json),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome.to_json())?);
    } else {
        print!("{}", outcome.message());
    }
    Ok(())
}

/// Exit 2 with nothing written — the same "refused, nothing changed" answer
/// `ksx winusb` gives, so one script can key on one code.
#[cfg(windows)]
fn refuse(message: &str, advice: &str, json_value: &serde_json::Value, json: bool) -> ! {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(json_value).unwrap_or_default()
        );
    } else {
        eprintln!("REFUSED: {message}");
        if !advice.is_empty() {
            eprintln!("\n{advice}");
        }
    }
    std::process::exit(crate::winusb::EXIT_REFUSED);
}

#[cfg(not(windows))]
pub fn pick(_spec: PickSpec, _json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx device pick` enumerates Windows USB devices and is Windows-only")
}

#[cfg(not(windows))]
pub fn remove(_spec: RemoveSpec, _json: bool) -> anyhow::Result<()> {
    anyhow::bail!("`ksx device remove` reads the Windows device tree and is Windows-only")
}

#[cfg(test)]
mod tests {
    use ksx_config::{ConfigRoot, GameEntry, GameSlotEntry, SlotEntry};
    use ksx_core::DeviceId;
    use ksx_platform::winusb::{DeviceNode, KEYBOARD_CLASS_GUID, WINUSB_SERVICE};

    use super::*;

    const HID_CLASS: &str = "{745a17a0-74d3-11d0-b6fe-00a0c90f57da}";
    const PANEL: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
    const PANEL_B: &str = r"USB\VID_D209&PID_0430&MI_00\6&1B2C3D4E&0&0000";
    const AUX: &str = r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001";

    fn node(id: &str, class: &str, service: &str, prefix: Option<&str>) -> DeviceNode {
        DeviceNode::new(
            id,
            Some(class.to_owned()),
            Some(service.to_owned()),
            Some("@input.inf,%hid.devicedesc%;USB Input Device".to_owned()),
            prefix.map(str::to_owned),
        )
    }

    /// A claimable board: a USB interface on the HID stack plus the HID keyboard
    /// child it produces. Without the child the interface reads as
    /// `NotAKeyboard`, which is the difference between the two fixtures below.
    fn keyboard_nodes(interface: &str, prefix: &str) -> Vec<DeviceNode> {
        let key = interface
            .split('\\')
            .nth(1)
            .expect("an instance path has a device key");
        vec![
            node(interface, HID_CLASS, "HidUsb", Some(prefix)),
            node(
                &format!(r"HID\{key}\{prefix}&0000"),
                KEYBOARD_CLASS_GUID,
                "kbdhid",
                None,
            ),
        ]
    }

    /// The reference cabinet: one I-PAC on the keyboard stack, plus a desk
    /// keyboard so nothing here is ever the last keyboard on the machine.
    fn cabinet() -> Survey {
        let mut nodes = keyboard_nodes(PANEL, "8&2A0D0500&0");
        nodes.extend(keyboard_nodes(
            r"USB\VID_046D&PID_C31C&MI_00\5&1E4F2A&0&0000",
            "9&1F3E5D7C&0",
        ));
        // The I-PAC's second interface: HID, on the HID stack, and with no
        // keyboard child. It is the thing a picker must not offer.
        nodes.push(node(AUX, HID_CLASS, "HidUsb", Some("8&2A0D0501&0")));
        Survey::from_nodes(&nodes)
    }

    fn facts(id: &str, interface: u8, serial: Option<&str>) -> DeviceFacts {
        DeviceFacts {
            id: DeviceId::new(id),
            vendor_id: 0xD209,
            product_id: 0x0430,
            interface_number: interface,
            serial: serial.map(str::to_owned),
            instance: DeviceFacts::instance_of(id).to_owned(),
        }
    }

    fn one_ipac() -> Vec<DeviceFacts> {
        vec![facts(PANEL, 0, Some("4")), facts(AUX, 1, Some("4"))]
    }

    fn spec(query: &str) -> PickSpec {
        PickSpec {
            query: query.to_owned(),
            alias: None,
        }
    }

    fn config_with(devices: Vec<DeviceEntry>) -> ConfigFile {
        ConfigFile {
            devices,
            ..ConfigFile::default()
        }
    }

    fn entry(id: &str, alias: &str, backend: Backend) -> DeviceEntry {
        DeviceEntry {
            id: ksx_core::DeviceRef::parse(id).expect("test fixture id must parse"),
            alias: alias.to_owned(),
            backend,
        }
    }

    fn slot(number: u8, keyboard: Option<&str>) -> SlotEntry {
        SlotEntry {
            number,
            keyboard: keyboard.map(str::to_owned),
            mouse: None,
            preset: "P1".to_owned(),
            persona: Default::default(),
            socd: Default::default(),
            macros: Default::default(),
        }
    }

    /// A throwaway config root, same shape as `slots.rs`'s so the two writers'
    /// tests read the same way.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-device-edit-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn store(&self, config: &ConfigFile) -> Store {
            let store = Store::new(ConfigRoot::at(&self.0));
            store.save_config(config).unwrap();
            store
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // --- pick --------------------------------------------------------------

    /// The happy path, and the two things it must never do: pin a port it did
    /// not have to, and claim anything.
    #[test]
    fn picking_a_board_writes_the_weakest_selector_and_claims_nothing() {
        let root = TempRoot::new("pick");
        let config = ConfigFile::default();
        let store = root.store(&config);

        let plan = plan_pick(&cabinet(), &one_ipac(), &config, &spec("MI_00\\7&25EEA38C"))
            .expect("one I-PAC, on the keyboard stack");
        assert_eq!(plan.selector.to_string(), "usb:d209:0430:00");
        assert!(plan.selector.survives_replug());
        assert_eq!(plan.alias, "Ultimarc I-PAC 4X");

        let outcome = apply_pick(&store, &plan).unwrap();
        let written = store.load_config().unwrap().value;
        assert_eq!(written.devices.len(), 1);
        assert_eq!(written.devices[0].id.raw(), "usb:d209:0430:00");

        let message = outcome.message();
        assert!(
            message.contains("Nothing was claimed"),
            "a menu choice must never read as a claim: {message}"
        );
        assert!(
            message.contains(&format!("ksx winusb claim {PANEL}")),
            "and the claim must be offered as the explicit next step: {message}"
        );
    }

    /// Rule (b), `docs/DEVICE-IDENTITY.md` §7. `backend = "winusb"` is a
    /// statement about the binding that EXISTS. Writing it for an interface
    /// that is merely claimable turns a working Interception keyboard into a
    /// config `ksx run` refuses to start on — in one command that looked like
    /// picking a name off a list.
    #[test]
    fn only_an_already_bound_interface_gets_backend_winusb() {
        let config = ConfigFile::default();

        let claimable = plan_pick(&cabinet(), &one_ipac(), &config, &spec("MI_00\\7&25EEA38C"))
            .expect("the I-PAC is claimable");
        assert_eq!(claimable.backend, Backend::Interception);
        assert!(!claimable.claimed);

        // The same board after `ksx winusb claim`: bound to winusb.sys, and the
        // HID keyboard child is gone — which is exactly why it stopped typing.
        let mut nodes = vec![node(PANEL, HID_CLASS, WINUSB_SERVICE, Some("8&2A0D0500&0"))];
        nodes.extend(keyboard_nodes(
            r"USB\VID_046D&PID_C31C&MI_00\5&1E4F2A&0&0000",
            "9&1F3E5D7C&0",
        ));
        let claimed = plan_pick(
            &Survey::from_nodes(&nodes),
            &one_ipac(),
            &config,
            &spec("MI_00\\7&25EEA38C"),
        )
        .expect("a claimed board is still pickable");
        assert_eq!(claimed.backend, Backend::Winusb);
        assert!(claimed.claimed);
        assert!(
            !PickOutcome {
                path: PathBuf::new(),
                backup: None,
                plan: claimed,
            }
            .message()
            .contains("ksx winusb claim"),
            "telling someone to claim what is already claimed is noise"
        );
    }

    /// Rule (c), `docs/DEVICE-IDENTITY.md` §8: `strongest_for` can hand back a
    /// PORT selector that is STILL ambiguous, because two boards that are not
    /// composite can carry the same instance tail. Persisting that id would
    /// name whichever board Windows enumerated first — the two-identical-boards
    /// failure, reintroduced by the very function meant to prevent it.
    #[test]
    fn a_selector_that_still_names_two_boards_is_refused_rather_than_written() {
        let twin_a = facts(PANEL, 0, None);
        // Same vendor, product, interface AND instance tail: every rung the
        // selector has to offer collapses onto both boards.
        let mut twin_b = facts(PANEL_B, 0, None);
        twin_b.instance = twin_a.instance.clone();
        let connected = vec![twin_a, twin_b];

        let err = plan_pick(&cabinet(), &connected, &ConfigFile::default(), &spec(PANEL))
            .expect_err("no rung separates them");
        assert_eq!(err.code(), "ambiguous-selector");
        let advice = err.advice();
        assert!(
            advice.contains(PANEL_B),
            "both boards must be named: {advice}"
        );
        assert!(
            advice.contains("Unplug one"),
            "and there must be a way forward: {advice}"
        );
    }

    /// Twins that a port CAN separate get the port rung — and the message says
    /// out loud that the board is now pinned to a socket, because a user with
    /// two identical encoders has to know which of theirs that applies to.
    #[test]
    fn twins_the_port_can_separate_get_a_pinned_selector_that_states_the_cost() {
        // Measured on this hardware: both I-PAC 4X boards answer "4", so the
        // serial rung is no help and the port rung is the honest fallback.
        let connected = vec![facts(PANEL, 0, Some("4")), facts(PANEL_B, 0, Some("4"))];
        let plan = plan_pick(&cabinet(), &connected, &ConfigFile::default(), &spec(PANEL))
            .expect("the port separates them");
        assert_eq!(plan.selector.rung(), "port");
        assert!(!plan.selector.survives_replug());

        let message = PickOutcome {
            path: PathBuf::new(),
            backup: None,
            plan,
        }
        .message();
        assert!(message.contains("PORT-PINNED"), "{message}");
        assert!(message.contains("stops matching"), "{message}");
        // The half that is easy to miss: a `port=` value names THIS PC's USB
        // topology, so the entry does not survive being copied to another
        // cabinet. Said at write time because `run` cannot tell a move from
        // an unplug once the board is gone.
        assert!(message.contains("another cabinet"), "{message}");
    }

    /// The resolver is `ksx winusb claim`'s, so the refusals are its refusals —
    /// same codes, same list of what to pass instead.
    #[test]
    fn an_unknown_or_ambiguous_query_is_refused_with_the_candidates_listed() {
        let config = ConfigFile::default();
        let unknown = plan_pick(&cabinet(), &one_ipac(), &config, &spec("nothing-like-this"))
            .expect_err("no such interface");
        assert_eq!(unknown.code(), "unknown-device");
        assert!(unknown.advice().contains(PANEL), "{}", unknown.advice());

        // `MI_00` alone matches the I-PAC and the desk keyboard.
        let ambiguous =
            plan_pick(&cabinet(), &one_ipac(), &config, &spec("MI_00")).expect_err("two boards");
        assert_eq!(ambiguous.code(), "ambiguous-device");
    }

    /// An interface with no keyboard behind it can never deliver a keystroke.
    /// Writing an entry for the I-PAC's MI_01 would produce a slot that
    /// silently never fires.
    #[test]
    fn an_interface_with_no_keyboard_behind_it_is_refused() {
        let err = plan_pick(
            &cabinet(),
            &one_ipac(),
            &ConfigFile::default(),
            &spec("MI_01"),
        )
        .expect_err("MI_01 carries no keys");
        assert_eq!(err.code(), "not-a-keyboard");
    }

    /// Aliases are how `[[slot]]` entries name a board, so stealing one would
    /// silently repoint another player's panel. `ksx setup` auto-numbers
    /// instead; a verb someone typed a name INTO has to say the name is taken.
    #[test]
    fn an_alias_that_already_names_another_board_is_refused_rather_than_renamed() {
        let desk = entry(
            r"USB\VID_046D&PID_C31C&MI_00\5&1E4F2A&0&0000",
            "panel",
            Backend::Interception,
        );
        let config = config_with(vec![desk]);
        let err = plan_pick(
            &cabinet(),
            &one_ipac(),
            &config,
            &PickSpec {
                query: PANEL.to_owned(),
                alias: Some("panel".to_owned()),
            },
        )
        .expect_err("the name is taken");
        assert_eq!(err.code(), "alias-taken");
        assert!(
            err.advice().contains("ksx device remove"),
            "{}",
            err.advice()
        );
    }

    /// The upgrade path §5 describes: `scan` prints the replug-proof selector
    /// but never rewrites the file, and `pick` is what applies it. Re-picking a
    /// configured board must replace its entry — not add a second one that
    /// competes with it — and must keep the name the user gave it, because
    /// every `[[slot]]` refers to the board by that name.
    #[test]
    fn re_picking_a_configured_board_upgrades_its_entry_in_place() {
        let root = TempRoot::new("upgrade");
        // The legacy spelling: a full instance path, which dies the moment the
        // board changes socket.
        let config = config_with(vec![entry(PANEL, "P1 panel", Backend::Interception)]);
        let store = root.store(&config);

        let plan = plan_pick(&cabinet(), &one_ipac(), &config, &spec("P1 panel"))
            .expect("the alias resolves to the board it names");
        assert_eq!(plan.replaces.as_deref(), Some("P1 panel"));
        assert_eq!(plan.alias, "P1 panel");
        assert_eq!(plan.selector.to_string(), "usb:d209:0430:00");

        apply_pick(&store, &plan).unwrap();
        let written = store.load_config().unwrap().value;
        assert_eq!(written.devices.len(), 1, "upgraded, not duplicated");
        assert_eq!(written.devices[0].alias, "P1 panel");
        assert_eq!(written.devices[0].id.raw(), "usb:d209:0430:00");
    }

    #[test]
    fn the_write_takes_a_timestamped_backup_of_the_file_it_replaces() {
        let root = TempRoot::new("backup");
        let config = ConfigFile::default();
        let store = root.store(&config);
        let plan = plan_pick(&cabinet(), &one_ipac(), &config, &spec(PANEL)).unwrap();
        let outcome = apply_pick(&store, &plan).unwrap();
        let backup = outcome
            .backup
            .expect("config.toml existed before the write");
        assert!(backup.is_file());
        assert!(backup.to_string_lossy().contains(".bak-"), "{backup:?}");
    }

    // --- remove ------------------------------------------------------------

    fn games_with(profile: &str, slots: Vec<GameSlotEntry>) -> GamesFile {
        GamesFile {
            games: vec![GameEntry {
                title: profile.to_owned(),
                notes: String::new(),
                path: r"C:\mame.exe".to_owned(),
                arguments: String::new(),
                process_name: None,
                launcher_grace_ms: None,
                block_keyboards: ksx_core::Blocking::Whole,
                block_mice: false,
                slots,
            }],
        }
    }

    fn game_slot(number: u8, keyboard: &str) -> GameSlotEntry {
        GameSlotEntry {
            number,
            user_index: Some(number),
            keyboard: Some(keyboard.to_owned()),
            mouse: None,
            preset: "P1".to_owned(),
            persona: Default::default(),
            socd: Default::default(),
            macros: Default::default(),
        }
    }

    /// Deleting a device a slot still names leaves that slot pointing at
    /// nothing, and `ksx run` refuses to start on an unknown alias — at the
    /// next boot, long after whoever typed this walked away. So it is refused
    /// now, naming every slot that would break, in BOTH files.
    #[test]
    fn removing_a_device_slots_still_name_is_refused_and_lists_them() {
        let mut config = config_with(vec![entry(PANEL, "panel", Backend::Interception)]);
        config.slots = vec![slot(1, Some("panel")), slot(2, Some("other"))];
        let games = games_with("MAME", vec![game_slot(3, "panel")]);

        let err = plan_remove(
            &cabinet(),
            &one_ipac(),
            &config,
            &games,
            &RemoveSpec {
                alias: "panel".to_owned(),
                force: false,
            },
        )
        .expect_err("two slots still use it");
        assert_eq!(err.code(), "device-in-use");
        let message = err.to_string();
        assert!(message.contains("slot 1 (keyboard)"), "{message}");
        assert!(
            message.contains("slot 3 (keyboard) in profile \"MAME\""),
            "a games.toml slot resolves through the same [[device]] table, so it \
             breaks just as hard: {message}"
        );
        assert!(!message.contains("slot 2"), "{message}");
        assert!(err.advice().contains("--force"), "{}", err.advice());
    }

    #[test]
    fn force_removes_it_anyway_and_says_which_slots_now_point_at_nothing() {
        let root = TempRoot::new("force");
        let mut config = config_with(vec![entry(PANEL, "panel", Backend::Interception)]);
        config.slots = vec![slot(1, Some("panel"))];
        let store = root.store(&config);

        let plan = plan_remove(
            &cabinet(),
            &one_ipac(),
            &config,
            &GamesFile::default(),
            &RemoveSpec {
                alias: "panel".to_owned(),
                force: true,
            },
        )
        .expect("--force overrides the refusal");
        let outcome = apply_remove(&store, &plan).unwrap();
        assert!(store.load_config().unwrap().value.devices.is_empty());

        let message = outcome.message();
        assert!(message.contains("slot 1 (keyboard)"), "{message}");
        assert!(message.contains("refuse to start"), "{message}");
    }

    /// The whole reason `remove` looks at the device tree. A claimed board
    /// stays claimed: the binding lives in the driver store, not in
    /// `config.toml`. Refusing would be wrong — forgetting a board and
    /// releasing it are different intentions — but saying nothing is how
    /// someone ends up with a dead panel and no config explaining why.
    #[test]
    fn removing_a_claimed_device_warns_and_prints_the_release_command() {
        let root = TempRoot::new("claimed");
        let config = config_with(vec![entry("usb:d209:0430:00", "panel", Backend::Winusb)]);
        let store = root.store(&config);
        let claimed =
            Survey::from_nodes(&[node(PANEL, HID_CLASS, WINUSB_SERVICE, Some("8&2A0D0500&0"))]);

        let plan = plan_remove(
            &claimed,
            &one_ipac(),
            &config,
            &GamesFile::default(),
            &RemoveSpec {
                alias: "panel".to_owned(),
                force: false,
            },
        )
        .expect("a claimed board is removed, not refused");
        assert_eq!(plan.still_claimed.as_deref(), Some(PANEL));

        let message = apply_remove(&store, &plan).unwrap().message();
        assert!(message.contains("STILL CLAIMED"), "{message}");
        assert!(
            message.contains(&format!("ksx winusb release {PANEL}")),
            "{message}"
        );
    }

    /// A board on the keyboard stack is not claimed, and saying it is would
    /// send someone to run a release that refuses.
    #[test]
    fn removing_an_unclaimed_device_says_nothing_about_releasing_it() {
        let config = config_with(vec![entry(
            "usb:d209:0430:00",
            "panel",
            Backend::Interception,
        )]);
        let plan = plan_remove(
            &cabinet(),
            &one_ipac(),
            &config,
            &GamesFile::default(),
            &RemoveSpec {
                alias: "panel".to_owned(),
                force: false,
            },
        )
        .unwrap();
        assert_eq!(plan.still_claimed, None);
        let message = RemoveOutcome {
            path: PathBuf::new(),
            backup: None,
            plan,
        }
        .message();
        assert!(!message.contains("winusb release"), "{message}");
    }

    #[test]
    fn an_unknown_alias_is_refused_with_the_aliases_that_exist() {
        let config = config_with(vec![entry(PANEL, "panel", Backend::Interception)]);
        let err = plan_remove(
            &cabinet(),
            &one_ipac(),
            &config,
            &GamesFile::default(),
            &RemoveSpec {
                alias: "nope".to_owned(),
                force: false,
            },
        )
        .expect_err("no such alias");
        assert_eq!(err.code(), "unknown-device-alias");
        assert!(err.to_string().contains("panel"), "{err}");
    }
}
