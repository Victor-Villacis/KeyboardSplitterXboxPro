# KeyboardSplitterXboxPro — Rust Rewrite Implementation Design

Target: Windows 11 Pro (build 26200 / 25H2), dedicated arcade cabinet, Ultimarc I-PAC encoders (`VID_D209`), backend first, UI later. Working name for the new binary: **`ksx`**.

---

## 1. Driver stack — committed choices

### 1.1 Virtual gamepad (output): **ViGEmBus 1.22.0 + vendored `vigem-client`**

- **ViGEmBus 1.22.0** (driver binary = 1.21.442.0) is the committed output driver. It is attestation/WHQL-signed by Microsoft's HW Compatibility Publisher, **already installed and running on the target machine**, unaffected by the April 2026 cross-signing trust removal, and is the only stack that produces *real* XInput slots via Microsoft's own `xusb22.sys` (no synthesis layer, no WGI double-input bug).
- Client: **vendor `vigem-client` (CasualX, MIT, ~2k LOC) into the repo** as `crates/vigem-client/`. Do not take a crates.io dependency on a 4-years-dormant crate for a load-bearing component. It is pure Rust (raw `DeviceIoControl` to `\\.\ViGEmBus`), no C DLL, and includes `get_user_index()` and the X360 `request_notification` API (rumble + LED player index) as of the commit included in 0.1.4.
- **Use `request_notification` from day one** even though the cab has no rumble — the LED/player-index callback is the authoritative slot mapping and it deletes the entire legacy `XInputWrapper` 30 Hz poller + the `!IsConnected && Tag == null` slot-guessing heuristic (root cause of the legacy "Slot is invalidated" bug family).
- **Bundle the ViGEmBus 1.22.0 setup EXE** in the installer (as Sunshine does); verify Authenticode publisher `Nefarius Software Solutions e.U.` before executing. Never download at runtime.
- ScpVBus, `VirtualXboxNative.dll`, `VirtualXbox.dll`, `XInputWrapper`, `devcon.exe` — **deleted entirely**, no compatibility path. The x64 ScpVBus cert expired in 2017 and the x86 one is test-signed garbage; a fresh install on this machine would fail anyway.
- Successor watch: HIDMaestro (plan B — MIT, reimplement its shared-memory client in Rust if ever needed) and libvirtualhid (plan C — revisit when Sunshine PR #5368 merges and XInput slot behavior is verified). Neither is acted on now; the `VirtualPadBackend` trait (§3) is the insurance policy.

### 1.2 Keyboard capture (input): **Interception now, WinUSB/`nusb` as the strategic primary**

Committed: a `CaptureBackend` trait with **two blocking backends plus one identify-only helper**:

1. **`InterceptionBackend`** (day-one default) — via the **`kanata-interception` 0.3.0** crate (jtroo's maintained fork, battle-tested by kanata's shipping builds). The driver is already installed and live on this cab. **This backend ships with a loud EOL warning**: the research confirms the machine is running the `{784c4414-…}` "Cross Certificates … Audit Policy" that will eventually block the 2012-cross-signed `keyboard.sys`/`mouse.sys`. On startup, the backend checks the signature on `%windir%\system32\drivers\keyboard.sys` and the active CI policy, and surfaces "this backend is on borrowed time" plus the 10-keyboard-slot exhaustion counter (detect ID climb past 10 → report "reboot required" instead of going silently deaf). Licensing note recorded in the repo: LGPL non-commercial, dynamic-link `interception.dll` only through the published API; fine for a personal cab, closed for commercial distribution.
2. **`WinUsbBackend`** (target state for I-PACs, built in M6) — rebind each I-PAC's keyboard interface `USB\VID_D209&PID_0430&MI_00` to in-box `winusb.sys`, read interrupt-IN HID reports directly with **`nusb`** (0.2.x, very active, pure Rust). Blocking is structural (the interface leaves the keyboard class stack — Windows never sees a keystroke), identity is structural (one `nusb::Device` per board), no 10-device ceiling, no ID drift, immune to the 2026 signing cliff (winusb.sys is inbox). Leave `MI_01`/`MI_02` bound normally so the trackball/spinner keeps working. Recovery documented: `pnputil /remove-device` + a non-claimed keyboard on a spare port.
3. **`RawInputIdentify`** (not a blocking backend) — `RegisterRawInputDevices` + `RIDEV_INPUTSINK` on a message-only window, used only for the "press a key on Player 1's panel" device-picker wizard and diagnostics. **The Raw Input + `WH_KEYBOARD_LL` correlation hack is explicitly rejected** as a blocking backend: its failure mode (ambiguous simultaneous identical keys from identical devices) is precisely the two-I-PACs-mashing-fighting-game case.

Hardware escape hatch (documented, not built): I-PAC Multi-Mode firmware 1.5x dual-XInput gives 2 pads/board with zero software.

**HidHide: not used** — it cannot hide keyboards/mice (nefarius states this directly), and the cab's inputs are keyboards. Revisit only if real gamepads are ever plugged into the cab alongside virtual ones.

---

## 2. Cargo workspace layout

```
/ (repo root)
├── Cargo.toml                     # [workspace], resolver = "2"
├── crates/
│   ├── ksx-core/                  # PURE. No windows deps, no I/O, no threads.
│   │   # Key/scancode newtypes, preset & mapping model, slot state machine,
│   │   # the translation engine: (DeviceId, KeyEvent) -> Vec<PadDelta>.
│   │   # Deps: bitflags, thiserror. 100% CI-testable; proptest here.
│   ├── ksx-config/                # TOML schema + load/save + versioning + paths.
│   │   # Deps: serde, toml, directories, thiserror. Depends on ksx-core (types).
│   ├── ksx-legacy-import/         # splitter_presets.xml / splitter_games.xml / v1-schema -> config.
│   │   # Deps: quick-xml, encoding_rs (UTF-16 files!). Depends on ksx-core, ksx-config.
│   ├── ksx-capture/               # trait CaptureBackend + backends.
│   │   # interception.rs (kanata-interception), winusb.rs (nusb), rawinput_identify.rs.
│   │   # Deps: kanata-interception, nusb, windows-sys, crossbeam-channel. Depends on ksx-core.
│   ├── ksx-output/                # trait VirtualPadBackend + vigem.rs + xinput_verify.rs.
│   │   # Deps: vigem-client (path = "../vigem-client"), rusty-xinput (dev/test only).
│   ├── vigem-client/              # vendored CasualX/vigem-client (MIT), LICENSE preserved.
│   ├── ksx-platform/              # windows-rs plumbing: thread priority, WM_DEVICECHANGE /
│   │   # CM_Register_Notification hotplug, driver state detection (signature + CI policy),
│   │   # driver install orchestration (pnputil, bundled installers), Task Scheduler autostart.
│   │   # Deps: windows 0.62 (tight feature gates), windows-sys.
│   ├── ksx-games/                 # game registry, GameStatus validation, launch + >3s exit
│   │   # detection, block-flags application. Deps: ksx-core, ksx-config.
│   └── ksx-app/                   # bin `ksx`. Orchestrator: thread supervision, wiring,
│       # emergency escapes policy, clap CLI, tracing, config hot-reload (notify).
│       # Subcommands: run | devices | monitor | pads | test | import-legacy |
│       #              install-drivers | autostart | doctor
├── legacy/                        # the entire C# solution (see §6)
└── docs/                          # ARCHITECTURE.md, DRIVERS.md, RECOVERY.md
```

Dependency graph (arrows = depends-on):

```
ksx-app ──→ ksx-capture ──→ ksx-core
   │            └─────────→ (kanata-interception, nusb, windows-sys)
   ├──────→ ksx-output ───→ vigem-client (vendored), ksx-core
   ├──────→ ksx-games ────→ ksx-config ──→ ksx-core
   ├──────→ ksx-legacy-import ──→ ksx-config, ksx-core
   └──────→ ksx-platform ─→ windows / windows-sys
```

Published crates committed: `windows` 0.62.x + `windows-sys` (Microsoft), `kanata-interception` 0.3.0, `nusb` 0.2.x, `crossbeam-channel`, `serde`/`toml`, `quick-xml`, `encoding_rs`, `directories`, `notify`, `clap`, `tracing` + `tracing-appender`, `anyhow`/`thiserror`, `bitflags`, `proptest` + `insta` (dev), `rusty-xinput` (dev), `hdrhistogram` (dev/diagnostic). **No tokio anywhere in v1** — the hot path is dedicated OS threads; if a UI/IPC process appears later, `interprocess` (named pipes) confined to that crate.

---

## 3. Core domain model & pipeline

### 3.1 Types (all in `ksx-core` unless noted)

```rust
/// Stable identity = device instance path, NEVER a positional index.
/// Legacy bug being fixed: Keyboard_01..10 were Interception slot indices.
pub struct DeviceId(String);          // e.g. HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000

pub struct KeyEvent { pub device: DeviceId, pub key: Key, pub down: bool, pub t: Instant }

/// ONE key enum. Collapses legacy InterceptionKey + InputKey (kept in lockstep by a
/// runtime assertion) into a single ushort-backed scancode-derived type, including the
/// mouse pseudo-keys (MouseLeftButton..MouseMoveDown) for slot mouse support.
pub enum Key { /* scancode set + E0/E1 extended handling + mouse pseudo-keys */ }

/// Pad state is kept in XInput wire shape (== vigem_client::XGamepad) so no backend
/// needs a translation layer.
pub struct PadState { buttons: XButtons, lt: u8, rt: u8, lx: i16, ly: i16, rx: i16, ry: i16 }

pub enum Binding {                    // one preset entry
    Button(XButton),                  // legacy <button id>
    Trigger(Trigger),                 // legacy <trigger id>
    Axis { axis: Axis, value: i16 },  // legacy <axis id value> — custom values preserved
    Dpad(DpadDirection),              // legacy <dpad direction>
}
// Legacy <custom function> collapses into Binding: the flat XboxCustomFunction enum was
// only a UI convenience. The importer maps it; many-keys-to-one and one-key-to-many are
// both native (Preset = Vec<(Key, Binding)>, no uniqueness constraint either way).

pub struct Preset { name: String, entries: Vec<(Key, Binding)>, protected: bool }

pub struct Slot {
    number: u8,                          // 1..=4
    keyboard: Option<DeviceId>,
    mouse: Option<DeviceId>,
    preset: PresetRef,
    pad: PadHandle,                      // from VirtualPadBackend
    user_index: Option<u8>,              // XInput dwUserIndex, read back post-plug
    invalidation: Option<InvalidationReason>,  // ported 13-variant enum, each with context
}
```

### 3.2 The two traits (the load-bearing abstraction)

```rust
// ksx-capture
pub trait CaptureBackend: Send {
    fn devices(&self) -> Vec<DeviceInfo>;                       // id, friendly name, kind
    fn set_captured(&mut self, ids: &[DeviceId]) -> Result<()>; // captured == blocked from OS
    fn run(self, tx: Sender<KeyEvent>, ctl: Receiver<CaptureCtl>) -> Result<()>; // owns its thread loop
}

// ksx-output
pub trait VirtualPadBackend {
    fn plug(&mut self) -> Result<PadHandle>;
    fn user_index(&self, h: PadHandle) -> Option<u8>;
    fn update(&mut self, h: PadHandle, s: &PadState) -> Result<()>;
    fn poll_feedback(&mut self, h: PadHandle) -> Option<Feedback>; // LED + rumble
    fn unplug(&mut self, h: PadHandle) -> Result<()>;
}
```

### 3.3 Pipeline & threading (the #1 fix over legacy)

```
[capture thread]   blocking interception_receive / nusb interrupt read.
  THREAD_PRIORITY_TIME_CRITICAL, zero-alloc, zero-lock. Its ONLY jobs:
  resolve device id, decide pass/suppress (precomputed capture set via arc-swap
  snapshot), push KeyEvent into a bounded crossbeam-channel, and — for
  Interception — re-send non-captured strokes. Nothing else. Ever.
        │ crossbeam-channel (bounded 1024; if full, drop + count, never block producer)
        ▼
[engine thread]    ksx-core Engine: per-device key-state table → per-slot binding lookup
  (precompiled Key -> SmallVec<(slot, Binding)> index, no per-event iteration/LINQ-alikes)
  → PadState mutation with legacy semantics preserved:
    • all-keys-up rule: a function releases only when EVERY key mapped to it is up
    • opposite-axis rule: releasing Left while Right held snaps to Right's bound value
      (FIX: snap to the opposite *binding's* value, not hardcoded ±32767 — resolves the
      legacy custom-axis inconsistency, Splitter.cs:161)
    • one keyboard → many slots stays supported (no break in slot iteration)
    • emergency escapes evaluated here: Ctrl+Alt+Del combo → stop emulation;
      LCtrl ×5 → toggle keyboard capture; RCtrl ×5 → toggle mouse capture (audio cues)
  → diff new PadState vs last-submitted → emit only genuine deltas.
        │ per-pad slot in a triple-buffer / channel
        ▼
[output thread]    vigem update() per changed pad; poll_feedback for LED/rumble;
                   no ownership-check IOCTL per press (legacy did 2 syscalls/press).

[main thread]      clap / supervision / config hot-reload (notify) / later: tray + UI.
```

**Latency strategy:** the legacy app synchronously `Dispatcher.Invoke`d every keystroke onto the WPF UI thread (system-wide input freeze if UI stalled — the app warns about it at startup). Here the UI can crash entirely and the capture→engine→output path never notices. Instrumentation: `Instant` at capture and at IOCTL submit, `hdrhistogram` behind `ksx doctor --latency`; budget p99 < 1 ms capture→submit. Shutdown is cooperative: a control message + `interception_wait_with_timeout` loop (no `Thread.Abort` equivalent exists or is needed). A crash-safe drop guard re-sends/uncaptures everything so a panic never leaves keyboards dead.

### 3.4 Legacy behaviors preserved / dropped

**Preserved:** 4-slot model with decoupled slot-number vs XInput user index; per-slot keyboard+mouse; global block-keyboards/block-mice flags scoped to assigned devices only; emergency escapes; input monitor (as `ksx monitor`, engine-thread ring buffer, not a per-line file open); slot invalidation reasons (with root-cause context strings — legacy collapsed 13 causes into one confusing message); device suggestion for new slots; mouse-move deadzone + 50 ms auto-release of wheel/move pseudo-keys; games list + CLI autostart.
**Dropped:** the `xinput.dll` SubType proxy (port last or never), Xbox 360 Accessories check, single-EXE resource-extraction/`%TEMP%`/PATH tricks (modern AV bait — ship a normal installed layout), the XInput 30 Hz poller, `requireAdministrator` at runtime (admin only for `ksx install-drivers`).

---

## 4. Config strategy

### 4.1 New native format: TOML

Location: `%APPDATA%\ksx\` via `directories`, with a **portable override** (`ksx.toml` next to the exe wins — arcade cabs love portable). Files:

```toml
# %APPDATA%\ksx\config.toml
schema_version = 1

[settings]
block_keyboards = true
block_mice = false
mouse_move_deadzone = 5          # 0..12
starting_user_index = 1          # 1..4

[[device]]                        # stable identity, learned via the picker wizard
id = "HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"
alias = "P1 I-PAC"
backend = "interception"          # or "winusb"

[[slot]]
number = 1
keyboard = "P1 I-PAC"             # alias ref
preset = "street-fighter-p1"
```

```toml
# %APPDATA%\ksx\presets\street-fighter-p1.toml   (one file per preset — diffable, shareable)
name = "street-fighter-p1"
[bindings]
A = "S"                # button = key
B = "D"
lt = "Q"
"lx.min" = "Left"      # axis endpoints
"lx.-16384" = "None"   # custom axis values are first-class, not a hand-edit hack
dpad.up = "I"
# many-to-one and one-to-many both allowed; arrays where needed: A = ["S", "Enter"]
```

Games move to `%APPDATA%\ksx\games.toml` (same schema content as legacy: title, path, args, block flags, per-slot device-by-id + preset-by-name). Log via `tracing-appender` daily-rolling in `%APPDATA%\ksx\logs\`. Lenient parse (unknown keys warned, not fatal — legacy threw on any unknown XML node) + automatic timestamped backup before any migrating write.

### 4.2 Migration: `ksx import-legacy`

`ksx-legacy-import` reads the legacy **UTF-16** XML with `quick-xml` + `encoding_rs`:

- `splitter_presets.xml` → per-preset TOML. Must honor the exact legacy ID tables (XboxButton `0x0010..0x8000`, triggers `0x10000/0x20000`, axes `1/2/4/8` with signed 16-bit `value`, dpad flags, and the flat 26-endpoint `XboxCustomFunction` space) and the v1 legacy schema upgrader semantics (`LeftTrigger`→`Left`, `<pov>`→dpad).
- `splitter_games.xml` → `games.toml`; hardware-ID strings map onto `DeviceId` (legacy already persisted HWIDs here, so this is clean).
- Ships with **golden-file tests**: real preset XML corpora in `crates/ksx-legacy-import/tests/fixtures/` → `insta` snapshots of emitted TOML. The `default` protected preset is re-created natively (including its `Button_A → Enter` custom-function quirk); the phantom `empty` preset becomes a real built-in (that's what the docs always promised).
- One-shot: XML is never the native format and no XML writer is ever built.

---

## 5. Milestone plan (backend first)

| M | Scope | Exit criteria | Manually testable on the cab |
|---|---|---|---|
| **M0 — Repo & scaffold** | Move C# to `/legacy` (tag `legacy-csharp-final` first), workspace skeleton, all crates compiling with stub traits, CI (fmt+clippy+test on `windows-latest`), README rewrite, vendored `vigem-client` building. | `cargo test` green in CI; `ksx --version` runs. | Nothing yet (nothing regressed either — legacy exe still works from `/legacy` releases). |
| **M1 — Pure core + config + import** | `ksx-core` engine (bindings, all-keys-up, opposite-axis incl. custom-value fix, diffing), `ksx-config`, `ksx-legacy-import`. Proptest invariants: no stuck keys, unplug-mid-press releases all, SOCD determinism. | Property + golden tests green in CI; `ksx import-legacy` converts the user's real preset/game XML with zero warnings. | Run `ksx import-legacy` against the cab's actual `splitter_presets.xml`/`splitter_games.xml`; eyeball the TOML. No drivers touched. |
| **M2 — Output layer** | `ksx-output/vigem.rs`, feedback/LED channel, `ksx pads plug/test`. **XInput loopback test**: push state via vigem, read back via `rusty-xinput`, assert (cab-only, feature-gated). | Loopback test passes on the cab; `get_user_index` deterministic across plug order. | `ksx pads test 4` → 4 pads visible in `joy.cpl` and hardwaretester.com/gamepad, LED indices correct, then verified in one real XInput-only game. |
| **M3 — Capture layer** | `InterceptionBackend` on `kanata-interception`, `RawInputIdentify` picker, `ksx devices` / `ksx monitor` (per-device live scancodes), driver EOL + CI-policy warning, 10-slot exhaustion detection. Capture/suppress toggle. | Each physical I-PAC shows a distinct stable `DeviceId`; suppression verified (keys vanish from Notepad while captured); emergency LCtrl×5 un-captures. | Press buttons on each panel, watch `ksx monitor` attribute them correctly; yank/replug USB and confirm identity survives. |
| **M4 — End-to-end parity** | Wire capture→engine→output; slots, invalidation reasons, hotplug (`CM_Register_Notification`, not 1 Hz polling), emergency escapes, config hot-reload, crash-safe uncapture guard. This is **feature parity with the legacy app minus UI**. | Two-player session in a real game driven entirely by `ksx run`; kill -9 the process mid-game → all keyboards instantly live again. | The real thing: 2–4 players on the cab; Ctrl+Alt+Del stop; LCtrl×5 toggle; latency histogram via `ksx doctor --latency` (p99 < 1 ms). |
| **M5 — Games, autostart, packaging** | `ksx-games` (launch, >3 s exit rule, `ksx run --game "Title"` autostart), Task Scheduler autostart command, `ksx install-drivers` (bundled ViGEmBus setup + Interception installer, signature-verified, pnputil where applicable), Inno Setup installer + `cargo-dist` release pipeline, `docs/RECOVERY.md`. | Cold-boot cab → autostart → frontend launches game → pads live → game exit stops emulation. | Full boot-to-game flow with LaunchBox/frontend; uninstall/reinstall cycle. |
| **M6 — WinUSB backend** | `WinUsbBackend` via `nusb`: rebind `MI_00` of each I-PAC, boot-protocol + NKRO report parsing (pull descriptor at runtime), make it default for `VID_D209`; Interception demoted to fallback for generic keyboards. | Side-by-side latency + correctness vs Interception on the cab; recovery procedure rehearsed. | Same 2-player session with Interception driver *uninstalled* — proves the post-2026 survival path. |
| **M7 — UI (later, separate decision)** | Recommended: **egui/eframe + `tray-icon`** in-process (device picker, live monitor, preset editor, pad tester — the legacy tester was its best feature). Tauri v2 only if the UI becomes a product surface; Iced revisit at 1.0. Decision deliberately deferred; the backend's channel-based design means any of these bolt on without touching the pipeline. | UI never appears in any input-path profile. | Configure a new preset end-to-end without editing TOML by hand. |

---

## 6. Repo strategy (within the existing fork)

- **Tag first**: `legacy-csharp-final` on current `master` before any restructuring.
- **`git mv` the entire C# tree into `/legacy/`** (all 8 projects, `.sln`, `Xbox360Accessories_x64_1.2.exe`, the PNG diagrams) in one commit — history stays traceable with `git log --follow`. `/legacy/README.md` states: unmaintained reference implementation, VS2013 toolchain, kept for behavior archaeology and as the source of golden-test fixtures; not expected to build.
- **New Rust workspace at repo root** (`Cargo.toml`, `crates/`, `docs/`) — root ownership signals the rewrite is the project now. Work directly on `master` (it's the user's fork; no upstream PRs are ever going back to djlastnight).
- **Root README rewrite**: what ksx is, driver stack + why (ViGEmBus/Interception/WinUSB with the 2026 signing story), quick start, `ksx import-legacy` migration guide, link to `/legacy`, credits block retained (djlastnight, oblitum, nefarius, shauleiz), and a **LICENSE decision** — the fork currently has *no* license file anywhere; new Rust code should get MIT or Apache-2.0/MIT dual, with `docs/DRIVERS.md` recording third-party terms (Interception LGPL/non-commercial, vendored vigem-client MIT, ViGEmBus BSD-3-Clause installer redistribution). Do **not** carry `devcon.exe` or the legacy embedded binaries anywhere into release artifacts.
- `.github/workflows/`: `ci.yml` (fmt, clippy `-D warnings`, `cargo test` on `windows-latest` — driver-touching tests behind a `cab-tests` feature) and `release.yml` (`cargo-dist` + Inno).

---

### Critical Files for Implementation

Legacy sources that encode the behavior the Rust code must reproduce (the golden references for M1's engine and importer):

- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Models/Splitter.cs — translation pipeline: all-keys-up rule, opposite-axis snap, per-slot dispatch, state diffing, emergency handling
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Presets/Preset.cs — bidirectional custom-function/GetKeys aggregation (the subtlest domain rule) + the `default` preset contents
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/Interceptor/Interception.cs — capture loop, suppress/pass-through semantics, scancode correction, device-ID conventions to replicate/replace
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Presets/PresetDataManager.cs — legacy XML lifecycle/quirks (UTF-16, protected-preset stripping, backup-on-parse-failure) the importer must handle
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/VirtualXbox/Enums/XboxCustomFunction.cs — the bit-exact ID tables (with siblings in the same directory) required for preset import fidelity