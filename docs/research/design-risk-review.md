All three spot-checks confirm the research inputs (immutable presets, synchronous `Dispatcher.Invoke` at `InputManager.cs:278`, and the `id < MaxDeviceCount` off-by-one at `Interception.cs:721`). I have enough independently-verified ground truth to write the adversarial assessment.

# Risk Review — KeyboardSplitterXboxPro Rust Rewrite (Skeptic's Report)

## 0. Independent verification performed (this machine, read-only)

| Claim from research | My check | Result |
|---|---|---|
| Interception drivers cross-signed, cert expired 2012 | `Get-AuthenticodeSignature` on `keyboard.sys`/`mouse.sys` | **Confirmed** — `CN=Francisco Lopes da Silva`, NotAfter 2012-10-21, currently "Valid" (grandfathered) |
| Both filters live in class stacks | UpperFilters registry | **Confirmed** — `keyboard,kbdclass` and `mouse,mouclass` |
| Cross-signing-removal audit CI policy active | `CodeIntegrity\CiPolicies\Active` | **Confirmed** — `{784C4414-79F4-4C32-A6A5-F0FB42A51D0D}.cip` present, dated **2026-07-30** (four days ago) |
| ScpVBus and ViGEmBus coexist, both running | `Win32_SystemDriver` | **Confirmed** — both `Running` |
| I-PAC = exactly one Keyboard-class device per board | `Get-PnpDevice` VID_D209 | **Confirmed** — single `Keyboard`-class node at `HID\VID_D209&PID_0430&MI_00\...`; mouse/consumer/system collections on MI_01; vendor collections on MI_02; separate trackball `PID_15A2`; also an idle `U-HID Firmware upgrade` device (PID_0750, status Unknown) |
| Legacy source claims (immutable presets, sync `Dispatcher.Invoke` per keystroke, device-20 enumeration off-by-one) | Grep of legacy source | **All three confirmed** at `Preset.cs:21`, `InputManager.cs:278`, `Interception.cs:721` |

The research inputs are trustworthy. My disagreements below are about *conclusions*, not facts.

---

## 1. Ranked risks

### R1 — Interception cross-signing enforcement can take down the whole cabinet, not just the app (SEVERITY: CRITICAL)

**Likelihood: High** (the audit-mode CI policy is already active on this machine; Microsoft's stated rollout is audit → per-machine enforcement). **Impact: Catastrophic** — and worse than the research states.

The research frames this as "the app loses capture." The actual failure mode is worse: `keyboard` and `mouse` are **class upper filters**. If Code Integrity refuses to load a driver listed in `UpperFilters`, PnP fails to start every device in that class — the classic Code 39 "all keyboards dead at boot" failure, recoverable only via Safe Mode or an offline registry edit stripping the filter entries. On a cabinet, that is a machine you cannot type into to fix.

Also note the perverse dynamic the research identified: a loading cross-signed driver *resets* the enforcement-evaluation counters. The cabinet stays "safe" only as long as Interception keeps loading — until a servicing update or policy refresh flips it, at an unpredictable time.

**Mitigations (do all of these):**
1. **Image the cabinet drive before any driver work.** Non-negotiable first step of the whole project.
2. Write and test the **recovery runbook now**, while everything works: Safe Mode → remove `keyboard`/`mouse` from `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e96b…}\UpperFilters` and `{4d36e96f…}\UpperFilters` → reboot. Keep it printed near the cab, plus a spare mouse (mouse class is equally exposed).
3. Treat Interception as a **bridge backend with a scheduled retirement**, not the permanent primary. The `CaptureBackend` trait must exist from commit one, and a non-Interception backend must reach working state *before* enforcement, not after.
4. Pin Windows Update on the cabinet (Win11 Pro deferral policies). This buys time; it is not a fix.
5. The app should check, at every startup: `keyboard.sys` signature status, the CI policy GUID set, and whether enforcement mode has activated — and scream, not log quietly.

**Fallback if Interception becomes unloadable:** uninstall it cleanly (the legacy `InterceptionUninstall` rescue tool, or `keyboard_driver.exe /uninstall`), then run the WinUSB backend (R3) for the I-PACs. There is no third option that both identifies and blocks per-device.

### R2 — Multi-identical-device identity is genuinely unsolved in the legacy design (SEVERITY: HIGH)

**Likelihood: High** the moment a second identical board or any USB flakiness enters the picture. **Impact: High** — wrong player mapping, or devices going silently dead.

Three distinct sub-problems a rewrite must not conflate:
- **Identical boards share HWID.** Two I-PAC2s both report `HID\VID_D209&PID_0430&MI_00…` as their hardware ID. The legacy games file persists devices *by HWID* — ambiguous with two identical boards. The Interception API only exposes HWID + a positional index (`Keyboard_01`…), so **Interception alone cannot distinguish two identical I-PACs stably.**
- **Interception indices drift.** Replug/hibernate increments a device's slot ID; past 10 the device **ceases to function until reboot** (evilC: "nothing I can do"). A cabinet with hubs and power cycling will hit this. The legacy code even has an off-by-one that never enumerates slot 20 (confirmed, `Interception.cs:721`).
- **Instance paths are stable but backend-specific.** The observed instance `USB\VID_D209&PID_0430\4` suggests this board reports a serial-ish suffix; the HID child path (`8&2A0D0500&0&0000`) is port-topology-derived. Stable *if boards stay in their ports* — a reasonable cabinet constraint, but it must be a documented invariant, not an accident.

**Mitigations:**
1. Persist identity as **device instance path** (from PnP/Raw Input), never HWID, never Interception index. First verify whether Ultimarc serials (`\4`) are unique per board — check a second board before trusting it.
2. Build an **index↔instance-path correlation pass at session start**: while *not* blocking, Raw Input (`WM_INPUT` → `hDevice` → `RIDI_DEVICENAME`) and Interception both see the same keystroke; correlate by timing to bind Interception slot N to instance path P. Re-run on every hotplug event.
3. Ship a **"press a key on Player 1's panel" learn flow** as the fallback when correlation is ambiguous (two identical boards mashed simultaneously during learn).
4. Track Interception ID climb explicitly; when a device's ID exceeds 10, surface **"reboot required"** loudly instead of going deaf — the legacy app's silent version of this failure is one of its worst traits.

### R3 — The "WinUSB as primary" recommendation is riskier than the research admits (SEVERITY: HIGH — pushback on a research conclusion)

The keyboard-capture research promotes WinUSB/`nusb` rebinding of MI_00 to primary. I recommend **demoting it to "fallback developed early, promoted only when Interception dies."** Reasons:

- **Structural blocking cuts both ways.** With Interception, if the Rust app crashes, the driver context closes and keystrokes flow to Windows again — the cabinet frontend remains navigable. With WinUSB rebind, the board types *nowhere* unless the Rust app is alive and healthy. Every app bug becomes "cabinet has no controls at all."
- **Unverified interaction with WinIPAC.** The Ultimarc programming utility must still reach the board (likely via the MI_02 vendor collections, but *unverified*). If reprogramming requires the keyboard interface, a rebound board can't be remapped without unbinding first. Test before committing.
- **NKRO descriptor parsing is real work.** Interception sees post-`kbdhid` scancodes; NKRO is transparent. Raw interrupt-endpoint reads require parsing the I-PAC's actual (extended) report descriptor, per firmware revision.
- **Self-signed cert into Trusted Root** (libwdi/Zadig path) is an acceptable wart on a dedicated cab but a genuine machine-wide trust downgrade — and Zadig HID rebinds are documented as painful to reverse.
- It covers **only devices you claim** — any non-I-PAC keyboard needs a different backend anyway.

**Likelihood of at least one nasty surprise: Medium-High. Impact: Medium** (recoverable with `pnputil /remove-device` + rescan, but plan the rollback before the first attempt, and keep a never-claimed keyboard on a separate port).

**Concrete sequencing:** v1 primary = Interception (`kanata-interception` 0.3.0); WinUSB spike as a standalone experiment early (it de-risks R1); promotion decision gated on the M2 test matrix below.

### R4 — ViGEmBus is abandoned (SEVERITY: MEDIUM, frequently overrated)

**Likelihood of breakage within the cabinet's horizon: Low-Medium.** Attestation/HLK-signed (not cross-signed — **unaffected by the April 2026 policy**, unlike Interception), massive installed base (Sunshine bundles it and hard-fails without it), driver frozen and stable. **Impact if it does break: High** — no successor has Rust bindings, XInput-correct slots, or (for libvirtualhid) even documented XInput behavior.

**Mitigations:**
1. **Vendor `vigem-client`** (~2k LOC, MIT, dormant) into the workspace; do not take a crates.io dependency on an unmaintained load-bearing crate.
2. **Bundle the ViGEmBus 1.22.0 installer** in the repo/release; verify its Authenticode signer (`Nefarius Software Solutions e.U.`) before executing. Do not rely on download endpoints Nefarius has said will rot.
3. **Rehearse a clean install now**: fresh Win11 25H2 VM, install ViGEmBus 1.22.0, run the loopback test. Do this while it's not an emergency — it also proves your installer story.
4. `VirtualPadBackend` trait from day one; `PadState` kept in `XGamepad`/XInput wire shape. Plan B is a hand-written Rust client for HIDMaestro's MIT-documented shared-memory protocol — but **only after** verifying the WGI double-input bug (#8) doesn't affect the cab's actual frontend/games, and XInput slot behavior is acceptable. Do not plan around Nefarius VirtualPad (closed, unresponsive) or libvirtualhid (XInput behavior undocumented, driver license is source-available).

### R5 — Rust crate immaturity (SEVERITY: MEDIUM-LOW, but with specific traps)

- `kanata-interception` 0.3.0: **fine** — actively exercised by kanata's shipping builds. Use it, not bozbez's 2020 `interception` crate.
- `vigem-client` 0.1.4: complete for X360 including `get_user_index()` and the notification channel (verify by reading `src/x360.rs`, not docs.rs) — **fine if vendored** (R4).
- `multiinput`: dead 6 years — **do not depend**; hand-roll Raw Input on `windows` 0.62.x (~200 lines).
- `nusb`: healthy and active.
- **The real gaps have no crates at all:** Interception driver install/state detection (legacy used WMI + FileVersionInfo — port the *logic*, replace WMI with direct service/registry queries), the CI-policy health check from R1, and any successor-bus client. Budget these as hand-written `windows-rs` code, and note the legacy state check only ever validated `keyboard.sys`, never `mouse.sys` — don't copy that.
- Threading: keep tokio out of the hot path entirely. Blocking `interception_receive` on a dedicated high-priority thread → bounded crossbeam channel. Any async runtime in the capture path is a self-inflicted R7.

### R6 — I-PAC composite-device quirks (SEVERITY: MEDIUM — mostly good news, two traps)

**Good news (verified):** per-"keyboard" capture maps cleanly to per-board. The I-PAC exposes exactly one Keyboard-class device (MI_00). Interception sees it as one keyboard slot; nothing composite leaks into the keyboard class on this board.

**Trap 1 — one board ≠ one player.** An I-PAC2 carries 2 players, an I-PAC4 carries 4, all on **one** keyboard device. The legacy engine handles this only because its dispatch loop feeds **every** slot whose device matches, with no `break` (`Splitter.cs:408-419`) — one keyboard can drive 2-4 virtual pads with disjoint presets. A rewrite that "simplifies" to a 1:1 device→pad map silently breaks every multi-player-per-board cabinet. This must be a stated requirement and a property test.

**Trap 2 — slot-budget pollution.** The 10-keyboard ceiling is shared with ghost keyboards (101/102-key driver artifacts, mice that enumerate as keyboards, wireless dongles). With ID drift on replug (R2), the budget is tighter than it looks. Enumerate and display *everything* Interception sees, with instance paths, so ghosts are diagnosable.

Minor: the trackball (`PID_15A2`) is a separate plain mouse — cutting mouse mapping from v1 (see §4) conveniently leaves it untouched for MAME. NKRO is transparent under Interception, a cost only for the WinUSB backend (R3). The Multi-Mode firmware (2 XInput pads per board, zero software) remains a documented hardware escape hatch — correct as an escape hatch only, since it caps at 2 pads/board and kills per-key remapping.

### R7 — Reintroducing the legacy latency/deadlock architecture by accident (SEVERITY: MEDIUM)

The single worst legacy defect is the synchronous per-keystroke marshal onto the UI thread (`InputManager.cs:278`, confirmed) — a blocked UI thread freezes every keyboard and mouse system-wide until reboot. A Rust rewrite kills this *by default*, but can reintroduce it via: logging/allocation in the capture thread, an unbounded channel that backpressures the receive loop, a mutex shared with a UI/IPC thread, or "just one" synchronous call into the ViGEm IOCTL path from the capture thread. **Mitigation:** hard architectural rule — capture thread does *only* receive → decide block → enqueue; a watchdog thread that force-disables filtering if the consumer stalls for N ms; latency histogram (p99, not mean) as a permanent debug feature.

### R8 — Scope creep ("we have lots of time" is a risk factor) (SEVERITY: MEDIUM)

Three backends, four trait abstractions, an importer, an installer, and a UI is a two-year project that dies at 60%. Mitigation is §4's cut list plus milestone gates: each milestone ends with the manual cab test for that milestone (§5), and nothing from a later milestone starts until the gate passes.

---

## 2. Risk summary table

| # | Risk | Likelihood | Impact | Primary mitigation | Fallback |
|---|---|---|---|---|---|
| R1 | Interception blocked by CI enforcement → all input dead at boot | High (audit policy live on this machine) | Catastrophic | Disk image + recovery runbook + update pinning + startup health check | Uninstall filters; WinUSB backend |
| R2 | Identity of identical boards / index drift / 10-slot exhaustion | High | High | Instance-path identity + RawInput correlation + learn flow + exhaustion detector | Same-port discipline; reboot prompt |
| R3 | WinUSB rebind surprises (WinIPAC access, NKRO, rollback, app-death = no input) | Med-High | Medium | Early spike, rollback tested first, never-claimed recovery keyboard | Stay on Interception until forced |
| R4 | ViGEmBus rot / no successor | Low-Med | High | Vendor client, bundle installer, clean-VM rehearsal, trait boundary | Rust client for HIDMaestro protocol (after WGI-bug verification) |
| R5 | Crate immaturity / missing crates | Med-Low | Medium | kanata-interception + vendored vigem-client; hand-roll RawInput & driver-state code | windows-rs covers everything raw |
| R6 | I-PAC per-board mapping traps | Medium | Medium | Preserve one-keyboard→many-slots; ghost-device visibility | Multi-Mode firmware (hardware) |
| R7 | Hot-path regression / system input freeze | Medium | High | Capture-thread purity rule + watchdog + p99 histogram | Watchdog force-passthrough |
| R8 | Scope creep | High | Medium (project death) | Cut list + milestone gates | — |

---

## 3. Load-bearing legacy behaviors a rewrite will get wrong if not named explicitly

1. **Crash vs hang blocking semantics.** On process **death**, Interception handles close and keystrokes flow to Windows again — keyboards come back. On **hang** (context open, receive loop not pumped), all system input is dead until reboot. The rewrite must be crash-only (no cleanup required for input to recover) and add a watchdog for the hang case. **Test both explicitly** (§5 M3). Also replicate the session-ending hook: on logoff/shutdown, tear down the filter *before* the process is killed.
2. **Emergency escapes, evaluated pre-block, on any device:** LeftCtrl×5 → toggle keyboard blocking, RightCtrl×5 → toggle mouse blocking, any Ctrl+Alt+Del combo → full emulation stop. These are the only way out when a fullscreen game holds focus. The ×5 counters reset on any intervening key. Audio feedback distinguishes on/off. Also preserve the corollary: binding LeftControl in a preset deserves a warning (collides with the escape hatch and with multi-scancode keys).
3. **Blocking scope rules:** only devices assigned to slots are ever blocked; unassigned keyboards keep typing; blocking active only while emulation runs; "own window focused" forces passthrough so the app can't lock itself out.
4. **One keyboard → many slots** (the I-PAC4 case, R6 Trap 1). Conversely many keyboards → one slot is *not* supported and shouldn't be accidentally added.
5. **All-keys-up release rule + bidirectional custom-function aggregation.** A function is released only when *every* key mapped to it — through its native category *or* through a custom function targeting the same endpoint — is up (`Preset.GetKeys`, `Preset.cs:71-228`). This cross-category reverse lookup is the subtlest logic in the app. Port it with property tests, not by eye.
6. **Opposite-axis snap:** releasing Left while Right is held snaps to the opposite extreme, not center. Known legacy inconsistency: the snap hardcodes Min/Max even for custom-valued axis bindings, and custom functions always drive full-scale. Decide deliberately (recommend: snap to the held key's own bound value) and document the divergence.
7. **State-diff before submit:** legacy early-returns when the cached pad state is unchanged, so only real transitions hit the driver. Keep as a diffed `XGamepad` per pad. Do **not** keep the legacy `IsControllerOwned` IOCTL-per-setter (doubled syscalls); ViGEm ownership makes it moot.
8. **Slot invalidation taxonomy** (13 reasons) and unplug-mid-game handling: device removal invalidates the slot and unplugs its pad. Do not port the confirmed bug where a slot with a null device matches any removal. ViGEm's `get_user_index()` + notification callback legitimately deletes the XInput-slot-guessing heuristic (`!IsConnected && Tag == null`) — that whole mechanism is a workaround, not a behavior to preserve.
9. **Slot number ≠ user index ≠ LED.** Preserve `StartingVirtualControllerUserIndex` semantics; get LED/player index from ViGEm's callback, not from polling xinput1_3. Account for real pads stealing XInput slots (arrival order); surface it, optionally mitigate with HidHide (driving its IOCTLs directly — the 25H2 CLI is broken, issue #215).
10. **Driver version discipline the legacy app never had:** it pinned ScpVBus "22.52.24.182" purely in README folklore with zero runtime check. The rewrite must runtime-report: ViGEmBus presence/version, Interception presence + signature status + CI policy state (R1), and refuse-with-explanation rather than "Slot is invalidated."
11. **Preset compatibility:** the bitflag ID tables (`XboxButton 0x1000=A`… `XboxCustomFunction` axis min/max flags) must survive bit-for-bit in the importer; input is UTF-16 XML including the v1 legacy schema (`PresetUpgrader` semantics: `LeftTrigger`→`Left`, `<pov>`→dpad); the phantom `empty` preset is referenced by docs/UI but absent from `ImuttablePresets` — restore it in the new model rather than perpetuating the dangling reference. Native format should be TOML; XML is import/export only.
12. **The 5-second start/stop cooldown** is almost certainly a ScpVBus settle workaround. Replace with ViGEm `wait_ready()`; don't cargo-cult the timer.
13. **Mouse pseudo-keys** (50 ms auto-release for wheel/move, dead zone 0-12) — only if/when mouse mapping is ported (§4 defers it).
14. **Modal dialogs vs mouse capture:** legacy brackets every file dialog with `DisableMouseEvents`. Any future UI that coexists with mouse blocking needs the equivalent.

---

## 4. Cut list for Rust v1, and what to keep in legacy form

**Cut from v1 entirely (revisit later or never):**
- `xinput/` subtype-proxy DLL + `XinputSubTypesWindow` (per-game file drop; orthogonal to the pipeline)
- On-screen mouse-click controller
- Games database + launcher + `game=` CLI autostart (the >3s exit heuristic, icon extraction, all of it). The cabinet frontend (LaunchBox/etc.) already owns launching. Replace with one thing: an **autostart profile** — "on app start, load config X and start emulation" — which is what a cabinet actually needs at boot.
- **Mouse mapping altogether.** Don't set the Interception mouse filter at all in v1. This halves the risk surface (mouse.sys stays passive, no dialog-bracket problem, no 50 ms pseudo-key timers) and leaves the trackball/spinner working natively in MAME, which is what it's for. Phase 2 if a real need appears.
- Preset-editing UI (hand-edit TOML + importer covers v1), input monitor UI, FAQ/help windows, custom Aero chrome.
- The single-EXE embedded-resource/`%TEMP%`-extraction/PATH trick — it's exactly what modern AV/ASR flags. Ship normal files.
- Installer and driver-install orchestration **in Rust**: v1 documents manual install (drivers are already on the cab); later, an Inno Setup script shelling `pnputil` + the bundled signed installers. Never rewrite `devcon` shelling — the bundled `devcon.exe` is unsigned and non-redistributable anyway.
- WMI-based driver detection, Xbox 360 Accessories logic, `Thread.Abort` patterns, UTF-16 CWD-relative config files.

**Keep the legacy stack alongside during transition (do not rewrite):**
- **The legacy app + ScpVBus stay installed as the working fallback** until the Rust pipeline passes the M4 game matrix. Verified: ScpVBus and ViGEmBus coexist happily. Constraint: never run both apps simultaneously (8 virtual pads > 4 XInput slots).
- **`InterceptionUninstall/` rescue tool**: keep the existing binary as-is in the recovery kit. It's 200 lines of C# that only matters in disaster scenarios — rewriting it adds risk to the exact tool you need when everything else is broken.
- Legacy preset XML corpus: input to the importer, never a maintained format.
- `XInputWrapper` concept: don't port. Feedback comes from ViGEm's notification callback; a generic tester is `rusty-xinput` against `xinput1_4` inside `ksx test`.

---

## 5. Verification / test protocol for the cabinet

**Standing automation (CI, GitHub Actions `windows-latest` — no drivers ever):**
- Property tests on the pure mapping engine: stuck-key invariant ("device removed mid-press → all its contributions released"), all-keys-up rule incl. custom-function aggregation (behavior #5), opposite-axis snap, one-keyboard→many-slots fan-out, state-diff idempotence.
- Golden-file tests: legacy XML (v1 and v2 schemas, UTF-16) → TOML importer, byte-exact ID-table round-trips.
- Everything driver-touching behind a feature flag CI never enables.

**Automated on the cab (self-hosted runner or `ksx selftest`):**
- **Loopback test:** `vigem-client` plugs a pad, writes a state sequence, `rusty-xinput` reads it back via XInput and asserts; also asserts `get_user_index()` stability across replug.
- Interception synthetic replay: `interception_send` recorded per-device stroke traces (caveat: seed each device with one real keypress first) → assert resulting pad states end-to-end.
- Latency histogram: capture-receive → ViGEm-submit, assert p99 under budget (e.g. < 2 ms).

**Milestone gates (manual matrix — each gate blocks the next milestone):**

**M0 — Before touching anything:**
1. Full disk image of the cabinet. 2. Export all presets + `splitter_games.xml`. 3. Record baseline: `joy.cpl` view, legacy tester screenshots, subjective input feel in the two most timing-sensitive games. 4. Write + rehearse the R1 recovery runbook (Safe Mode UpperFilters strip) on a VM. 5. Snapshot driver inventory (`Get-AuthenticodeSignature`, CI policy list) — this is your drift baseline.

**M1 — Output layer (vigem-client):**
`ksx test` plugs 4 pads → all four visible in `joy.cpl` with correct types; LED/player indices 1-4 via notification callback; an XInput-*only* game sees all four; unplug/replug cycling 10× leaves no ghost pads; **kill the process** → pads vanish cleanly; plug a real Xbox pad first → observe slot displacement and confirm the app reports actual `get_user_index()` per pad. Also: clean Win11 25H2 VM → ViGEmBus 1.22.0 installer → repeat (R4 rehearsal).

**M2 — Capture layer (observe-only, no blocking yet):**
Enumerated devices match physical boards with instance paths; keystroke attribution correct per board (mash P1 and P2 on different boards simultaneously); identity survives: replug same port, replug *different* port (document the expected failure/relearn), reboot, sleep/resume; **ID-exhaustion drill** — replug a board 8+ times in one session, confirm the app detects ID climb past 10 and demands reboot instead of going silent; ghost-keyboard census (does the count match physical reality? which devices are ghosts?).

**M3 — End-to-end with blocking (the safety gate — do not skip any):**
1. Emulation on → assigned board drives pad, keystrokes invisible to Notepad; unassigned USB keyboard still types.
2. Fullscreen game focused → LeftCtrl×5 unblocks, ×5 reblocks (audio confirm); Ctrl+Alt+Del variant stops emulation.
3. **Kill-recovery:** `taskkill /f` while blocking → keyboards must return within ~1 s, no reboot. Repeat 5×.
4. **Hang simulation:** suspend the process (Process Explorer) while blocking → confirm watchdog releases input within its timeout; document behavior without watchdog so you know what failure looks like.
5. Logoff and shutdown while blocking → next session's input is normal.
6. Win+L and Ctrl+Alt+Del reachability while blocked (know exactly what is and isn't blockable).
7. p99 latency capture during a 10-minute 4-player mash session.

**M4 — Game matrix (the real acceptance test):**
Every title class in the cab's actual library: one XInput-only title, MAME (incl. a trackball game — verifies the mouse-cut decision), RetroArch, Steam Big Picture, anything with anti-cheat if present. Per title: all 4 pads respond, correct player order, no double input, frontend keyboard nav still works with emulation stopped. Plus the ghosting sweep: simultaneous worst-case chords per panel (use the built-in tester equivalent before blaming software — the legacy README's ghosting guidance still applies).

**M5 — Durability (run for two weeks before decommissioning legacy):**
Autostart ordering (pads enumerated before frontend launches — old games enumerate at startup only); reboot persistence daily; Windows Update deferral confirmed; a scheduled weekly `ksx selftest` + driver-signature/CI-policy drift check with a visible alarm.

---

### Critical Files for Implementation
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Models/Splitter.cs (translation pipeline, blocking scope, opposite-axis and release rules to preserve)
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Managers/InputManager.cs (emergency hotkeys, focus override, the hot-path defect to not reproduce)
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/Interceptor/Interception.cs (capture loop, blocking mechanism, device enumeration limits and bugs)
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Presets/Preset.cs (bidirectional GetKeys aggregation, ID tables, immutable presets — the subtlest domain logic)
- C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro/KeyboardSplitter/Managers/EmulationManager.cs (slot lifecycle, invalidation taxonomy, start/stop guards)