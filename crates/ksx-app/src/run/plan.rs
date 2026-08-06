//! Config → runnable plan.
//!
//! One pure function ([`build_plan`]) turns already-loaded config/games/presets
//! into everything `ksx run` needs: resolved slots (device ids + real presets),
//! block flags, and the exact set of devices that may ever be captured. All the
//! refusal policy lives here so `--dry-run` and a live run agree by
//! construction.
//!
//! Refusal rules (risk review §3 item 3 — blocking scope is not negotiable):
//! - `ksx_config::validate` issues ⇒ refuse to start, print them, exit 2.
//!   Loading is lenient; *starting emulation* is not.
//! - only devices bound to a slot may be captured, and in M4 only keyboards:
//!   the mouse class filter is never set, so a slot's mouse is routed but never
//!   blocked.
//! - a slot with no input device at all is dropped with
//!   [`InvalidationReason::NoInputDeviceSelected`] rather than silently
//!   plugging a pad nothing can drive.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ksx_config::{
    validate, validate_games, ConfigFile, ConfigRoot, GamesFile, Issue, PresetFile, Store,
};
use ksx_core::{DeviceId, InvalidationReason, Preset, ResolvedSlot};

/// Where the slot layout came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanSource {
    /// `[[slot]]` entries in the main config file.
    Config,
    /// A `[[game]]` profile from `games.toml` (`ksx run --game <Title>`).
    Game(String),
}

impl PlanSource {
    pub fn label(&self) -> String {
        match self {
            PlanSource::Config => "config".to_owned(),
            PlanSource::Game(title) => format!("game profile '{title}'"),
        }
    }
}

/// A fully resolved, runnable configuration.
#[derive(Clone, Debug)]
pub struct RunPlan {
    pub source: PlanSource,
    pub config_path: PathBuf,
    /// Sorted by slot number; every entry has at least one input device.
    pub slots: Vec<ResolvedSlot>,
    pub block_keyboards: bool,
    /// Parsed and reported, but never acted on in M4 (see module docs).
    pub block_mice: bool,
    /// Distinct keyboards bound to slots — exactly the `SetCaptured` set.
    pub captureable: Vec<DeviceId>,
    /// The subset of [`Self::captureable`] whose `[[device]]` entry selects
    /// `backend = "winusb"` (M6).
    ///
    /// Backend choice is *per device*, so a run can need one claimed WinUSB
    /// interface per rebound board **and** an Interception context for whatever
    /// is still on the keyboard stack. Resolving it here, in the pure planner,
    /// is what lets `--dry-run` tell you which backends a session would touch
    /// before it touches one.
    pub winusb: Vec<DeviceId>,
    /// Non-fatal findings worth printing (dropped slots, ignored flags,
    /// lenient-load warnings).
    pub notes: Vec<String>,
}

impl RunPlan {
    /// Does this plan need an Interception context at all?
    ///
    /// `false` is the M6 exit state: every bound board is claimed through
    /// WinUSB, so the run never loads the end-of-life driver — which is the
    /// whole point of the milestone, and the thing the cabinet gate checks.
    pub fn needs_interception(&self) -> bool {
        self.captureable.iter().any(|id| !self.winusb.contains(id))
    }

    /// Slot numbers driven by `device`, in slot order.
    pub fn slots_using(&self, device: &DeviceId) -> Vec<u8> {
        self.slots
            .iter()
            .filter(|s| {
                s.spec.keyboard.as_ref() == Some(device) || s.spec.mouse.as_ref() == Some(device)
            })
            .map(|s| s.spec.number)
            .collect()
    }
}

/// One `[[game]]` profile, as the "nothing to run" message needs to describe it.
///
/// The slot count is the part that matters: a profile with no `[[game.slot]]`
/// cannot be suggested as a way out, and recommending it would hand the user a
/// second identical error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSummary {
    pub title: String,
    pub slots: usize,
}

impl ProfileSummary {
    fn of(games: &GamesFile) -> Vec<Self> {
        games
            .games
            .iter()
            .map(|g| Self {
                title: g.title.clone(),
                slots: g.slots.len(),
            })
            .collect()
    }
}

/// Why a plan could not be built. Everything here is an exit-code-2 refusal:
/// nothing was plugged and no filter was set.
#[derive(Debug)]
pub enum PlanError {
    /// `validate`/`validate_games` findings — the config is not startable.
    Issues(Vec<Issue>),
    /// `--game <Title>` matched nothing in `games.toml`.
    UnknownGame { title: String, known: Vec<String> },
    /// The plan resolved to zero usable slots.
    ///
    /// Carries what the user needs to get past it, not just what went wrong: on
    /// this cabinet the slots live in `games.toml` profiles and `config.toml`
    /// has none of its own, so "config defines no usable slot" was *correct* and
    /// completely unactionable. `profiles` is every `[[game]]` title in
    /// `games.toml`, and `invoked_as` is the command to repeat with `--game`.
    NoSlots {
        source: PlanSource,
        profiles: Vec<ProfileSummary>,
        /// The command the user actually typed (`ksx run` / `ksx daemon`), so
        /// the suggestion is copy-pasteable rather than approximately right.
        invoked_as: &'static str,
    },
    /// A slot names a preset that is neither a file nor a built-in.
    UnknownPreset { slot: u8, preset: String },
    /// A slot's device reference or preset body could not be resolved.
    Config(ksx_config::ConfigError),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::Issues(issues) => {
                writeln!(
                    f,
                    "refusing to start: {} configuration problem(s) must be fixed first",
                    issues.len()
                )?;
                for issue in issues {
                    writeln!(f, "  [FAIL] {issue}")?;
                }
                Ok(())
            }
            PlanError::UnknownGame { title, known } => {
                write!(f, "no game profile titled '{title}' in games.toml")?;
                if known.is_empty() {
                    write!(f, " (the file has no [[game]] entries)")
                } else {
                    write!(f, "; known titles: {}", known.join(", "))
                }
            }
            PlanError::NoSlots {
                source,
                profiles,
                invoked_as,
            } => render_no_slots(f, source, profiles, invoked_as),
            PlanError::UnknownPreset { slot, preset } => write!(
                f,
                "slot {slot} references preset '{preset}', which is neither a preset file \
                 nor a built-in"
            ),
            PlanError::Config(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// The "nothing to run" message.
///
/// The old text — *"config defines no usable slot (a slot needs a number, a
/// preset, and at least one input device)"* — was true and useless: the
/// cabinet's `config.toml` has no `[[slot]]` at all because its slots live in
/// `games.toml` profiles, so the message described a file the user had never
/// filled in and said nothing about the file they had. This one names the file
/// that is empty, lists what *is* configured, and ends with a command to run.
fn render_no_slots(
    f: &mut std::fmt::Formatter<'_>,
    source: &PlanSource,
    profiles: &[ProfileSummary],
    invoked_as: &str,
) -> std::fmt::Result {
    // A profile the user explicitly asked for is a different problem: they named
    // the right file, it is just empty. No profile list helps there.
    if let PlanSource::Game(title) = source {
        return write!(
            f,
            "game profile '{title}' defines no usable slot (a slot needs a number, a preset, \
             and at least one input device). Add a [[game.slot]] to its entry in games.toml, \
             or run without --game to use config.toml's own [[slot]] layout"
        );
    }

    writeln!(
        f,
        "config.toml defines no [[slot]] (a slot needs a number, a preset, and at least one \
         input device), so there is nothing to run without --game."
    )?;

    let runnable: Vec<&ProfileSummary> = profiles.iter().filter(|p| p.slots > 0).collect();
    if profiles.is_empty() {
        writeln!(
            f,
            "games.toml has no [[game]] profiles either, so ksx has no slot layout from any \
             source."
        )?;
        writeln!(
            f,
            "  - coming from the legacy KeyboardSplitter? `ksx import-legacy` converts its \
             splitter_games.xml / splitter_presets.xml into both files;"
        )?;
        return write!(
            f,
            "  - starting fresh? add a [[slot]] to config.toml. `ksx devices` prints the \
             keyboard ids to paste into it."
        );
    }
    if runnable.is_empty() {
        writeln!(
            f,
            "games.toml has {} profile(s), but none of them define a [[game.slot]] either:",
            profiles.len()
        )?;
        for p in profiles {
            writeln!(f, "  - {}", p.title)?;
        }
        return write!(
            f,
            "Add a [[game.slot]] to one of them, or a [[slot]] to config.toml. \
             `ksx devices` prints the keyboard ids to paste in."
        );
    }

    writeln!(
        f,
        "These {} game profile(s) in games.toml do define slots:",
        runnable.len()
    )?;
    for p in &runnable {
        writeln!(f, "  - {} ({} slot(s))", p.title, p.slots)?;
    }
    write!(
        f,
        "Run one of them, for example:\n    {invoked_as} --game \"{}\"",
        runnable[0].title
    )
}

impl From<ksx_config::ConfigError> for PlanError {
    fn from(err: ksx_config::ConfigError) -> Self {
        PlanError::Config(err)
    }
}

/// What [`build_plan`] assumes it was called for. [`resolve_as`] corrects it.
const DEFAULT_INVOCATION: &str = "ksx run";

impl PlanError {
    /// Re-label the suggested command for the command that is actually running.
    ///
    /// `ksx daemon` must suggest `ksx daemon --game "…"`, not `ksx run --game
    /// "…"`: a user who pastes the suggestion gets a foreground session that
    /// ends when they close it, which is not what they asked for, and they will
    /// reasonably assume the daemon cannot do it.
    fn invoked_as(mut self, command: &'static str) -> Self {
        if let PlanError::NoSlots { invoked_as, .. } = &mut self {
            *invoked_as = command;
        }
        self
    }
}

/// Load everything under `root` and build the plan, as `ksx run`.
pub fn resolve(root: &ConfigRoot, game: Option<&str>) -> Result<RunPlan, PlanError> {
    resolve_as(root, game, DEFAULT_INVOCATION)
}

/// [`resolve`], for a caller that is not `ksx run`.
///
/// `invoked_as` is the command name to print in any suggestion — the only thing
/// it affects.
pub fn resolve_as(
    root: &ConfigRoot,
    game: Option<&str>,
    invoked_as: &'static str,
) -> Result<RunPlan, PlanError> {
    let store = Store::new(root.clone());
    let config = store.load_config()?;
    let presets = store.load_presets()?;
    let games = store.load_games()?;

    let mut notes: Vec<String> = Vec::new();
    for warning in config
        .warnings
        .iter()
        .chain(&presets.warnings)
        .chain(&games.warnings)
    {
        notes.push(format!("[WARN] {warning}"));
    }

    let mut plan = build_plan(&config.value, &games.value, &presets.value, game)
        .map_err(|err| err.invoked_as(invoked_as))?;
    plan.config_path = root.config_path();
    notes.extend(std::mem::take(&mut plan.notes));
    plan.notes = notes;
    Ok(plan)
}

/// The pure core: no filesystem, no clock, no drivers.
pub fn build_plan(
    config: &ConfigFile,
    games: &GamesFile,
    presets: &[PresetFile],
    game: Option<&str>,
) -> Result<RunPlan, PlanError> {
    // Config-wide validation always runs: a broken `[[device]]` table or preset
    // is a problem no matter which slot layout we are about to use.
    let mut issues = validate(config, presets);

    let (source, specs, block_keyboards, block_mice) = match game {
        Some(title) => {
            let Some(entry) = games.games.iter().find(|g| g.title == title) else {
                return Err(PlanError::UnknownGame {
                    title: title.to_owned(),
                    known: games.games.iter().map(|g| g.title.clone()).collect(),
                });
            };
            // Only this profile's findings block the run — an unrelated broken
            // game entry must not stop the one the user asked for.
            issues.extend(
                validate_games(games, presets)
                    .into_iter()
                    .filter(|i| game_title_of(i) == Some(title)),
            );
            let specs = entry
                .slots
                .iter()
                .map(|s| s.to_spec())
                .collect::<Result<Vec<_>, _>>()?;
            (
                PlanSource::Game(title.to_owned()),
                specs,
                entry.block_keyboards,
                entry.block_mice,
            )
        }
        None => {
            let specs = config
                .slots
                .iter()
                .map(|s| config.slot_spec(s))
                .collect::<Result<Vec<_>, _>>()?;
            (
                PlanSource::Config,
                specs,
                config.settings.block_keyboards,
                config.settings.block_mice,
            )
        }
    };

    // Advice is not a fault: a chord layered over already-bound keys works
    // exactly as written and only costs the documented flash, so it warns and
    // the session starts (`Issue::is_advisory`).
    let (advisories, issues): (Vec<_>, Vec<_>) =
        issues.into_iter().partition(ksx_config::Issue::is_advisory);
    if !issues.is_empty() {
        return Err(PlanError::Issues(issues));
    }

    let mut notes: Vec<String> = advisories
        .iter()
        .map(|advisory| format!("[WARN] {advisory}"))
        .collect();
    let mut slots = Vec::new();
    for spec in specs {
        if spec.keyboard.is_none() && spec.mouse.is_none() {
            notes.push(format!(
                "[WARN] slot {} skipped: {}",
                spec.number,
                InvalidationReason::NoInputDeviceSelected.explanation()
            ));
            continue;
        }
        if spec.keyboard.is_none() {
            notes.push(format!(
                "[WARN] slot {} has only a mouse; M4 never sets the mouse class filter, so \
                 that device is routed but never blocked",
                spec.number
            ));
        }
        let mut preset = resolve_preset(presets, spec.number, &spec.preset)?;
        // SOCD cleaning is generated HERE, once, onto the resolved preset —
        // it is chords, not an engine rule (docs/INPUT-TRANSFORMS.md §2.6).
        // `socd = "off"` (the default) generates nothing, so this line is a
        // no-op for every configuration that predates the feature.
        preset.apply_socd(spec.socd);
        slots.push(ResolvedSlot { spec, preset });
    }
    slots.sort_by_key(|s| s.spec.number);

    if slots.is_empty() {
        return Err(PlanError::NoSlots {
            source,
            // The profiles are listed even when the failure is about
            // config.toml, because on a cabinet imported from legacy that is
            // where every slot actually lives — and "there is nothing to run"
            // beside a games.toml full of profiles is the exact unhelpfulness
            // this replaces.
            profiles: ProfileSummary::of(games),
            invoked_as: DEFAULT_INVOCATION,
        });
    }

    if block_mice {
        notes.push(
            "[WARN] block_mice is set but ignored in M4: ksx never touches mouse.sys \
             (design cut list — mouse mapping lands later)"
                .to_owned(),
        );
    }
    if !block_keyboards {
        notes.push(
            "[INFO] block_keyboards is false: pads are driven, but assigned keyboards keep \
             typing into Windows as well"
                .to_owned(),
        );
    }

    // Deduplicate while keeping slot order — one I-PAC feeding four slots must
    // appear exactly once in the capture set.
    let mut seen = BTreeSet::new();
    let captureable: Vec<DeviceId> = slots
        .iter()
        .filter_map(|s| s.spec.keyboard.clone())
        .filter(|id| seen.insert(id.clone()))
        .collect();

    // Backend selection is a property of the *device*, not of the slot layout,
    // so it comes from `[[device]]` in both the config and the `--game` path.
    let winusb: Vec<DeviceId> = captureable
        .iter()
        .filter(|id| {
            config
                .devices
                .iter()
                .any(|d| d.id == id.as_str() && d.backend == ksx_config::Backend::Winusb)
        })
        .cloned()
        .collect();

    Ok(RunPlan {
        source,
        config_path: PathBuf::new(),
        slots,
        block_keyboards,
        block_mice,
        captureable,
        winusb,
        notes,
    })
}

/// Preset files first, then the two built-ins (`default`, `empty`).
fn resolve_preset(presets: &[PresetFile], slot: u8, name: &str) -> Result<Preset, PlanError> {
    if let Some(file) = presets.iter().find(|p| p.name == name) {
        return Ok(file.to_core()?);
    }
    // `validate` already reports this as UnknownPresetRef for config slots, so
    // this branch is normally unreachable; it is the real check for game-slot
    // presets and for anything validation might miss.
    Preset::builtins()
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| PlanError::UnknownPreset {
            slot,
            preset: name.to_owned(),
        })
}

fn game_title_of(issue: &Issue) -> Option<&str> {
    match issue {
        Issue::GameSlotNumberOutOfRange { game, .. }
        | Issue::GameDuplicateSlotNumber { game, .. }
        | Issue::GameUnknownPresetRef { game, .. }
        | Issue::GameUserIndexOutOfRange { game, .. } => Some(game),
        _ => None,
    }
}

/// Human `--dry-run` report. Pure: same plan, same text, any platform.
pub fn render_human(plan: &RunPlan) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "plan from {} ({})",
        plan.source.label(),
        plan.config_path.display()
    );
    let _ = writeln!(
        out,
        "  block keyboards: {}    block mice: {} (never applied in M4)",
        yes_no(plan.block_keyboards),
        yes_no(plan.block_mice)
    );
    // Which capture drivers this session would touch. Worth a line of its own:
    // "does this run still load the end-of-life Interception driver" is the
    // question M6 exists to answer, and --dry-run is where you ask it.
    let _ = writeln!(
        out,
        "  backends: {}",
        match (plan.needs_interception(), plan.winusb.is_empty()) {
            (true, true) => "interception".to_owned(),
            (false, false) => format!("winusb ({} claimed board(s))", plan.winusb.len()),
            (true, false) => format!(
                "mixed - winusb ({} board(s)) + interception for the rest",
                plan.winusb.len()
            ),
            (false, true) => "none (no keyboard bound to a slot)".to_owned(),
        }
    );
    for slot in &plan.slots {
        let keyboard = slot
            .spec
            .keyboard
            .as_ref()
            .map_or("-", |d| d.as_str())
            .to_owned();
        let _ = writeln!(
            out,
            "  slot {}  preset \"{}\" ({} binding(s))  keyboard {keyboard}",
            slot.spec.number,
            slot.preset.name,
            // Chords are bindings too: a preset made only of chords must not
            // print "0 binding(s)".
            slot.preset.entries.len() + slot.preset.chords.len()
        );
        if let Some(mouse) = &slot.spec.mouse {
            let _ = writeln!(out, "           mouse {} (routed, never blocked)", mouse);
        }
        // Macros are the one binding kind you cannot read off a key→function
        // list: what matters is the sequence, how long it takes, and what
        // happens when the player lets go (docs/INPUT-TRANSFORMS.md §1c).
        for (i, mac) in slot.preset.macros.defs.iter().enumerate() {
            let keys: Vec<&str> = slot
                .preset
                .macros
                .keys_for(i as u16)
                .map(|k| k.name())
                .collect();
            let _ = writeln!(
                out,
                "           macro \"{}\" {} step(s), {} ms  on_release={} retrigger={} interrupt={}  key(s) {}",
                mac.name,
                mac.steps.len(),
                mac.total_ms(),
                mac.on_release,
                mac.retrigger,
                mac.interrupt,
                if keys.is_empty() {
                    "-  (defined but nothing starts it)".to_owned()
                } else {
                    keys.join(", ")
                }
            );
        }
    }
    let _ = writeln!(
        out,
        "  devices that may be captured: {}",
        plan.captureable.len()
    );
    for device in &plan.captureable {
        let slots = plan.slots_using(device);
        let _ = writeln!(out, "    {device}  -> slot(s) {slots:?}");
    }
    for note in &plan.notes {
        let _ = writeln!(out, "  {note}");
    }
    out
}

/// `--dry-run --json` object.
pub fn plan_json(plan: &RunPlan) -> serde_json::Value {
    let slots: Vec<serde_json::Value> = plan
        .slots
        .iter()
        .map(|s| {
            serde_json::json!({
                "number": s.spec.number,
                "preset": s.preset.name,
                "bindings": s.preset.entries.len() + s.preset.chords.len(),
                "chords": s.preset.chords.len(),
                "keyboard": s.spec.keyboard.as_ref().map(|d| d.as_str()),
                "mouse": s.spec.mouse.as_ref().map(|d| d.as_str()),
                "macros": s.preset.macros.defs.iter().enumerate().map(|(i, mac)| serde_json::json!({
                    "name": mac.name,
                    "steps": mac.steps.len(),
                    "total_ms": mac.total_ms(),
                    "on_release": mac.on_release.as_str(),
                    "retrigger": mac.retrigger.as_str(),
                    "interrupt": mac.interrupt.as_str(),
                    "keys": s.preset.macros.keys_for(i as u16)
                        .map(|k| k.name())
                        .collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "source": match &plan.source {
            PlanSource::Config => serde_json::json!({"kind": "config"}),
            PlanSource::Game(title) => serde_json::json!({"kind": "game", "title": title}),
        },
        "config_path": plan.config_path.display().to_string(),
        "block_keyboards": plan.block_keyboards,
        "block_mice": plan.block_mice,
        "slots": slots,
        "captureable": plan.captureable.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
        "winusb": plan.winusb.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
        "needs_interception": plan.needs_interception(),
        "notes": plan.notes,
    })
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPAC: &str = r"HID\VID_D209&PID_0430&REV_0056&MI_00";

    fn presets() -> Vec<PresetFile> {
        vec![
            toml::from_str("name = \"IPAC P1\"\n[bindings]\nA = \"S\"\n").unwrap(),
            toml::from_str("name = \"IPAC P2\"\n[bindings]\nB = \"D\"\n").unwrap(),
        ]
    }

    fn config(body: &str) -> ConfigFile {
        toml::from_str(body).unwrap()
    }

    fn games(body: &str) -> GamesFile {
        toml::from_str(body).unwrap()
    }

    /// The cabinet's real shape: one I-PAC, four slots, disjoint presets.
    const CAB_CONFIG: &str = r#"
schema_version = 1

[[device]]
id = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
alias = "cab"

[[slot]]
number = 2
keyboard = "cab"
preset = "IPAC P2"

[[slot]]
number = 1
keyboard = "cab"
preset = "IPAC P1"
"#;

    #[test]
    fn config_slots_resolve_sorted_with_one_shared_device() {
        let plan =
            build_plan(&config(CAB_CONFIG), &GamesFile::default(), &presets(), None).unwrap();
        assert_eq!(plan.source, PlanSource::Config);
        assert_eq!(
            plan.slots.iter().map(|s| s.spec.number).collect::<Vec<_>>(),
            vec![1, 2],
            "slots must be sorted by number regardless of file order"
        );
        assert_eq!(plan.slots[0].preset.name, "IPAC P1");
        // One physical keyboard feeding several slots appears ONCE in the
        // capture set (risk review R6 trap 1).
        assert_eq!(plan.captureable, vec![DeviceId::from(IPAC)]);
        assert_eq!(plan.slots_using(&DeviceId::from(IPAC)), vec![1, 2]);
        assert!(plan.block_keyboards);
        assert!(!plan.block_mice);
    }

    #[test]
    fn game_profile_overrides_slots_and_block_flags() {
        let games = games(
            r#"
[[game]]
title = "Steam"
path = 'C:\steam.exe'
block_keyboards = false
block_mice = true

[[game.slot]]
number = 3
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P2"
"#,
        );
        let plan = build_plan(&config(CAB_CONFIG), &games, &presets(), Some("Steam")).unwrap();
        assert_eq!(plan.source, PlanSource::Game("Steam".into()));
        assert_eq!(
            plan.slots.iter().map(|s| s.spec.number).collect::<Vec<_>>(),
            vec![3],
            "the game's slots replace the config's, they do not merge"
        );
        assert!(!plan.block_keyboards);
        assert!(plan.block_mice);
        assert!(
            plan.notes.iter().any(|n| n.contains("block_mice")),
            "{:?}",
            plan.notes
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("block_keyboards")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn unknown_game_lists_the_known_titles() {
        let games = games("[[game]]\ntitle = \"Steam\"\npath = 'C:\\steam.exe'\n");
        let err = build_plan(&config(CAB_CONFIG), &games, &presets(), Some("Doom")).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("'Doom'"), "{text}");
        assert!(text.contains("Steam"), "{text}");
    }

    #[test]
    fn validation_issues_refuse_the_run() {
        // Slot 1 references a preset that does not exist.
        let cfg = config(
            r#"
schema_version = 1
[[slot]]
number = 1
keyboard = 'HID\X\1'
preset = "nope"
"#,
        );
        let err = build_plan(&cfg, &GamesFile::default(), &presets(), None).unwrap_err();
        let PlanError::Issues(issues) = &err else {
            panic!("expected Issues, got {err:?}");
        };
        assert!(issues
            .iter()
            .any(|i| matches!(i, Issue::UnknownPresetRef { .. })));
        assert!(err.to_string().contains("refusing to start"), "{err}");
    }

    /// A chord layered over already-bound keys is a legitimate choice with a
    /// documented cost. It must WARN and start — refusing to run the cabinet
    /// over a latency note would be the wrong trade
    /// (docs/INPUT-TRANSFORMS.md §1b).
    #[test]
    fn a_chord_flash_advisory_warns_but_still_starts() {
        let presets = vec![
            toml::from_str(
                "name = \"IPAC P1\"\n[bindings]\nA = \"S\"\nrt = { key = \"S\", when = [\"D\"] }\n",
            )
            .unwrap(),
            toml::from_str("name = \"IPAC P2\"\n[bindings]\nB = \"D\"\n").unwrap(),
        ];
        let plan = build_plan(&config(CAB_CONFIG), &GamesFile::default(), &presets, None).unwrap();
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("does not defer input")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn a_broken_unrelated_game_does_not_block_the_chosen_one() {
        let games = games(
            r#"
[[game]]
title = "Broken"
path = 'C:\a.exe'
[[game.slot]]
number = 9
preset = "IPAC P1"

[[game]]
title = "Good"
path = 'C:\b.exe'
[[game.slot]]
number = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P1"
"#,
        );
        // "Broken" has an out-of-range slot number...
        assert!(build_plan(&config(CAB_CONFIG), &games, &presets(), Some("Broken")).is_err());
        // ...which must not stop "Good" from running.
        let plan = build_plan(&config(CAB_CONFIG), &games, &presets(), Some("Good")).unwrap();
        assert_eq!(plan.slots.len(), 1);
    }

    #[test]
    fn slot_without_any_device_is_dropped_with_the_legacy_reason() {
        let cfg = config(
            r#"
schema_version = 1
[[slot]]
number = 1
preset = "IPAC P1"

[[slot]]
number = 2
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P2"
"#,
        );
        let plan = build_plan(&cfg, &GamesFile::default(), &presets(), None).unwrap();
        assert_eq!(
            plan.slots.iter().map(|s| s.spec.number).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("slot 1 skipped")
                && n.contains(InvalidationReason::NoInputDeviceSelected.explanation())),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn empty_layout_is_refused_rather_than_starting_with_nothing() {
        let cfg = config("schema_version = 1\n");
        let err = build_plan(&cfg, &GamesFile::default(), &presets(), None).unwrap_err();
        assert!(matches!(
            err,
            PlanError::NoSlots {
                source: PlanSource::Config,
                ..
            }
        ));
    }

    /// **The cabinet's message.** `config.toml` has no `[[slot]]` because every
    /// slot lives in a `games.toml` profile — so the old text ("config defines
    /// no usable slot") was correct and useless. It must name the empty file,
    /// list what is actually configured, and end with a command that works.
    #[test]
    fn no_slots_names_the_file_lists_the_profiles_and_gives_the_command() {
        let games = games(
            r#"
[[game]]
title = "Steam"
path = 'C:\Program Files (x86)\Steam\steam.exe'
[[game.slot]]
number = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P1"
[[game.slot]]
number = 2
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P2"

[[game]]
title = "MAME"
path = 'C:\mame\mame.exe'
[[game.slot]]
number = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P1"
"#,
        );
        let err =
            build_plan(&config("schema_version = 1\n"), &games, &presets(), None).unwrap_err();
        let text = err.to_string();

        assert!(
            text.contains("config.toml defines no [[slot]]"),
            "the empty file must be named: {text}"
        );
        assert!(text.contains("Steam"), "profiles must be listed: {text}");
        assert!(text.contains("MAME"), "profiles must be listed: {text}");
        assert!(
            text.contains("2 slot(s)"),
            "say how many slots each profile brings: {text}"
        );
        assert!(
            text.contains("ksx run --game \"Steam\""),
            "the exact command must be shown: {text}"
        );

        // ...and `ksx daemon` gets its own command, not `ksx run`.
        let daemon = err.invoked_as("ksx daemon").to_string();
        assert!(
            daemon.contains("ksx daemon --game \"Steam\""),
            "the daemon must suggest itself: {daemon}"
        );
        assert!(!daemon.contains("ksx run --game"), "{daemon}");
    }

    /// Nothing anywhere: say so, and point at the importer rather than listing
    /// an empty set.
    #[test]
    fn no_slots_and_no_profiles_points_at_import_legacy() {
        let err = build_plan(
            &config("schema_version = 1\n"),
            &GamesFile::default(),
            &presets(),
            None,
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("no [[game]] profiles either"), "{text}");
        assert!(text.contains("ksx import-legacy"), "{text}");
        assert!(text.contains("ksx devices"), "{text}");
        assert!(
            !text.contains("--game \""),
            "there is no profile to suggest: {text}"
        );
    }

    /// Profiles that exist but define no slots must not be suggested — running
    /// one would produce the same refusal a second time.
    #[test]
    fn profiles_without_slots_are_listed_but_never_recommended() {
        let games = games(
            r#"
[[game]]
title = "Empty"
path = 'C:\a.exe'
"#,
        );
        let err =
            build_plan(&config("schema_version = 1\n"), &games, &presets(), None).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("none of them define a [[game.slot]]"),
            "{text}"
        );
        assert!(text.contains("Empty"), "{text}");
        assert!(
            !text.contains("--game \"Empty\""),
            "suggesting it would just fail again: {text}"
        );
    }

    /// `--game <Title>` on a profile with no slots is a different problem — the
    /// user named the right file, it is simply empty — and gets its own text.
    #[test]
    fn an_empty_game_profile_is_told_about_game_slot_not_about_other_profiles() {
        let games = games("[[game]]\ntitle = \"Empty\"\npath = 'C:\\a.exe'\n");
        let err = build_plan(
            &config("schema_version = 1\n"),
            &games,
            &presets(),
            Some("Empty"),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("game profile 'Empty'"), "{text}");
        assert!(text.contains("[[game.slot]]"), "{text}");
        assert!(!text.contains("config.toml defines no"), "{text}");
    }

    #[test]
    fn builtin_preset_names_resolve_without_a_file() {
        let cfg = config(
            r#"
schema_version = 1
[[slot]]
number = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "default"
"#,
        );
        let plan = build_plan(&cfg, &GamesFile::default(), &[], None).unwrap();
        assert_eq!(plan.slots[0].preset.name, "default");
        assert!(!plan.slots[0].preset.entries.is_empty());
    }

    #[test]
    fn mouse_only_slot_is_kept_but_never_captureable() {
        let cfg = config(
            r#"
schema_version = 1
[[slot]]
number = 1
mouse = 'HID\VID_046D&PID_C077\1'
preset = "IPAC P1"
"#,
        );
        let plan = build_plan(&cfg, &GamesFile::default(), &presets(), None).unwrap();
        assert_eq!(plan.slots.len(), 1);
        assert!(
            plan.captureable.is_empty(),
            "M4 never captures a mouse — mouse.sys stays untouched"
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("only a mouse")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn rendering_covers_human_and_json() {
        let plan =
            build_plan(&config(CAB_CONFIG), &GamesFile::default(), &presets(), None).unwrap();
        let text = render_human(&plan);
        assert!(text.contains("slot 1"), "{text}");
        assert!(text.contains("IPAC P1"), "{text}");
        assert!(text.contains(IPAC), "{text}");

        let v = plan_json(&plan);
        assert_eq!(
            v.pointer("/source/kind"),
            Some(&serde_json::json!("config"))
        );
        assert_eq!(v.pointer("/slots/0/number"), Some(&serde_json::json!(1)));
        assert_eq!(
            v.pointer("/slots/0/keyboard"),
            Some(&serde_json::json!(IPAC))
        );
        assert_eq!(
            v.pointer("/captureable/0"),
            Some(&serde_json::json!(IPAC)),
            "{v}"
        );
    }
}
