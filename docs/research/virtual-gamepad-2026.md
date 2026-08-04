# Virtual Xbox 360 / XInput Gamepads on Windows 11 — State of the Art, August 2026

## TL;DR

**Target ViGEmBus 1.22.0 via the `vigem-client` Rust crate today, behind a `VirtualPadBackend` trait.** It is the only stack that (a) is production-signed and installable on stock Windows 11 with Secure Boot on, (b) produces *genuine* XInput slots via Microsoft's own `xusb22.sys`, and (c) has a working pure-Rust client. It is abandoned but not broken. Design for replacement: the two credible successors (**HIDMaestro**, **libvirtualhid**) are both < 1 year old and neither has Rust bindings yet.

---

## 1. ViGEmBus — status

| Fact | Detail |
|---|---|
| Archived | 2 Nov 2023, repo read-only, banner "🧟 THIS PROJECT HAS BEEN RETIRED 🧟" |
| Reason | Trademark conflict with **ViGEM GmbH** (discovered May 2023); mutual agreement forced retirement of the name "ViGEm" and the `vigem.org` domain |
| Last release | **v1.22.0** (2 Nov 2023) — "It's dead, Jim". Driver binary unchanged from **v1.21.442.0** (30 Aug 2023), which was the last functional release (added ARM64, DS4 output-report SDK APIs) |
| License | **BSD-3-Clause** — legally forkable; only the *name* is encumbered |
| Signing | Production-signed by Nefarius Software Solutions e.U. via the Windows Hardware Dev Center. **Still valid** — cross-signed driver catalogs do not expire the way an EV-signed binary does |
| Works on Win11 24H2/25H2? | **Yes.** No credible reports of the 1.22.0 driver failing to load on 24H2 or 25H2. Reported failures are installer/orchestration issues (e.g. Sunshine #4862, Mar 2026), not driver-load rejections |
| HVCI / Memory Integrity | No published incompatibility. It is a WDF/DMF-based bus driver with no known HVCI violations |

**What Nefarius recommends now:** nothing, publicly. The EOL statement only tells you to repoint the auto-updater to `aiu.api.nefarius.systems` or uninstall. It says a successor "**VirtualPad**" is in the works.

**VirtualPad** is **commercial and closed**: *"Nefarius VirtualPad is a commercial framework for gaming peripherals emulation available to business partners of Nefarius Software Solutions e.U."* There is no public SDK, no pricing, no self-serve access. In March 2026 the PadForge author formally asked Nefarius to license VirtualPad's DualSense emulation to open-source projects (open-source the component / non-commercial license / anything). Nefarius replied on 5 Mar 2026 with *"Thanks & received, please give me some time to reply 🙏"* and **never followed up**. As of Aug 2026 the thread is still unresolved.

**Verdict: VirtualPad is not an option for a hobby Rust project.**

- https://github.com/nefarius/ViGEmBus
- https://github.com/nefarius/ViGEmBus/releases
- https://docs.nefarius.at/projects/ViGEm/
- https://docs.nefarius.at/projects/ViGEm/End-of-Life/
- https://docs.nefarius.at/projects/VirtualPad/
- https://github.com/nefarius/DsHidMini/discussions/424

---

## 2. Community successors / forks

**There is no maintained hard fork of ViGEmBus.** ~400 GitHub forks exist; none has taken over maintenance. The blocker is not code — it's that anyone shipping a rebuilt `ViGEmBus.sys` needs their own Dev Center account + EV cert + attestation submission, and would have to rename it (trademark).

Instead, two **from-scratch** replacements appeared in 2026, both deliberately avoiding kernel-mode:

### 2a. HIDMaestro (hifihedgehog) — the most interesting one

- **MIT licensed**, free and open source, commercial use explicitly welcome
- **Pure user-mode UMDF2** HID minidriver hosted by Windows' inbox `mshidumdf.sys`. No kernel component, no BSOD surface
- **228 device profiles / 32 vendors** with byte-exact VID/PID + HID report descriptors (Xbox 360 Wired, Xbox Series, DualSense, DS4v2, Switch Pro with real subcommand handshake, wheels, HOTAS…)
- **Presents to every API simultaneously**: DirectInput, XInput, SDL3/HIDAPI, browser Gamepad API, WGI/GameInput, RawInput
- **XInput slots**: a *companion device registers the XUSB interface*, so Xbox-family profiles get real XInput slots, capped at Windows' native 4. Non-Xbox profiles are unlimited
- **Signing**: installs via a locally-generated, locally-trusted certificate. **No EV cert, no test-signing boot mode, no reboot** — admin rights only. (Composite audio/haptics personas bundle WHLK-certified `usbip-win2 0.9.7.7`)
- **Latency**: ~35 µs median input; ~0.15 ms output (rumble) callback latency since v1.4.0
- **Force feedback**: HID PID 1.0 writes + rumble raise an `OutputReceived` event; routing to real hardware is the consumer's job
- **Latest**: **v1.4.3** (1 Aug 2026). 506 commits, ~51 stars, 3 forks
- **API**: **C# only** (`HIDMaestro.Core.dll`, `HMContext`/`HMGamepadState`/`HMButton`). **No C, C++, or Rust bindings.** Requires .NET 10 runtime
- **Known open bug**: dual enumeration (xinputhid companion + separate HID interface) causes **double-input on the WGI surface** — D-pad moves two positions in the Start menu / Xbox accessories app. Unfixed as of v1.1.15 line; fixed on the Chromium Gamepad surface only

Sites: https://github.com/hifihedgehog/HIDMaestro · https://hidmaestro.org/ · https://github.com/hifihedgehog/HIDMaestro/issues/8

### 2b. libvirtualhid (LizardByte / Sunshine)

- **Cross-platform C++** virtual HID library; on Windows a **UMDF2 driver over Microsoft's Virtual HID Framework (VHF)**
- Descriptor-driven profiles: Xbox 360 / One / Series, DS4, DualSense, Switch Pro
- API: `lvh::Runtime::create()` → `runtime->create_gamepad(lvh::profiles::dualsense())`; callbacks for rumble, LEDs, adaptive triggers, raw HID output reports
- **Split license**: library **MIT**; the Windows UMDF driver source and driver packages are **LizardByte Source-Available License 1.0 (LB-SAL 1.0)** — *not* OSI open source
- ARM64 blocked on WHQL/Dev Center signing (Azure Trusted Signing insufficient); x64 is the primary target
- ~54 commits on master, active CI. **No tagged stable release, no C API, no bindings**
- **XInput/`dwUserIndex` behaviour is undocumented** — this is the single biggest unknown for your use case

Sites: https://github.com/LizardByte/libvirtualhid

---

## 3. What currently-maintained projects actually use

| Project | Backend (Aug 2026) | Notes |
|---|---|---|
| **PadForge** v4.1.0 (hifihedgehog) | **HIDMaestro** — migrated *off* ViGEmBus | Fork of x360ce rewritten on SDL3 + HIDMaestro + HidHide + .NET 10. CC BY-NC-SA 4.0. The only shipping proof that HIDMaestro works in production. https://github.com/hifihedgehog/PadForge · https://padforge.org/ |
| **DS4Windows** | **Still ViGEmBus** | `schmaldeo/DS4Windows` **archived 5 Mar 2026**; `nefarius/DS4Windows-schmaldeo` mirror also carries the discontinuation notice; `hbashton/DS4Windows` continues the lineage. All still ship ViGEmBus + .NET 8. https://github.com/hbashton/DS4Windows |
| **Sunshine** (LizardByte) | **ViGEmBus, now *bundled* in the installer** — was download-on-demand | Since v2025.110.45857 Sunshine *hard-fails* without ViGEmBus. Issue #3527 "Transition away from ViGEmBus" (opened 10 Jan 2025) closed as duplicate. Replacement is **PR #5368 "feat(input): implement libvirtualhid"**, opened 3 Jul 2026, **still draft, 21/61 tasks done**; rumble incomplete for some pad types, Xbox Series Share button missing; ARM64 still falls back to ViGEmBus. https://github.com/LizardByte/Sunshine/issues/3527 · https://github.com/LizardByte/Sunshine/pull/5368 |
| **XOutput** | **ViGEmBus** | Explicitly chose ViGEmBus over DLL-injection precisely because the virtual pad is indistinguishable from real hardware to UWP/Store games and anti-cheat. https://xoutput.org/ |
| **x360ce 4.x** | **ViGEmBus** | Last stable **4.17.15.0, 15 Nov 2020**. Effectively dormant — PadForge is its living descendant. https://github.com/x360ce/x360ce |
| **JoyShockMapper** (Electronicks) | **ViGEmBus, optional** | JSM 3 exposes virtual Xbox/DS4 pads *if* ViGEm Bus is installed. https://github.com/Electronicks/JoyShockMapper |
| **Parsec** | Own gamepad path; **ViGEmBus documented as the "legacy controller emulation method"** | https://support.parsec.app/hc/en-us/articles/32381705301908-Setup-Gamepad |

**Takeaway:** in Aug 2026 the installed base is still overwhelmingly ViGEmBus. Exactly one shipping consumer app (PadForge) has moved off it, and the highest-profile migration (Sunshine) is a draft PR a month old.

---

## 4. vJoy — not relevant to you

- **DirectInput-only.** Produces a generic HID joystick with a feeder API (`vJoyInterface.dll`). It **cannot** produce an XInput device
- Original repo archived; official final build **v2.2.1.1** has **expired signing certificates**, so Windows 10/11 block `vjoy.sys`
- `njz3/vJoy` fork (improved FFB) could not ship on Win11 for the same signing reason
- Community rescue: forks (e.g. **BrunnerInnovation/vJoy**) sponsored the Microsoft **attestation signing** run and now distribute signed Win11 drivers; `vjoy.pro` is a community manifest/mirror hub
- Still the right tool for >32-button / many-axis DirectInput devices (flight sim, pedals, head tracking). **Wrong tool for Xbox 360 emulation.**

https://github.com/njz3/vJoy · https://github.com/BrunnerInnovation/vJoy · https://vjoy.pro/

---

## 5. Rust ecosystem

### `vigem-client` (CasualX) — the one to use
- https://github.com/CasualX/vigem-client · https://docs.rs/vigem-client · https://crates.io/crates/vigem-client
- **100% pure Rust**, talks `DeviceIoControl` to `\\.\ViGEmBus` directly — **no `ViGEmClient.dll` dependency**. This matters: it means you have no C++ redistributable/native-lib link step
- **v0.1.4, published 1 Aug 2022. Last commit 1 Aug 2022.** ~11k recent downloads. MIT. Unmaintained but *complete* for X360
- API coverage:
  - `Client`, `TargetId`, `Xbox360Wired<C>`, `DualShock4Wired<C>` (DS4 marked WIP)
  - `XGamepad`, `XButtons` for state
  - `plugin()`, `unplug()`, `wait_ready()`, `is_attached()`, `update(&XGamepad)`
  - **`get_user_index()` → the XInput `dwUserIndex`.** This is exactly what you need to map physical keyboard → deterministic XInput slot
  - **Request Notification API for X360 targets** landed in commit `b863eb7` (6 Jun 2022) — i.e. **it IS in 0.1.4**. This is the rumble + LED-player-number feedback channel. docs.rs surfaces it poorly; read `src/x360.rs`
- **Risk:** single maintainer, 4 years dormant, 23 stars. But it's ~2k lines of stable IOCTL plumbing against a driver that will never change again. Vendoring it is entirely reasonable.

### `vigem-rust` (arounre)
- https://github.com/arounre/vigem-rust · https://crates.io/crates/vigem-rust
- Higher-level, thread-safe, **rumble + LED delivered over Rust channels**, DS4 motion/touchpad, RAII cleanup. Dual MIT/Apache-2.0. Updated Nov 2025
- **3 commits, 6 stars, 1 fork.** Not battle-tested. Good API ideas to crib; risky as a dependency

### `vigem` — older/abandoned wrapper over the native `ViGEmClient.dll`. Skip.

### For the successors
- **HIDMaestro: no Rust bindings.** The SDK is a .NET 10 assembly. Your options if you migrate: (a) NativeAOT-compile a thin C-ABI shim from C# and P/Invoke it from Rust, (b) spawn a small C# sidecar process and IPC to it, or (c) **reimplement the client in Rust** — the driver, the shared-memory section layout, and the profile JSON are all MIT and documented in `docs/INTERNALS.md`. (c) is the clean answer and is maybe a few hundred lines
- **libvirtualhid: no Rust bindings, no C API** (C++-only). Would need a `cxx`/`autocxx` bridge or a hand-written `extern "C"` shim. Also note the driver package is **LB-SAL 1.0, not MIT** — read it before you redistribute

---

## 6. XInput slot semantics — the part that decides your architecture

This is where ViGEmBus is genuinely hard to replace:

- ViGEmBus creates a PDO whose hardware ID matches Microsoft's inbox **`xusb22.sys`**. Windows loads the real Microsoft XUSB driver on top. The result is **not an emulation of XInput — it *is* XInput.** `XInputGetState` sees it, DirectInput sees it (XUSB exposes both an XUSB class interface and a HID class interface), the Xbox Game Bar sees it, anti-cheat sees a normal Xbox pad
- **`get_user_index()`** returns the zero-based `dwUserIndex`, which is the same number as the **LED quadrant** on a physical pad. For a 4-player cabinet you can read this back per virtual pad and know that keyboard #3 really is P3
- **LED / player number** comes back through the **notification callback** (`request_notification` / the `Xbox360Notification` payload), together with the rumble motor values. The XUSB stack pushes it down exactly as it would to real hardware
- **Slot count:** ViGEmBus itself will happily create more than 4 X360 targets, but **XInput only ever exposes 4**. Targets beyond the 4th exist in the device tree and in DirectInput but get no `dwUserIndex`. For your 10-keyboard → 4-pad design this is fine, and it's why KeyboardSplitterXbox capped at 4 in the first place
- **Coexistence with real controllers:** slots are assigned **in arrival order** across real + virtual devices. A real Xbox pad plugged into the cabinet will steal a slot and shift your virtual pads. Two mitigations: (a) plug your virtual pads in at boot before anything else and read back `get_user_index()`, (b) **HidHide** the real devices. ViGEm's own docs cite "working around player slot assignment order issues in XInput" as a primary use case
- **HIDMaestro's approach is different and weaker here.** It publishes a HID gamepad and a *companion device registering the XUSB interface*, so Windows' `xinputhid` synthesizes a 16-button XInput layout over the 12-button HID descriptor. It works, but it's a synthesis layer — and it's the direct cause of the unfixed **WGI double-input bug** (#8), because WGI enumerates both the xinputhid companion and the raw HID interface
- **libvirtualhid** is VHF-based and publishes plain HID gamepads. Whether Xbox profiles land in real XInput slots is **undocumented**. Assume DirectInput/RawInput/SDL work and **verify XInput yourself before committing**

https://learn.microsoft.com/en-us/windows/win32/xinput/directinput-and-xusb-devices

---

## 7. HidHide — yes, it matters to you

- https://github.com/nefarius/HidHide · https://docs.nefarius.at/projects/HidHide/
- Kernel upper-filter "device firewall": hides selected HID devices from all processes except an allowlist. This is how DS4Windows/PadForge stop games from seeing *both* the physical DS4 and the virtual X360 pad
- **Latest: v1.5.230.0** (May 2026) — so it *is* still getting releases, unlike ViGEmBus
- **But**: the maintainer states there is *"no capacity for any major works on HidHide"*; meaningful development requires community PRs
- **Active Windows 11 25H2 bug**: issue **#215** (30 Jun 2026) — `HidHideClient.exe` (and the CLI) crash on every launch with `ERROR_INVALID_PARAMETER` in `GetWhitelist` (`FilterDriverProxy.cpp:91`), reproducible across driver versions **1.2.98.0 → 1.5.230.0**. The *driver* loads and runs fine; it's the config UI/CLI that dies. Unresolved
- **Relevance to your cabinet:** **medium, not critical.** Your inputs are *keyboards*, captured exclusively by the Interception driver — HidHide's classic job (hiding a physical pad that would otherwise double up with the virtual pad) doesn't apply. You *would* want it if a user also plugs a real gamepad in, or if you want to hide the I-PAC keyboards from games that read RawInput/DirectInput keyboard state. Given #215, plan to drive HidHide via its **driver IOCTLs directly from Rust** rather than shelling out to the broken CLI

https://github.com/nefarius/HidHide/issues/215

---

## 8. Driver signing reality in 2026

**Kernel-mode (what ViGEmBus is):**
- Since Windows 10 1607, Windows will not load any new kernel-mode driver not signed by the **Microsoft Dev Portal**
- Requires: a Windows Hardware Dev Center account **backed by an EV code-signing certificate** (~**$226–250/yr**, hardware token or HSM), then either **attestation signing** (no HLK testing, Win10+ client only) or full **WHQL/HLK**
- **Azure Trusted Signing does *not* support EV certificates and does *not* sign Windows drivers.** It cannot replace the EV cert requirement, and it explicitly cannot produce ARM64 driver packages
- Verdict: **a hobby project cannot realistically ship its own signed kernel bus driver.** The recurring cost, the EV identity-validation process (business entity required in practice), and the Dev Center onboarding are all real barriers

**User-mode (UMDF2 — what HIDMaestro and libvirtualhid are):**
- This is the loophole both projects exploit. UMDF2 driver DLLs run in user mode under `WUDFHost`/`mshidumdf.sys`; the **kernel-mode code signing policy does not apply to them**
- On **x64**, installing a UMDF2 driver package whose catalog is signed by a **self-generated certificate placed in Trusted Root CAs + Trusted Publishers** works, with admin rights, **without disabling Secure Boot and without test-signing boot mode**. Multiple developers confirm this on OSR/MS Q&A, and HIDMaestro ships it as its standard install path
- On **ARM64** this breaks — ARM64 requires a Microsoft dashboard signing path. Both HIDMaestro and libvirtualhid hit this wall
- **Caveat**: you are asking the user to trust a machine-generated root CA. That's an acceptable trade on a dedicated arcade cabinet; it would be a red flag on a shared machine

Sources: https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later- · https://learn.microsoft.com/en-us/answers/questions/2110207/azure-trusted-signing-certificate-attestation-sign · https://community.osr.com/t/windows-11-arm64-umdf2-driver-signing/57690

---

## Recommendation for your Rust backend

### Primary: **ViGEmBus 1.22.0 + `vigem-client`**

Rationale specific to an arcade cabinet:

1. **Real XInput slots via Microsoft's own XUSB driver.** No synthesis layer, no WGI double-input bug, no "does MAME/RetroArch/Steam see it?" uncertainty. `get_user_index()` gives you deterministic P1–P4 mapping — which is the entire point of KeyboardSplitterXbox
2. **It is a dedicated, pinned machine.** The #1 argument against a dead driver — "what if a future Windows breaks it?" — is defused by simply not letting the cabinet chase Windows feature updates. Windows 11 Pro lets you defer/pin
3. **Pure-Rust client, zero native deps.** `vigem-client` speaks IOCTL directly. No `ViGEmClient.dll`, no MSVC redist coupling in your build
4. **You don't need what the successors offer.** HIDMaestro's headline features are byte-exact DualSense/Switch Pro identity, adaptive triggers, controller audio, haptics. An arcade cabinet with joysticks and buttons needs none of it. You need 4 X360 pads with correct slots. ViGEmBus does exactly that and nothing else
5. **Still the ecosystem default.** DS4Windows, XOutput, JoyShockMapper, Sunshine (bundled), Parsec (legacy path) all still ship it. It is not going to stop working quietly

**Implementation notes:**
- **Vendor `vigem-client`** into your repo (MIT, ~2k LOC, dormant upstream). Don't take a crates.io dependency on an unmaintained crate for a load-bearing component
- Use `request_notification` even though the cabinet has no rumble — it's how you receive the **LED player index**, which is your ground truth for slot assignment
- **Bundle the ViGEmBus 1.22.0 installer** with your app (this is exactly what Sunshine now does) rather than downloading it — the download endpoints are the part Nefarius explicitly warned would rot
- Verify the installer's signature is `Nefarius Software Solutions e.U.` before running it

### Architecture: abstract the backend from day one

```
trait VirtualPadBackend {
    fn plug(&mut self, slot_hint: u8) -> Result<PadHandle>;
    fn update(&mut self, h: PadHandle, state: &PadState) -> Result<()>;
    fn user_index(&self, h: PadHandle) -> Option<u8>;   // XInput dwUserIndex
    fn poll_feedback(&mut self, h: PadHandle) -> Option<Feedback>; // rumble + LED
    fn unplug(&mut self, h: PadHandle) -> Result<()>;
}
```

This is ~1 day of work and it is what makes plan B cheap. Keep `PadState` in ViGEm's `XGamepad` shape (it's the XInput wire format anyway) so no backend needs a translation layer.

### Plan B (in order of preference, if ViGEmBus dies)

1. **HIDMaestro** — MIT, actively developed (v1.4.3, 1 Aug 2026), user-mode, no EV cert, proven in production by PadForge, and the *only* successor with a shipping consumer. **Write a native Rust client against its shared-memory protocol** rather than P/Invoking the .NET SDK — everything you need (driver, section layout, profile JSON, `docs/INTERNALS.md`) is MIT. Before committing, **test whether the WGI double-input bug (#8) affects your frontend** (it will hit the Xbox Game Bar / Start menu; it may not matter in MAME/RetroArch/Steam Big Picture)
2. **libvirtualhid** — backed by an organisation (LizardByte) rather than one person, which is a real durability argument. But: no stable release, C++-only API, **driver is source-available not open-source (LB-SAL 1.0)**, and **XInput slot behaviour is unverified**. Revisit when Sunshine PR #5368 merges out of draft
3. **Fork ViGEmBus yourself** — BSD-3-Clause permits it; rename it (drop "ViGEm"). But you'd need an EV cert + Dev Center account (~$250/yr + entity validation) to sign a kernel driver. Only worth it if the project grows well past one cabinet
4. **VirtualPad** — closed, commercial, business-partners-only, and Nefarius has not answered an open-source licensing request in five months. **Do not plan around it.**

### Also worth knowing
- Your existing **Interception** driver (per-keyboard capture) is in a similar boat: still functional on Win11, still receiving user issues (#218 May 2026, #220 Jul 2026), no active development. Common failure is `Could not write to \system32\drivers` under Secure Boot/UAC. Budget for the same "pin the OS, bundle the installer" strategy there
- If you ever want >4 players, only DirectInput can go past 4 — that would mean vJoy (signed community fork) or a HIDMaestro non-Xbox profile, not XInput

---

## Sources

- [nefarius/ViGEmBus (archived)](https://github.com/nefarius/ViGEmBus) · [Releases](https://github.com/nefarius/ViGEmBus/releases)
- [About ViGEm — Nefarius docs](https://docs.nefarius.at/projects/ViGEm/) · [End of Life Statement](https://docs.nefarius.at/projects/ViGEm/End-of-Life/)
- [About Nefarius VirtualPad Framework](https://docs.nefarius.at/projects/VirtualPad/)
- [nefarius/DsHidMini Discussion #424 — VirtualPad licensing for OSS](https://github.com/nefarius/DsHidMini/discussions/424)
- [hifihedgehog/HIDMaestro](https://github.com/hifihedgehog/HIDMaestro) · [hidmaestro.org](https://hidmaestro.org/) · [Issue #8 (WGI double-input)](https://github.com/hifihedgehog/HIDMaestro/issues/8)
- [hifihedgehog/PadForge](https://github.com/hifihedgehog/PadForge) · [padforge.org](https://padforge.org/)
- [LizardByte/libvirtualhid](https://github.com/LizardByte/libvirtualhid)
- [Sunshine Issue #3527 — Transition away from ViGEmBus](https://github.com/LizardByte/Sunshine/issues/3527) · [PR #5368 — implement libvirtualhid](https://github.com/LizardByte/Sunshine/pull/5368)
- [nefarius/HidHide](https://github.com/nefarius/HidHide) · [Releases](https://github.com/nefarius/HidHide/releases) · [Issue #215 — 25H2 crash](https://github.com/nefarius/HidHide/issues/215)
- [schmaldeo/DS4Windows (archived)](https://github.com/schmaldeo/DS4Windows) · [hbashton/DS4Windows](https://github.com/hbashton/DS4Windows) · [nefarius/DS4Windows-schmaldeo](https://github.com/nefarius/DS4Windows-schmaldeo)
- [XOutput](https://xoutput.org/) · [x360ce/x360ce](https://github.com/x360ce/x360ce) · [JoyShockMapper](https://github.com/JibbSmart/JoyShockMapper) · [Parsec gamepad setup](https://support.parsec.app/hc/en-us/articles/32381705301908-Setup-Gamepad)
- [njz3/vJoy](https://github.com/njz3/vJoy) · [BrunnerInnovation/vJoy](https://github.com/BrunnerInnovation/vJoy) · [vjoy.pro](https://vjoy.pro/)
- [CasualX/vigem-client](https://github.com/CasualX/vigem-client) · [docs.rs/vigem-client](https://docs.rs/vigem-client) · [crates.io](https://crates.io/crates/vigem-client) · [arounre/vigem-rust](https://github.com/arounre/vigem-rust)
- [MS Learn — DirectInput and XUSB Devices](https://learn.microsoft.com/en-us/windows/win32/xinput/directinput-and-xusb-devices) · [Kernel-Mode Code Signing Policy](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later-) · [Azure Trusted Signing & driver attestation](https://learn.microsoft.com/en-us/answers/questions/2110207/azure-trusted-signing-certificate-attestation-sign) · [Virtual HID Framework (VHF)](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/virtual-hid-framework--vhf-)
- [OSR — Windows 11 ARM64 UMDF2 driver signing](https://community.osr.com/t/windows-11-arm64-umdf2-driver-signing/57690)
- [oblitum/Interception](https://github.com/oblitum/Interception)