//! One resolution pass: `[[device]]` spellings → the concrete interfaces that
//! answer to them, right now (`docs/DEVICE-IDENTITY.md` §9 item 2).
//!
//! # What this replaces
//!
//! A `[[device]] id` used to be a raw string that was byte-compared against
//! enumerated hardware, deep in the pipeline, in four separate places. Which
//! meant the id had to *be* a devnode path — a value the user cannot author,
//! cannot verify, and whose stability depends on whether their board happens to
//! report a USB serial (`docs/DEVICE-IDENTITY.md` §1, measured in §3). When it
//! stopped matching, for that reason or any other, the config named nothing: no
//! error, no warning, a dead panel and a plan whose candidate list was simply
//! empty several layers down.
//!
//! Now the config holds a selector, and exactly one pass — this one — turns
//! each selector into the id the rest of the session uses. Everything
//! downstream keeps comparing concrete ids byte-exactly, unchanged.
//!
//! # Three outcomes, and none of them is a guess
//!
//! - [`Match::One`] — proceed, rewriting the plan's ids to the matched devnode.
//! - [`Match::None`] — **degrade, naming the board**, at the top, before
//!   anything is claimed or filtered: the slot that needed it is dropped with a
//!   `[WARN]`, everyone else plays, and the refusal comes only when no slot
//!   survives. An earlier draft of this list said "refuse" flatly, and so did
//!   `docs/DEVICE-IDENTITY.md` §9 item 2 — but one loose cable behind an
//!   eight-player cabinet must not blank the machine for everybody. See
//!   [`drop_missing`] for the whole argument.
//! - [`Match::Ambiguous`] — refuse, listing every hit *and* the port-pinned
//!   selector that would separate each one. Never pick. Two identical boards
//!   staying tellable apart is the entire reason WinUSB capture beats
//!   Interception, and silently taking the first would give that away with a
//!   friendlier face.
//!
//! # Where it runs, and why exactly there
//!
//! Inside [`crate::run::plan::resolve_as`], which is the one call
//! `LiveFactory::make()`, `LiveFactory::resolve_plan()` (the tray's "Reload
//! config"), `ksx run` and `ksx daemon` all go through. Hot-swap eligibility
//! (`SessionShape::bounce_reason`) compares `DeviceId`s to decide whether a
//! config edit is structural enough to bounce a live session. Resolve anywhere
//! downstream of that comparison and every preset edit reports "slot N's input
//! device changed" and restarts the pads mid-game.

use std::collections::BTreeSet;

use ksx_config::DeviceEntry;
use ksx_core::{DeviceFacts, DeviceId, DeviceSelector, Match, Qualifier};

use crate::run::plan::RunPlan;

/// Why a session's devices could not be resolved. Every variant is a refusal
/// taken before anything is claimed or filtered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// Nothing connected answers to a `[[device]]` entry a slot needs.
    Missing {
        alias: String,
        /// The `id` exactly as the file spells it, so it can be found.
        written: String,
        replug_proof: bool,
    },
    /// Two or more connected interfaces answer. Carries each one with the
    /// port-pinned selector that would name it alone.
    Ambiguous {
        alias: String,
        written: String,
        /// `(matched id, the selector that would pin just this one)`.
        candidates: Vec<(DeviceId, String)>,
    },
    /// Two different `[[device]]` spellings landed on one physical interface.
    Collision {
        first: String,
        second: String,
        id: DeviceId,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Missing {
                alias,
                written,
                replug_proof,
            } => {
                write!(
                    f,
                    "device \"{alias}\" (id = '{written}') is not connected: no USB interface \
                     answers to it"
                )?;
                if *replug_proof {
                    write!(
                        f,
                        ". The board is unplugged, or its interface is not enumerable — \
                         `ksx devices` lists what is actually there"
                    )
                } else {
                    // Measured on the cabinet 2026-08-07, and the reason this
                    // does not simply say "you moved it": an instance path is
                    // port-derived ONLY for boards with no USB serial. The
                    // I-PAC 4X reports serial "4", so Windows keys its devnode
                    // off the serial and the path survived a move from root
                    // port 5 to port 7 byte-for-byte. Asserting "you moved it"
                    // would send someone hunting a port problem they do not
                    // have, when the board is simply unplugged. State both, let
                    // `ksx devices` settle it.
                    write!(
                        f,
                        ". Either the board is unplugged, or it moved: a full instance path is \
                         port-derived only for boards that report no USB serial (boards that do \
                         keep the same path across sockets). `ksx devices` prints the id it has \
                         now, and the `usb:` selector that names the board either way \
                         (docs/DEVICE-IDENTITY.md §1)"
                    )
                }
            }
            ResolveError::Ambiguous {
                alias,
                written,
                candidates,
            } => {
                writeln!(
                    f,
                    "device \"{alias}\" (id = '{written}') matches {} connected interfaces, so \
                     ksx cannot tell which board you mean — and will not guess:",
                    candidates.len()
                )?;
                for (id, pinned) in candidates {
                    writeln!(f, "  {id}")?;
                    writeln!(f, "      id = '{pinned}'")?;
                }
                write!(
                    f,
                    "Put one of those ids in the [[device]] entry. A `port=` selector pins the \
                     board to that socket: it tells your two boards apart, and it stops working \
                     if you move one"
                )
            }
            ResolveError::Collision { first, second, id } => write!(
                f,
                "devices \"{first}\" and \"{second}\" both resolve to the same interface {id}. \
                 One physical board cannot be two configured devices: whichever slots they feed, \
                 ksx would claim that one interface twice. Point one of them at the other board, \
                 or delete it"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Enumerate what is connected, as the matcher wants it.
///
/// Read-only: `ksx_capture::usb_candidates` opens nothing and claims nothing
/// (see its module docs), so this is safe on a cabinet mid-session.
#[cfg(windows)]
pub fn connected() -> std::io::Result<Vec<DeviceFacts>> {
    Ok(ksx_capture::usb_candidates()?
        .iter()
        .map(ksx_capture::UsbCandidate::facts)
        .collect())
}

/// Off Windows there is no USB tree to read. Nothing that needs a resolution
/// can run there anyway — the capture backends are all `#[cfg(windows)]` — so
/// this keeps the pure planner compiling and testable everywhere.
#[cfg(not(windows))]
pub fn connected() -> std::io::Result<Vec<DeviceFacts>> {
    Ok(Vec::new())
}

/// Does this plan name a `[[device]]` whose id has to be matched against
/// hardware at all?
///
/// An Interception-only configuration answers `false`, and then nothing
/// enumerates: M3–M5 sessions do exactly what they did before, on machines
/// where the USB tree may not even be readable.
pub fn needs_enumeration(plan: &RunPlan, devices: &[DeviceEntry]) -> bool {
    let used = used_ids(plan);
    devices.iter().any(|d| {
        needs_matching(d.id.selector())
            // A slot names this entry by the spelling it was written with...
            && (used.contains(&d.id.as_device_id())
                // ...or the entry declares a WinUSB board, which must be
                // classified against the live tree no matter how a slot spells
                // it. Without this clause an entry a slot names longhand is
                // invisible here, enumeration is skipped, `classify_backends`
                // never runs, and the session quietly falls back to Interception
                // on a claimed board — a dead panel that reports success.
                // Interception-only configurations still never enumerate.
                || d.backend == ksx_config::Backend::Winusb)
    })
}

/// Rewrite every configured spelling in `plan` to the concrete interface that
/// answers to it. See the module docs for the outcome rules.
pub fn apply(
    plan: &mut RunPlan,
    devices: &[DeviceEntry],
    connected: &[DeviceFacts],
) -> Result<(), ResolveError> {
    let used = used_ids(plan);

    // Only entries this plan actually needs. A cabinet's config accumulates
    // boards — the second panel, a trackball, a laptop keyboard — and refusing
    // to start a one-player game because an unrelated `[[device]]` is unplugged
    // would be the same unhelpfulness this module exists to delete.
    let mut resolved: Vec<(&DeviceEntry, DeviceId)> = Vec::new();
    let mut missing: Vec<&DeviceEntry> = Vec::new();
    for device in devices {
        let written = device.id.as_device_id();
        if !used.contains(&written) || !needs_matching(device.id.selector()) {
            continue;
        }
        match device.id.selector().match_against(connected) {
            Match::One(facts) => resolved.push((device, facts.id.clone())),
            Match::None => missing.push(device),
            Match::Ambiguous(hits) => {
                return Err(ResolveError::Ambiguous {
                    alias: device.alias.clone(),
                    written: device.id.raw().to_owned(),
                    candidates: hits
                        .iter()
                        .map(|facts| (facts.id.clone(), pin_to_port(facts).to_string()))
                        .collect(),
                })
            }
        }
    }

    collision_check(&resolved)?;
    drop_missing(plan, &missing)?;

    // Identity mappings are the common case (a legacy full instance path
    // resolves to itself), and they must not be reported as a change.
    let rewrites: Vec<(DeviceId, DeviceId)> = resolved
        .iter()
        .filter(|(device, id)| device.id.raw() != id.as_str())
        .map(|(device, id)| (device.id.as_device_id(), id.clone()))
        .collect();
    if rewrites.is_empty() {
        return classify_backends(plan, devices, connected);
    }

    let swap = |id: &mut DeviceId| {
        if let Some((_, to)) = rewrites.iter().find(|(from, _)| from == id) {
            *id = to.clone();
        }
    };
    for slot in &mut plan.slots {
        slot.spec.keyboard.iter_mut().for_each(swap);
        slot.spec.mouse.iter_mut().for_each(swap);
    }
    plan.captureable.iter_mut().for_each(swap);
    plan.winusb.iter_mut().for_each(swap);

    // Two DIFFERENT `[[device]]` entries landing on one interface was already
    // refused above. What can still collapse here is an alias and a slot that
    // spells the same board out longhand — which is one keyboard feeding two
    // slots, i.e. what a splitter IS, and `build_plan` deduplicates it for
    // exactly that reason. Doing it again after the rewrite keeps that promise.
    dedupe(&mut plan.captureable);
    dedupe(&mut plan.winusb);

    for (device, id) in &resolved {
        if device.id.raw() != id.as_str() {
            plan.notes.push(format!(
                "[INFO] device \"{}\" (id = '{}') resolved to {id}",
                device.alias,
                device.id.raw()
            ));
        }
    }
    classify_backends(plan, devices, connected)
}

/// Which captured boards are on `winusb.sys`, decided by **identity** rather
/// than by spelling.
///
/// `build_plan` seeds `plan.winusb` by comparing a `[[device]]` entry's written
/// id against a slot's, as strings. That is correct only while both spell the
/// board the same way — and they routinely do not. A slot may name a board by
/// alias, or spell its devnode out longhand; `games.toml` files written before
/// `ksx device pick` existed hold the longhand path, while the `[[device]]`
/// entry `pick` writes holds a `usb:` selector. Two spellings, one board, no
/// string match.
///
/// Found on the cabinet 2026-08-07, and the consequence is not cosmetic:
/// `winusb` came back empty, so the session fell back to Interception — which
/// **cannot see a WinUSB-claimed board at all**. It would have started cleanly,
/// reported success, and left the panel completely dead.
///
/// A `[[device]]` entry declares what a board *is*, including which driver owns
/// it, so that declaration has to reach the board however a slot happens to name
/// it. Runs after the rewrites above, when every id is concrete.
fn classify_backends(
    plan: &mut RunPlan,
    devices: &[DeviceEntry],
    connected: &[DeviceFacts],
) -> Result<(), ResolveError> {
    for device in devices {
        if device.backend != ksx_config::Backend::Winusb || !needs_matching(device.id.selector()) {
            continue;
        }
        // Post-rewrite the entry may already be listed; matching is idempotent.
        let id = match device.id.selector().match_against(connected) {
            Match::One(facts) => facts.id.clone(),
            // Nothing answers. When a slot needs this entry `drop_missing` has
            // already dealt with it; when no slot does, it is not this
            // session's business.
            Match::None => continue,
            // **This used to be `_ => continue`, on the reasoning that an
            // ambiguity had already been refused above. It had not.** `apply`'s
            // loop only visits entries a slot names by THIS spelling, so an
            // entry a slot spells longhand is skipped there and this is the
            // first — and only — place its ambiguity can be seen. Falling
            // through left `plan.winusb` empty for a board the session is about
            // to capture, so the run fell back to Interception on a
            // WinUSB-claimed interface: a dead panel that reports success,
            // which is the exact failure this function exists to fix.
            Match::Ambiguous(hits) => {
                // Two boards answer, and this session captures neither of them:
                // an unrelated entry's twins must not blank a cabinet, which is
                // the same trade `drop_missing` makes for an absent board.
                if !hits
                    .iter()
                    .any(|facts| plan.captureable.contains(&facts.id))
                {
                    continue;
                }
                return Err(ResolveError::Ambiguous {
                    alias: device.alias.clone(),
                    written: device.id.raw().to_owned(),
                    candidates: hits
                        .iter()
                        .map(|facts| (facts.id.clone(), pin_to_port(facts).to_string()))
                        .collect(),
                });
            }
        };
        if plan.captureable.contains(&id) && !plan.winusb.contains(&id) {
            plan.winusb.push(id);
        }
    }
    Ok(())
}

/// A board a slot names is not connected: drop what cannot work, keep what can.
///
/// Refusing the whole session here is the wrong trade for an arcade cabinet.
/// One unplugged trackball would take all eight players down, and before this
/// module existed an absent device took down only what actually needed it — so
/// a blanket refusal would be a regression wearing a safety check's clothes.
///
/// The mouse case is the sharpest: M4 never sets the mouse class filter at all
/// (see [`crate::run::plan`]), so a slot's mouse is parsed, stored and never
/// read. Refusing eight players over a device ksx does not yet even capture is
/// indefensible. A missing mouse is therefore dropped silently-but-noted, and
/// that slot keeps playing on its keyboard.
///
/// A missing *keyboard* is different — that player has no input at all — so the
/// slot goes, and its number is named so it is obvious who lost their controls.
/// Everyone else still plays.
///
/// The one case that still refuses outright is a plan with **no slots left**.
/// There is then nothing to run, and starting silently would look like success
/// while the panel stayed dead — the exact failure this module exists to end.
fn drop_missing(plan: &mut RunPlan, missing: &[&DeviceEntry]) -> Result<(), ResolveError> {
    if missing.is_empty() {
        return Ok(());
    }
    let gone: BTreeSet<DeviceId> = missing.iter().map(|d| d.id.as_device_id()).collect();

    let mut lost_mice: Vec<u8> = Vec::new();
    for slot in &mut plan.slots {
        if slot.spec.mouse.as_ref().is_some_and(|m| gone.contains(m)) {
            slot.spec.mouse = None;
            lost_mice.push(slot.spec.number);
        }
    }

    let lost_slots: Vec<u8> = plan
        .slots
        .iter()
        .filter(|s| s.spec.keyboard.as_ref().is_some_and(|k| gone.contains(k)))
        .map(|s| s.spec.number)
        .collect();
    plan.slots
        .retain(|s| !s.spec.keyboard.as_ref().is_some_and(|k| gone.contains(k)));

    plan.captureable.retain(|id| !gone.contains(id));
    plan.winusb.retain(|id| !gone.contains(id));

    if plan.slots.is_empty() {
        let first = missing[0];
        return Err(ResolveError::Missing {
            alias: first.alias.clone(),
            written: first.id.raw().to_owned(),
            replug_proof: first.id.selector().survives_replug(),
        });
    }

    for device in missing {
        // The same correction as `ResolveError::Missing`'s Display, thirty
        // lines up, and it has to be made HERE too — this is the string a user
        // actually reads, because a dropped slot is the common case and a total
        // refusal is the rare one. Measured on the cabinet 2026-08-07: an
        // instance path is port-derived only for a board that reports no USB
        // serial; the I-PAC 4X reports "4" and its path survived a root-port
        // move byte-for-byte. The board is absent, so ksx cannot read its
        // serial and cannot know which kind of path this is. Offer both causes,
        // assert neither (`docs/DEVICE-IDENTITY.md` §3).
        plan.notes.push(format!(
            "[WARN] device \"{}\" (id = '{}') is not connected{}",
            device.alias,
            device.id.raw(),
            if device.id.selector().survives_replug() {
                ""
            } else {
                " — either it is unplugged, or it moved and this id followed the \
                 socket: a full instance path is port-derived only for boards that \
                 report no USB serial (docs/DEVICE-IDENTITY.md §1). `ksx devices` \
                 prints the id it has now, and the `usb:` selector that names the \
                 board either way"
            }
        ));
    }
    if !lost_slots.is_empty() {
        plan.notes.push(format!(
            "[WARN] slot(s) {lost_slots:?} have no keyboard and will NOT play this session; \
             the remaining slots start normally"
        ));
    }
    if !lost_mice.is_empty() {
        plan.notes.push(format!(
            "[WARN] slot(s) {lost_mice:?} lost their mouse, which M4 never reads anyway; \
             those slots play on their keyboard as usual"
        ));
    }
    Ok(())
}

/// A hardware id is passed through **verbatim**: it names a devnode on the
/// keyboard stack, which is not a USB interface and never appears in this
/// enumeration. M3–M5 depend on that byte-exact path, and matching one here
/// could only ever produce [`Match::None`] — a refusal for every Interception
/// configuration ksx has ever run.
fn needs_matching(selector: &DeviceSelector) -> bool {
    !matches!(selector, DeviceSelector::HardwareId(_))
}

/// Every device id this plan would actually touch.
fn used_ids(plan: &RunPlan) -> BTreeSet<DeviceId> {
    let mut used: BTreeSet<DeviceId> = plan
        .captureable
        .iter()
        .chain(&plan.winusb)
        .cloned()
        .collect();
    for slot in &plan.slots {
        used.extend(slot.spec.keyboard.iter().cloned());
        used.extend(slot.spec.mouse.iter().cloned());
    }
    used
}

/// **Two spellings on one board is a refusal, not a dedupe**
/// (`docs/DEVICE-IDENTITY.md` §8).
///
/// Deduping would let one physical board silently drive two configured
/// devices' worth of slots — the exact two-identical-boards confusion this
/// design exists to protect against. Today it fails loudly, because the second
/// WinUSB claim on one interface errors. This keeps it loud, and moves it to
/// where the aliases are still known.
///
/// Entries that spell the id *identically* are not a collision: they are one
/// board under two names, they resolve to one id today, and they have always
/// worked.
fn collision_check(resolved: &[(&DeviceEntry, DeviceId)]) -> Result<(), ResolveError> {
    for (i, (a, a_id)) in resolved.iter().enumerate() {
        for (b, b_id) in &resolved[i + 1..] {
            if a_id == b_id && !a.id.raw().eq_ignore_ascii_case(b.id.raw()) {
                return Err(ResolveError::Collision {
                    first: a.alias.clone(),
                    second: b.alias.clone(),
                    id: a_id.clone(),
                });
            }
        }
    }
    Ok(())
}

/// The selector that names exactly this interface and no other: the port rung,
/// which always discriminates because the instance tail *is* the socket.
fn pin_to_port(facts: &DeviceFacts) -> DeviceSelector {
    DeviceSelector::Usb {
        vendor_id: facts.vendor_id,
        product_id: facts.product_id,
        interface_number: facts.interface_number,
        qualifier: Qualifier::Port(facts.instance.to_uppercase()),
    }
}

fn dedupe(ids: &mut Vec<DeviceId>) {
    let mut seen = BTreeSet::new();
    ids.retain(|id| seen.insert(id.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::{ConfigFile, GamesFile, PresetFile};

    /// The cabinet's board, in the socket it was in when the config was written.
    const LIVE_PATH: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
    /// The same board, one socket over.
    const MOVED_PATH: &str = r"USB\VID_D209&PID_0430&MI_00\6&1B2C3D4E&0&0000";

    fn ipac(instance: &str) -> DeviceFacts {
        DeviceFacts {
            id: DeviceId::new(format!(r"USB\VID_D209&PID_0430&MI_00\{instance}")),
            vendor_id: 0xD209,
            product_id: 0x0430,
            interface_number: 0,
            serial: Some("4".into()),
            instance: instance.to_owned(),
        }
    }

    fn presets() -> Vec<PresetFile> {
        vec![toml::from_str("name = \"P1\"\n[bindings]\nA = \"S\"\n").unwrap()]
    }

    /// A config with one `[[device]]` on WinUSB feeding slot 1.
    fn plan_for(id: &str) -> (RunPlan, ConfigFile) {
        let config: ConfigFile = toml::from_str(&format!(
            "schema_version = 1\n\n\
             [[device]]\n\
             id = '{id}'\n\
             alias = \"ipac\"\n\
             backend = \"winusb\"\n\n\
             [[slot]]\n\
             number = 1\n\
             keyboard = \"ipac\"\n\
             preset = \"P1\"\n"
        ))
        .unwrap();
        let plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        (plan, config)
    }

    fn resolve(id: &str, connected: &[DeviceFacts]) -> Result<RunPlan, ResolveError> {
        let (mut plan, config) = plan_for(id);
        apply(&mut plan, &config.devices, connected)?;
        Ok(plan)
    }

    /// A board that is NOT in `connected` in any of these tests — a second
    /// panel, or a trackball, that is simply unplugged tonight.
    const ABSENT: &str = r"USB\VID_D209&PID_0430&MI_00\7&DEADBEEF&0&0000";

    fn resolve_config(toml_src: &str, connected: &[DeviceFacts]) -> Result<RunPlan, ResolveError> {
        let config: ConfigFile = toml::from_str(toml_src).unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        apply(&mut plan, &config.devices, connected)?;
        Ok(plan)
    }

    /// **The eight-players-down bug.** An unplugged second panel must cost its
    /// own slot and nothing else. Refusing here would mean one loose cable in
    /// the back of the cabinet blanks the whole machine for everybody.
    #[test]
    fn an_unplugged_second_panel_drops_only_its_own_slot_and_everyone_else_plays() {
        let plan = resolve_config(
            &format!(
                "schema_version = 1\n\n\
                 [[device]]\nid = '{LIVE_PATH}'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
                 [[device]]\nid = '{ABSENT}'\nalias = \"panel2\"\nbackend = \"winusb\"\n\n\
                 [[slot]]\nnumber = 1\nkeyboard = \"ipac\"\npreset = \"P1\"\n\n\
                 [[slot]]\nnumber = 2\nkeyboard = \"panel2\"\npreset = \"P1\"\n"
            ),
            &[ipac("7&25EEA38C&0&0000")],
        )
        .expect("one unplugged panel must not refuse the session");

        let numbers: Vec<u8> = plan.slots.iter().map(|s| s.spec.number).collect();
        assert_eq!(
            numbers,
            vec![1],
            "slot 1 keeps playing; only slot 2 is lost"
        );
        // The absent board must not survive anywhere the session would act on.
        assert!(!plan.captureable.iter().any(|id| id.as_str() == ABSENT));
        assert!(!plan.winusb.iter().any(|id| id.as_str() == ABSENT));
        // ...and losing a player is never silent.
        assert!(
            plan.notes.iter().any(|n| n.contains("panel2")),
            "the missing board must be named: {:?}",
            plan.notes
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("[2]")),
            "the slot that lost its controls must be named: {:?}",
            plan.notes
        );
    }

    /// **The sentence that was refused once and survived thirty lines below.**
    ///
    /// `ResolveError::Missing`'s `Display` was corrected after the 2026-08-07
    /// cabinet measurement (an instance path is port-derived only for a board
    /// with no USB serial), and a test pins it. The `[WARN]` note a *dropped
    /// slot* produces was not corrected, and that is the string most users
    /// actually see: dropping one slot is the common case, refusing the whole
    /// run is the rare one. The existing test could not catch it — it reads
    /// `err.to_string()`, never `plan.notes`.
    ///
    /// Breaks against: the note re-acquiring any wording that asserts the board
    /// moved, in either case.
    #[test]
    fn a_dropped_slots_warning_offers_both_causes_and_asserts_neither() {
        let plan = resolve_config(
            &format!(
                "schema_version = 1\n\n\
                 [[device]]\nid = '{LIVE_PATH}'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
                 [[device]]\nid = '{ABSENT}'\nalias = \"panel2\"\nbackend = \"winusb\"\n\n\
                 [[slot]]\nnumber = 1\nkeyboard = \"ipac\"\npreset = \"P1\"\n\n\
                 [[slot]]\nnumber = 2\nkeyboard = \"panel2\"\npreset = \"P1\"\n"
            ),
            &[ipac("7&25EEA38C&0&0000")],
        )
        .expect("one unplugged panel must not refuse the session");

        let note = plan
            .notes
            .iter()
            .find(|n| n.contains("panel2"))
            .expect("the missing board is named");
        assert!(
            !note.to_lowercase().contains("one specific usb socket"),
            "a note must not assert a move it cannot know happened — the board is \
             absent, so its serial cannot be read: {note}"
        );
        assert!(
            note.contains("unplugged"),
            "offer the other cause, as the refusal does: {note}"
        );
        assert!(
            note.contains("port-derived only for boards that report no USB serial"),
            "and say what actually decides it: {note}"
        );
    }

    /// **The dead panel, second edition.** The first fix taught a `[[device]]`
    /// entry's backend to reach the board however a slot spells it; this is
    /// what that fix does when the entry is AMBIGUOUS.
    ///
    /// `apply`'s loop skips the entry (no slot names it by this spelling), so
    /// the ambiguity refusal at the top never runs for it, and
    /// `classify_backends` used to answer `_ => continue`. Result: `Ok`, an
    /// empty `plan.winusb`, `needs_interception()` true — an Interception
    /// fallback on a WinUSB-claimed board, which is a session that starts
    /// cleanly, reports success, and leaves the panel completely dead.
    ///
    /// Breaks against: any version that swallows `Match::Ambiguous` here.
    #[test]
    fn an_ambiguous_winusb_entry_is_refused_even_when_a_slot_spells_the_board_longhand() {
        let config: ConfigFile = toml::from_str(&format!(
            "schema_version = 1\n\n\
             [[device]]\nid = 'usb:d209:0430:00'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
             [[slot]]\nnumber = 1\nkeyboard = '{LIVE_PATH}'\npreset = \"P1\"\n"
        ))
        .unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        let twins = [ipac("7&25EEA38C&0&0000"), ipac("6&1B2C3D4E&0&0000")];

        let err = apply(&mut plan, &config.devices, &twins)
            .expect_err("two boards answer to this entry; ksx must not pick one");
        let ResolveError::Ambiguous {
            alias, candidates, ..
        } = &err
        else {
            panic!("expected Ambiguous, got {err:?}");
        };
        assert_eq!(alias, "ipac");
        assert_eq!(candidates.len(), 2, "every hit is carried");
        let text = err.to_string();
        assert!(
            text.contains("usb:d209:0430:00:port=7&25EEA38C&0&0000")
                && text.contains("usb:d209:0430:00:port=6&1B2C3D4E&0&0000"),
            "and the selector that would pin each: {text}"
        );
    }

    /// The other half of that trade: twins on a `[[device]]` entry **this
    /// session does not capture** must not refuse the run. Refusing would be
    /// the eight-players-down bug arriving by a different door — a spare board
    /// in the config, plugged in twice, blanking a game that never used it.
    #[test]
    fn twins_on_an_entry_this_session_never_captures_do_not_refuse_the_run() {
        let config: ConfigFile = toml::from_str(&format!(
            "schema_version = 1\n\n\
             [[device]]\nid = '{LIVE_PATH}'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
             [[device]]\nid = 'usb:d209:15a2:00'\nalias = \"spare\"\nbackend = \"winusb\"\n\n\
             [[slot]]\nnumber = 1\nkeyboard = \"ipac\"\npreset = \"P1\"\n"
        ))
        .unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        let mut spare_a = ipac("7&AAAAAAAA&0&0000");
        spare_a.product_id = 0x15A2;
        spare_a.id = DeviceId::new(r"USB\VID_D209&PID_15A2&MI_00\7&AAAAAAAA&0&0000");
        let mut spare_b = spare_a.clone();
        spare_b.instance = "6&BBBBBBBB&0&0000".into();
        spare_b.id = DeviceId::new(r"USB\VID_D209&PID_15A2&MI_00\6&BBBBBBBB&0&0000");

        apply(
            &mut plan,
            &config.devices,
            &[ipac("7&25EEA38C&0&0000"), spare_a, spare_b],
        )
        .expect("the ambiguous entry feeds no slot in this plan");
        assert_eq!(plan.winusb, vec![DeviceId::from(LIVE_PATH)]);
    }

    /// **The sharpest case in the audit.** M4 never sets the mouse class filter,
    /// so a slot's mouse is stored and never read — refusing a session over one
    /// would take players down for a device ksx does not even capture yet.
    #[test]
    fn an_unplugged_mouse_never_costs_a_player_their_keyboard() {
        let plan = resolve_config(
            &format!(
                "schema_version = 1\n\n\
                 [[device]]\nid = '{LIVE_PATH}'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
                 [[device]]\nid = '{ABSENT}'\nalias = \"ball\"\n\n\
                 [[slot]]\nnumber = 1\nkeyboard = \"ipac\"\nmouse = \"ball\"\npreset = \"P1\"\n"
            ),
            &[ipac("7&25EEA38C&0&0000")],
        )
        .expect("an unplugged trackball must not refuse the session");

        assert_eq!(plan.slots.len(), 1, "the player keeps playing");
        assert_eq!(plan.slots[0].spec.number, 1);
        assert!(plan.slots[0].spec.mouse.is_none(), "the dead mouse is gone");
        assert_eq!(
            plan.slots[0].spec.keyboard.as_ref().map(|k| k.as_str()),
            Some(LIVE_PATH),
            "their keyboard is untouched"
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("ball")),
            "still reported: {:?}",
            plan.notes
        );
    }

    /// **The dead-panel bug, found on the cabinet.** A game profile written
    /// before `device pick` existed spells the board out longhand, while the
    /// `[[device]]` entry now holds a `usb:` selector. `build_plan` compares
    /// those as strings, finds no match, and leaves `winusb` empty — so the
    /// session falls back to Interception, which cannot see a claimed board.
    /// It starts, reports success, and nothing works.
    #[test]
    fn a_slot_that_spells_the_board_longhand_still_gets_its_winusb_backend() {
        let config: ConfigFile = toml::from_str(&format!(
            "schema_version = 1\n\n\
             [[device]]\nid = 'usb:d209:0430:00'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
             [[slot]]\nnumber = 1\nkeyboard = '{LIVE_PATH}'\npreset = \"P1\"\n"
        ))
        .unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        // The defect, before resolution: one board, two spellings, no match.
        assert!(
            plan.winusb.is_empty(),
            "precondition: build_plan cannot match two spellings"
        );
        // ...and the half that made the first fix silently do nothing: the
        // entry is not in `used_ids` under this spelling, so without the
        // backend clause nothing would enumerate and `apply` would never run.
        assert!(
            needs_enumeration(&plan, &config.devices),
            "a winusb entry must force enumeration however a slot spells it"
        );

        apply(&mut plan, &config.devices, &[ipac("7&25EEA38C&0&0000")]).unwrap();

        assert_eq!(
            plan.winusb,
            vec![DeviceId::from(LIVE_PATH)],
            "the [[device]] entry's backend must reach the board a slot named longhand"
        );
        assert!(
            !plan.needs_interception(),
            "falling back to Interception on a claimed board is the dead panel"
        );
    }

    /// Degrading is not the same as pretending. With nothing left that could
    /// play, a "successful" start is a dead panel with no error anywhere — the
    /// precise failure this module was written to end.
    #[test]
    fn when_no_slot_survives_it_still_refuses_rather_than_starting_dead() {
        let err = resolve_config(
            &format!(
                "schema_version = 1\n\n\
                 [[device]]\nid = '{ABSENT}'\nalias = \"ipac\"\nbackend = \"winusb\"\n\n\
                 [[slot]]\nnumber = 1\nkeyboard = \"ipac\"\npreset = \"P1\"\n"
            ),
            &[ipac("7&25EEA38C&0&0000")],
        )
        .expect_err("no playable slot left must refuse");
        let ResolveError::Missing { alias, .. } = &err else {
            panic!("expected Missing, got {err:?}");
        };
        assert_eq!(alias, "ipac");
    }

    /// **The owner's live config.** A legacy port-pinned entry whose board has
    /// not moved resolves to itself, changes nothing, and reports nothing.
    #[test]
    fn a_legacy_instance_path_that_still_names_its_board_keeps_working_untouched() {
        let plan = resolve(LIVE_PATH, &[ipac("7&25EEA38C&0&0000")]).expect("still connected");
        assert_eq!(plan.captureable, vec![DeviceId::from(LIVE_PATH)]);
        assert_eq!(plan.winusb, vec![DeviceId::from(LIVE_PATH)]);
        assert_eq!(plan.slots[0].spec.keyboard, Some(DeviceId::from(LIVE_PATH)));
        assert!(
            !plan.notes.iter().any(|n| n.contains("resolved to")),
            "nothing changed, so nothing is reported: {:?}",
            plan.notes
        );
    }

    /// **The case that unblocks `ksx device pick`.** `pick` writes
    /// `usb:d209:0430:00`; before this pass, nothing resolved it and the config
    /// it wrote matched no hardware at all.
    #[test]
    fn a_usb_model_selector_resolves_to_the_concrete_interface() {
        let plan = resolve("usb:d209:0430:00", &[ipac("7&25EEA38C&0&0000")]).expect("one board");
        assert_eq!(plan.captureable, vec![DeviceId::from(LIVE_PATH)]);
        assert_eq!(
            plan.winusb,
            vec![DeviceId::from(LIVE_PATH)],
            "the backend choice has to follow the id it was made for"
        );
        assert_eq!(plan.slots[0].spec.keyboard, Some(DeviceId::from(LIVE_PATH)));
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("usb:d209:0430:00") && n.contains(LIVE_PATH)),
            "a rewrite is said out loud: {:?}",
            plan.notes
        );
    }

    /// **The defect, end to end.** Move the board one socket over: the `usb:`
    /// spelling still finds it; the port-pinned one does not, and says which
    /// entry is the problem instead of dying with an empty candidate list.
    #[test]
    fn moving_the_board_breaks_only_the_port_pinned_spelling_and_it_says_so_by_name() {
        let moved = [ipac("6&1B2C3D4E&0&0000")];

        let plan = resolve("usb:d209:0430:00", &moved).expect("the model rung survives a replug");
        assert_eq!(plan.captureable, vec![DeviceId::from(MOVED_PATH)]);

        let err = resolve(LIVE_PATH, &moved).expect_err("the socket changed");
        let ResolveError::Missing { alias, .. } = &err else {
            panic!("expected Missing, got {err:?}");
        };
        assert_eq!(alias, "ipac");
        let text = err.to_string();
        assert!(text.contains("\"ipac\""), "name the board: {text}");
        assert!(text.contains(LIVE_PATH), "quote the id in the file: {text}");
        // Measured on the cabinet 2026-08-07: an instance path is port-derived
        // only when the board reports no USB serial. The I-PAC 4X reports "4",
        // and its path survived root port 5 -> 7 unchanged. So this must offer
        // BOTH causes and assert neither, or it sends someone hunting a port
        // problem they do not have.
        assert!(text.contains("unplugged"), "offer the other cause: {text}");
        assert!(
            text.contains("port-derived only for boards that report no USB serial"),
            "say what actually decides it: {text}"
        );
        assert!(
            !text.contains("one specific USB SOCKET"),
            "must not assert a move it cannot know happened: {text}"
        );
        assert!(text.contains("ksx devices"), "say what to run: {text}");
    }

    /// A board that is simply not plugged in gets the other half of the
    /// sentence — no socket lecture, because its id is replug-proof.
    #[test]
    fn a_replug_proof_selector_that_matches_nothing_blames_the_missing_board() {
        let err = resolve("usb:d209:0430:00", &[]).expect_err("nothing connected");
        let text = err.to_string();
        assert!(text.contains("not connected"), "{text}");
        assert!(text.contains("unplugged"), "{text}");
        assert!(!text.contains("USB SOCKET"), "{text}");
    }

    /// **Two identical boards.** The model rung cannot separate them and their
    /// firmware serials are identical too (measured: the I-PAC 4X answers "4"),
    /// so the refusal has to hand over the two ids that WOULD work.
    #[test]
    fn two_identical_boards_are_refused_with_the_selector_that_separates_each() {
        let both = [ipac("7&25EEA38C&0&0000"), ipac("6&1B2C3D4E&0&0000")];
        let err = resolve("usb:d209:0430:00", &both).expect_err("twins");
        let ResolveError::Ambiguous { candidates, .. } = &err else {
            panic!("expected Ambiguous, got {err:?}");
        };
        assert_eq!(candidates.len(), 2);
        let text = err.to_string();
        assert!(text.contains(LIVE_PATH), "every hit is listed: {text}");
        assert!(text.contains(MOVED_PATH), "every hit is listed: {text}");
        assert!(
            text.contains("usb:d209:0430:00:port=7&25EEA38C&0&0000"),
            "and the selector that pins each one: {text}"
        );
        assert!(
            text.contains("usb:d209:0430:00:port=6&1B2C3D4E&0&0000"),
            "and the selector that pins each one: {text}"
        );
        assert!(text.contains("will not guess"), "{text}");

        // ...and each of those pins really does resolve to one board.
        let pinned = resolve("usb:d209:0430:00:port=7&25EEA38C&0&0000", &both)
            .expect("the port rung always separates");
        assert_eq!(pinned.captureable, vec![DeviceId::from(LIVE_PATH)]);
    }

    /// **A refusal, not a dedupe** (`docs/DEVICE-IDENTITY.md` §8). Two
    /// `[[device]]` entries landing on one interface means one board silently
    /// driving two slots' worth of capture — with two WinUSB claims on one
    /// handle behind it.
    #[test]
    fn two_entries_resolving_to_one_interface_is_refused_naming_both_aliases() {
        let config: ConfigFile = toml::from_str(&format!(
            "schema_version = 1\n\n\
             [[device]]\n\
             id = 'usb:d209:0430:00'\n\
             alias = \"panel\"\n\
             backend = \"winusb\"\n\n\
             [[device]]\n\
             id = '{LIVE_PATH}'\n\
             alias = \"spare\"\n\
             backend = \"winusb\"\n\n\
             [[slot]]\n\
             number = 1\n\
             keyboard = \"panel\"\n\
             preset = \"P1\"\n\n\
             [[slot]]\n\
             number = 2\n\
             keyboard = \"spare\"\n\
             preset = \"P1\"\n"
        ))
        .unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        let err = apply(&mut plan, &config.devices, &[ipac("7&25EEA38C&0&0000")])
            .expect_err("one board cannot be two devices");
        let text = err.to_string();
        assert!(text.contains("\"panel\""), "name both aliases: {text}");
        assert!(text.contains("\"spare\""), "name both aliases: {text}");
        assert!(text.contains(LIVE_PATH), "and the board: {text}");
        assert!(text.contains("claim that one interface twice"), "{text}");
    }

    /// One board, two names, both spelled identically — a config that has
    /// always worked and resolves to one id today. Refusing it would be a
    /// regression dressed as a safety check.
    #[test]
    fn one_board_under_two_identical_spellings_is_not_a_collision() {
        let config: ConfigFile = toml::from_str(
            "schema_version = 1\n\n\
             [[device]]\n\
             id = 'usb:d209:0430:00'\n\
             alias = \"p1\"\n\n\
             [[device]]\n\
             id = 'usb:d209:0430:00'\n\
             alias = \"p2\"\n\n\
             [[slot]]\n\
             number = 1\n\
             keyboard = \"p1\"\n\
             preset = \"P1\"\n\n\
             [[slot]]\n\
             number = 2\n\
             keyboard = \"p2\"\n\
             preset = \"P1\"\n",
        )
        .unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        apply(&mut plan, &config.devices, &[ipac("7&25EEA38C&0&0000")]).expect("one keyboard");
        assert_eq!(
            plan.captureable,
            vec![DeviceId::from(LIVE_PATH)],
            "one physical keyboard feeding two slots is what a splitter IS"
        );
        assert_eq!(plan.slots_using(&DeviceId::from(LIVE_PATH)), vec![1, 2]);
    }

    /// **Interception spellings pass through byte-exactly.** They name a
    /// devnode on the keyboard stack, which never appears in a USB enumeration,
    /// so matching them would refuse every configuration M3–M5 ever ran.
    #[test]
    fn an_interception_hardware_id_is_never_matched_against_the_usb_tree() {
        const HWID: &str = r"HID\VID_D209&PID_0430&REV_0056&MI_00";
        let plan = resolve(HWID, &[]).expect("nothing to enumerate, nothing to refuse");
        assert_eq!(plan.captureable, vec![DeviceId::from(HWID)]);
        assert!(plan.notes.iter().all(|n| !n.contains("resolved to")));
    }

    /// A laptop keyboard, whose ACPI id ksx's own setup wizard wrote. It gets
    /// opaque hardware-id semantics, so it is left exactly alone.
    #[test]
    fn an_acpi_keyboard_is_left_alone_rather_than_refused() {
        const ACPI: &str = r"ACPI\PNP0303\4&1A2B3C4D&0";
        let plan = resolve(ACPI, &[ipac("7&25EEA38C&0&0000")]).expect("not a USB interface");
        assert_eq!(plan.captureable, vec![DeviceId::from(ACPI)]);
    }

    /// Enumeration is not free, and an Interception-only session must not pay
    /// for it — nor depend on a readable USB tree, which is the state a machine
    /// with the M6 rebind half-done can be in.
    #[test]
    fn an_interception_only_plan_never_asks_for_an_enumeration() {
        let (plan, config) = plan_for(r"HID\VID_D209&PID_0430&REV_0056&MI_00");
        assert!(!needs_enumeration(&plan, &config.devices));

        let (plan, config) = plan_for("usb:d209:0430:00");
        assert!(needs_enumeration(&plan, &config.devices));
    }

    /// A `[[device]]` no slot in THIS plan uses is not this plan's problem. A
    /// cabinet's config accumulates boards, and refusing a one-player game
    /// because the second panel is unplugged is the unhelpfulness this module
    /// deletes, not one it adds.
    #[test]
    fn an_unused_device_entry_is_not_resolved_and_cannot_refuse_the_run() {
        let config: ConfigFile = toml::from_str(
            "schema_version = 1\n\n\
             [[device]]\n\
             id = 'usb:d209:0430:00'\n\
             alias = \"panel\"\n\n\
             [[device]]\n\
             id = 'usb:dead:beef:00'\n\
             alias = \"gone\"\n\n\
             [[slot]]\n\
             number = 1\n\
             keyboard = \"panel\"\n\
             preset = \"P1\"\n",
        )
        .unwrap();
        let mut plan =
            crate::run::plan::build_plan(&config, &GamesFile::default(), &presets(), None).unwrap();
        apply(&mut plan, &config.devices, &[ipac("7&25EEA38C&0&0000")])
            .expect("the unplugged board is not in this plan");
        assert_eq!(plan.captureable, vec![DeviceId::from(LIVE_PATH)]);
    }
}
