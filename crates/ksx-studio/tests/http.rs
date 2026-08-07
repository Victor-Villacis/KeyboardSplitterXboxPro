//! Full HTTP round trips against the real server: GET / with the session
//! panel, and the POST → 303 → flash loop. Raw `TcpStream` HTTP/1.1 on
//! purpose — no client dependency, and what a browser sends is exactly this.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Refusals are typed now (docs/M9-DECISION.md §6): a fake daemon refuses with
/// the same `Refusal` a real one does, so what the page renders here is what it
/// renders in the field.
use ksx_api::Refusal;
use ksx_studio::{
    BindConflict, BindOutcome, BindRequest, ControlSource, LearnView, MacroOutcome, MacroSnapshot,
    MacroStepView, MacroView, MacroWrite, MapperSlot, MapperSnapshot, PadRow, ProfileRow,
    RestoreMode, SessionView, StatusSnapshot, StatusSource,
};

/// The "nothing answers the pipe" refusal a real `PipeTransport` produces —
/// code, sentence and the way out, so the page under test sees exactly what a
/// cabinet sees.
fn no_channel(message: &str) -> Refusal {
    Refusal::with_remedy(ksx_api::codes::NO_CHANNEL, message, "ksx daemon")
}

struct FixedStatus;

/// The newest timestamped backup this preset has, as `collect_mapper` reads
/// it off disk — the label the mapper's third restore button wears.
const BACKUP_LABEL: &str = "2026-08-05 14:32:07 UTC";

impl StatusSource for FixedStatus {
    fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "test".into(),
            vigem: "installed".into(),
            interception: "installed".into(),
            daemon_running: true,
            daemon_detail: "test".into(),
            autostart: "not registered".into(),
            pads: vec![PadRow {
                persona: "Xbox 360 pad".into(),
                instance: "USB\\TEST\\1".into(),
            }],
            profiles: vec![ProfileRow {
                title: "Street Fighter".into(),
                detail: "C:\\sf.exe — 2 slots".into(),
            }],
            config_root: "C:\\cfg".into(),
        }
    }

    fn mapper(&self) -> MapperSnapshot {
        let mut bindings = std::collections::BTreeMap::new();
        bindings.insert("A".to_owned(), vec!["G".to_owned()]);
        // MANY KEYS → ONE CONTROL, exactly as a preset file can hold it
        // (docs/INPUT-TRANSFORMS.md §1a) — the shape Victor's imported preset
        // already had, and the one the add/remove-one routes are computed
        // against.
        bindings.insert("B".to_owned(), vec!["S".to_owned(), "Enter".to_owned()]);
        MapperSnapshot {
            generated_at: "test".into(),
            source: "slots of profile \"Steam\" (games.toml)".into(),
            profile: Some("Steam".into()),
            config_root: "C:\\cfg".into(),
            slots: vec![MapperSlot {
                number: 1,
                persona: "xbox360".into(),
                persona_label: "Xbox 360".into(),
                preset: "IPAC P1".into(),
                keyboard: "HID\\TEST".into(),
                bindings,
                backup: Some(BACKUP_LABEL.to_owned()),
                // One AUTO-FIRING control, so the legend badge is covered by
                // the ordinary page assertions.
                turbo: std::collections::BTreeMap::from([("B".to_owned(), 12)]),
                macros_off: false,
            }],
        }
    }

    /// The preset's `[macros]` tables, in the file's own shape — what
    /// ksx-app's collector reads off disk (`ms` and `frames` kept apart, the
    /// `macro.<name>` rows resolved into `triggers`).
    fn macros(&self, preset: &str) -> MacroSnapshot {
        MacroSnapshot::read(
            preset,
            vec![MacroView {
                name: "hadouken".into(),
                steps: vec![
                    MacroStepView {
                        hold: vec!["dpad.down".into()],
                        ms: Some(50),
                        frames: None,
                        allow_short: false,
                    },
                    MacroStepView {
                        hold: vec!["A".into()],
                        ms: None,
                        frames: Some(3),
                        allow_short: false,
                    },
                ],
                on_release: "finish".into(),
                retrigger: "ignore".into(),
                interrupt: "none".into(),
                repeat: "once".into(),
                turbo_hz: None,
                gap_ms: None,
                triggers: vec!["P".into()],
                disabled: false,
            }],
        )
    }
}

/// Scriptable control: session() flips between idle and running; start()
/// records the profile it was given and either succeeds or refuses.
struct ScriptedControl {
    running: AtomicBool,
    refuse_start: bool,
    /// Every ControlSource call fails the way an absent daemon fails.
    no_daemon: bool,
    started_with: std::sync::Mutex<Option<Option<String>>>,
    learning: AtomicBool,
    bound_with: std::sync::Mutex<Option<BindRequest>>,
    restored_with: std::sync::Mutex<Option<(String, String)>>,
    cleared: std::sync::Mutex<Option<String>>,
    saved_macro: std::sync::Mutex<Option<MacroWrite>>,
}

impl ScriptedControl {
    fn new(refuse_start: bool) -> Self {
        Self {
            running: AtomicBool::new(false),
            refuse_start,
            no_daemon: false,
            started_with: std::sync::Mutex::new(None),
            learning: AtomicBool::new(false),
            bound_with: std::sync::Mutex::new(None),
            restored_with: std::sync::Mutex::new(None),
            cleared: std::sync::Mutex::new(None),
            saved_macro: std::sync::Mutex::new(None),
        }
    }

    /// Nothing answers the pipe — the state Victor hit when he quit the tray
    /// daemon and then clicked around /map.
    fn dead() -> Self {
        Self {
            no_daemon: true,
            ..Self::new(true)
        }
    }
}

/// What every verb says when there is no daemon, matching the real
/// PipeControlSource's `NO_CHANNEL`.
const NO_CHANNEL: &str = "no daemon control channel — start the daemon (tray, or `ksx daemon`)";

impl ControlSource for ScriptedControl {
    /// The verb `/setup`'s step 2 performs. `restarted` is TRUE, because the
    /// real one bounces the pads — the flash has to say so, and a fake that
    /// quietly said otherwise would let that regression ship.
    fn assign_slot(&self, request: &ksx_api::SlotAssignRequest) -> ksx_api::SlotOutcome {
        if self.no_daemon {
            return ksx_api::SlotOutcome::failed(NO_CHANNEL, "ksx daemon");
        }
        if request.preset != "IPAC P1" {
            return ksx_api::SlotOutcome {
                ok: false,
                error: Some(format!("no preset named \"{}\" on disk", request.preset)),
                code: Some(ksx_api::codes::UNKNOWN_PRESET.into()),
                ..ksx_api::SlotOutcome::default()
            };
        }
        ksx_api::SlotOutcome {
            ok: true,
            message: Some(format!(
                "slot {} now uses \"{}\".",
                request.slot, request.preset
            )),
            slot: Some(request.slot),
            preset: Some(request.preset.clone()),
            profile: request.profile.clone(),
            restarted: request.reload,
            reloaded: request.reload,
            ..ksx_api::SlotOutcome::default()
        }
    }

    fn session(&self) -> SessionView {
        if self.no_daemon {
            // The profile still comes from the config, so the banner can print
            // a command that actually starts THIS cabinet.
            return SessionView {
                profile: Some("Steam".into()),
                ..SessionView::unreachable(NO_CHANNEL)
            };
        }
        if self.running.load(Ordering::SeqCst) {
            SessionView {
                reachable: true,
                running: true,
                line: "running — 4 pad(s)".into(),
                profile: Some("Street Fighter".into()),
            }
        } else {
            SessionView {
                reachable: true,
                running: false,
                line: "idle".into(),
                profile: None,
            }
        }
    }

    fn start(&self, profile: Option<&str>) -> Result<String, Refusal> {
        *self.started_with.lock().unwrap() = Some(profile.map(str::to_owned));
        if self.refuse_start {
            Err(no_channel("no ksx daemon control channel at the pipe"))
        } else {
            self.running.store(true, Ordering::SeqCst);
            Ok("running (4 slot(s))".into())
        }
    }

    fn stop(&self) -> Result<String, Refusal> {
        if self.no_daemon {
            return Err(no_channel(NO_CHANNEL));
        }
        self.running.store(false, Ordering::SeqCst);
        Ok("stopped".into())
    }

    fn reload(&self) -> Result<String, Refusal> {
        Ok("running (4 slot(s))".into())
    }

    fn learn_start(&self) -> LearnView {
        self.learning.store(true, Ordering::SeqCst);
        LearnView {
            ok: true,
            state: "listening".into(),
            remaining_ms: Some(10_000),
            device: None,
            key: None,
            error: None,
        }
    }

    fn learn_poll(&self) -> LearnView {
        if self.learning.load(Ordering::SeqCst) {
            LearnView {
                ok: true,
                state: "listening".into(),
                remaining_ms: Some(9_000),
                device: None,
                key: None,
                error: None,
            }
        } else {
            LearnView {
                ok: true,
                state: "idle".into(),
                remaining_ms: None,
                device: None,
                key: None,
                error: None,
            }
        }
    }

    fn learn_cancel(&self) -> LearnView {
        self.learning.store(false, Ordering::SeqCst);
        LearnView {
            ok: true,
            state: "cancelled".into(),
            remaining_ms: None,
            device: None,
            key: None,
            error: None,
        }
    }

    fn restore(&self, preset: &str, mode: RestoreMode) -> Result<String, Refusal> {
        *self.restored_with.lock().unwrap() = Some((preset.to_owned(), mode.as_str().to_owned()));
        if self.no_daemon {
            return Err(no_channel(NO_CHANNEL));
        }
        match mode.as_str() {
            "session-backup" => Err(Refusal::new(
                ksx_api::codes::UNKNOWN_PRESET,
                format!("no session backup for \"{preset}\""),
            )),
            "latest-backup" => Ok(format!(
                "\"{preset}\": bindings restored from the newest timestamped backup"
            )),
            _ => Ok(format!(
                "\"{preset}\": bindings reset to the generic keyboard layout (S/D/A/W…)"
            )),
        }
    }

    fn clear_all(&self, preset: &str) -> Result<String, Refusal> {
        *self.cleared.lock().unwrap() = Some(preset.to_owned());
        if self.no_daemon {
            return Err(no_channel(NO_CHANNEL));
        }
        Ok(format!("\"{preset}\": every binding cleared"))
    }

    fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
        *self.saved_macro.lock().unwrap() = Some(request.clone());
        if self.no_daemon {
            return MacroOutcome::failed(NO_CHANNEL);
        }
        // The one refusal the daemon answers with rows rather than a sentence.
        if request
            .steps
            .iter()
            .any(|s| s.hold.iter().any(|f| f == "warp"))
        {
            return MacroOutcome {
                code: Some("macro-invalid".into()),
                problems: vec!["macro 'hadouken' step 1 holds 'warp'".into()],
                ..MacroOutcome::failed("refusing to write macro \"hadouken\"")
            };
        }
        MacroOutcome {
            ok: true,
            message: Some(if request.delete {
                format!("\"{}\": macro \"{}\" deleted", request.preset, request.name)
            } else {
                format!(
                    "\"{}\": macro \"{}\" = {} step(s)",
                    request.preset,
                    request.name,
                    request.steps.len()
                )
            }),
            warnings: vec!["step 2 asks for 5 ms and was raised to 33 ms".into()],
            deleted: request.delete,
            backup: Some(BACKUP_LABEL.to_owned()),
            reloaded: request.reload,
            ..MacroOutcome::default()
        }
    }

    fn bind(&self, request: &BindRequest) -> BindOutcome {
        *self.bound_with.lock().unwrap() = Some(request.clone());
        if request.key.as_deref() == Some("G") && !request.force {
            BindOutcome {
                ok: false,
                message: None,
                error: Some("refusing to bind G: G is \"IPAC P2\"'s A".into()),
                code: Some("conflict".into()),
                conflicts: vec![BindConflict {
                    scope: "profile".into(),
                    preset: "IPAC P2".into(),
                    function: "A".into(),
                    profile: Some("Steam".into()),
                    slot: Some(2),
                }],
                also_drives: Vec::new(),
                turbo_hz: None,
                turbo_effective_hz: None,
                reloaded: false,
            }
        } else {
            BindOutcome {
                ok: true,
                message: Some(format!(
                    "\"{}\": {} = {}",
                    request.preset,
                    request.function,
                    request.key.as_deref().unwrap_or("None")
                )),
                error: None,
                code: None,
                conflicts: Vec::new(),
                // A same-preset duplicate is a multi-bind, not a refusal: the
                // write succeeds and names the controls the key also drives.
                also_drives: match request.key.as_deref() {
                    Some("P") => vec!["A".into(), "B".into()],
                    _ => Vec::new(),
                },
                turbo_hz: None,
                turbo_effective_hz: None,
                reloaded: request.reload,
            }
        }
    }
}

/// The MACHINE provider, scripted: one reference cabinet, and every write
/// RECORDED rather than performed.
///
/// Recorded rather than performed for the reason the cross-site test below
/// spells out — the assertion that matters about a refused write is not the
/// status code, it is that the writer never saw it. A fake that wrote to a real
/// config store could not tell those two apart.
///
/// The tree is the reference cabinet as `device_scan` shapes it: one I-PAC
/// wearing two devnodes with the keyboard on `MI_00`, one fan controller with
/// no keyboard interface at all, and one configured entry whose id is
/// PORT-PINNED.
#[derive(Default)]
struct ScriptedMachine {
    picked: Mutex<Vec<(String, Option<String>)>>,
    removed: Mutex<Vec<(String, bool)>>,
    /// Refuse the read — the "this surface cannot enumerate devices" path.
    refuse: bool,
    /// The scan ANSWERS but the USB enumeration inside it failed: empty lists
    /// that are not a reading of the machine. A third state, distinct from
    /// `refuse` and from an actually-empty cabinet, and the one the page
    /// shipped without a single test reaching it.
    blind: bool,
    /// The last `profile_new` spec this provider was asked for, so a test can
    /// prove the FORM's values reached the verb rather than a default.
    created_profile: Mutex<Option<ksx_api::NewProfile>>,
    created_preset: Mutex<Option<ksx_api::NewPreset>>,
    /// Both machine READS behind /profiles refuse — the state a machine with
    /// a syntax error in games.toml and a permission problem on the presets
    /// folder is in. Distinct from "the machine is empty", which is what the
    /// page used to render for it — and distinct from [`Self::refuse`], which
    /// is the DEVICE scan refusing.
    reads_refuse: bool,
}

const IPAC_KB: &str = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
const IPAC_AUX: &str = r"USB\VID_D209&PID_0430&MI_01\7&25EEA38C&0&0001";
const FAN_HID: &str = r"USB\VID_1E71&PID_300E&MI_01\7&8FBF878&0&0001";

impl ScriptedMachine {
    fn refusing() -> Self {
        Self {
            refuse: true,
            ..Self::default()
        }
    }

    /// The scan answers, and answers "I could not read the USB bus".
    fn blind() -> Self {
        Self {
            blind: true,
            ..Self::default()
        }
    }

    /// Both /profiles reads refuse (games.toml AND the presets folder).
    fn reads_refusing() -> Self {
        Self {
            reads_refuse: true,
            ..Self::default()
        }
    }

    fn iface(id: &str, state: &str, boot: bool) -> ksx_api::UsbRow {
        ksx_api::UsbRow {
            instance_id: id.to_owned(),
            description: "USB Input Device".to_owned(),
            state: state.to_owned(),
            verdict: "bound to winusb.sys — ksx can capture this".to_owned(),
            alias: None,
            selected: false,
            ready: false,
            vendor: Some("Ultimarc I-PAC 4X".to_owned()),
            board: Some(r"USB\VID_D209&PID_0430\4".to_owned()),
            boot_keyboard: boot,
            // The selector `scan` would write. A constant, not derived from `id`:
            // UsbRow::selector exists so no surface re-derives what the writer
            // decided (docs/SURFACES.md section 1), and a fixture that computed it
            // would be re-deriving it in a third place to test the other two.
            selector: Some("usb:d209:0430:00".to_owned()),
        }
    }
}

impl ksx_api::MachineSource for ScriptedMachine {
    fn device_scan(&self) -> Result<ksx_api::DeviceScanView, Refusal> {
        if self.refuse {
            return Err(Refusal::not_here("listing devices", "run `ksx devices`"));
        }
        if self.blind {
            // Empty lists with `usb_available: false` — nothing could be read.
            // Built through `read` like every other path, so the summary lines
            // and the `no_*` flags are the ones the real backend would send.
            return Ok(ksx_api::DeviceScanView::read(
                "test".to_owned(),
                false,
                Vec::new(),
                Vec::new(),
                vec!["the USB enumeration returned no interfaces".to_owned()],
            ));
        }
        // Through `DeviceScanView::read`, never a struct literal. A fixture
        // that wrote the summary lines, the counts and the health verdict as
        // literals would already contain the answers these tests ask about,
        // and could not disagree with the page even when the page was wrong.
        Ok(ksx_api::DeviceScanView::read(
            "test".to_owned(),
            true,
            vec![
                ksx_api::BoardRow {
                    name: "Ultimarc I-PAC 4X".to_owned(),
                    interfaces: vec![
                        Self::iface(IPAC_KB, "claimed", true),
                        Self::iface(IPAC_AUX, "not-a-keyboard", false),
                    ],
                    keyboard: Some(IPAC_KB.to_owned()),
                    keyboard_verdict: "bound to winusb.sys — ksx can capture this".to_owned(),
                    looks_like_a_keyboard: true,
                    claimed: true,
                    alias: Some("panel".to_owned()),
                    claim_command: None,
                    release_command: Some(format!("ksx winusb release {IPAC_KB} --yes")),
                    ..ksx_api::BoardRow::default()
                },
                ksx_api::BoardRow {
                    name: "NZXT fan controller".to_owned(),
                    interfaces: vec![Self::iface(FAN_HID, "not-a-keyboard", false)],
                    keyboard: None,
                    keyboard_verdict: "no keyboard interface — ksx cannot capture this board"
                        .to_owned(),
                    looks_like_a_keyboard: false,
                    claimed: false,
                    alias: None,
                    claim_command: None,
                    release_command: None,
                    ..ksx_api::BoardRow::default()
                },
            ],
            vec![ksx_api::ConfiguredDevice {
                alias: "panel".to_owned(),
                id: "port=7&25EEA38C&0&0000".to_owned(),
                backend: "winusb".to_owned(),
                rung: "port".to_owned(),
                survives_replug: false,
                means: "this exact USB socket".to_owned(),
                port_pinned_warning: Some(
                    "PORT-PINNED — nothing weaker than the Windows instance path separates this \
                     board from its twin, so this entry matches only while Windows keeps \
                     reporting that exact path. Moving the board to another USB socket is the \
                     usual way that changes, and the entry then stops matching. It is also \
                     specific to THIS machine, so do not copy this config to another cabinet — \
                     run `ksx device pick` there instead."
                        .to_owned(),
                ),
                present: true,
                board: Some("Ultimarc I-PAC 4X".to_owned()),
                instance_id: Some(IPAC_KB.to_owned()),
                claimed: true,
                claim_command: None,
                release_command: Some(format!("ksx winusb release {IPAC_KB} --yes")),
                used_by: vec!["slot 1 (keyboard)".to_owned()],
                ..ksx_api::ConfiguredDevice::default()
            }],
            Vec::new(),
        ))
    }

    fn device_pick(
        &self,
        spec: &ksx_api::DevicePickSpec,
    ) -> Result<ksx_api::DevicePickView, Refusal> {
        self.picked
            .lock()
            .unwrap()
            .push((spec.query.clone(), spec.alias.clone()));
        let alias = spec
            .alias
            .clone()
            .unwrap_or_else(|| "Ultimarc I-PAC 4X".to_owned());
        Ok(ksx_api::DevicePickView {
            alias: alias.clone(),
            id: "model=d209:0430".to_owned(),
            backend: "winusb".to_owned(),
            board: "Ultimarc I-PAC 4X".to_owned(),
            instance_id: IPAC_KB.to_owned(),
            replaced: None,
            claimed: true,
            port_pinned: false,
            next_step: None,
            backup: None,
            summary: format!("wrote [[device]] \"{alias}\" — nothing was claimed"),
        })
    }

    fn device_remove(
        &self,
        spec: &ksx_api::DeviceRemoveSpec,
    ) -> Result<ksx_api::DeviceRemoveView, Refusal> {
        self.removed
            .lock()
            .unwrap()
            .push((spec.alias.clone(), spec.force));
        Ok(ksx_api::DeviceRemoveView {
            alias: spec.alias.clone(),
            id: "port=7&25EEA38C&0&0000".to_owned(),
            still_claimed: Some(IPAC_KB.to_owned()),
            release_command: Some(format!("ksx winusb release {IPAC_KB} --yes")),
            breaks: Vec::new(),
            backup: None,
            summary: format!(
                "removed [[device]] \"{}\" — the board is STILL CLAIMED; releasing it is a \
                 separate step",
                spec.alias
            ),
        })
    }

    /// The profile list is the reference cabinet's as of 2026-08-07, including
    /// the one that is actually broken there: "MAME 4P" points at a mame.exe
    /// that is not on the disk. The provider — not the page — is what decides
    /// that, which is why the fixture states it as `state: "broken"` with the
    /// path, exactly as `LocalMachine::profiles` composes it from
    /// `ksx_games::preflight`.
    fn profiles(&self) -> Result<ksx_api::ProfilesView, Refusal> {
        if self.reads_refuse {
            return Err(Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                "games.toml could not be read: expected `=` at line 4",
                "run `ksx config export --what games`",
            ));
        }
        Ok(ksx_api::ProfilesView {
            generated_at: "test".into(),
            config_root: "C:\\cfg".into(),
            games_path: "C:\\cfg\\games.toml".into(),
            profiles: vec![
                ksx_api::ProfileDetail {
                    title: "Street Fighter".into(),
                    path: "C:\\sf.exe".into(),
                    arguments: String::new(),
                    slots: 2,
                    presets: vec!["Arcade".into()],
                    state: "ok".into(),
                    verdict: "the program is there".into(),
                    broken_path: None,
                },
                ksx_api::ProfileDetail {
                    title: "MAME 4P".into(),
                    path: "D:\\emu\\mame\\mame.exe".into(),
                    arguments: String::new(),
                    slots: 4,
                    presets: vec!["Arcade".into()],
                    state: "broken".into(),
                    verdict: "game profile 'MAME 4P' points at 'D:\\emu\\mame\\mame.exe', \
                              which does not exist"
                        .into(),
                    broken_path: Some("D:\\emu\\mame\\mame.exe".into()),
                },
            ],
            notes: Vec::new(),
        })
    }

    fn presets(&self) -> Result<ksx_api::PresetsView, Refusal> {
        if self.reads_refuse {
            return Err(Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                "the presets folder could not be read: access is denied",
                "run `ksx doctor`",
            ));
        }
        Ok(ksx_api::PresetsView {
            config_root: "C:\\cfg\\presets".into(),
            presets: vec![ksx_api::PresetRow {
                name: "Arcade".into(),
                bound: 25,
                macros: 0,
                protected: false,
                source: "C:\\cfg\\presets\\Arcade.toml".into(),
            }],
            templates: vec![ksx_api::TemplateRow {
                id: "keyboard-2p".into(),
                label: "Two players sharing ONE keyboard: WASD vs the arrows".into(),
                detail: "Two people on one ordinary keyboard, no encoder.".into(),
                players: vec![1, 2],
            }],
        })
    }

    fn profile_new(&self, spec: &ksx_api::NewProfile) -> Result<String, Refusal> {
        *self.created_profile.lock().unwrap() = Some(spec.clone());
        if spec.title.trim().is_empty() {
            return Err(Refusal::new(
                ksx_api::codes::REFUSED,
                "a profile needs a title — it is the name `ksx run --game` takes",
            ));
        }
        Ok(format!(
            "created profile \"{}\" — {} slot(s) on preset \"{}\" → {}",
            spec.title, spec.slots, spec.preset, spec.path
        ))
    }

    fn preset_new(&self, spec: &ksx_api::NewPreset) -> Result<String, Refusal> {
        *self.created_preset.lock().unwrap() = Some(spec.clone());
        // The refusal `LocalMachine` composes from
        // `preset_edit::PresetError::Exists` + its `advice()`, verbatim in
        // shape: a message that names the file it protected, and a remedy that
        // is the ONLY way forward. "Arcade" is the preset `presets()` lists.
        if spec.name == "Arcade" && !spec.force {
            return Err(Refusal::with_remedy(
                ksx_api::codes::REFUSED,
                "a preset called \"Arcade\" already exists (C:\\cfg\\presets\\Arcade.toml)",
                "--force overwrites it (a timestamped backup is taken first).",
            ));
        }
        Ok(format!(
            "created preset \"{}\" — 30 controls from \"{}\" (player {})",
            spec.name, spec.template, spec.player
        ))
    }

    // ── The M10 verbs behind /setup: the config in and out, and the first-run
    //    state. Synthetic, and deliberately just enough to prove the ROUTES —
    //    what the real provider does to a config root is `ksx-app`'s to test
    //    (`onboard.rs` + `config_io.rs`), and testing it twice would only pin
    //    the fake.

    fn setup_state(&self) -> Result<ksx_api::SetupView, Refusal> {
        Ok(ksx_api::SetupView {
            generated_at: "test".into(),
            config_root: "C:\\cfg".into(),
            config_exists: true,
            devices: vec![ksx_api::SetupDeviceRow {
                alias: "P1 board".into(),
                id: "usb:d209:0430:00".into(),
                backend: "interception".into(),
            }],
            slots: vec![ksx_api::SetupSlotRow {
                number: 1,
                device: "P1 board".into(),
                preset: "IPAC P1".into(),
                persona: "Xbox 360 pad".into(),
                source: "config.toml".into(),
            }],
            presets: vec!["IPAC P1".into()],
            profiles: vec!["Street Fighter".into()],
            steps: vec![
                ksx_api::SetupStep {
                    id: ksx_api::setup_steps::BOARD.into(),
                    title: "Find your board and name it".into(),
                    detail: "One board is named.".into(),
                    state: ksx_api::setup_states::DONE.into(),
                },
                ksx_api::SetupStep {
                    id: ksx_api::setup_steps::SLOT.into(),
                    title: "Wire a slot".into(),
                    detail: "One slot is wired.".into(),
                    state: ksx_api::setup_states::DONE.into(),
                },
                ksx_api::SetupStep {
                    id: ksx_api::setup_steps::PROVE.into(),
                    title: "Press a button and watch it land".into(),
                    detail: "Start the listener and press a button.".into(),
                    state: ksx_api::setup_states::NOW.into(),
                },
            ],
            notes: Vec::new(),
        })
    }

    fn config_export(
        &self,
        _request: &ksx_api::ExportRequest,
    ) -> Result<ksx_api::ConfigExport, Refusal> {
        let document = "{\n  \"ksx_interop\": 1,\n  \"schema_version\": 1\n}\n".to_owned();
        Ok(ksx_api::ConfigExport {
            filename: "ksx-config-20260807-120000.json".into(),
            bytes: document.len(),
            parts: vec!["config".into(), "games".into(), "presets".into()],
            presets: 1,
            warnings: Vec::new(),
            document,
        })
    }

    /// The consent shape, faithfully: no `apply`, no write.
    fn config_import(
        &self,
        request: &ksx_api::ImportRequest,
    ) -> Result<ksx_api::ImportReport, Refusal> {
        if !request.document.contains("ksx_interop") {
            return Err(Refusal::new(
                ksx_api::codes::BAD_REQUEST,
                "the pasted document does not say what it is",
            ));
        }
        Ok(ksx_api::ImportReport {
            ok: true,
            applied: request.apply,
            summary: if request.apply {
                "imported config, games — 2 file(s) written, 2 backed up first".to_owned()
            } else {
                "nothing written yet — this would replace your settings. Tick \"write it\" \
                 and import again to apply."
                    .to_owned()
            },
            ..ksx_api::ImportReport::default()
        })
    }
}

/// Bind port 0 to learn a free port, release it, and serve there. The tiny
/// race is acceptable in a local test.
fn start_server(control: Arc<ScriptedControl>) -> SocketAddr {
    start_server_with_machine(control, Arc::new(ScriptedMachine::default()))
}

fn start_server_with_machine(
    control: Arc<ScriptedControl>,
    machine: Arc<ScriptedMachine>,
) -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    struct SharedControl(Arc<ScriptedControl>);
    impl ControlSource for SharedControl {
        fn session(&self) -> SessionView {
            self.0.session()
        }
        fn start(&self, profile: Option<&str>) -> Result<String, Refusal> {
            self.0.start(profile)
        }
        fn stop(&self) -> Result<String, Refusal> {
            self.0.stop()
        }
        fn reload(&self) -> Result<String, Refusal> {
            self.0.reload()
        }
        fn learn_start(&self) -> LearnView {
            self.0.learn_start()
        }
        fn learn_poll(&self) -> LearnView {
            self.0.learn_poll()
        }
        fn learn_cancel(&self) -> LearnView {
            self.0.learn_cancel()
        }
        fn bind(&self, request: &BindRequest) -> BindOutcome {
            self.0.bind(request)
        }
        fn restore(&self, preset: &str, mode: RestoreMode) -> Result<String, Refusal> {
            self.0.restore(preset, mode)
        }
        fn clear_all(&self, preset: &str) -> Result<String, Refusal> {
            self.0.clear_all(preset)
        }
        fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
            self.0.save_macro(request)
        }
        fn assign_slot(&self, request: &ksx_api::SlotAssignRequest) -> ksx_api::SlotOutcome {
            self.0.assign_slot(request)
        }
    }
    struct SharedMachine(Arc<ScriptedMachine>);
    impl ksx_api::MachineSource for SharedMachine {
        fn device_scan(&self) -> Result<ksx_api::DeviceScanView, Refusal> {
            self.0.device_scan()
        }
        fn device_pick(
            &self,
            spec: &ksx_api::DevicePickSpec,
        ) -> Result<ksx_api::DevicePickView, Refusal> {
            self.0.device_pick(spec)
        }
        fn device_remove(
            &self,
            spec: &ksx_api::DeviceRemoveSpec,
        ) -> Result<ksx_api::DeviceRemoveView, Refusal> {
            self.0.device_remove(spec)
        }
        fn profiles(&self) -> Result<ksx_api::ProfilesView, Refusal> {
            self.0.profiles()
        }
        fn presets(&self) -> Result<ksx_api::PresetsView, Refusal> {
            self.0.presets()
        }
        fn profile_new(&self, spec: &ksx_api::NewProfile) -> Result<String, Refusal> {
            self.0.profile_new(spec)
        }
        fn preset_new(&self, spec: &ksx_api::NewPreset) -> Result<String, Refusal> {
            self.0.preset_new(spec)
        }
        fn setup_state(&self) -> Result<ksx_api::SetupView, Refusal> {
            self.0.setup_state()
        }
        fn config_export(
            &self,
            request: &ksx_api::ExportRequest,
        ) -> Result<ksx_api::ConfigExport, Refusal> {
            self.0.config_export(request)
        }
        fn config_import(
            &self,
            request: &ksx_api::ImportRequest,
        ) -> Result<ksx_api::ImportReport, Refusal> {
            self.0.config_import(request)
        }
    }
    std::thread::spawn(move || {
        let _ = ksx_studio::serve(
            addr,
            Box::new(FixedStatus),
            Box::new(SharedControl(control)),
            Box::new(SharedMachine(machine)),
        );
    });
    // Wait until it accepts.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(addr).is_ok() {
            return addr;
        }
        assert!(Instant::now() < deadline, "server never came up on {addr}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn http(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn get(addr: SocketAddr, path: &str) -> String {
    http(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    )
}

fn post_form(addr: SocketAddr, path: &str, body: &str) -> String {
    http(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn post_json(addr: SocketAddr, path: &str, body: &str) -> String {
    http(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

/// The response body (everything after the blank line).
fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

/// [`get`], for responses whose body is not UTF-8 — the brand icons.
/// Returns `(headers, body)` with the body left as bytes.
fn get_binary(addr: SocketAddr, path: &str) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("{path}: no header/body separator in the response"));
    let headers = String::from_utf8_lossy(&response[..split]).into_owned();
    (headers, response[split + 4..].to_vec())
}

/// The brand icons are served at the ROOT paths their consumers hard-code —
/// a browser asks for `/favicon.ico` and iOS probes `/apple-touch-icon.png`
/// with no prompting from the markup — with content types that make them
/// icons rather than downloads.
///
/// Compared BYTE FOR BYTE against the embedded files. A 404 would be found by
/// anything; the failure worth a test is a 200 carrying a truncated or
/// re-encoded image, which renders as a perfectly normal page with a blank
/// tab and no error anywhere.
#[test]
fn the_brand_icons_are_served_at_their_root_paths() {
    let addr = start_server(Arc::new(ScriptedControl::new(true)));

    for (path, mime, expected) in [
        (
            "/favicon.ico",
            "image/x-icon",
            include_bytes!("../brand/favicon.ico").as_slice(),
        ),
        (
            "/favicon.svg",
            "image/svg+xml",
            include_bytes!("../brand/favicon.svg").as_slice(),
        ),
        (
            "/apple-touch-icon.png",
            "image/png",
            include_bytes!("../brand/apple-touch-icon.png").as_slice(),
        ),
    ] {
        let (headers, body) = get_binary(addr, path);
        assert!(headers.starts_with("HTTP/1.1 200"), "{path}: {headers}");
        assert!(
            headers.to_ascii_lowercase().contains(mime),
            "{path}: expected content-type {mime}\n{headers}"
        );
        assert_eq!(
            body.len(),
            expected.len(),
            "{path}: served {} bytes, embed has {}",
            body.len(),
            expected.len()
        );
        assert!(
            body == expected,
            "{path}: served bytes differ from the embed"
        );
    }

    // And the page points at all three, so the icons are not merely reachable
    // by luck of the browser's default probing.
    let page = get(addr, "/");
    for link in [
        r#"href="/favicon.svg""#,
        r#"href="/favicon.ico""#,
        r#"href="/apple-touch-icon.png""#,
    ] {
        assert!(page.contains(link), "status page missing {link}");
    }
}

#[test]
fn the_session_panel_round_trips_start_stop_and_the_flash() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // Idle: the Start form and the profile dropdown render.
    let page = get(addr, "/");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains(r#"action="/session/start""#), "{page}");
    assert!(page.contains("Street Fighter"), "{page}");
    assert!(page.contains("idle"), "{page}");

    // Start with a profile: 303 back to / with the outcome flashed.
    let response = post_form(addr, "/session/start", "profile=Street+Fighter");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("location: /?flash=running"), "{response}");
    assert_eq!(
        control.started_with.lock().unwrap().clone(),
        Some(Some("Street Fighter".to_owned())),
        "the form's profile field must reach the control verb"
    );

    // Following the redirect renders the flash and, now, Stop/Reload.
    let page = get(addr, "/?flash=running%20%284%20slot%28s%29%29");
    assert!(page.contains("running (4 slot(s))"), "{page}");
    assert!(page.contains(r#"action="/session/stop""#), "{page}");
    assert!(page.contains(r#"action="/config/reload""#), "{page}");
    assert!(!page.contains(r#"action="/session/start""#), "{page}");

    // The empty sentinel option means "no profile override".
    let response = post_form(addr, "/session/stop", "");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let response = post_form(addr, "/session/start", "profile=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert_eq!(control.started_with.lock().unwrap().clone(), Some(None));
}

/// The whole mapper loop over real HTTP: the page renders zones with real
/// bindings, the learn flow answers listening → cancel, and /api/bind
/// round-trips the conflict → Replace(force) decision.
#[test]
fn the_mapper_page_learn_flow_and_bind_round_trip() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // The page: slot context, art, a zone with its binding tag, credit line.
    let page = get(addr, "/map");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("P1 · Xbox 360 · IPAC P1"), "{page}");
    assert!(page.contains("/_assets/pad-xbox.svg"), "{page}");
    assert!(page.contains(r#"data-fn="A""#), "{page}");
    assert!(page.contains(">G<"), "{page}");
    assert!(
        page.contains("Gamepad-Asset-Pack (MIT) by AL2009man"),
        "{page}"
    );
    // Ledger #13(a): the CSP header must allow inline STYLE attributes (the
    // zone geometry rides them) while scripts stay nonce-locked.
    //
    // It used to assert `style-src 'self' 'unsafe-inline'` — the policy ksx's
    // own `relax_style_src` produced. forma-server 0.2.0 fixed the underlying
    // problem, that workaround is deleted, and the header now carries
    // upstream's answer: a separate `style-src-attr` permits the attributes,
    // so `style-src` keeps its nonce for `<style>` blocks and stylesheets.
    // Asserting the old string here would have quietly demanded a weaker
    // policy than the server ships.
    let headers = page.split("\r\n\r\n").next().unwrap_or("");
    assert!(
        headers.contains("style-src-attr 'unsafe-inline'"),
        "the mapper's zone geometry rides inline style attributes: {headers}"
    );
    assert!(
        headers.contains("style-src 'nonce-"),
        "style-src must stay nonce-locked now that attributes have their own \
         directive: {headers}"
    );
    assert!(headers.contains("script-src 'nonce-"), "{headers}");

    // The art itself is served with the right type, recolored for the theme
    // (the palette sheet build.mjs injects) — never the source's black blob.
    let art = get(addr, "/_assets/pad-xbox.svg");
    assert!(art.starts_with("HTTP/1.1 200"), "{art}");
    assert!(art.contains("image/svg+xml"), "{art}");
    assert!(art.contains("<svg"), "{art}");
    assert!(art.contains("pad-body"), "recolor classes missing: {art}");
    assert!(!art.contains("fill:#000000"), "source black leaked: {art}");

    // /api/map serves the payload the page embeds.
    let api = get(addr, "/api/map");
    let payload: serde_json::Value = serde_json::from_str(body_of(&api)).expect("json");
    assert_eq!(payload["mapper"]["slots"][0]["preset"], "IPAC P1");
    assert_eq!(payload["mapper"]["slots"][0]["bindings"]["A"][0], "G");
    assert_eq!(payload["selected"], 1);
    assert_eq!(payload["learn"]["state"], "idle");

    // Learn: start → listening with the countdown, poll agrees, cancel ends.
    let started = post_json(addr, "/api/learn/start", "");
    let learn: serde_json::Value = serde_json::from_str(body_of(&started)).expect("json");
    assert_eq!(learn["state"], "listening");
    assert_eq!(learn["remaining_ms"], 10_000);
    let polled = get(addr, "/api/learn");
    let learn: serde_json::Value = serde_json::from_str(body_of(&polled)).expect("json");
    assert_eq!(learn["state"], "listening");
    let cancelled = post_json(addr, "/api/learn/cancel", "");
    let learn: serde_json::Value = serde_json::from_str(body_of(&cancelled)).expect("json");
    assert_eq!(learn["state"], "cancelled");

    // Bind: the scripted conflict comes back structured; Replace (force)
    // succeeds and reports the reload.
    let refused = post_json(
        addr,
        "/api/bind",
        r#"{"preset":"IPAC P1","function":"B","key":"G","force":false,"reload":true}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&refused)).expect("json");
    assert_eq!(outcome["ok"], false);
    assert_eq!(outcome["code"], "conflict");
    assert_eq!(outcome["conflicts"][0]["preset"], "IPAC P2");
    assert_eq!(outcome["conflicts"][0]["slot"], 2);

    let forced = post_json(
        addr,
        "/api/bind",
        r#"{"preset":"IPAC P1","function":"B","key":"G","force":true,"reload":true}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&forced)).expect("json");
    assert_eq!(outcome["ok"], true, "{outcome}");
    assert_eq!(outcome["reloaded"], true);
    let bound = control
        .bound_with
        .lock()
        .unwrap()
        .clone()
        .expect("bind reached control");
    assert_eq!(bound.preset, "IPAC P1");
    assert_eq!(bound.function, "B");
    assert!(bound.force);
    assert!(bound.reload);

    // Preset restore: defaults succeeds, session-backup surfaces the honest
    // "nothing to undo", and a junk mode never reaches the control source.
    let restored = post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"defaults"}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&restored)).expect("json");
    assert_eq!(outcome["ok"], true, "{outcome}");
    assert!(
        outcome["message"]
            .as_str()
            .unwrap()
            .contains("generic keyboard layout"),
        "{outcome}"
    );
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "defaults".to_owned()))
    );

    let refused = post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"session-backup"}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&refused)).expect("json");
    assert_eq!(outcome["ok"], false);
    assert!(
        outcome["error"]
            .as_str()
            .unwrap()
            .contains("no session backup"),
        "{outcome}"
    );

    let junk = post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"yolo"}"#,
    );
    let outcome: serde_json::Value = serde_json::from_str(body_of(&junk)).expect("json");
    assert_eq!(outcome["ok"], false);
    assert!(
        outcome["error"]
            .as_str()
            .unwrap()
            .contains("unknown restore mode"),
        "{outcome}"
    );
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "session-backup".to_owned())),
        "the junk mode must have been rejected before the control source"
    );
}

/// FIX 1, over real HTTP: the exact failure Victor hit. Quit the daemon, load
/// either page, and the FIRST thing on it must be the banner — with the
/// command that starts one, profile flag included.
#[test]
fn a_dead_daemon_is_loud_on_both_pages_with_a_runnable_command() {
    let addr = start_server(Arc::new(ScriptedControl::dead()));

    for path in ["/", "/map"] {
        let page = get(addr, path);
        assert!(page.starts_with("HTTP/1.1 200"), "{path}: {page}");
        let body = body_of(&page);
        assert!(
            body.contains("No daemon — ksx Studio can see your config but cannot change anything."),
            "{path} has no banner: {body}"
        );
        assert!(body.contains("tray icon"), "{path}: {body}");
        assert!(
            body.contains("ksx daemon --game &quot;Steam&quot;")
                || body.contains(r#"ksx daemon --game "Steam""#),
            "{path} must print the command that actually starts THIS cabinet: {body}"
        );
        // Unmissable = above everything it is about. On both pages the banner
        // must precede the <main> content it warns you off touching.
        let banner = body.find("No daemon —").expect("banner");
        let footer = body.find("<footer").expect("footer");
        assert!(banner < footer, "{path}: banner is below the fold: {body}");
        let first_other_card = body[banner..]
            .find(r#"class="card"#)
            .map(|i| banner + i)
            .expect("another card follows the banner");
        assert!(
            banner < first_other_card,
            "{path}: the banner is not first inside <main>: {body}"
        );
    }

    // The mapper additionally renders every control visibly inert…
    let map = body_of(&get(addr, "/map")).to_owned();
    assert!(map.contains("z-dead"), "{map}");
    assert!(map.contains("l-dead"), "{map}");
    assert!(map.contains("card pactions off"), "{map}");
    // …and keeps the prefilled shell fallback, so the page is still useful.
    assert!(map.contains("ksx map --preset"), "{map}");
}

/// FIX 0 over HTTP: the mapper's own session controls are the same
/// ControlSource verbs the status page's forms use — one pipe verb each.
#[test]
fn the_mapper_can_pause_and_resume_emulation_over_json() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // Start something, then pause it from the mapper.
    let started = post_json(
        addr,
        "/api/session/start",
        r#"{"profile":"Street Fighter"}"#,
    );
    let out: serde_json::Value = serde_json::from_str(body_of(&started)).expect("json");
    assert_eq!(out["ok"], true, "{out}");

    let map = body_of(&get(addr, "/map")).to_owned();
    assert!(map.contains("Emulation is running"), "{map}");
    assert!(map.contains(r#"data-act="pause-map""#), "{map}");
    // v9: and it is a real form, so the pause is not a dead button on a page
    // without JavaScript — same `stop` verb, 303'd back to /map.
    assert!(map.contains(r#"action="/map/session/stop""#), "{map}");
    let response = post_form(addr, "/map/session/stop", "slot=1");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /map?slot=1&flash=stopped"),
        "{response}"
    );
    assert!(
        !control.session().running,
        "the form POST really stopped it"
    );
    let started = post_json(
        addr,
        "/api/session/start",
        r#"{"profile":"Street Fighter"}"#,
    );
    let out: serde_json::Value = serde_json::from_str(body_of(&started)).expect("json");
    assert_eq!(out["ok"], true, "{out}");

    let paused = post_json(addr, "/api/session/stop", "");
    let out: serde_json::Value = serde_json::from_str(body_of(&paused)).expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["message"], "stopped");

    // Resume names the profile the page remembered.
    let resumed = post_json(
        addr,
        "/api/session/start",
        r#"{"profile":"Street Fighter"}"#,
    );
    let out: serde_json::Value = serde_json::from_str(body_of(&resumed)).expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(
        control.started_with.lock().unwrap().clone(),
        Some(Some("Street Fighter".to_owned())),
        "Resume must restart the SAME profile that was paused"
    );
}

/// FIX 2 over HTTP: three destinations, and the label the third one wears.
#[test]
fn the_three_restore_destinations_and_clear_all_round_trip() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    let map = body_of(&get(addr, "/map")).to_owned();
    assert!(
        map.contains(&format!("Restore backup from {BACKUP_LABEL}")),
        "the newest backup's timestamp belongs in the label: {map}"
    );
    assert!(
        map.contains("Reset to generic keyboard layout (S/D/A/W…)"),
        "{map}"
    );
    assert!(!map.contains("Restore built-in defaults"), "{map}");

    let out: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/preset/restore",
        r#"{"preset":"IPAC P1","mode":"latest-backup"}"#,
    )))
    .expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "latest-backup".to_owned()))
    );

    let out: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/preset/clear-all",
        r#"{"preset":"IPAC P1"}"#,
    )))
    .expect("json");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(
        control.cleared.lock().unwrap().clone(),
        Some("IPAC P1".to_owned())
    );
}

/// v11 over HTTP: the macro editor READS a real preset and SAVES the whole
/// table back through the one control verb.
///
/// The read half proves the card is no longer a blank draft — the file's own
/// numbers reach the page, in the unit they were authored in — and the write
/// half proves the save is `ControlSource::save_macro` (= the daemon's
/// `map-macro`), carrying the toast, the advisories a successful write still
/// has to say, and the backup label that IS the undo.
#[test]
fn the_macro_editor_reads_a_preset_and_saves_the_whole_table() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // READ: the payload the island polls carries the file's shape.
    let map: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/map?slot=1"))).expect("json");
    assert_eq!(map["macros"]["available"], true, "{map}");
    assert_eq!(map["macros"]["preset"], "IPAC P1");
    assert_eq!(map["macros"]["macros"][0]["name"], "hadouken");
    assert_eq!(map["macros"]["macros"][0]["triggers"][0], "P");
    assert_eq!(map["macros"]["macros"][0]["steps"][0]["ms"], 50);
    // A duration authored in frames stays frames all the way to the client.
    assert_eq!(map["macros"]["macros"][0]["steps"][1]["frames"], 3);
    assert_eq!(
        map["macros"]["macros"][0]["steps"][1]["ms"],
        serde_json::Value::Null
    );
    // ...and the SSR paint says the same thing without any JavaScript.
    let page = body_of(&get(addr, "/map?slot=1")).to_owned();
    assert!(page.contains("hadouken"), "{page}");
    assert!(page.contains("started by P"), "{page}");

    // WRITE: one POST, one whole table.
    let saved: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","on_release":"abort",
            "steps":[{"hold":["dpad.down"],"ms":50},{"hold":["A"],"frames":3}]}"#,
    )))
    .expect("json");
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["backup"], BACKUP_LABEL, "the undo, named: {saved}");
    assert_eq!(
        saved["warnings"][0], "step 2 asks for 5 ms and was raised to 33 ms",
        "an advisory is never swallowed: {saved}"
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(write.preset, "IPAC P1");
    assert_eq!(write.name, "hadouken");
    assert_eq!(write.on_release, "abort");
    assert_eq!(write.steps.len(), 2);
    assert_eq!(write.steps[1].frames, Some(3));
    assert!(!write.delete);
    assert!(
        write.reload,
        "a macro body is a binding change: the running session takes it in place"
    );

    // A refusal comes back as rows a page can list, not a sentence to parse.
    let refused: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","steps":[{"hold":["warp"],"ms":50}]}"#,
    )))
    .expect("json");
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["code"], "macro-invalid", "{refused}");
    assert!(
        refused["problems"][0].as_str().unwrap().contains("warp"),
        "{refused}"
    );

    // DELETE is the same route with the explicit word.
    let deleted: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","delete":true}"#,
    )))
    .expect("json");
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert_eq!(deleted["deleted"], true, "{deleted}");
    assert!(control.saved_macro.lock().unwrap().as_ref().unwrap().delete);
}

/// END TO END over HTTP for the field that broke: `repeat`, and the turbo
/// rate that hangs off it.
///
/// The user set `repeat = while-held` in the card, clicked Save, was told
/// "saved", and watched the control snap back to `once` — because the value
/// was dropped between the wire and the preset file. Nothing about that was
/// visible from the outside: the POST returned `ok`. So this test asserts what
/// the POST actually DELIVERS, not just that it succeeded — the `MacroWrite`
/// that reached `ControlSource::save_macro` must carry the policy the request
/// asked for, in every spelling the card can produce.
#[test]
fn the_repeat_policy_and_its_rate_reach_the_control_source_over_http() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // The read half serves the field at all — an absent `repeat` on the wire
    // would leave the card with nothing to show.
    let map: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/map?slot=1"))).expect("json");
    assert_eq!(map["macros"]["macros"][0]["repeat"], "once", "{map}");

    // while-held: the exact edit that was reported lost.
    let saved: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","repeat":"while-held",
            "steps":[{"hold":["A"],"ms":50}]}"#,
    )))
    .expect("json");
    assert_eq!(saved["ok"], true, "{saved}");
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(
        write.repeat, "while-held",
        "the repeat policy must reach the writer, or Save is a lie: {saved}"
    );

    // turbo authored in hertz: the rate travels in the unit it was written in.
    let _ = post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","repeat":"turbo","turbo_hz":12,
            "steps":[{"hold":["A"],"ms":50}]}"#,
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(write.repeat, "turbo");
    assert_eq!(write.turbo_hz, Some(12));
    assert_eq!(write.gap_ms, None, "the other spelling is not invented");

    // ...and the same rate said the other way.
    let _ = post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","repeat":"turbo","gap_ms":50,
            "steps":[{"hold":["A"],"frames":2,"allow_short":true}]}"#,
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert_eq!(write.gap_ms, Some(50));
    assert_eq!(write.turbo_hz, None);
    // The step's own fields ride along untouched, in the author's unit.
    assert_eq!(write.steps[0].frames, Some(2));
    assert_eq!(write.steps[0].ms, None);
    assert!(write.steps[0].allow_short);

    // An omitted `repeat` is the file's own default, not an empty string the
    // daemon would refuse.
    let _ = post_json(
        addr,
        "/api/macro/save",
        r#"{"preset":"IPAC P1","name":"hadouken","steps":[{"hold":["A"],"ms":50}]}"#,
    );
    let write = control.saved_macro.lock().unwrap().clone().expect("saved");
    assert!(
        write.repeat.is_empty(),
        "blank = the file's omitted-field rule"
    );
}

/// Clearing ONE binding is the plain `map` verb with a null key — no second
/// writer, no GUI-only path.
#[test]
fn clearing_one_binding_goes_through_the_bind_verb_with_a_null_key() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());
    let out: serde_json::Value = serde_json::from_str(body_of(&post_json(
        addr,
        "/api/bind",
        r#"{"preset":"IPAC P1","function":"A","key":null,"force":false,"reload":true}"#,
    )))
    .expect("json");
    assert_eq!(out["ok"], true, "{out}");
    let bound = control.bound_with.lock().unwrap().clone().expect("bind");
    assert_eq!(bound.function, "A");
    assert_eq!(bound.key, None, "a null key is a CLEAR");
}

/// v9, over real HTTP and with no JavaScript anywhere in sight: the mapper
/// page ships forms, and posting one form-encoded body writes a binding and
/// 303s back to /map with the outcome flashed. This is the whole no-JS
/// contract — if it holds here, a browser with scripting off can map a
/// cabinet.
#[test]
fn the_mapper_is_fully_operable_with_form_posts_only() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // The page a scripting-off browser gets: real forms, real action URLs,
    // real key options, and slot switching as links.
    let page = body_of(&get(addr, "/map")).to_owned();
    assert!(page.contains(r#"action="/map/bind""#), "{page}");
    assert!(page.contains(r#"formaction="/map/clear""#), "{page}");
    assert!(page.contains(r#"action="/map/preset/restore""#), "{page}");
    assert!(page.contains(r#"action="/map/preset/clear-all""#), "{page}");
    assert!(
        page.contains(r#"<select class="keysel" name="key""#),
        "{page}"
    );
    assert!(page.contains("<option>NumpadEnter</option>"), "{page}");
    assert!(page.contains(r#"href="/map?slot=1""#), "{page}");

    // Bind: form-encoded in, 303 back to the slot we were on, outcome flashed.
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=H");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /map?slot=1&flash=B%20is%20now%20H."),
        "{response}"
    );
    let bound = control.bound_with.lock().unwrap().clone().expect("bind");
    assert_eq!(bound.preset, "IPAC P1", "the slot resolved to its preset");
    assert_eq!(bound.function, "B");
    assert_eq!(bound.key.as_deref(), Some("H"));
    assert!(
        bound.reload,
        "a binding edit is hot-swapped, pads stay plugged"
    );
    assert!(!bound.force, "the row form never forces on its own");

    // Clear: the same `map` verb with a null key — no second unbind path.
    let response = post_form(addr, "/map/clear", "slot=1&function=A");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /map?slot=1&flash=A%20is%20now%20unbound."),
        "{response}"
    );
    assert_eq!(
        control.bound_with.lock().unwrap().clone().unwrap().key,
        None,
        "a null key is a CLEAR"
    );

    // The empty placeholder is refused in words, never read as a clear.
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("flash=error%3A%20no%20key%20picked"),
        "{response}"
    );

    // Cross-slot refusal: the flash names the other slot AND the checkbox
    // that says yes to it — a form's version of the Replace dialog.
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=G");
    assert!(
        response.contains("location: /map?slot=1&flash=error"),
        "{response}"
    );
    assert!(response.contains("IPAC%20P2"), "{response}");
    let response = post_form(addr, "/map/bind", "slot=1&function=B&key=G&force=1");
    assert!(response.contains("flash=B%20is%20now%20G."), "{response}");
    assert!(control.bound_with.lock().unwrap().clone().unwrap().force);

    // The preset writes and the pause, same shape.
    let response = post_form(addr, "/map/preset/clear-all", "slot=1");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert_eq!(
        control.cleared.lock().unwrap().clone(),
        Some("IPAC P1".to_owned())
    );
    let response = post_form(addr, "/map/preset/restore", "slot=1&mode=latest-backup");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "latest-backup".to_owned()))
    );
    // A junk mode is refused before the daemon is ever asked.
    let response = post_form(addr, "/map/preset/restore", "slot=1&mode=yolo");
    assert!(
        response.contains("flash=error%3A%20unknown%20restore%20mode"),
        "{response}"
    );
    assert_eq!(
        control.restored_with.lock().unwrap().clone(),
        Some(("IPAC P1".to_owned(), "latest-backup".to_owned())),
        "the junk mode must not have reached the control source"
    );

    // Following the redirect renders the outcome — the no-JS feedback loop
    // closed, exactly like the status page's.
    let page = body_of(&get(addr, "/map?slot=1&flash=B%20is%20now%20H.")).to_owned();
    assert!(page.contains("B is now H."), "{page}");
    assert!(page.contains("flash flash-ok"), "{page}");
    let page = body_of(&get(addr, "/map?slot=1&flash=error%3A%20nope")).to_owned();
    assert!(page.contains("flash flash-err"), "{page}");
}

/// v10, MANY KEYS → ONE CONTROL over real HTTP and with no JavaScript: the
/// same row form that binds can also ADD the picked key to what a control
/// already holds, and REMOVE just one of the keys it holds. Both are
/// read-modify-write on the key list the page already read, so a form body
/// never carries a key list it made up.
#[test]
fn the_no_js_forms_add_and_remove_one_key_at_a_time() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    let page = body_of(&get(addr, "/map")).to_owned();
    assert!(page.contains(r#"formaction="/map/add""#), "{page}");
    assert!(page.contains(r#"formaction="/map/key/remove""#), "{page}");
    // The fixture's B holds two keys: both are on the page, each with its own
    // remove payload, and neither reader spells them as a chord.
    assert!(page.contains(r#"data-rmkey="B|S""#), "{page}");
    assert!(page.contains(r#"data-rmkey="B|Enter""#), "{page}");
    assert!(!page.contains("S+Enter"), "{page}");

    // REMOVE ONE: B keeps S, loses Enter — and because one key is left, this
    // daemon's single-key `map` verb can express it exactly.
    let response = post_form(addr, "/map/key/remove", "slot=1&function=B&key=Enter");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("flash=Enter%20removed.%20B%20is%20now%20S."),
        "{response}"
    );
    assert_eq!(
        control
            .bound_with
            .lock()
            .unwrap()
            .clone()
            .unwrap()
            .key
            .as_deref(),
        Some("S"),
        "the survivor is what gets written"
    );

    // A key the control does not have is a refusal that names what it DOES
    // have — never a silent no-op, and never a write.
    let response = post_form(addr, "/map/key/remove", "slot=1&function=B&key=J");
    assert!(response.contains("flash=error%3A"), "{response}");
    assert!(
        response.contains("it%20has%20S%20%C2%B7%20Enter"),
        "{response}"
    );

    // ADD onto an UNBOUND control is an ordinary bind: nothing to keep.
    let response = post_form(addr, "/map/add", "slot=1&function=X&key=J");
    assert!(response.contains("flash=X%20is%20now%20J."), "{response}");

    // ADD onto a control that already has keys is the OR-chain the engine
    // executes — and the honest limit of today's wire: the map verb writes one
    // key per control and would drop the rest, so the write is REFUSED in
    // words rather than made silently lossy. (The day the verb takes a key
    // list, `ControlSource::bind_keys` writes it and this flash becomes the
    // success sentence, with nothing else on the page changing.)
    let before = control.bound_with.lock().unwrap().clone();
    let response = post_form(addr, "/map/add", "slot=1&function=B&key=J");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("flash=error%3A"), "{response}");
    assert!(response.contains("ONE%20key%20per%20control"), "{response}");
    assert_eq!(
        control.bound_with.lock().unwrap().clone().map(|b| b.key),
        before.map(|b| b.key),
        "a refused multi-key write must not have written anything"
    );

    // Adding a key the control already has changes nothing and says so.
    let response = post_form(addr, "/map/add", "slot=1&function=A&key=G");
    assert!(response.contains("already%20has%20G"), "{response}");

    // No key picked: the same honest refusal the Bind button gives.
    let response = post_form(addr, "/map/add", "slot=1&function=B&key=");
    assert!(
        response.contains("flash=error%3A%20no%20key%20picked"),
        "{response}"
    );
}

/// The JSON twin: the island computes the SET it wants and posts it whole, so
/// add, remove-one and undo all land through one writer.
#[test]
fn the_key_list_route_writes_a_whole_set() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    // One key: an ordinary bind.
    let response = post_json(
        addr,
        "/api/bind/keys",
        r#"{"preset":"IPAC P1","function":"B","keys":["H"],"force":false,"reload":true}"#,
    );
    assert!(body_of(&response).contains(r#""ok":true"#), "{response}");
    let bound = control.bound_with.lock().unwrap().clone().unwrap();
    assert_eq!(bound.key.as_deref(), Some("H"));
    assert!(bound.reload);

    // No keys: a clear, through the same `map` verb.
    let response = post_json(
        addr,
        "/api/bind/keys",
        r#"{"preset":"IPAC P1","function":"B","keys":[],"reload":true}"#,
    );
    assert!(body_of(&response).contains(r#""ok":true"#), "{response}");
    assert_eq!(
        control.bound_with.lock().unwrap().clone().unwrap().key,
        None
    );

    // Two keys: refused, in words that name the missing wire field — and
    // nothing was written.
    let response = post_json(
        addr,
        "/api/bind/keys",
        r#"{"preset":"IPAC P1","function":"B","keys":["S","Enter"],"reload":true}"#,
    );
    let body = body_of(&response);
    assert!(body.contains(r#""ok":false"#), "{response}");
    assert!(body.contains("ONE key per control"), "{response}");
    assert_eq!(
        control.bound_with.lock().unwrap().clone().unwrap().key,
        None,
        "the refusal must not have written the first key"
    );
}

/// No daemon: the forms are still there (dimmed by CSS, never removed) and a
/// post still answers with the reason — the no-JS half of FIX 1's "never a
/// silent no-op".
#[test]
fn a_no_js_post_without_a_daemon_flashes_the_reason() {
    let addr = start_server(Arc::new(ScriptedControl::dead()));
    let page = body_of(&get(addr, "/map")).to_owned();
    assert!(page.contains(r#"class="lbind nojs off""#), "{page}");
    assert!(page.contains(r#"action="/map/bind""#), "{page}");

    let response = post_form(addr, "/map/preset/clear-all", "slot=1");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("flash=error%3A"), "{response}");
    assert!(
        response.contains("no%20daemon%20control%20channel"),
        "{response}"
    );
}

#[test]
fn a_refused_action_comes_back_as_an_error_flash_never_silence() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);

    let response = post_form(addr, "/session/start", "profile=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("location: /?flash=error%3A%20no%20ksx%20daemon"),
        "{response}"
    );

    // And the redirect target renders it.
    let page = get(
        addr,
        "/?flash=error%3A%20no%20ksx%20daemon%20control%20channel",
    );
    assert!(
        page.contains("error: no ksx daemon control channel"),
        "{page}"
    );
}

/// The attack this guard exists to stop, executed over a real socket.
///
/// A page on another site cannot read ksx's responses — but it never needed to.
/// A cross-origin `<form method="post">` is a CORS *simple request*: no
/// preflight, no permission, and the side effect lands before anyone could
/// object. `/map/preset/clear-all` wipes a preset; `/map/session/stop` ends a
/// game. The port is 4460 and is not a secret.
///
/// So this posts exactly what `evil.example` would post, byte for byte, and
/// requires that the scripted control never sees it.
#[test]
fn a_cross_site_form_post_is_refused_before_it_reaches_the_control() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control.clone());

    for path in [
        "/map/preset/clear-all",
        "/map/session/stop",
        "/session/stop",
        "/map/clear",
    ] {
        let body = "preset=IPAC+P1&slot=1";
        let response = http(
            addr,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                 Origin: https://evil.example\r\nConnection: close\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 403"),
            "{path} must refuse a cross-site write, got: {response}"
        );
        assert!(
            response.contains("refused a request from another site"),
            "the refusal must say what happened: {response}"
        );
    }

    // Not "it returned 403" — that the write never happened. A refusal that
    // still performed the write would pass a status-code assertion and fail
    // the user, so assert on what the control surface actually recorded.
    assert!(
        control.cleared.lock().unwrap().is_none(),
        "clear-all must not have run"
    );
    assert!(
        control.bound_with.lock().unwrap().is_none(),
        "no cross-site request may reach the control surface"
    );
}

/// The same routes, from ksx Studio's own page, still work.
///
/// A guard that also blocks the real UI is not a fix, and this is the assertion
/// that would fail if the origin comparison were tightened past correctness.
#[test]
fn the_pages_own_origin_still_writes() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control.clone());

    let body = "slot=1&function=A";
    let response = http(
        addr,
        &format!(
            "POST /map/clear HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             Origin: http://127.0.0.1:{port}\r\nConnection: close\r\n\
             Content-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {len}\r\n\r\n{body}",
            port = addr.port(),
            len = body.len(),
        ),
    );
    assert!(
        response.starts_with("HTTP/1.1 303"),
        "Studio's own form must still post: {response}"
    );
    assert!(
        control.bound_with.lock().unwrap().is_some(),
        "the write must actually have reached the control surface"
    );
}

/// DNS rebinding: the packet really does arrive on 127.0.0.1, so the bind
/// cannot tell. Only the name the browser asked for can, and a rebound request
/// carries the attacker's.
#[test]
fn a_rebound_host_cannot_even_read() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);

    let response = http(
        addr,
        "GET /api/map HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    );
    assert!(
        response.starts_with("HTTP/1.1 421"),
        "a rebound read must be refused, got: {response}"
    );
}

// ---------------------------------------------------------------------------
// /devices — the picker, end to end
// ---------------------------------------------------------------------------

/// The read. One PHYSICAL board per row (an I-PAC is one device to a human and
/// two devnodes here), the configured entry beside it, and the PORT-PINNED
/// paragraph in full — including the machine-specific half, which is the half
/// people miss and the reason a shared config silently stops matching.
#[test]
fn the_devices_page_lists_boards_not_devnodes_and_keeps_the_port_pinned_warning() {
    let addr = start_server(Arc::new(ScriptedControl::new(true)));
    let page = get(addr, "/devices");
    let body = body_of(&page);

    assert!(body.contains("Ultimarc I-PAC 4X"), "{body}");
    assert!(body.contains("1 keyboard-capable board"), "{body}");
    assert!(body.contains("1 [[device]] entry in config.toml"), "{body}");
    // The board with no keyboard interface is LISTED, not hidden: "ksx cannot
    // see my board" is a real support question.
    assert!(body.contains("NZXT fan controller"), "{body}");
    assert!(body.contains("PORT-PINNED"), "{body}");
    assert!(
        body.contains("do not copy this config to another cabinet"),
        "the machine-specific half of the warning must reach the page: {body}"
    );
    // The two words that decide whether the entry can capture anything. The
    // page carried `backend` in its row object and rendered it nowhere, so it
    // never said `winusb` or `interception` — the field the health pill above
    // is reasoning about — and `rung` was not carried at all.
    assert!(body.contains(">backend</span>"), "{body}");
    assert!(body.contains(">rung</span>"), "{body}");
    assert!(body.contains(">winusb<"), "{body}");
    assert!(body.contains(">port<"), "{body}");
    // Claiming needs elevation, so the command is TEXT and there is no form.
    assert!(body.contains("ksx winusb release"), "{body}");
    assert!(body.contains("ELEVATED shell"), "{body}");
    assert!(
        !body.contains(r#"action="/devices/claim""#),
        "a claim form on a surface that cannot elevate: {body}"
    );
}

/// A refused scan renders as a refusal, never as an empty machine. The two are
/// indistinguishable in the data and completely different to a person standing
/// at a cabinet with four boards plugged in.
#[test]
fn a_refused_scan_renders_the_refusal_rather_than_an_empty_list() {
    let addr = start_server_with_machine(
        Arc::new(ScriptedControl::new(true)),
        Arc::new(ScriptedMachine::refusing()),
    );
    let page = get(addr, "/devices");
    let body = body_of(&page);
    assert!(body.contains("could not be read"), "{body}");
    assert!(body.contains("run `ksx devices`"), "{body}");
    for claim in [
        ksx_api::NO_BOARDS_LINE,
        "no board it found exposes a",
        "No board is configured yet",
        "no [[device]] entries in config.toml",
    ] {
        assert!(
            !body.contains(claim),
            "a refused read printed an assertion of absence ({claim:?}): {body}"
        );
    }
}

/// **A scan that ANSWERS "I could not read the USB bus" is not an empty
/// cabinet either** — and this is the state nothing tested.
///
/// FAILS against the shipped page. `ScriptedMachine` only ever returned
/// `usb_available: true`, so no HTTP test could reach the path where the
/// enumeration itself failed; the page printed the banner "nothing could be
/// READ" and, directly beneath it, "No board here exposes a keyboard
/// interface". This is the shape of the failure that started the whole
/// project: a session reporting success while the arcade panel was dead
/// because a WinUSB board had fallen back to Interception. "I could not read
/// this" and "there is nothing here" are different sentences, and the user
/// acts on them differently.
#[test]
fn a_failed_enumeration_never_renders_as_an_empty_cabinet() {
    let addr = start_server_with_machine(
        Arc::new(ScriptedControl::new(true)),
        Arc::new(ScriptedMachine::blind()),
    );
    let body = body_of(&get(addr, "/devices")).to_owned();

    assert!(
        body.contains("nothing could be READ"),
        "the page must say the list is empty because nothing was read: {body}"
    );
    for claim in [ksx_api::NO_BOARDS_LINE, "no board it found exposes a"] {
        assert!(
            !body.contains(claim),
            "a failed enumeration printed the empty-machine sentence ({claim:?}): {body}"
        );
    }

    // The poller gets the same answer, in the field the island actually reads.
    // A page that got this right while `/api/devices` sent
    // `no_pickable_board_found: true` would go wrong two seconds later.
    let json: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/devices"))).unwrap();
    assert_eq!(
        json.pointer("/scan/no_pickable_board_found"),
        Some(&serde_json::json!(false)),
        "the poll licensed the island to draw an empty machine: {json}"
    );
    assert_eq!(
        json.pointer("/scan/usb_available"),
        Some(&serde_json::json!(false))
    );

    // And the ordinary cabinet is unaffected — it still has boards and says so.
    let ok = start_server(Arc::new(ScriptedControl::new(true)));
    assert!(body_of(&get(ok, "/devices")).contains("1 keyboard-capable board"));
}

/// A refusal degrades to `DeviceScanView::default()`, and that default must
/// license nothing. This is the invariant the `show:` flags depend on: they
/// read `no_pickable_board_found` / `no_configured_device` alone, with no
/// `&& unavailable.is_empty()` in either language, which is only sound while
/// every refusing path in `collect_devices` hands over a defaulted scan.
#[test]
fn a_refusal_serves_a_scan_that_asserts_nothing() {
    let addr = start_server_with_machine(
        Arc::new(ScriptedControl::new(true)),
        Arc::new(ScriptedMachine::refusing()),
    );
    let json: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/devices"))).unwrap();

    assert_ne!(
        json.pointer("/unavailable"),
        Some(&serde_json::json!("")),
        "the refusal itself must be on the wire: {json}"
    );
    assert_eq!(
        json.pointer("/scan/no_pickable_board_found"),
        Some(&serde_json::json!(false))
    );
    assert_eq!(
        json.pointer("/scan/no_configured_device"),
        Some(&serde_json::json!(false))
    );
    for line in ["/scan/boards_summary", "/scan/configured_summary"] {
        let value = json.pointer(line).and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            value.contains("nothing could be READ"),
            "{line} must say why it is empty, got {value:?}"
        );
    }
}

/// The page and the poller serve one shape.
#[test]
fn api_devices_serves_the_same_payload_the_page_embeds() {
    let addr = start_server(Arc::new(ScriptedControl::new(true)));
    let json: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/devices"))).unwrap();
    assert_eq!(
        json.pointer("/scan/boards/0/name"),
        Some(&serde_json::json!("Ultimarc I-PAC 4X"))
    );
    assert_eq!(
        json.pointer("/scan/configured/0/alias"),
        Some(&serde_json::json!("panel"))
    );
    // A poll is not an action.
    assert_eq!(json.pointer("/flash"), Some(&serde_json::json!(null)));
}

/// The pick write: 303 back to the page with the outcome as the flash, and the
/// spec that reached the backend is the KEYBOARD interface — not the board's
/// composite parent, which no resolver would accept.
#[test]
fn picking_a_board_calls_the_backend_and_redirects_with_the_outcome() {
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(Arc::new(ScriptedControl::new(true)), machine.clone());

    let response = post_form(
        addr,
        "/devices/pick",
        "query=USB%5CVID_D209%26PID_0430%26MI_00%5C7%2625EEA38C%260%260000&alias=panel",
    );
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("/devices?flash="), "{response}");

    let picked = machine.picked.lock().unwrap();
    assert_eq!(picked.len(), 1, "exactly one pick reached the backend");
    assert_eq!(picked[0].0, IPAC_KB);
    assert_eq!(picked[0].1.as_deref(), Some("panel"));
}

/// A blank name box is "derive one from the board", exactly like the absent
/// `--alias` flag. The form always submits the field, so the emptiness has to
/// survive the wire; `LocalMachine::device_pick` is what turns it back into
/// `None` before the writer sees it.
#[test]
fn a_blank_alias_still_posts_and_is_accepted() {
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(Arc::new(ScriptedControl::new(true)), machine.clone());

    let response = post_form(addr, "/devices/pick", "query=MI_00&alias=");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let picked = machine.picked.lock().unwrap();
    assert_eq!(picked[0].1.as_deref(), Some(""), "the form sends it empty");
}

/// The remove write, and the fact that surprises people: deleting the entry did
/// not release the board. It has to be in the flash, because the flash is all
/// the user sees on the way back.
#[test]
fn removing_an_entry_says_the_board_is_still_claimed() {
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(Arc::new(ScriptedControl::new(true)), machine.clone());

    let response = post_form(addr, "/devices/remove", "alias=panel");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("STILL%20CLAIMED"),
        "the flash must carry the claim warning: {response}"
    );

    let removed = machine.removed.lock().unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].0, "panel");
    assert!(
        !removed[0].1,
        "an unticked checkbox is not sent at all, so no --force"
    );
}

/// The checkbox is the consent, and HTML omits an unchecked box entirely — so
/// `force` is "present at all", never a parsed boolean.
#[test]
fn a_ticked_force_box_reaches_the_backend_as_force() {
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(Arc::new(ScriptedControl::new(true)), machine.clone());

    post_form(addr, "/devices/remove", "alias=panel&force=yes");
    assert!(machine.removed.lock().unwrap()[0].1, "--force must carry");
}

/// Both writes are POST and both sit inside the guarded router. The assertion
/// that matters is not the status code — it is that the WRITER never saw the
/// request.
#[test]
fn a_cross_site_post_never_reaches_the_device_writer() {
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(Arc::new(ScriptedControl::new(true)), machine.clone());

    for (path, body) in [
        ("/devices/pick", "query=MI_00&alias=stolen"),
        ("/devices/remove", "alias=panel&force=yes"),
    ] {
        let response = http(
            addr,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                 Origin: https://evil.example\r\nConnection: close\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 403"),
            "{path} must refuse a cross-site write, got: {response}"
        );
    }
    assert!(
        machine.picked.lock().unwrap().is_empty(),
        "no cross-site request may reach the device writer"
    );
    assert!(machine.removed.lock().unwrap().is_empty());
}

/// DNS rebinding: the read is guarded too, on every request, by NAME.
#[test]
fn a_rebound_host_cannot_read_the_device_list() {
    let addr = start_server(Arc::new(ScriptedControl::new(true)));
    let response = http(
        addr,
        "GET /api/devices HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    );
    assert!(
        response.starts_with("HTTP/1.1 421"),
        "a rebound read must be refused, got: {response}"
    );
}

/// The nav is static markup duplicated per island, so a page nobody links to is
/// a page nobody finds. Both existing screens must carry the link.
#[test]
fn every_page_links_to_the_device_picker() {
    let addr = start_server(Arc::new(ScriptedControl::new(true)));
    for route in ["/", "/map", "/devices"] {
        let page = get(addr, route);
        let body = body_of(&page);
        assert!(
            body.contains(r#"href="/devices""#),
            "{route} has no link to the device picker: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// /profiles — the games.toml profiles and the presets (v15)
// ---------------------------------------------------------------------------

/// The page renders both machine reads, and a profile whose program is gone is
/// broken ON THE PAGE with the path that is wrong — the whole reason this
/// screen exists. That fact used to surface only when a session refused to
/// start.
#[test]
fn the_profiles_page_shows_a_broken_profile_with_its_path() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    let response = get(addr, "/profiles");
    let body = body_of(&response);

    assert!(body.contains("Broken profiles"), "{body}");
    assert!(body.contains("MAME 4P"), "{body}");
    assert!(body.contains("D:\\emu\\mame\\mame.exe"), "{body}");
    assert!(body.contains("which does not exist"), "{body}");
    // The healthy one is listed too.
    assert!(body.contains("Street Fighter"), "{body}");
    // The presets and the in-box templates both arrived — the second is what
    // `LocalMachine::presets` used to answer with an empty list.
    assert!(body.contains("Arcade"), "{body}");
    assert!(body.contains("keyboard-2p"), "{body}");
}

/// The JSON twin serves the same shape the page embeds — one struct, one
/// serializer, like `/api/status` and `/api/map`.
#[test]
fn the_profiles_api_serves_the_pages_own_payload() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    let response = get(addr, "/api/profiles");
    assert!(response.contains("no-store"), "{response}");

    let value: serde_json::Value = serde_json::from_str(body_of(&response)).expect("json");
    assert_eq!(
        value.pointer("/profiles/profiles/1/state"),
        Some(&serde_json::json!("broken"))
    );
    assert_eq!(
        value.pointer("/profiles/profiles/1/broken_path"),
        Some(&serde_json::json!("D:\\emu\\mame\\mame.exe"))
    );
    assert_eq!(
        value.pointer("/presets/templates/0/id"),
        Some(&serde_json::json!("keyboard-2p"))
    );
    // A poll is not an action.
    assert_eq!(value.pointer("/flash"), Some(&serde_json::json!(null)));
}

/// Creating a profile: the form's own values reach the backend verb, and the
/// outcome comes back as a flash on a 303 — never HTML from a POST.
#[test]
fn creating_a_profile_reaches_the_verb_and_flashes_the_outcome() {
    let control = Arc::new(ScriptedControl::new(true));
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(control, machine.clone());

    let response = post_form(
        addr,
        "/profiles/new",
        "title=Tekken&path=C%3A%5Cgames%5Ctekken.exe&arguments=-windowed&slots=4&preset=Arcade",
    );
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("/profiles?flash="), "{response}");
    assert!(response.contains("created%20profile"), "{response}");

    let spec = machine
        .created_profile
        .lock()
        .unwrap()
        .clone()
        .expect("spec");
    assert_eq!(spec.title, "Tekken");
    assert_eq!(spec.path, "C:\\games\\tekken.exe");
    assert_eq!(spec.arguments, "-windowed");
    assert_eq!(spec.slots, 4);
    assert_eq!(spec.preset, "Arcade");
}

/// A refusal flashes too, prefixed `error:` so the page's `show:flashError`
/// pair picks the red side. Nothing fails silently.
#[test]
fn a_refused_profile_create_flashes_the_reason() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    let response = post_form(
        addr,
        "/profiles/new",
        "title=&path=C%3A%5Cx.exe&slots=1&preset=Arcade",
    );
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(response.contains("flash=error%3A"), "{response}");
}

/// A post with a field missing OR EMPTY still comes back as a 303 with a
/// worded flash, NOT a 422.
///
/// The distinction is not pedantry. The island fetch-submits and reads its
/// outcome out of the redirect's `?flash=` — a 422 carries no `Location`, so
/// the page would show nothing whatsoever and the user would be left pressing
/// a button that appears to do nothing. That is the failure mode this whole
/// screen replaced; it must not come back through the extractor.
///
/// The EMPTY cases are the ones that matter, and the earlier version of this
/// test did not have them: it covered only the absent key, which
/// `#[serde(default)]` already handled, so it passed against the broken build.
/// A browser sends `slots=` — present, empty — the instant a user clears a
/// non-`required` `<input type="number">`, and serde_urlencoded answers
/// "cannot parse integer from empty string" for an `Option<u8>`. The rows
/// below fail against the `Option<u8>` version with a 422 and no `Location`;
/// the `garbage` rows fail against any version that lets the extractor do the
/// parsing at all.
#[test]
fn a_post_with_a_missing_or_empty_number_still_flashes_instead_of_422() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    for (path, body, why) in [
        ("/profiles/new", "path=C%3A%5Cx.exe", "the key is absent"),
        (
            "/profiles/new",
            "title=T&path=C%3A%5Cx.exe&slots=&preset=Arcade",
            "the user cleared the slots box",
        ),
        (
            "/profiles/new",
            "title=T&path=C%3A%5Cx.exe&slots=lots&preset=Arcade",
            "the slots box holds something that is not a number",
        ),
        ("/profiles/preset/new", "name=Couch", "the key is absent"),
        (
            "/profiles/preset/new",
            "name=Couch&template=keyboard-2p&player=",
            "the user cleared the player box",
        ),
        (
            "/profiles/preset/new",
            "name=Couch&template=keyboard-2p&player=two",
            "the player box holds something that is not a number",
        ),
    ] {
        let response = post_form(addr, path, body);
        assert!(
            response.starts_with("HTTP/1.1 303"),
            "{path} must redirect with a flash when {why}, not reject the \
             body — a 422 carries no Location and the island renders it as \
             nothing at all: {response}"
        );
        assert!(
            response
                .to_ascii_lowercase()
                .contains("location: /profiles"),
            "{path} ({why}) must carry a Location the island can read a flash \
             out of: {response}"
        );
        assert!(response.contains("flash="), "{path} ({why}): {response}");
    }
}

/// A refusal arrives with its REMEDY, not just its message.
///
/// `flash_of` used to return `refusal.message` and drop `refusal.remedy`,
/// justified by "the page has a place for the remedy already: the no-daemon
/// banner". True of the control verbs it was written for; false of every
/// machine verb this page added. `preset-exists` is the case that proves it —
/// the message names the file it protected and the remedy names `--force`,
/// which is the only path forward that exists anywhere on this screen, and the
/// page has nowhere else that carries one.
///
/// Fails against the shipped version: the flash there stops at the filename.
#[test]
fn a_refusal_flashes_the_way_out_and_not_only_the_reason() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    let response = post_form(
        addr,
        "/profiles/preset/new",
        "name=Arcade&template=keyboard-2p&player=1",
    );
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response.contains("already%20exists"),
        "the refusal must name what it protected: {response}"
    );
    assert!(
        response.contains("--force"),
        "…and the flag that means yes, which is the only way forward on this \
         page: {response}"
    );
}

/// A REFUSED read must not render as an assertion of absence.
///
/// This is the page's own stated purpose turned on itself, and it is this
/// project's signature bug: a surface answering for a read it never completed
/// (the session that reported success while the arcade panel was dead because
/// a WinUSB board had silently fallen back to Interception).
///
/// Fails against the shipped version on the first two assertions: there,
/// `collect_profiles` substituted `ProfilesView::default()` / `PresetsView::
/// default()` on `Err`, so the page printed "no profiles in games.toml" and
/// "No presets on disk … Make a preset from an in-box template below first" —
/// the second of which points at a form whose `<select>` is empty for exactly
/// the same reason, so the only route it offers cannot succeed.
#[test]
fn a_refused_read_is_not_rendered_as_an_empty_machine() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server_with_machine(control, Arc::new(ScriptedMachine::reads_refusing()));
    let response = get(addr, "/profiles");
    let body = body_of(&response);

    assert!(
        !body.contains("no profiles in games.toml"),
        "a failed read must not be reported as an empty games.toml: {body}"
    );
    assert!(
        !body.contains("Make a preset from an in-box template below"),
        "a failed presets read must not send the user to a form fed by the \
         same read: {body}"
    );
    assert!(
        !body.contains(r#"action="/profiles/preset/new""#),
        "…and that form must not be on the page at all: {body}"
    );
    // What it says instead: the failure, and both reasons, in words.
    assert!(body.contains("could NOT be read"), "{body}");
    assert!(body.contains("expected `=` at line 4"), "{body}");
    assert!(body.contains("access is denied"), "{body}");
    // Each read's remedy travels with it — this string replaces the list the
    // user came for, so it is the one place that cannot be a dead end.
    assert!(body.contains("ksx config export --what games"), "{body}");
    assert!(body.contains("ksx doctor"), "{body}");

    // The JSON twin says it in a machine-readable field, not by omission.
    let value: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/profiles"))).expect("json");
    assert_eq!(
        value.pointer("/view/profiles_unreadable"),
        Some(&serde_json::json!(true)),
        "a poller must be able to tell a refused read from an empty one: \
         {value}"
    );
    assert_eq!(
        value.pointer("/view/presets_unreadable"),
        Some(&serde_json::json!(true)),
        "{value}"
    );
    assert_eq!(
        value.pointer("/view/no_presets_yet"),
        Some(&serde_json::json!(false)),
        "'no presets yet' is a claim about the FOLDER; nothing was read: \
         {value}"
    );
}

/// Creating a preset from a template, through the same `preset_new` verb
/// `ksx preset new` performs.
#[test]
fn creating_a_preset_from_a_template_reaches_the_verb() {
    let control = Arc::new(ScriptedControl::new(true));
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(control, machine.clone());

    let response = post_form(
        addr,
        "/profiles/preset/new",
        "name=Couch&template=keyboard-2p&player=2",
    );
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    let spec = machine
        .created_preset
        .lock()
        .unwrap()
        .clone()
        .expect("spec");
    assert_eq!(spec.name, "Couch");
    assert_eq!(spec.template, "keyboard-2p");
    assert_eq!(spec.player, 2);
    // Overwriting a 25-binding mapping is not something a web form may do by
    // accident; `--force` stays the CLI's consent step.
    assert!(!spec.force);
}

/// Switching profile is the SAME `ControlSource::start` the status page posts
/// — one backend verb, no second "switch" path — and it comes back to
/// /profiles so the user keeps their place.
#[test]
fn switching_profile_calls_start_and_returns_to_profiles() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control.clone());

    let response = post_form(addr, "/profiles/switch", "profile=MAME+4P");
    assert!(response.starts_with("HTTP/1.1 303"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("location: /profiles?flash="),
        "the redirect must come back HERE, not to / like the status page's \
         twin of this verb: {response}"
    );
    assert_eq!(
        control.started_with.lock().unwrap().clone(),
        Some(Some("MAME 4P".to_owned())),
        "the profile the row named must reach `start`"
    );
}

/// The guard is a router-wide layer, so a route declared in the same chain is
/// guarded by construction — but "by construction" is exactly the claim worth
/// testing, once, per new mutating route.
#[test]
fn the_profiles_write_routes_refuse_a_cross_site_post() {
    let control = Arc::new(ScriptedControl::new(false));
    let machine = Arc::new(ScriptedMachine::default());
    let addr = start_server_with_machine(control.clone(), machine.clone());

    for (path, body) in [
        (
            "/profiles/new",
            "title=Evil&path=C%3A%5Cevil.exe&slots=1&preset=Arcade",
        ),
        (
            "/profiles/preset/new",
            "name=Evil&template=keyboard-2p&player=1",
        ),
        ("/profiles/switch", "profile=MAME+4P"),
    ] {
        let response = http(
            addr,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                 Origin: http://evil.example\r\nConnection: close\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Content-Length: {len}\r\n\r\n{body}",
                port = addr.port(),
                len = body.len(),
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 403"),
            "{path} must refuse a cross-site POST: {response}"
        );
    }
    // Not "it returned 403" — that no write happened.
    assert!(machine.created_profile.lock().unwrap().is_none());
    assert!(machine.created_preset.lock().unwrap().is_none());
    assert!(control.started_with.lock().unwrap().is_none());
}

/// A rebound host cannot even READ the profile list. The same Host check
/// covers every route; it is asserted on this one because a page that lists
/// filesystem paths is worth naming explicitly.
#[test]
fn a_rebound_host_cannot_read_the_profiles() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    let response = http(
        addr,
        "GET /api/profiles HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 421"), "{response}");
}

/// Every page's nav must list every page, or a screen is unreachable. The nav
/// is static markup INSIDE each island — not server-injected, not a shared
/// component — so this is one edit per island, and exactly the kind that gets
/// forgotten.
#[test]
fn every_page_links_to_every_other_page() {
    let control = Arc::new(ScriptedControl::new(true));
    let addr = start_server(control);
    for route in ["/", "/map", "/devices", "/profiles", "/setup"] {
        let response = get(addr, route);
        let body = body_of(&response);
        for link in [
            r#"href="/""#,
            r#"href="/map""#,
            r#"href="/devices""#,
            r#"href="/profiles""#,
            r#"href="/setup""#,
        ] {
            assert!(
                body.contains(link),
                "{route} does not link to {link} — the page is unreachable from it"
            );
        }
    }
}

// ── /setup: the config first, and the first run ────────────────────────────

/// The page a first run lands on, over real HTTP: the configuration is the
/// first card, the two verbs are on it, and the checklist is what the backend
/// decided rather than anything this page worked out.
#[test]
fn the_setup_page_leads_with_the_config_and_its_two_verbs() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);
    let response = get(addr, "/setup");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("cache-control: no-store"),
        "a config read is point-in-time: {response}"
    );
    let body = body_of(&response);

    // The two verbs, both reachable with no JavaScript at all.
    assert!(body.contains(r#"href="/setup/export.json""#), "{body}");
    assert!(body.contains(r#"action="/setup/import""#), "{body}");
    // The checklist, straight off the provider.
    assert!(body.contains("Press a button and watch it land"), "{body}");
    assert!(body.contains(r#"class="step now""#), "{body}");
    // Onward, rather than duplicated: the board step belongs to /devices.
    assert!(body.contains(r#"href="/devices""#), "{body}");
    // The path is present exactly once, in the support line — never as a
    // control. This is the owner's brief, checked over the wire.
    let at = body.find("C:\\cfg").expect("the config root, for support");
    let smallprint = body.find(r#"class="smallprint""#).expect("support line");
    assert!(
        smallprint < at,
        "the config root must be inside the small print"
    );
    assert!(!body.contains(r#"href="C:\cfg"#), "{body}");
}

/// The page and the poller serve one shape (the parity render_setup.rs pins
/// server-side, observed here end to end).
#[test]
fn the_setup_api_serves_the_payload_the_page_embeds() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);
    let payload: serde_json::Value =
        serde_json::from_str(body_of(&get(addr, "/api/setup"))).expect("json");
    assert_eq!(payload["setup"]["available"], serde_json::json!(true));
    assert_eq!(
        payload["setup"]["view"]["config_exists"],
        serde_json::json!(true)
    );
    assert_eq!(payload["setup"]["view"]["steps"][2]["state"], "now");
    assert_eq!(payload["learn"]["state"], "idle");
    // A poll is not an action.
    assert_eq!(payload["flash"], serde_json::json!(null));
}

/// EXPORT is a download, not a path. The bytes come back with a file name
/// attached, so a plain `<a download>` finishes the job.
#[test]
fn export_hands_back_the_document_itself() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);
    let response = get(addr, "/setup/export.json");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("content-disposition: attachment; filename=\"ksx-config-"),
        "{response}"
    );
    assert!(
        response.contains("content-type: application/json"),
        "{response}"
    );
    assert!(body_of(&response).contains("ksx_interop"), "{response}");
}

/// The consent shape, over HTTP: no "write it" box, no write — and the answer
/// says so in a sentence short enough to have survived the redirect.
#[test]
fn import_is_a_dry_run_until_the_box_is_ticked() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);

    let document = "%7B%22ksx_interop%22%3A1%7D";
    let dry = post_form(addr, "/setup/import", &format!("document={document}"));
    assert!(dry.starts_with("HTTP/1.1 303"), "{dry}");
    assert!(dry.contains("nothing%20written%20yet"), "{dry}");
    assert!(
        !dry.contains("flash=error"),
        "a clean dry run is not an error: {dry}"
    );

    let applied = post_form(
        addr,
        "/setup/import",
        &format!("document={document}&apply=yes"),
    );
    assert!(applied.starts_with("HTTP/1.1 303"), "{applied}");
    assert!(applied.contains("imported%20config"), "{applied}");

    // A document that does not say what it is comes back as an ERROR flash —
    // never silence, and never a claim that something was written.
    let junk = post_form(addr, "/setup/import", "document=%7B%7D");
    assert!(junk.contains("flash=error"), "{junk}");
    // …and an empty box is refused before the provider is even asked.
    let empty = post_form(addr, "/setup/import", "document=");
    assert!(empty.contains("flash=error"), "{empty}");
    assert!(empty.contains("paste%20a%20configuration"), "{empty}");
}

/// Step 2 is ONE backend verb — `ControlSource::assign_slot`, the same pipe
/// verb `ksx slot assign` performs — and the flash says the pads replugged,
/// because that is the surprising half of the outcome.
#[test]
fn wiring_a_slot_goes_through_assign_slot_and_names_the_pad_bounce() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);

    let ok = post_form(addr, "/setup/slot", "slot=2&preset=IPAC+P1&profile=");
    assert!(ok.starts_with("HTTP/1.1 303"), "{ok}");
    assert!(ok.contains("slot%202%20now%20uses"), "{ok}");
    assert!(ok.contains("pads%20replugged"), "{ok}");

    // A preset that is not there is a refusal, flashed as one.
    let bad = post_form(addr, "/setup/slot", "slot=2&preset=Nope");
    assert!(bad.contains("flash=error"), "{bad}");
    // …and so is submitting the form with nothing chosen.
    let empty = post_form(addr, "/setup/slot", "slot=2&preset=");
    assert!(empty.contains("flash=error"), "{empty}");
}

/// Step 3 is the daemon's own learner, and it is operable with scripting off:
/// POST, 303, and the next render shows the state the poll would have.
#[test]
fn proving_a_button_uses_the_daemon_learner_with_no_javascript() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);

    let started = post_form(addr, "/setup/prove", "");
    assert!(started.starts_with("HTTP/1.1 303"), "{started}");
    assert!(started.contains("Listening"), "{started}");

    // The page itself now says the learner is listening — no client code
    // involved, which is what makes the <noscript> refresh enough.
    let page = body_of(&get(addr, "/setup")).to_owned();
    assert!(page.contains("press any button on the panel"), "{page}");
    assert!(page.contains(r#"action="/setup/prove/cancel""#), "{page}");

    let stopped = post_form(addr, "/setup/prove/cancel", "");
    assert!(stopped.starts_with("HTTP/1.1 303"), "{stopped}");
}

/// Every mutating /setup route is inside the guarded router, and the reads are
/// covered by the Host check like every other read. Written as a loop over the
/// routes rather than as one case, because the failure this catches is a route
/// added later and attached in the wrong place.
#[test]
fn the_setup_routes_are_guarded_like_every_other_one() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);

    for path in [
        "/setup/import",
        "/setup/slot",
        "/setup/prove",
        "/setup/prove/cancel",
    ] {
        let body = "document=%7B%7D&slot=1&preset=IPAC+P1";
        let response = http(
            addr,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                 Origin: https://evil.example\r\nConnection: close\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 403"),
            "{path} must refuse a cross-site write, got: {response}"
        );
    }

    // Reads too: a rebound name never reaches a handler, on any route.
    for path in ["/setup", "/api/setup", "/setup/export.json"] {
        let response = http(
            addr,
            &format!("GET {path} HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n"),
        );
        assert!(
            response.starts_with("HTTP/1.1 421"),
            "{path} must refuse a rebound read, got: {response}"
        );
    }
}

/// With no daemon, the config half of the page keeps working and the two verbs
/// stay live — Import and Export are the config store, not the pipe. The steps
/// that DO need a daemon say so instead of offering a dead button.
#[test]
fn the_config_verbs_survive_a_dead_daemon() {
    let control = Arc::new(ScriptedControl::dead());
    let addr = start_server(control);
    let body = body_of(&get(addr, "/setup")).to_owned();

    assert!(body.contains(r#"href="/setup/export.json""#), "{body}");
    assert!(body.contains(r#"action="/setup/import""#), "{body}");
    assert!(
        body.contains("Import and Export below still work"),
        "{body}"
    );
    // …and no live control that would silently do nothing.
    assert!(!body.contains(r#"action="/setup/slot""#), "{body}");
    assert!(!body.contains(r#"action="/setup/prove""#), "{body}");
    assert!(body.contains("the listener lives in the daemon"), "{body}");

    // An export still produces a document.
    assert!(
        get(addr, "/setup/export.json").starts_with("HTTP/1.1 200"),
        "the config store needs no daemon"
    );
}

/// A new page is invisible until the pages that already exist link to it.
#[test]
fn the_existing_pages_link_to_setup() {
    let control = Arc::new(ScriptedControl::new(false));
    let addr = start_server(control);
    for path in ["/", "/map"] {
        let body = body_of(&get(addr, path)).to_owned();
        assert!(
            body.contains(r#"href="/setup""#),
            "{path} must reach /setup from its nav: {body}"
        );
    }
}
