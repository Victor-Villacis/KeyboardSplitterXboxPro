//! One resolution pass: `[[device]]` spellings → the concrete interfaces that
//! answer to them, right now (`docs/DEVICE-IDENTITY.md` §9 item 2).
//!
//! # What this replaces
//!
//! A `[[device]] id` used to be a raw string that was byte-compared against
//! enumerated hardware, deep in the pipeline, in four separate places. Which
//! meant the id had to *be* a devnode path — and a devnode path's tail is
//! derived from which USB socket the board is in. Move the board one port over
//! and the config named nothing: no error, no warning, a dead panel and a plan
//! whose candidate list was simply empty several layers down.
//!
//! Now the config holds a selector, and exactly one pass — this one — turns
//! each selector into the id the rest of the session uses. Everything
//! downstream keeps comparing concrete ids byte-exactly, unchanged.
//!
//! # Three outcomes, and none of them is a guess
//!
//! - [`Match::One`] — proceed, rewriting the plan's ids to the matched devnode.
//! - [`Match::None`] — refuse, **naming the board**, at the top, before
//!   anything is claimed or filtered.
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
                    // The whole reason selectors exist. Say it here, because
                    // this is the moment the user meets the failure.
                    write!(
                        f,
                        ". This id names one specific USB SOCKET, so moving the board to another \
                         port breaks it. `ksx devices` prints the id it has now, and the \
                         replug-proof `usb:` selector that would survive the next move \
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
    devices
        .iter()
        .any(|d| used.contains(&d.id.as_device_id()) && needs_matching(d.id.selector()))
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
    for device in devices {
        let written = device.id.as_device_id();
        if !used.contains(&written) || !needs_matching(device.id.selector()) {
            continue;
        }
        match device.id.selector().match_against(connected) {
            Match::One(facts) => resolved.push((device, facts.id.clone())),
            Match::None => {
                return Err(ResolveError::Missing {
                    alias: device.alias.clone(),
                    written: device.id.raw().to_owned(),
                    replug_proof: device.id.selector().survives_replug(),
                })
            }
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

    // Identity mappings are the common case (a legacy full instance path
    // resolves to itself), and they must not be reported as a change.
    let rewrites: Vec<(DeviceId, DeviceId)> = resolved
        .iter()
        .filter(|(device, id)| device.id.raw() != id.as_str())
        .map(|(device, id)| (device.id.as_device_id(), id.clone()))
        .collect();
    if rewrites.is_empty() {
        return Ok(());
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
        assert!(
            text.contains("one specific USB SOCKET"),
            "say why it broke: {text}"
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
