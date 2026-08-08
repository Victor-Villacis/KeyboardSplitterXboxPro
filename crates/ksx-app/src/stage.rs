//! The staged setup's two exits: **save it** and **play it** —
//! `docs/FIRST-RUN.md` §2, moment 7.
//!
//! [`ksx_core::StagedSetup`] is the value a visit accumulates; it has no path
//! to a file and no path to a driver. This module is the only thing that gives
//! it either, and it gives it **one translation for both**:
//!
//! ```text
//!                      ┌─ [`to_config`] ─┐
//!   CommitSpec  ───────┤                 ├──► ConfigFile
//!                      └─────────────────┘        │
//!                                 ┌───────────────┴────────────────┐
//!                        [`apply`] (save)                  [`plan`] (play)
//!                     store.backup + save_config        run::plan::build_plan
//! ```
//!
//! # Why one translation is the requirement, not a tidiness
//!
//! §2: *"What is staged is what plays. There is no second translation step
//! where a saved file means something different from what the screen showed."*
//! Two functions — one that turned a stage into a session and one that turned
//! it into TOML — would be two chances to disagree, and the disagreement would
//! surface as "it worked when I pressed Play and the pads were wrong after I
//! restarted". So both exits are built from [`to_config`], and
//! [`tests::what_is_staged_is_what_plays`] pins it by planning the in-memory
//! config and the one read back off disk and comparing the two plans.
//!
//! # Which existing writers this feeds, and which it does not duplicate
//!
//! - **The config writer is `ksx_config::Store::save_config`**, behind
//!   `Store::backup`, which is the identical six lines
//!   [`crate::device_edit::apply_pick`] uses and the pattern
//!   [`crate::slots::assign`] documents. There is no second writer here: this
//!   module composes a `ConfigFile` and hands it over.
//! - **The `[[slot]]` shape comes from `ConfigFile::slot_entry`**, ksx-config's
//!   own reverse of the `slot_spec` that `build_plan` reads. Writing
//!   `SlotEntry` by hand here would be exactly the second translation above —
//!   and the round trip is already pinned by ksx-config's
//!   `slot_spec_resolves_alias_to_instance_path`.
//! - **The planner is `crate::run::plan::build_plan`**, unchanged and pure:
//!   the same function `ksx run`, `ksx daemon`, autostart and the tray's
//!   "Reload config" all reach through `resolve_as`. Play-without-saving is
//!   that function called on a `ConfigFile` that was never written, which is
//!   why a staged session refuses for exactly the reasons a saved one would.
//! - **The preset writer is `Store::save_preset`**, the same atomic save
//!   `ksx setup` and the mapper use.
//!
//! # Nothing here claims or plugs
//!
//! [`plan`] returns a plan. Claiming a board is still `ksx winusb claim`, and
//! plugging pads is still the supervisor's job at session start — `SURFACES.md`
//! §3 and `FIRST-RUN.md` §6 both require that an action which looked like a
//! menu choice never turns out to have installed a driver or claimed a board.

use std::path::PathBuf;

use ksx_config::{ConfigFile, DeviceEntry, GamesFile, PresetFile, SlotEntry, Store};
use ksx_core::stage::CommitSpec;
use ksx_core::DeviceRef;

use crate::run::plan::{build_plan, PlanError, PlanSource, RunPlan};

/// What a save landed on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    pub config: PathBuf,
    /// The timestamped copy taken before the config write; `None` when there
    /// was no config file yet.
    pub backup: Option<PathBuf>,
    /// The preset files written, in slot order (deduplicated: two slots may
    /// deliberately share one preset).
    pub presets: Vec<PathBuf>,
    /// The timestamped copies taken of preset files that ALREADY EXISTED.
    ///
    /// Empty on a first run, and that is the ordinary case. It is non-empty
    /// when a staged preset name collided with one on disk — see [`apply`] for
    /// why that is a backup and not a refusal.
    pub preset_backups: Vec<PathBuf>,
    /// The `[[device]]` alias every saved slot now names.
    pub alias: String,
    pub slots: Vec<u8>,
}

impl Committed {
    /// The one line a surface prints. It never says "claimed" and never says
    /// "plugged", because saving does neither.
    ///
    /// It DOES say when an existing preset was replaced, and it says it in the
    /// same sentence as the success. A save that quietly overwrote somebody's
    /// mapped preset and reported only "saved" is the shape `FIRST-RUN.md` §6
    /// bans — a screen reporting a success that is not the whole truth — and
    /// the note names the button that undoes it rather than a command.
    pub fn message(&self) -> String {
        let replaced = match self.preset_backups.len() {
            0 => String::new(),
            n => format!(
                ". {n} preset(s) of the same name were already there and have been REPLACED — a \
                 timestamped copy of each was kept, and the mapper's \"Restore backup\" puts one \
                 back"
            ),
        };
        format!(
            "saved {} controller(s) on \"{}\" to {} — nothing was claimed and no pad was plugged; \
             Play starts them{replaced}",
            self.slots.len(),
            self.alias,
            self.config.display()
        )
    }
}

/// The `ConfigFile` a staged setup MEANS, laid over whatever is already there.
///
/// `base` is the config on disk (or [`ConfigFile::default`] for a play that
/// never reads one). Its `[[device]]` and `[[slot]]` tables are *replaced* for
/// the entries this stage names and left alone otherwise:
///
/// - the staged device upserts by ALIAS, so re-picking the same board keeps one
///   entry rather than growing a second one;
/// - a staged slot replaces the `[[slot]]` of that number, because that is what
///   the screen showed for it;
/// - a `[[slot]]` the stage does not mention is untouched. A first-run visit
///   has none, but a returning user's other players must not vanish because
///   somebody re-staged player 1.
///
/// `backend` is deliberately NOT set from staging: `backend = "winusb"`
/// describes what an interface IS, never what it could become
/// (`docs/DEVICE-IDENTITY.md` §7), and a stage cannot know. An existing entry
/// keeps the backend it already had; a new one gets the default.
pub fn to_config(base: &ConfigFile, spec: &CommitSpec) -> ConfigFile {
    let mut config = base.clone();

    // `from_selector`, not `parse`: this is an id ksx is writing for the first
    // time, so the canonical spelling IS the written one.
    let id = DeviceRef::from_selector(spec.device.selector.clone());
    match config
        .devices
        .iter_mut()
        .find(|d| d.alias.eq_ignore_ascii_case(&spec.device.alias))
    {
        // Keep the backend the machine already established for this entry.
        Some(existing) => existing.id = id.clone(),
        None => config.devices.push(DeviceEntry {
            id: id.clone(),
            alias: spec.device.alias.clone(),
            backend: Default::default(),
        }),
    }

    for staged in &spec.slots {
        // `ConfigFile::slot_entry` — ksx-config's own reverse of the
        // `slot_spec` that `build_plan` reads back. It folds the device id to
        // the alias by matching the `[[device]]` entry pushed above, which is
        // why the device goes in first.
        let entry: SlotEntry = config.slot_entry(&staged.spec);
        match config.slots.iter_mut().find(|s| s.number == entry.number) {
            Some(existing) => *existing = entry,
            None => config.slots.push(entry),
        }
    }
    config.slots.sort_by_key(|s| s.number);
    config.settings.block_keyboards = spec.blocking;
    config
}

/// The preset files a staged setup would write, deduplicated by name.
///
/// Two slots may deliberately share one preset (two players, one key map);
/// `StagedSetup::commit` has already refused the case where two slots hold
/// DIFFERENT bindings under one name, so a duplicate here is genuinely the same
/// preset and writing it twice would be the same bytes.
fn preset_files(spec: &CommitSpec) -> Vec<PresetFile> {
    let mut out: Vec<PresetFile> = Vec::with_capacity(spec.slots.len());
    for slot in &spec.slots {
        if out
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&slot.preset.name))
        {
            continue;
        }
        out.push(PresetFile::from_core(&slot.preset));
    }
    out
}

/// **Play without saving.** The runnable plan this staged setup means, built
/// with no filesystem read and no filesystem write.
///
/// It is `build_plan` — the pure planner every other start path funnels through
/// — over the config [`to_config`] composes and the presets the stage is
/// carrying. So a staged session gets the same validation, the same refusals
/// and the same notes a saved one would, and the `[[slot]]`/preset resolution
/// happens through exactly the code `ksx run` uses.
///
/// What this does NOT do, on purpose: resolve selectors against live hardware.
/// That is `crate::run::resolve`'s one pass at session start
/// (`plan::resolve_as`), and doing it here would make merely *planning* a
/// staged setup enumerate USB — which `FIRST-RUN.md` §5 forbids of anything on
/// the looking side. The supervisor still resolves before it captures.
pub fn plan(spec: &CommitSpec) -> Result<RunPlan, PlanError> {
    let config = to_config(&ConfigFile::default(), spec);
    let presets = preset_files(spec);
    let mut plan = build_plan(&config, &GamesFile::default(), &presets, None)?;
    plan.source = PlanSource::Config;
    Ok(plan)
}

/// [`plan`], plus **the one resolution pass** — what a live staged session
/// actually starts on.
///
/// A staged setup names a board by [`ksx_core::DeviceSelector`]
/// (`usb:d209:0430:00`), and a capture backend needs a devnode. That
/// translation is `crate::run::resolve`, run once at session start, and this
/// calls the SAME `plan::resolve_devices` that `plan::resolve_as` calls — not a
/// copy — so a staged session and a saved one resolve "which board is this"
/// identically. A second answer to that question is the one this project has
/// already been wrong about (`docs/DEVICE-IDENTITY.md` §8).
///
/// Kept apart from [`plan`] because the split is the point: [`plan`] touches no
/// hardware and is what a *preview* calls, and enumeration belongs on the
/// starting side of the line `FIRST-RUN.md` §5 draws.
pub fn resolve(spec: &CommitSpec) -> Result<RunPlan, PlanError> {
    let config = to_config(&ConfigFile::default(), spec);
    let mut plan = plan(spec)?;
    crate::run::plan::resolve_devices(&mut plan, &config.devices)?;
    Ok(plan)
}

/// **Save.** Write the staged setup: presets first, then one config write
/// behind one timestamped backup.
///
/// Preset-first is the ordering `ksx setup` documents and for its reason: a
/// `[[slot]]` pointing at a preset that does not exist is the one ordering that
/// leaves a machine worse than it started — it refuses to start at the next
/// boot, long after whoever pressed Save walked away.
///
/// # A preset that already exists is BACKED UP, not silently replaced
///
/// `Store::save_preset` is an atomic overwrite with no backup of its own —
/// correct for the mapper, which is editing the file it just read. A staged
/// setup is not: it names a preset the user picked from a menu, and the name
/// it offers first (`ksx_api::preset_name_for_slot` — "Player 1") is exactly
/// the name a previous run of this same flow would have left on disk. So a
/// second visit through the journey could replace a mapped preset with an empty
/// one, and the only surviving evidence would be a pad that does nothing in a
/// game.
///
/// It is a backup rather than a refusal because the collision is usually the
/// user re-doing the setup they already did, and refusing that would leave them
/// with no way forward that is not a shell. The road back is the one the mapper
/// already renders: `MapperSlot::backup` reads these files and
/// `/map/preset/restore` puts one back — `FIRST-RUN.md` §6's "the way out of a
/// mistake is never a shell command", satisfied by a button that already
/// exists.
pub fn apply(store: &Store, spec: &CommitSpec) -> Result<Committed, ksx_config::ConfigError> {
    let mut preset_backups = Vec::new();
    let mut presets = Vec::with_capacity(spec.slots.len());
    for preset in preset_files(spec) {
        // `preset_path` resolves to the file this store would actually write —
        // the `.json` interop spelling when that is the only one on disk — so
        // the backup and the overwrite cannot land on two different files.
        if let Some(existing) = store
            .preset_path(&preset.name)
            .ok()
            .filter(|path| path.exists())
        {
            preset_backups.extend(store.backup(&existing)?);
        }
        presets.push(store.save_preset(&preset)?);
    }

    let base = store.load_config()?.value;
    let config = to_config(&base, spec);
    let path = store.root().config_path();
    let backup = store.backup(&path)?;
    let config_path = store.save_config(&config)?;

    Ok(Committed {
        config: config_path,
        backup,
        presets,
        preset_backups,
        alias: spec.device.alias.clone(),
        slots: spec.slots.iter().map(|s| s.spec.number).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_config::ConfigRoot;
    use ksx_core::key::Key;
    use ksx_core::pad::XButton;
    use ksx_core::preset::Binding;
    use ksx_core::stage::{StagedDevice, StagedSetup};
    use ksx_core::{Blocking, DeviceSelector, Persona, Preset};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ksx-stage-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn store(&self) -> Store {
            let store = Store::new(ConfigRoot::at(&self.0));
            store.save_config(&ConfigFile::default()).unwrap();
            store
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn device() -> StagedDevice {
        StagedDevice {
            selector: DeviceSelector::parse("usb:d209:0430:00").unwrap(),
            alias: "panel".to_owned(),
            label: "Ultimarc I-PAC 4".to_owned(),
        }
    }

    fn preset(name: &str, key: Key, button: XButton) -> Preset {
        Preset {
            name: name.to_owned(),
            entries: vec![(key, Binding::Button(button))],
            chords: Vec::new(),
            macros: Default::default(),
            turbo: Vec::new(),
            protected: false,
        }
    }

    /// Two players on one board: an Xbox pad and a PlayStation pad, split
    /// keyboard, which is the shape §3's second answer exists for.
    fn staged() -> StagedSetup {
        StagedSetup::new()
            .choose_device(device())
            .unwrap()
            .add_slot(1, Persona::Xbox360, preset("Player 1", Key::A, XButton::A))
            .unwrap()
            .add_slot(
                2,
                Persona::PlayStation,
                preset("Player 2", Key::L, XButton::B),
            )
            .unwrap()
            .set_blocking(Blocking::BoundKeys)
    }

    /// Everything about a plan a staged setup decides, as one comparable
    /// value. Deliberately not `render_human`: that report omits the persona,
    /// which is the field the whole staging flow is about.
    fn fingerprint(plan: &RunPlan) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "block={} mice={} captureable={:?} winusb={:?} interception={}",
            plan.block_keyboards.as_str(),
            plan.block_mice,
            plan.captureable
                .iter()
                .map(|d| d.as_str())
                .collect::<Vec<_>>(),
            plan.winusb.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            plan.needs_interception(),
        );
        for slot in &plan.slots {
            let _ = writeln!(
                out,
                "slot {} persona={} preset={} keyboard={:?} socd={} macros={} entries={:?}",
                slot.spec.number,
                slot.spec.persona,
                slot.spec.preset,
                slot.spec.keyboard.as_ref().map(|k| k.as_str()),
                slot.spec.socd.as_str(),
                slot.spec.macros.is_on(),
                slot.preset.entries,
            );
        }
        out
    }

    /// **§2's requirement, pinned: what is staged is what plays.**
    ///
    /// The plan built from the staged setup with no disk anywhere near it, and
    /// the plan built from the bytes a save left behind, must be the same plan.
    ///
    /// Breaks against two versions that were both easy to write:
    ///
    /// 1. a save that fed [`crate::slots::assign`] — the existing `[[slot]]`
    ///    writer — which creates a slot with `keyboard: None`. The staged plan
    ///    has two slots; the saved one has ZERO, because `build_plan` drops a
    ///    slot with no input device as `NoInputDeviceSelected`. A cabinet that
    ///    played correctly before Save and was dead after it;
    /// 2. any save that dropped the persona or the split-or-freeze answer:
    ///    slot 2 comes back as an Xbox pad (`Persona::default()`) and the
    ///    keyboard comes back frozen, so the desk keyboard that was still
    ///    typing during Play stops typing after a restart.
    #[test]
    fn what_is_staged_is_what_plays() {
        let root = TempRoot::new("same-plan");
        let store = root.store();
        let spec = staged().commit().expect("a device and two controllers");

        // Play — no file was read and none was written to build this.
        let live = plan(&spec).expect("a staged setup plans");

        // Save, then plan the way a fresh `ksx run` would: from the bytes.
        apply(&store, &spec).expect("a save");
        let config = store.load_config().unwrap().value;
        let presets = store.load_presets().unwrap().value;
        let saved = build_plan(&config, &GamesFile::default(), &presets, None)
            .expect("the saved config plans");

        assert_eq!(
            fingerprint(&live),
            fingerprint(&saved),
            "a saved setup must mean exactly what the screen showed"
        );
        // ...and it is not two empty plans agreeing about nothing.
        assert_eq!(live.slots.len(), 2);
        assert_eq!(live.slots[1].spec.persona, Persona::PlayStation);
        assert_eq!(live.block_keyboards, Blocking::BoundKeys);
        assert_eq!(live.captureable.len(), 1, "one board, two slots");
    }

    /// Play writes nothing. Not "writes the same thing" — nothing.
    ///
    /// Breaks against a play path that went through `apply` first (the obvious
    /// shortcut: save, then plan from disk). That version makes moment 7's
    /// "saving and playing are separate acts" false, and a user who pressed
    /// Play to try something has a config.toml they never asked for.
    #[test]
    fn playing_leaves_the_config_untouched() {
        let root = TempRoot::new("play-writes-nothing");
        let store = root.store();
        let before = std::fs::read_to_string(store.root().config_path()).unwrap();
        let listing_before = std::fs::read_dir(&root.0).unwrap().count();

        let spec = staged().commit().unwrap();
        let plan = plan(&spec).unwrap();
        assert_eq!(plan.slots.len(), 2);

        assert_eq!(
            std::fs::read_to_string(store.root().config_path()).unwrap(),
            before,
            "playing must not move a byte of config.toml"
        );
        assert_eq!(
            std::fs::read_dir(&root.0).unwrap().count(),
            listing_before,
            "and must not leave a preset file or a backup behind"
        );
    }

    /// A save lands the `[[device]]`, the `[[slot]]` rows with their personas,
    /// the presets and the blocking answer — in one config write, behind one
    /// backup.
    #[test]
    fn a_save_writes_the_device_the_slots_the_presets_and_the_answer() {
        let root = TempRoot::new("save");
        let store = root.store();
        let spec = staged().commit().unwrap();

        let committed = apply(&store, &spec).unwrap();
        assert_eq!(committed.slots, vec![1, 2]);
        assert_eq!(committed.presets.len(), 2);
        assert!(committed.backup.is_some(), "a whole-file write backs up");
        assert!(
            committed.message().contains("nothing was claimed"),
            "{}",
            committed.message()
        );

        let text = std::fs::read_to_string(store.root().config_path()).unwrap();
        assert!(text.contains("alias = \"panel\""), "{text}");
        assert!(text.contains("id = \"usb:d209:0430:00\""), "{text}");
        assert!(text.contains("keyboard = \"panel\""), "{text}");
        assert!(text.contains("persona = \"playstation\""), "{text}");
        assert!(text.contains("block_keyboards = \"bound-keys\""), "{text}");

        // The presets are real files the mapper can go on editing.
        let names: Vec<String> = store
            .load_presets()
            .unwrap()
            .value
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert!(names.contains(&"Player 1".to_owned()), "{names:?}");
        assert!(names.contains(&"Player 2".to_owned()), "{names:?}");
    }

    /// A returning user's other players survive a re-stage, and re-picking the
    /// same board does not grow a second `[[device]]`.
    ///
    /// Breaks against a `to_config` that replaced the whole `[[slot]]` table:
    /// staging player 1 again would delete players 3 and 4 off a working
    /// cabinet, with a Save button as the only warning.
    #[test]
    fn saving_replaces_the_staged_slots_and_leaves_the_others_alone() {
        let root = TempRoot::new("merge");
        let store = root.store();

        // A cabinet that already has a player 4 and a device entry.
        let mut base = ConfigFile::default();
        base.devices.push(DeviceEntry {
            id: "usb:d209:0430:00".parse().unwrap(),
            alias: "panel".into(),
            backend: ksx_config::Backend::Winusb,
        });
        base.slots.push(SlotEntry {
            number: 4,
            keyboard: Some("panel".into()),
            mouse: None,
            preset: "Player 4".into(),
            persona: Persona::PlayStation,
            socd: Default::default(),
            macros: Default::default(),
        });
        store.save_config(&base).unwrap();

        let spec = staged().commit().unwrap();
        apply(&store, &spec).unwrap();

        let after = store.load_config().unwrap().value;
        assert_eq!(after.devices.len(), 1, "the same board, not a second entry");
        assert_eq!(
            after.devices[0].backend,
            ksx_config::Backend::Winusb,
            "staging cannot know a board is claimed, so it must not un-say it"
        );
        let numbers: Vec<u8> = after.slots.iter().map(|s| s.number).collect();
        assert_eq!(numbers, vec![1, 2, 4], "player 4 is still there");
        assert_eq!(after.slots[2].preset, "Player 4");
    }

    /// **A preset the save would replace is copied first.**
    ///
    /// Breaks against the shipped `apply`, which called `Store::save_preset`
    /// straight — an atomic overwrite with no backup of its own. The staging
    /// flow offers "Player 1" as the FIRST preset name it suggests
    /// (`ksx_api::preset_name_for_slot`), so the second visit through the
    /// journey silently replaced the mapped preset from the first one with an
    /// empty table. The only symptom is a pad that does nothing in a game, and
    /// by then there is no file left to compare against.
    ///
    /// The assertion is on the CONTENTS of the backup, not merely on its
    /// existence: a copy taken after the write would satisfy "a .bak exists"
    /// and preserve nothing.
    #[test]
    fn a_save_copies_a_preset_it_is_about_to_replace() {
        let root = TempRoot::new("preset-backup");
        let store = root.store();

        // What the user mapped on their first visit.
        let mapped = PresetFile::from_core(&preset("Player 1", Key::Z, XButton::X));
        store.save_preset(&mapped).unwrap();
        let before = std::fs::read_to_string(store.preset_path("Player 1").unwrap()).unwrap();

        // The second visit stages a DIFFERENT "Player 1" — a fresh layout,
        // under the name the flow offers first — and saves it. (An empty one
        // cannot get this far any more: `commit()` refuses a controller that
        // binds nothing. The collision is the same either way, and this is the
        // shape a second visit really has.)
        let spec = StagedSetup::new()
            .choose_device(device())
            .unwrap()
            .add_slot(1, Persona::Xbox360, preset("Player 1", Key::Q, XButton::Y))
            .unwrap()
            .set_blocking(Blocking::Whole)
            .commit()
            .unwrap();
        let committed = apply(&store, &spec).unwrap();

        assert_eq!(
            committed.preset_backups.len(),
            1,
            "an existing preset was overwritten with no copy kept: {committed:?}"
        );
        let kept = std::fs::read_to_string(&committed.preset_backups[0]).unwrap();
        assert_eq!(
            kept, before,
            "the copy must be of the file as it was BEFORE the write"
        );
        assert!(kept.contains("Z"), "{kept}");
        // …and the flash says so. A save that reported only "saved" while it
        // had replaced a mapped preset is a screen reporting a success that is
        // not the whole truth.
        let said = committed.message();
        assert!(said.contains("REPLACED"), "{said}");
        assert!(said.contains("Restore backup"), "{said}");

        // ...and a preset that did NOT exist leaves no backup behind, so a
        // first run does not litter the presets folder with copies of nothing.
        let root = TempRoot::new("preset-no-backup");
        let store = root.store();
        let committed = apply(&store, &spec).unwrap();
        assert!(
            committed.preset_backups.is_empty(),
            "{committed:?} backed up a file that was not there"
        );
    }

    /// The device is written as the SELECTOR the stage held, never as a raw
    /// instance path.
    ///
    /// Breaks against a commit path that reached for whatever devnode a scan
    /// had reported: that entry pins the cabinet to one USB socket, and it is
    /// the string `FIRST-RUN.md` §5 bans from ever being the identifier.
    #[test]
    fn the_saved_device_id_is_the_selector_and_survives_a_replug() {
        let root = TempRoot::new("selector");
        let store = root.store();
        apply(&store, &staged().commit().unwrap()).unwrap();

        let entry = &store.load_config().unwrap().value.devices[0];
        assert_eq!(entry.id.raw(), "usb:d209:0430:00");
        assert!(!entry.id.raw().contains('\\'), "never a raw path");
        assert!(entry.id.selector().survives_replug());
        assert_eq!(entry.id.selector().rung(), "model");
    }
}
