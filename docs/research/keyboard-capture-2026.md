# Per-Device Keyboard Capture & Blocking on Windows 11 (2026) — Research Report

## 0. Headline finding (time-sensitive)

**The Interception driver is on a hard deadline, and this machine is already running the policy that will kill it.**

Interception's kernel drivers are signed with a legacy cross-signed certificate (`CN=Francisco Lopes da Silva, C=BR`, GlobalSign, **cert expired 2012-10-21**, countersigned 2009). Microsoft's April 2026 servicing update removes default trust for the entire cross-signed driver program on Windows 11 24H2 / 25H2 / 26H1 and Server 2025 — rolling out in *evaluation mode* first, then flipping to *enforcement* per-machine.

Verified on this system (read-only inspection):

| Check | Result |
|---|---|
| OS | Windows 11 Pro **25H2**, build **26200.8875** — an affected build |
| `C:\Windows\System32\drivers\keyboard.sys` | `Oblita` / `Interception` / "Keyboard Upper Filter Driver", v1.00 built by WinDDK |
| Signature | `CN=Francisco Lopes da Silva, C=BR`, **NotAfter 10/21/2012** |
| Keyboard class UpperFilters | `keyboard`, `kbdclass` ← Interception is live in the stack |
| Mouse class UpperFilters | `mouse`, `mouclass` |
| Active CI policy | `{784c4414-79f4-4c32-a6a5-f0fb42a51d0d}` **"Microsoft Windows Cross Certificates for Code Integrity Exceptions Audit Policy"**, id `10.29611.0.0`, activated 8/3/2026 |
| Also active | `{d2bda982-…}` "Microsoft Windows Driver Policy" id `10.0.29520.0` |

That GUID is exactly the audit policy from Microsoft's cross-signed-trust removal. Enforcement activates once the machine accumulates clean uptime/boot sessions with no violating driver loads; a loaded cross-signed driver resets the counters (so the cabinet stays in evaluation mode until Interception is removed — and gets blocked the moment enforcement lands via allow-list/policy refresh).

**Do not architect a 2026 rebuild around Interception as the permanent primary.**

- https://support.microsoft.com/en-us/windows/hardware/drivers/the-windows-driver-policy
- https://techcommunity.microsoft.com/blog/windows-itpro-blog/advancing-windows-driver-security-removing-trust-for-the-cross-signed-driver-pro/4504818
- https://securetron.net/microsoft-is-removing-trust-for-cross-signed-kernel-drivers-and-here-is-how-to-validate/
- https://www.theregister.com/software/2026/03/27/microsoft-cracks-down-on-old-windows-kernel-drivers/

---

## 1. oblitum/Interception — current status

**Maintenance:** effectively abandoned. Last release **v1.0.1, 2017-05-12** (v1.0.0 was 2015-07-25). README still says *"Tested from Windows XP to Windows 10"* — Windows 11 is never mentioned. Open issues as recent as 2026 go unanswered, including #220 *"It's strange that the creator disappeared out of nowhere"* (Jul 2026), #215 "Can't install interception in windows 11" (Mar 2026), #214 Secure Boot / `\system32\drivers` write failures (Jan 2026), #213 "ARM64 support?", #212 "keyboard.sys loads but mouse.sys does not", #217 "blocked by EAC". Capsicain's author independently notes *"the creator of the Inception driver seems offline; a future Windows version might require a new driver solution."*

**Architecture:** two class upper-filter drivers (`keyboard.sys` on `kbdclass`, `mouse.sys` on `mouclass`) plus a user-mode `interception.dll`. Because it filters the *class* stack, it sees every keyboard including PS/2 and laptop internal, and can drop events before they reach `win32k`.

**The 10-device limit is real and is a genuine hazard for an arcade cabinet:** device IDs 1–10 are keyboards, 11–20 are mice. Every unplug/replug or hibernate/resume cycle **increments** a device's Interception ID, and *"if the ID of a device goes above 10 (for keyboards) or 20 (for mice), the device will completely cease to function until the next reboot."* evilC states plainly: *"There is nothing I can do to fix this issue, it is a limitation of the Interception driver."* A cabinet with 4 I-PACs plus a real keyboard plus a wireless dongle will burn through 10 slots after a handful of USB events.

**Other known issues:** kanata documents that *system sleep or repeated device connection cycles cause input failure* requiring a full restart, and that less-common keys are unsupported or mishandled.

**Licensing:** dual-licensed. Non-commercial = **LGPL for the library and source**, with redistribution rights for the binary drivers/installers *only if* your code talks to the drivers solely through the published API. Commercial use requires a paid license (contact `francisco@oblita.com`) — either "Interception API License" (removes the commercial restriction, adds a silent-install library) or "Interception License" (grants driver + installer source). **The driver source is not public.** Practical consequence: fine for a personal arcade cabinet; a blocker for anything you'd sell, and — critically — **since the seller appears to have vanished, nobody can obtain a commercial license or a re-signed driver anymore.**

- https://github.com/oblitum/Interception
- https://github.com/oblitum/Interception/releases
- https://github.com/oblitum/Interception/issues
- https://github.com/evilC/AutoHotInterception/blob/master/README.md
- https://raw.githubusercontent.com/jtroo/kanata/main/docs/platform-known-issues.adoc

---

## 2. What actively-maintained remappers actually use

| Tool | Mechanism | Per-device? | Status |
|---|---|---|---|
| **kanata** (Rust) | Two backends: `kanata.exe`/`kanata_winIOv2.exe` = `WH_KEYBOARD_LL` + `SendInput`; `kanata_wintercept.exe` = Interception | **Only** in the Interception build, via `windows-interception-keyboard-hwids` | Very active — **v1.12.0, 2026-07-05**, x64 + ARM64, both variants shipped |
| **KMonad** (Haskell) | Windows: low-level hook only. Linux gets per-device via evdev, Windows does not | No | Maintained, Windows backend acknowledged as weak |
| **capsicain** (C++) | Interception | Yes | Issues active into Mar 2026, but author moved to Linux |
| **AutoHotInterception / UCR** (C#/AHK) | Interception | Yes — VID/PID + instance index, or device handle for PS/2 | The canonical per-device-blocking reference implementation |
| **HidHide** (nefarius) | HID-class filter driver | **Cannot hide keyboards or mice** | Actively maintained, attestation/HLK-signed |
| **reWASD** (commercial) | Virtual controller (ViGEm-style bus) + HidHide-style hiding for gamepads; keyboard remap uses an emulated keyboard device | Yes for gamepads | Commercial, closed |
| **PowerToys Keyboard Manager** | Low-level hook | No | Microsoft, active |

The important signal: **every tool that offers real per-device blocking on Windows in 2026 is sitting on the same 2017 Interception binary.** There is no maintained replacement. kanata is the healthiest consumer and still just ships the same `interception.dll` + `keyboard.sys`.

kanata's device selection is worth copying:
```lisp
windows-interception-keyboard-hwids (
  "90, 80, 11, 34"
  "99, 88, 77, 66"
)
;; also: windows-interception-keyboard-hwids-exclude (...)
```
HWIDs are byte-array representations of the ASCII hardware IDs visible in Device Manager; run with the list empty and kanata logs each device's hwid on first keypress.

**HidHide is not an option for you** — nefarius states directly that mouse and keyboard input *"travel through different means and routes through Windows"* and HidHide's blocking cannot interfere with them; a major redesign would be required. (nefarius/HidHide issue #4.)

- https://github.com/jtroo/kanata · https://jtroo.github.io/config.html · https://deepwiki.com/jtroo/kanata/4.2-windows-implementation
- https://github.com/cajhin/capsicain
- https://github.com/evilC/AutoHotInterception · https://github.com/snoothy/ucr/wiki/Core_Interception
- https://github.com/nefarius/HidHide/issues/4 · https://docs.nefarius.at/projects/HidHide/FAQ/
- https://github.com/kmonad/kmonad/blob/master/doc/faq.md

---

## 3. Raw Input API — device identity without blocking

**Yes, Raw Input distinguishes devices.** `WM_INPUT` carries `RAWINPUT.header.hDevice`; `GetRawInputDeviceInfo(RIDI_DEVICENAME)` yields the device interface path (e.g. `\\?\HID#VID_D209&PID_0430&MI_00#8&2a0d0500&0&0000#{...}`), which is stable per physical port. Raymond Chen's canonical article confirms this is the supported way to read multiple keyboards individually.

**No, it cannot block.** `RIDEV_NOLEGACY` suppresses `WM_KEYDOWN`/`WM_CHAR` **only for the registering application**, not system-wide. Microsoft's docs are explicit: *"the system does not generate any legacy message for that device **for the application**."* Other processes still receive everything.

### The Raw Input + `WH_KEYBOARD_LL` correlation technique

The classic workaround (CodeProject "Combining Raw Input and keyboard Hook to selectively block input from multiple keyboards", and the Hackaday CNC-jog project that follows it) runs both:
- `WM_INPUT` gives *which device*
- `WH_KEYBOARD_LL` gives *the ability to return 1 and swallow the event*
- The two are matched by vkey + arrival order/timing.

**Reliability verdict: use only as a degraded fallback.** Documented and structural problems:

1. **No ordering guarantee.** `WH_KEYBOARD_LL` runs on your hook thread's message pump; `WM_INPUT` is queued to a window. The two paths are not synchronised, so the hook may have to decide before the correlating `WM_INPUT` arrives. Implementations resort to a bounded delay or a "guess from last known device" heuristic.
2. **Two identical keyboards pressing the same vkey within the correlation window are ambiguous** — precisely the arcade case (two identical I-PACs, or one I-PAC where P1 and P2 mash simultaneously).
3. **Key auto-repeat and high-rate input** flood the queue and break 1:1 pairing. An I-PAC in a fighting game generates dense simultaneous multi-key traffic — the worst case for this technique.
4. **The hook is a global timeout risk.** `LowLevelHooksTimeout` (default 300 ms) silently unhooks or bypasses slow hooks; any blocking wait inside the hook proc is dangerous. KMonad issue #102 ("Unregistering low-level Windows keyboard hook") is this failure mode in the wild.
5. **Cannot block Win+L, Ctrl+Alt+Del, or other secure-desktop/OS-reserved combos** (kanata issues #192, #428). Interception can't intercept CAD either, but it does get Win+L.
6. **Hook-vs-hook conflicts** with other LLHOOK users (kanata issues #55, #250, #430; espanso #1488).
7. The Hackaday author, after implementing it, calls it flawed and notes it *"requires hooking into the apps, which may not work on all apps that use RAWINPUT."*

The purebasic/AHK community consensus is blunt: *"Low-level hooks can block but cannot differentiate keyboards; Raw Input can differentiate but cannot block. It's not possible to combine them"* reliably for all keys.

- https://devblogs.microsoft.com/oldnewthing/20160627-00/?p=93755
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawinputdevice
- https://www.codeproject.com/articles/716591/combining-raw-input-and-keyboard-hook-to-selective
- https://hackaday.io/project/5364-cheap-windows-jogkeyboard-controller-for-cncs/log/16845-combing-the-keyboard-hook-with-rawinput
- https://www.purebasic.fr/english/viewtopic.php?t=66655

---

## 4. Newer options in 2026

**Nefarius has no keyboard capture driver.** His portfolio is ViGEmBus (archived Nov 2023 over a trademark dispute; still functional; successor "VirtualPad" announced), HidHide (HID only, explicitly not keyboards), BthPS3, DsHidMini. Nothing replaces Interception. His drivers *are* attestation/HLK-signed, so they survive the April 2026 policy.

**Microsoft's official position is hostile to this whole category.** In a 2026 Q&A, Microsoft confirmed there is **no** official USB HID keyboard filter sample (`kbfiltr` is PS/2-only), that `IOCTL_HID_READ_REPORT` is not a supported interception point, and recommends *against* kernel-mode keyboard remapping (breaks accessibility, IME, secure input, no forward-compat guarantee). Their suggested alternatives: fix it in firmware, use user-mode remapping, or use Virtual HID Framework (VHF) to *inject* rather than filter.

**Rolling your own filter driver:** technically the right answer (HID upper filter on the device stack, above `hidclass`, below `kbdhid.sys`), but post-April-2026 it must be WHCP- or attestation-signed through the Hardware Dev Center. That requires an EV code-signing certificate (~$215–$499/yr; DigiCert ~$409, Sectigo ~$290–499) or **Microsoft Trusted Signing at ~$9.99/month**, plus a Partner Center account. Attestation signing itself is free. Unsigned forks (e.g. `x64dbgg/keyboardfilter`, `Yoticc/Interception.netfork`) require test-signing mode — unacceptable for a cabinet with Secure Boot.

**The genuinely modern, signing-free option: WinUSB / `nusb` device claim.** Bind the I-PAC's keyboard interface to Microsoft's in-box `winusb.sys` (WHQL-signed, ships with Windows, immune to the 2026 policy). Consequences:
- The interface leaves the `kbdclass` stack entirely → **Windows never sees a single keystroke from it.** Blocking is total and free, with zero filter driver.
- You read the HID interrupt-IN endpoint directly, so **device identity is intrinsic** — no correlation heuristics, no 10-device ceiling, no ID-drift on replug.
- No LLHOOK timeout risk, no hook conflicts, no ordering races.
- Latency is a USB interrupt transfer, not a `win32k` round-trip.

Cost/caveat: rebinding requires a signed INF. Zadig/libwdi does this by generating a self-signed cert and installing it into Trusted Root/Trusted Publishers — acceptable for a dedicated cabinet, a wart for redistribution. Also note the confirmed side effect (libusb/hidapi issue reports, libwdi #120/#8): *"when WinUSB is installed for a keyboard device, the keyboard does not respond to inputs"* and Zadig has historically made HID rebinds hard to reverse — which is exactly the behaviour you want here, but keep a recovery keyboard on a different port and document the `pnputil` rollback.

- https://learn.microsoft.com/en-us/answers/questions/5689423/missing-usb-hid-filter-sample-in-windows-driver-sa
- https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/virtual-hid-framework--vhf-
- https://github.com/nefarius/ViGEmBus · https://docs.nefarius.at/projects/ViGEm/End-of-Life/
- https://github.com/libusb/libusb/wiki/Windows · https://github.com/pbatard/libwdi/issues/8

---

## 5. Ultimarc I-PAC specifics (measured on your machine)

Your board is **`VID_D209&PID_0430`** (Ultimarc), plus a second Ultimarc device `VID_D209&PID_15A2` (trackball/spinner). Actual enumeration:

```
USB\VID_D209&PID_0430\4                              USB Composite Device
 ├ USB\VID_D209&PID_0430&MI_00                       USB Input Device
 │   └ HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000   HID Keyboard Device   ← the keys
 ├ USB\VID_D209&PID_0430&MI_01
 │   ├ &COL01  HID-compliant system controller
 │   ├ &COL02  HID-compliant consumer control device
 │   └ &COL03  HID-compliant mouse
 └ USB\VID_D209&PID_0430&MI_02
     ├ &COL01  HID-compliant device
     └ &COL02  HID-compliant device
USB\VID_D209&PID_15A2                                USB Input Device
 └ HID\VID_D209&PID_15A2\7&14CBD05C&0&0000            HID-compliant mouse
```

Key points:
- **It is a USB composite device**, keyboard on **interface MI_00**, mouse/system/consumer on MI_01, two vendor/gamepad collections on MI_02.
- To Interception and to Raw Input it is a **completely ordinary HID keyboard** — Interception sees it as one of its 1–10 keyboard slots; Raw Input reports it with the device path above. Nothing special is required, and nothing special is possible: you cannot tell P1 from P2 within one I-PAC, only board-from-board. (Legacy KeyboardSplitter's model — one physical board per virtual pad — is therefore the right model, and an I-PAC4 counts as one device even though it wires 4 players.)
- Multiple identical I-PACs share VID/PID and are distinguished only by **device instance path / hardware handle** (`8&2A0D0500&0&0000` above), which is port-topology-derived and therefore **stable as long as boards stay in the same physical USB ports**. Design for that: key your config on the instance path, not on VID/PID alone.
- **Native XInput path (alternative, worth noting):** firmware ≥ v1.50 "Multi-Mode" on 2015+ I-PAC / I-PAC2 / I-PAC4 / Ultimate I/O supports keyboard+mouse+LED, dual gamepad+mouse+LED, or **dual XInput controller**. Latest firmware is 1.55 (Ultimate I/O 1.56). Mode switching is via the WinIPAC utility; if the board comes up in XInput unexpectedly, hold P1SW1 while plugging in to reset to keyboard. There's also a variant firmware exposing keyboard **and** dual standard gamepad simultaneously (no XInput in that build; version numbering: leading digit `4` = no gamepad, `3` = with gamepad). After any firmware change you must uninstall the "composite device" in Device Manager so Windows re-enumerates.
  - This gives 2 XInput pads per board with zero software — but caps you at 2 pads/board, loses per-key remapping flexibility, and doesn't cover non-I-PAC keyboards. Correct to keep as a hardware fallback, not the plan.

- https://www.ultimarc.com/MultiMode.pdf · https://www.ultimarc.com/downloads/
- https://www.ultimarc.com/control-interfaces/i-pacs/i-pac-ultimate-i-o/
- https://github.com/katie-snow/Ultimarc-linux/blob/master/README.fw

---

## 6. Rust ecosystem assessment

| Crate | Version / last release | Verdict |
|---|---|---|
| **`kanata-interception`** | 0.3.0, **2024-08-09**, maintained by jtroo | **Best Interception binding.** Fork of bozbez's crate, actively exercised by kanata's shipping `wintercept` builds on x64 + ARM64. Use this, not the original. |
| `interception` (bozbez) | 0.1.2, **2020-07-30**, ~8.6k downloads | Unmaintained upstream; superseded by the kanata fork |
| `interception-sys` | raw FFI to `interception.dll` | Only if you want to bind by hand |
| `multiinput` | 0.1.0, **2020-05-11**, 36k downloads | Raw Input wrapper, *can* differentiate keyboards/mice, hidden background message window. **Unmaintained 6 years.** Fine to read for reference; don't depend on it. |
| `rawinput` (Jonesey13) | deprecated, superseded by `multiinput` | Dead |
| **`windows`** (microsoft) | **0.62.2, 2025-10-06** | **Full coverage.** `windows::Win32::UI::Input::{RegisterRawInputDevices, GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList}` and `windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, CallNextHookEx, UnhookWindowsHookEx}` with `WH_KEYBOARD_LL`, `KBDLLHOOKSTRUCT`, `LLKHF_INJECTED`. Roll your own thin wrapper — it's ~200 lines and beats an abandoned crate. |
| **`nusb`** | **0.2.7, 2026-08-03**, 1.08M downloads | **Pure-Rust cross-platform USB, very actively maintained.** The right tool for the WinUSB path. |
| `hidapi` | maintained | Alternative for HID-level access; note Windows reserves exclusive access to HID keyboards/mice, so this only works *after* rebinding off `kbdhid` |
| `vigem-client` (CasualX) | pure-Rust ViGEmBus client, no C lib | **Output side** — the ScpVBus/vXboxInterface replacement |
| `vigem-rust` | updated Nov 2025, thread-safe, typed reports, rumble/LED notifications | Newer alternative for the output side |

Reference implementations to read: `kmgb/keyboardhook-rs`, `win-hotkeys`, `MaxMahem/windows_hook`.

- https://lib.rs/crates/kanata-interception · https://github.com/bozbez/interception-rs
- https://crates.io/crates/multiinput · https://github.com/Jonesey13/multiinput-rust
- https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/Input/fn.RegisterRawInputDevices.html
- https://github.com/kevinmehall/nusb
- https://github.com/casualx/vigem-client · https://docs.rs/vigem-rust

---

## 7. Recommendation for the Rust backend

**Architecture: define a `CaptureBackend` trait up front and ship three implementations.** The per-device capture layer is the single most volatile part of this stack in 2026; do not let it leak into your mapping engine.

```rust
pub struct DeviceId(pub String);          // stable device instance path
pub struct KeyEvent { pub device: DeviceId, pub scancode: u16, pub down: bool, pub t: Instant }

pub trait CaptureBackend: Send {
    fn devices(&self) -> Vec<DeviceInfo>;
    fn set_captured(&mut self, ids: &[DeviceId]) -> Result<()>;  // capture == block
    fn poll(&mut self) -> Result<Vec<KeyEvent>>;
}
```

### Primary: `WinUsbBackend` — claim the I-PAC via WinUSB + `nusb`

This is the recommendation. It is the only option that is *both* correct today and unaffected by the April 2026 signing cliff.

- Bind `USB\VID_D209&PID_0430&MI_00` (and each additional board's MI_00) to in-box `winusb.sys`. Leave MI_01/MI_02 alone if you want the trackball/spinner to keep working normally, or claim them too.
- Read interrupt-IN reports with `nusb` 0.2.x. Parse the 8-byte boot-protocol keyboard report (modifier byte + 6 usage slots) — the I-PAC is NKRO-capable, so also handle its extended report descriptor; pull the descriptor at runtime rather than hardcoding.
- **Blocking is structural**: the interface is no longer in the keyboard stack, so nothing else on the system can see it. No hook, no filter, no race.
- **Identity is structural**: one `nusb::Device` per board.
- No 10-device limit, no ID drift on replug, no hibernate breakage.
- Signing: none needed for `winusb.sys` itself. You need a signed INF to rebind — use libwdi/Zadig (self-signed cert into the local store) for the cabinet, or produce a properly attestation-signed INF later if you distribute.
- Recovery: document `pnputil /remove-device` + rescan, and keep one non-claimed keyboard on a separate port so you can never lock yourself out.

### Fallback A: `InterceptionBackend` — via `kanata-interception`

Keep it for parity with the legacy app, for non-I-PAC keyboards you don't want to WinUSB-claim, and for machines where the WinUSB rebind isn't acceptable.

- Use the **`kanata-interception` 0.3.0** crate, not `interception` 0.1.2.
- Copy kanata's HWID filter model (`windows-interception-keyboard-hwids` / `-exclude`).
- Emit a loud startup warning: check `Get-AuthenticodeSignature` on `keyboard.sys` and the `{784c4414-…}` CI policy state, and tell the user this backend has a known end-of-life.
- Handle the 10-slot exhaustion explicitly: track IDs, detect climb past 10, surface "reboot required" rather than silently going deaf.
- **Licensing constraint: LGPL, non-commercial only.** You may redistribute the driver binaries *only* if your code talks to them exclusively through the published API (so: dynamically link `interception.dll`, don't poke the device objects yourself). Any commercial distribution needs a paid license from an author who appears to be unreachable — treat commercial use of this backend as effectively closed.

### Fallback B: `RawInputHookBackend` — Raw Input + `WH_KEYBOARD_LL`

Ship it, but label it "best effort / degraded".

- `RegisterRawInputDevices` with `RIDEV_INPUTSINK` on a hidden message-only window for identity; `WH_KEYBOARD_LL` returning 1 for blocking; correlate on (vkey, scancode, arrival order) with a small bounded queue.
- Use the `windows` crate 0.62 directly; no third-party crate is maintained.
- Keep the hook proc allocation-free and under ~1 ms (`LowLevelHooksTimeout` is 300 ms by default). Do all work on a channel to a worker thread.
- Accept that identical simultaneous keys from two identical boards will occasionally mis-attribute, and that Win+L is unblockable.

### Output side (for completeness)

ScpVBus + vXboxInterface → **ViGEmBus** via `vigem-client` (pure Rust, no C dependency) or `vigem-rust`. ViGEmBus is archived (Nov 2023) but attestation/HLK-signed, so it is *not* affected by the April 2026 change and continues to work; watch nefarius' "VirtualPad" successor.

### Hardware escape hatch — document it, don't build on it

If the software path ever becomes untenable, reflashing the I-PACs to **dual-XInput Multi-Mode** (firmware 1.5x) gives 2 XInput pads per board with no drivers at all. Two boards = the 4 pads you need. You lose per-key remapping and non-I-PAC keyboard support, which is exactly why it's the escape hatch and not the plan.

### Migration order

1. Build the trait + `RawInputHookBackend` first — no admin rights, no driver, fastest to a working device-enumeration/identity UI.
2. Add `InterceptionBackend` next for immediate feature parity with KeyboardSplitterXbox (it's already installed on this machine).
3. Land `WinUsbBackend` as the target state and make it the default for recognised Ultimarc VID `D209` devices.
4. Wire `vigem-client` for output from day one; it's the least controversial piece.

**Sources:** all URLs are inline above; the primary ones are [Microsoft's Windows Driver Policy](https://support.microsoft.com/en-us/windows/hardware/drivers/the-windows-driver-policy), [Microsoft's cross-signed trust removal announcement](https://techcommunity.microsoft.com/blog/windows-itpro-blog/advancing-windows-driver-security-removing-trust-for-the-cross-signed-driver-pro/4504818), [oblitum/Interception](https://github.com/oblitum/Interception), [jtroo/kanata](https://github.com/jtroo/kanata), [evilC/AutoHotInterception](https://github.com/evilC/AutoHotInterception), [nefarius/HidHide](https://github.com/nefarius/HidHide), [Microsoft on HID filter drivers](https://learn.microsoft.com/en-us/answers/questions/5689423/missing-usb-hid-filter-sample-in-windows-driver-sa), [Ultimarc Multi-Mode firmware](https://www.ultimarc.com/MultiMode.pdf), and [kevinmehall/nusb](https://github.com/kevinmehall/nusb).