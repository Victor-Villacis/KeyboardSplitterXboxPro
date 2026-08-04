# Keyboard → Virtual Gamepad Splitter on Windows 11: Prior Art & Rust Architecture

## 1. Prior art in 2026 — is there a modern replacement?

**Honest answer: no. Nothing on the market does per-physical-keyboard capture → N virtual XInput pads except djlastnight's tool and AHK-based UCR.** Your fork is justified. Details:

| Tool | Status 2026 | Per-device (multi-keyboard) split? | Notes |
|---|---|---|---|
| **KeyboardSplitterXbox** (your base) | Abandoned ~2016, C#/WPF, XML presets | **Yes** — Interception + ScpVBus/vXboxInterface | Only tool that does exactly this. ScpVBus/vXbox is even more dead than ViGEm. |
| **Keyboard2Xinput** (RDCH106 / SchwingSK) | C#/.NET, INI config, ViGEmBus. Explicitly built for **I-PAC arcade panels** | **No** — one logical keyboard split by key ranges (`[pad1]`, `[pad2]`) | Closest living relative and the best design reference. Doesn't distinguish keyboards, so two I-PACs collide. |
| **reWASD** | Commercial, actively developed | **No** — devs confirm on their own forum that one device → multiple virtual pads is not supported, and emulated keyboards/mice are not separately recognizable. IPAC users repeatedly asked, still unimplemented | Rules out the obvious "just buy something" answer. |
| **UCR** (evilC / Snoothy) | AHK-based, perpetual alpha, stagnant | **Yes** — via AutoHotInterception + Interception, output to vJoy/ViGEm | Functionally the nearest thing. Worth mining its "Core_Interception" plugin design and its device-subscription model. |
| **XOutput / x360ce / InputMapper** | DirectInput→XInput translators | No keyboard-splitting at all | Not relevant except as ViGEm client examples. |
| **kb2xbox** (mPyKen), **xarcade-xinput** (mikew) | Small, low-activity | No / single keyboard | xarcade-xinput is the Xbox-arcade variant, JS+ViGEm. |
| **Keysticks** | Effectively dead (last real work ~2016) | Inverse direction (pad → keyboard) | Not applicable. |
| **JoyShockMapper** (Electronicks fork) | **Actively maintained** | Gyro/controller → KBM, wrong direction | Good reference for a clean config-DSL + hot-reload design. |
| **kanata** (jtroo) | **Actively maintained, Rust** | Yes, via Interception (`kanata_wintercept.exe`) | **The single most valuable Rust prior art.** It solves exactly your input-capture half in Rust and ships `kanata-interception` (a maintained safe Interception wrapper). Read its Windows backend before writing a line. |

**Reusable pieces:** kanata's Interception layer and Windows build/packaging; Keyboard2Xinput's I-PAC-specific mapping semantics and toggle-key UX; UCR's per-device subscription model; your own fork's XML preset corpus (write an importer, don't keep XML as the native format).

Sources: [KeyboardSplitterXbox](https://github.com/djlastnight/KeyboardSplitterXbox) · [Keyboard2Xinput](https://github.com/RDCH106/Keyboard2Xinput) · [reWASD forum: mapping keyboard to 2 controllers](https://forum.rewasd.com/forum/rewasd/technical-questions-aa/220587-mapping-keyboard-to-2-controllers) · [reWASD: emulated keyboards not separately recognized](https://forum.rewasd.com/forum/rewasd/technical-questions-aa/244735-can-emulated-keyboards-mice-be-recognized-separately) · [UCR](https://github.com/Snoothy/UCR) · [UCR Core_Interception](https://github.com/snoothy/ucr/wiki/Core_Interception) · [kanata](https://github.com/jtroo/kanata) · [kanata-interception](https://lib.rs/crates/kanata-interception) · [xarcade-xinput](https://github.com/mikew/xarcade-xinput) · [kb2xbox](https://github.com/mPyKen/kb2xbox) · [JoyShockMapper](https://github.com/Electronicks/JoyShockMapper)

---

## 2. The two driver decisions (these dominate the architecture)

### 2a. Output: virtual gamepad — ViGEmBus is retired but still the only XInput-correct option

- **ViGEmBus was retired 2023-11-02** over a trademark dispute with ViGEM GmbH, not for technical reasons. v1.22.0 is production-signed for Win10/11 x86/amd64/arm64 and still works on Win11 24H2/25H2. LizardByte's fork was also **archived (Aug 2025)**.
- **The successor everyone points to is LizardByte's `libvirtualhid`** (Sunshine PR #5368, July 2026): C++ core (MIT) + a **UMDF2 user-mode control driver backed by Microsoft's Virtual HID Framework (VHF)** — deliberately no kernel driver — driver source and MSI under LizardByte Source-Available License 1.0, WiX-built MSI, AMD64 only for now. Sunshine still falls back to ViGEmBus on ARM64 and when libvirtualhid's driver is absent.
- ⚠️ **Critical caveat for an arcade cab:** VHF publishes **HID** gamepads. Classic `XInput1_3/1_4` does **not** enumerate generic HID gamepads — it sees XUSB (`xusb22.sys`), GIP (`xboxgip.sys`), or HID devices that `xinputhid.sys` binds by hardware ID (Microsoft VID_045E + specific Xbox PIDs). GameInput/WGI *can* map HID gamepads via `HKLM\SYSTEM\CurrentControlSet\Control\GameInput\Devices` + `HKLM\SOFTWARE\Microsoft\GameInputRedist`, but a 2015-era XInput-only fighting game will not benefit. **ViGEmBus emulates the XUSB interface, which is why it "just works" everywhere.**
- **Recommendation:** target ViGEmBus 1.22.0 now via the pure-Rust **`vigem-client`** crate (100% Rust ioctls, no ViGEmClient.dll — one less native dep), but put it behind a `trait VirtualPadBackend` from day one and spike libvirtualhid on the actual cab with your actual games before betting on it. Also note **XInput itself caps at 4 controllers**, which is where the original tool's "up to 4 pads" limit comes from — that's a Windows limit, not a ViGEm one.

Sources: [ViGEmBus (archived)](https://github.com/nefarius/ViGEmBus) · [ViGEm EOL statement](https://docs.nefarius.at/projects/ViGEm/End-of-Life/) · [LizardByte fork (archived)](https://github.com/LizardByte/Virtual-Gamepad-Emulation-Bus) · [Sunshine PR #5368 → libvirtualhid](https://github.com/LizardByte/Sunshine/pull/5368) · [libvirtualhid](https://github.com/LizardByte/libvirtualhid) · [VHF docs](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/virtual-hid-framework--vhf-) · [GameInput device types / XUSB vs XInputHID vs GIP](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/hardware/input-hardware-interfaces) · [DirectInput and XUSB Devices](https://learn.microsoft.com/en-us/windows/win32/xinput/directinput-and-xusb-devices) · [Joystick-Input-Examples (best plain-English writeup)](https://github.com/MysteriousJ/Joystick-Input-Examples) · [vigem-client](https://github.com/CasualX/vigem-client)

### 2b. Input: per-keyboard capture **with blocking** — three options, ranked

1. **Interception (oblitum) — recommended, keep it.** Still the only thing that gives you *device-identified* strokes **and** suppression, at kernel level (so it also survives UIPI/elevated-window focus). Rust bindings exist and are proven at scale by kanata: `interception-sys` (raw FFI) or **`kanata-interception`** (maintained safe wrapper, actively used). Caveats: upstream is quiet, README still says "XP to Windows 10", **licensing is LGPL for non-commercial with a paid commercial license**, and it's a kernel filter driver on `kbdclass` — track whether it stays HVCI/memory-integrity-clean on 24H2+, since that's where old kernel drivers get blocked.
2. **Raw Input + low-level hook hybrid — don't.** Raw Input identifies the device but cannot block; `WH_KEYBOARD_LL` blocks but has no device identity, and installing an LL hook starves the Raw Input path. Documented as flaky, and Win key / Alt-Tab / lock keys are unblockable. `multiinput` crate exists if you only ever need *identification* without blocking (e.g. a "which keyboard did you just press?" wizard in the config UI — genuinely useful there).
3. **Bypass Windows entirely: rebind the I-PAC to WinUSB and read HID reports directly.** Windows opens keyboard/mouse HID collections **exclusively**, so `hidapi` can never read a keyboard — but on a *dedicated arcade cab* you can install a WinUSB INF over the I-PAC's interface (libwdi/Zadig-style) so it stops being a keyboard at all, then read interrupt-endpoint reports with **`nusb`** (pure Rust, async, no libusb). Zero kernel driver, zero suppression problem, zero leakage into Windows, lowest latency. Downside: the panel no longer types anywhere, it's per-device INF surgery, and it's useless on a normal PC. **Worth a weekend spike as `backend-winusb` — for your specific use case it may be the best long-term answer.**

Sources: [Interception](https://github.com/oblitum/Interception) · [interception-sys](https://crates.io/crates/interception-sys) · [kanata Windows variants](https://deepwiki.com/jtroo/kanata/1.3-installation-and-platform-support) · [LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc) · [combining LL hook + RawInput, and its flaws](https://hackaday.io/project/5364-cheap-windows-jogkeyboard-controller-for-cncs/log/16845-combing-the-keyboard-hook-with-rawinput) · [multiinput-rust](https://github.com/Jonesey13/multiinput-rust) · [hidapi: keyboards never openable on Windows](https://github.com/libusb/hidapi/issues/135) · [nusb](https://github.com/kevinmehall/nusb) · [libwdi/Zadig](https://github.com/pbatard/libwdi/wiki/faq)

---

## 3. Rust on Windows: process model, IPC, concurrency, config

**windows-rs is mature and is the default.** Microsoft-maintained, `windows` 0.62.x, generated from official Windows metadata, covers Win32 + COM + WinRT. `winapi` is legacy — don't start there. Use `windows` with tight feature gates (`Win32_UI_Input`, `Win32_Devices_HumanInterfaceDevice`, …) to keep compile times sane, or `windows-sys` for the raw-FFI bits where you don't want the COM machinery. ([windows-rs](https://github.com/microsoft/windows-rs) · [Rust for Windows](https://learn.microsoft.com/en-us/windows/dev-environment/rust/rust-for-windows))

**Tray app in the user session — NOT a Windows service.** This is unambiguous: on Win10/11 **all keyboard and mouse input in Session 0 is silently discarded**, and objects a service creates land in the session-0 namespace where user apps can't see them. A service also can't show your tray UI or a config window. Run one user-session process launched at logon. (`windows-service` crate is excellent if you ever need a service for something else — but not for this.) ([Session 0 isolation](https://www.firedaemon.com/post/microsoft-windows-interactive-services-and-session-0-isolation) · [MS: Impact of Session 0 Isolation](https://learn.microsoft.com/en-us/previous-versions/windows/hardware/design/dn653293(v=vs.85)) · [windows-service-rs](https://github.com/mullvad/windows-service-rs))

**Single process, not daemon+CLI.** For 10 keyboards and 4 pads there is no scaling reason to split. A split costs you an IPC protocol, a lifecycle problem, and a second thing to autostart. Instead:
- One binary, `ksx.exe`, that runs headless-with-tray by default.
- Same binary exposes subcommands (`ksx run`, `ksx devices`, `ksx test`, `ksx import-preset old.xml`) via `clap`.
- **If** a second process ever needs to talk to it (a Tauri UI, a remote control from your frontend/LaunchBox), add a named pipe *then*. Use the **`interprocess`** crate (local sockets → named pipes on Windows, has Tokio support) rather than hand-rolling `tokio::net::windows::named_pipe`. gRPC is absurd overkill here.
- Config reload: file-watch (`notify`) + a `reload` pipe command. A config file + reload genuinely covers ~90% of what you'd want IPC for.

**Threads, not tokio, in the hot path.** This is the consensus for input tooling and it matters for an arcade cab: the Interception read loop is a blocking `interception_receive` on a dedicated OS thread, ideally with `THREAD_PRIORITY_TIME_CRITICAL` (or at minimum `ABOVE_NORMAL`), no allocations, no locks — publish state to the output thread over a `crossbeam-channel` or a lock-free triple buffer. Async runtimes are for many-waiting-IO-tasks; a tight blocking input loop on tokio worker threads just adds scheduler jitter. Keep tokio out of the workspace entirely unless/until you add a UI process or network control; if you do, confine it to the IPC/UI crate.

**Config: TOML via `serde` + `toml`, with an XML importer for compat.**
- TOML is the Rust ecosystem convention and is hand-editable, which matters for an arcade cab you SSH/RDP into.
- Location: `%APPDATA%\ksx\config.toml` via the `directories` crate; support a portable `./config.toml` override next to the exe (arcade builds love portable).
- **Do not keep XML as the native format.** Write a one-shot `ksx import-preset` that reads djlastnight's XML presets and emits TOML. You keep migration value without carrying an XML serializer forever.
- Model keyboards by **stable device identity** (hardware ID / device instance path), never by Interception device *index* — indices shuffle on replug, which is the classic multi-keyboard bug. Persist `hwid` and resolve to index at startup, with a "learn device" flow (press a key on player 1's panel).

---

## 4. UI options for later (one paragraph each, deliberately brief)

**egui / eframe** — *Recommended default.* Immediate-mode, pure Rust, single crate, compiles into the same binary, trivially renders a live "which keys are down / what's the pad state" tester (exactly what your fork's built-in Xbox tester did), integrates with `tray-icon` (there's an official `examples/egui.rs`). Ugly-ish and not native-looking, which for an arcade utility is irrelevant. Lowest total cost by a wide margin.

**Tauri v2** — Stable since Oct 2024, currently ~2.10.x (Mar 2026), first-class built-in tray icon support, best-looking result if you want a real designed UI and are comfortable with web tooling. Cost: WebView2 dependency, a JS toolchain in your build, a much heavier installer, and it pushes you toward a two-process model. Choose only if the UI is genuinely a product surface.

**Slint 1.16** — Declarative `.slint` DSL, genuinely polished native-feeling desktop output, good Windows story, small binaries. The blocker is licensing: GPL / royalty-free-with-conditions / paid commercial. For an open-source fork that's fine; just decide consciously.

**Iced 0.14** — Released Dec 2025 as the last experimental release before 1.0; reactive rendering by default, big CPU wins on static UIs, plus time-travel debugging. Elm architecture is a nice fit for a state-machine app like this, but pre-1.0 churn is real. Revisit at 1.0.

**Thin separate UI process** — Only worth it if you want the UI to be replaceable (e.g. a web page served locally so you can configure from your phone while standing at the cab). That's a real arcade use case; if it appeals, plan the named-pipe/HTTP boundary early, otherwise don't pay for it.

**Tray, regardless of choice:** `tray-icon` (tauri-apps, v0.24+) + `winit` or `tao`; there's a Windows-only fork `tray-icon-win`. Create the icon on `StartCause::Init`.

Sources: [tray-icon](https://github.com/tauri-apps/tray-icon) · [Iced 0.14](https://www.phoronix.com/news/Iced-0.14-Rust-GUI-LIbrary) · [Tauri](https://en.wikipedia.org/wiki/Tauri_(software_framework)) · [Rust Windows GUI guide 2026](https://rust-pc.github.io/rust-windows-gui.html)

---

## 5. Packaging, elevation, autostart

**Elevation model — the important distinction:**
- **Driver install requires admin. Runtime should not.** Ship elevation *only* in the installer / a `ksx install-drivers` subcommand that re-launches with a UAC manifest.
- Interception's kernel driver means you do **not** need admin at runtime to beat UIPI (unlike hook-based tools that die when an elevated window has focus). Verify the ACL on `\\.\interception` on your cab; if a standard user can't open it, fall back to Task Scheduler elevated autostart rather than making the whole app `requireAdministrator`.
- ViGEmBus client access from a standard user works in the normal case.

**Installer:**
- **Inno Setup** — pragmatic first choice. Trivial to script, easy to shell out to `pnputil` / bundled `ViGEmBus_1.22.0_x64_x86_arm64.exe` / `install-interception.exe`, easy conditional "driver already present" checks. This is what most tools in this space ship.
- **WiX / `cargo-wix`** — if you want a proper MSI (better for unattended/silent installs and for enterprise-ish uninstall hygiene). Note MSI has no native driver support: you invoke **`pnputil /add-driver`** from a custom action. `pnputil` ships in every Windows and is Microsoft's current recommendation over DPInst (DIFx, deprecated) and DevCon.
- **MSIX — no.** Packaged apps can't install kernel drivers; a non-starter given ViGEmBus/Interception.
- **`cargo-dist`** (v0.32.0, May 2026, actively maintained) — use it for the *build/release/CI* side: cross-compilation, GitHub Releases, checksums, updater. Pair it with Inno/WiX for the driver-bearing installer rather than replacing them.
- Sign your binaries. Unsigned + "installs kernel drivers" is a SmartScreen nightmare.

**Autostart:** For an arcade cab, **Task Scheduler at-logon task** ("Run only when user is logged on" + "Run with highest privileges" if needed) beats the `Run` registry key and the Startup folder — no UAC prompt, auditable, and it can be configured with a delay so the drivers are settled and your frontend comes up second. Ship `ksx autostart enable|disable` that manipulates the task via `schtasks` or COM. Also add a `--start-delay` and a "wait for virtual pads to enumerate before launching frontend" hook, because **old games only enumerate joysticks at startup** — that's a documented Keyboard2Xinput gotcha and it will bite you.

Sources: [pnputil](https://learn.microsoft.com/is-is/windows-hardware/drivers/install/using-the-devcon-tool-to-install-a-driver-package) · [WiX driver installation](https://www.apriorit.com/dev-blog/164-driver-installation-with-wix) · [MSI + drivers](https://www.advancedinstaller.com/install-drivers-msi-package.html) · [cargo-dist](https://axodotdev.github.io/cargo-dist/) · [Task Scheduler elevated autostart](https://www.thewindowsclub.com/autostart-programs-windows-10-make)

---

## 6. Testing strategy

**What you can automate in CI (GitHub Actions `windows-latest`):**
- **Everything that isn't a driver.** Make the mapping engine a pure, `#![no_std]`-ish crate: `(device_id, scancode, up/down) → Vec<PadEvent>`. Property-test it (`proptest`) for stuck keys, SOCD/simultaneous-opposite-directions resolution, modifier handling, profile switching, and the "unplug keyboard mid-press → release everything" invariant. This is where 90% of real bugs live and it needs zero Windows.
- Config parsing round-trips, XML→TOML importer golden files.
- Compile checks for the Windows crates.
- **You cannot install ViGEmBus or Interception on hosted runners** reliably (kernel driver + reboot + signing). Don't try. Gate all driver-touching tests behind a feature flag.

**What runs on the cab (self-hosted runner or a `ksx test` subcommand):**
- **Loopback integration test — this one is genuinely automatable and is the highest-value test you can write:** create a virtual pad with `vigem-client`, push a state, then read it back through **XInput** using `rusty-xinput` (or `gilrs` with the `xinput` feature) and assert. That end-to-end-verifies your output layer without a human.
- **Synthetic multi-keyboard input:** Interception can `interception_send` strokes *on a given device*, so you can replay recorded per-device stroke traces on the cab. Caveat found in the wild: the driver needs to have *received* at least one stroke from a device before it can address it, so seed with a real keypress first. This gets you most of the way to automated end-to-end testing on the target hardware.
- **Latency measurement:** timestamp at Interception receive → timestamp at ViGEm submit, log a histogram (`hdrhistogram`). Track p99, not mean — arcade players feel the tail.

**Irreducibly manual on the cab:**
- Real per-panel button sweeps for all 10 keyboards / 4 pads (nothing simulates two physical I-PACs).
- `joy.cpl` ("Set up USB game controllers") — confirms the pads enumerate and shows XUSB properties.
- Browser gamepad testers (hardwaretester.com/gamepad, gamepad-tester.com) — fast visual sanity check, but note the browser Gamepad API goes through WGI/HID, so **a device passing a browser test does not prove XInput visibility**. Always confirm with an actual XInput-only game.
- Per-game verification against your real library (the fighting games are the strict ones), plus "does the frontend still get keyboard input when the splitter is idle/toggled off".
- Rebuild the original tool's **built-in controller tester** as `ksx test` — a TUI showing live per-device scancodes and resulting pad state side by side. It was the single most useful feature of the C# app and it's cheap in `ratatui` or egui.

Sources: [rusty-xinput](https://github.com/Lokathor/rusty-xinput) · [gilrs](https://docs.rs/gilrs/) · [Interception send-to-device limitation](https://github.com/jasonpang/Interceptor) · [vgamepad (Python; useful reference for a test harness)](https://github.com/yannbouteiller/vgamepad)

---

## 7. Recommended architecture

### Process model
**One user-session process.** Dedicated real-time-ish capture thread → lock-free channel → mapping engine → output thread → ViGEm. Tray icon on the main/UI thread. No service, no daemon split, no tokio in v1. Add a named-pipe control channel only when a second process actually exists.

```
[Interception thread]  blocking recv, TIME_CRITICAL, zero-alloc
        │  crossbeam-channel (bounded, never blocks producer)
        ▼
[Engine thread]  device_id → profile → pad state machine (pure logic)
        │  triple-buffer / channel
        ▼
[Output thread]  vigem-client, submit on change + heartbeat
        
[Main thread]  tray-icon + winit event loop, config watch (notify), egui window on demand
```

### Workspace layout

```
ksx/
├── Cargo.toml                  # workspace, resolver = "2"
├── crates/
│   ├── ksx-core/               # PURE, no Windows deps, no I/O — 100% unit-testable in CI
│   │   ├── scancode.rs         #   scancode/HID-usage newtypes
│   │   ├── mapping.rs          #   key → pad-element, SOCD, curves, deadzone-less digital
│   │   ├── profile.rs          #   profiles, per-player bindings, hotkey/toggle handling
│   │   └── engine.rs           #   Vec<InputEvent> → Vec<PadState>, deterministic
│   ├── ksx-config/             # serde + toml, schema versioning, %APPDATA% + portable paths
│   ├── ksx-legacy-import/      # djlastnight XML presets → ksx-config (quick-xml or serde-xml-rs)
│   ├── ksx-capture/            # trait KeyboardSource { devices(), recv() } + suppression
│   │   ├── interception.rs     #   default backend (kanata-interception / interception-sys)
│   │   ├── rawinput.rs         #   identify-only, for the device-picker wizard (multiinput)
│   │   └── winusb.rs           #   [experimental] nusb direct-read for WinUSB-bound I-PACs
│   ├── ksx-output/             # trait VirtualPadBackend { plug(), submit(), unplug() }
│   │   ├── vigem.rs            #   vigem-client  (XUSB — the one that works today)
│   │   └── virtualhid.rs       #   [experimental] libvirtualhid FFI, HID/VHF, verify XInput!
│   ├── ksx-platform/           # windows-rs bits: thread priority, device notifications,
│   │                           #   WM_DEVICECHANGE hotplug, task-scheduler autostart, elevation
│   └── ksx-app/                # tray + orchestration + supervision/restart of threads
└── src/main.rs (bin ksx)       # clap: run | devices | test | import-preset | install-drivers
                                #       | autostart enable/disable
```

### Key crates

| Concern | Crate |
|---|---|
| Win32/COM/WinRT | `windows` (0.62+), `windows-sys` for hot FFI |
| Per-device capture + block | `kanata-interception` (or `interception-sys`) |
| Device identification / picker | `multiinput` (or raw `windows` RawInput) |
| Virtual XInput pad | `vigem-client` (pure Rust, no ViGEmClient.dll) |
| Experimental USB backend | `nusb` |
| Config | `serde`, `toml`, `directories`, `notify` (hot reload), `quick-xml` (import only) |
| Concurrency | `crossbeam-channel`, `parking_lot`, `triple_buffer` |
| CLI / logging | `clap`, `tracing` + `tracing-appender`, `color-eyre`/`anyhow` |
| Tray / UI (later) | `tray-icon`, `winit`, `eframe`/`egui` |
| Testing | `proptest`, `insta`, `rusty-xinput` or `gilrs` (loopback verify), `hdrhistogram` |
| Release | `cargo-dist` + Inno Setup (or `cargo-wix`) |

### Build order
1. `ksx-core` + `ksx-config` + `ksx-legacy-import` — pure Rust, full CI coverage, no drivers. Import your existing presets and prove the engine reproduces the old behavior.
2. `ksx-output/vigem.rs` + the XInput loopback test. Now you can see fake pads in `joy.cpl` with no keyboard work done.
3. `ksx-capture/interception.rs` + `ksx test` TUI. End-to-end on the cab.
4. `ksx-app`: tray, hotplug via `WM_DEVICECHANGE`, autostart, installer.
5. Only then: UI (egui), and the `winusb`/`virtualhid` experiments.

### Three risks to retire early
1. **libvirtualhid's HID pads may be invisible to XInput-only games** — test before adopting; keep ViGEm as the default and the trait boundary honest.
2. **Interception's licensing (LGPL non-commercial / paid commercial) and its long-term HVCI/Win11 signing viability** — this is the load-bearing dependency; the `winusb.rs` backend is your escape hatch and is uniquely practical because your hardware is a dedicated I-PAC cab.
3. **Device-index instability across replug** — design identity on hardware IDs from commit one, not as a later fix.