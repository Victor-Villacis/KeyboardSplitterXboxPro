# Native / Driver Interop Deep-Dive — KeyboardSplitterXboxPro

Repo root: `C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro`

---

## 0. Layer map

| Layer | Project | Artifact | Talks to |
|---|---|---|---|
| Per-keyboard capture | `Interceptor/` (C#) | `Interceptor.dll` | `interception.dll` (C, Francisco Lopes) → `keyboard.sys`/`mouse.sys` upper filter drivers |
| Virtual pad — managed | `VirtualXbox/` (C#) | `VirtualXbox.dll` | `VirtualXboxNative.dll` |
| Virtual pad — native | `VirtualXboxNative/` (C++) | `VirtualXboxNative.dll` | **raw `DeviceIoControl` to ScpVBus** — *not* vXboxInterface.dll |
| Controller readback / tester | `XInputWrapper/` (C#) | `XinputWrapper.dll` | `System32\xinput1_3.dll` |
| Per-game xinput shim | `xinput/` (C++) | `xinput.dll` | proxies to system `xinput*.dll`, rewrites `SubType` |
| Orchestration | `KeyboardSplitter/Managers/DriversManager.cs` | — | installers |

**Key finding #1:** there is **no `vXboxInterface.dll` in this repo**. `VirtualXboxNative/virtualXbox.cpp` is djlastnight's own C++ reimplementation of shauleiz's vXbox API that issues the ScpVBus IOCTLs directly. The README credits `shauleiz/vXboxInterface` but only as source of the API shape.

---

## 1. Interceptor — P/Invoke over `interception.dll`

### P/Invoke surface
`Interceptor/NativeMethods.cs` — 13 imports, all `CallingConvention.Cdecl`:

`interception_create_context`, `destroy_context`, `get/set_precedence`, `get/set_filter`, `wait`, `wait_with_timeout`, `send`, `receive`, `get_hardware_id` (`CharSet.Unicode`, `StringBuilder`), `is_invalid`, `is_keyboard`, `is_mouse`.

Filter callback: `Interceptor/Predicate.cs` — `[UnmanagedFunctionPointer(Cdecl)] delegate int Predicate(int device)`. The delegate handed to `interception_set_filter` is `NativeMethods.IsKeyboard` / `IsMouse` — i.e. the *native* predicate function pointer is passed straight back in, so there is no managed callback lifetime problem.

Structs: `Interceptor/Structs/Stroke.cs` is an explicit-layout union of `KeyStroke` (`ushort Code; ushort State; uint Information`) and `MouseStroke` (`Interceptor/Structs/MouseStroke.cs`).

### Device enumeration (device IDs)
`Interceptor/Interception.cs`:
- `MaxKeyboardsCount = 10`, `MaxMiceCount = 10`, `MaxDeviceCount = 20`.
- IDs **1–10 = keyboards, 11–20 = mice** (Interception's fixed convention). `InterceptionDevice.StrongName` (`Interceptor/InterceptionDevice.cs:51`) computes `DeviceID` for keyboards and `DeviceID - 10` for mice, producing names like `Keyboard_01`, `Mouse_03`.
- `RescanInputDevices()` (`Interception.cs:717`) loops IDs, calls `GetHardwareID(id)`, and keeps the device if the HWID is non-null and `interception_is_invalid(id) == 0`.
- A background `connectionThread` re-scans every **1000 ms** (`Interception.cs:713`) — this is the hot-plug detection mechanism. Polling, not WM_DEVICECHANGE / CM notification.

**Bug:** the rescan loop is `for (int id = 1; id < MaxDeviceCount; id++)` (`Interception.cs:721`) — strict `<`, so **device 20 (the 10th mouse) is never enumerated**. The ctor's state-table loop at `Interception.cs:69` uses `i <= MaxDeviceCount`, so the two disagree.

### Hardware ID retrieval → friendly name
`GetHardwareID` (`Interception.cs:565`) passes a 500-capacity `StringBuilder` and the literal `500` as the buffer size. The C API expects **bytes**; the builder holds 1000 bytes, so it under-reports (safe, but truncates long HWIDs).

`InterceptionDevice.FriendlyName` (`Interceptor/InterceptionDevice.cs:58`) then does something fragile: it strips `REV_x&` out of the HWID string and uses the remainder as a **registry path** under `HKLM\System\CurrentControlSet\Enum\<hwid>`, recursively walks subkeys, filters on `Class == "Keyboard"`, and takes the substring of `DeviceDesc` after `;`. Any failure falls back to `"n/a"`. This is the piece most likely to silently degrade — it depends on Enum key layout and on `DeviceDesc` still being in `@file.inf,%str%;Friendly Name` form.

### Send/receive loop and blocking
`DriverCallback()` (`Interception.cs:587`) — a single `ThreadPriority.Highest` background thread:

```
SetFilter(ctx, IsKeyboard, KeyboardFilterMode.All)   // 0xFFFF
SetFilter(ctx, IsMouse,    MouseFilterMode)
while (Receive(ctx, deviceId = Wait(ctx), ref stroke, 1) > 0) {
    ... map stroke -> List<KeyInfo>, update keyStates ...
    InputActivity(this, args);
    if (args.Handled) continue;        // <-- BLOCK: stroke is never re-sent
    Send(ctx, deviceId, ref stroke, 1); // <-- PASSTHROUGH
}
```

**How blocking works:** filter mode `All` means the driver hands *every* keystroke from *every* keyboard to this process instead of to the OS. Passthrough is *opt-in*: the app must explicitly call `interception_send` to give the stroke back. Setting `e.Handled = true` just skips that call. Injection uses the same `Send` path (`SendKey`/`SendText`/`SendMouseEvent`, `Interception.cs:296-398`).

**Consequences that matter for the rewrite:**
- All 10 keyboards funnel through **one** blocking `Wait`/`Receive` thread.
- `KeyboardSplitter/Managers/InputManager.cs:278` does `this.Dispatcher.Invoke(action)` — a **synchronous marshal to the WPF UI thread on every keystroke**. If the UI thread stalls, the interception thread stalls, and **all keyboards and mice in the system go dead until reboot**. The app itself warns about this in `KeyboardSplitter/App.xaml.cs:36-45`.
- Emergency escape hatches are wired in `InputManager.cs:304` (`CheckForEmergencyHit`): Ctrl+Alt+Del, or tapping LeftCtrl / RightCtrl 5× to unblock.
- `Unload()` (`Interception.cs:247`) calls `Thread.Abort()` — **throws `PlatformNotSupportedException` on .NET 5+**, so this project cannot even be lifted to modern .NET without rework.
- `deviceId` is an instance field written by the receive thread and read by `SendKey` — a data race if injection is ever driven from another thread.

### Driver state detection
`Interceptor/InterceptionDriver.cs:21` — WMI `SELECT * FROM Win32_SystemDriver WHERE Name='keyboard'` **plus** a `FileVersionInfo` check on `%windir%\system32\drivers\keyboard.sys` for `CompanyName == "Oblita"`, `ProductVersion == "1.00"`, `FileDescription == "Keyboard Upper Filter Driver"`. Three-state result: `Installed` / `RebootRequired` (file present, service not yet registered) / `NotInstalled`. Note it **only ever checks the keyboard filter**, never `mouse.sys`.

---

## 2. VirtualXbox + VirtualXboxNative — ScpVBus IOCTL layer

### Managed API
`VirtualXbox/NativeMethods.cs` — 11 imports from `VirtualXboxNative.dll`, Cdecl: `VBusExists`, `GetEmptyBusSlotsCount`, `ControllerExists`, `IsControllerOwned`, `PlugIn`, `Unplug`, `GetLedNumber`, `SetButton`, `SetTrigger`, `SetDpad`, `SetAxis`.

`VirtualXbox/VirtualXboxController.cs` mirrors these and additionally keeps a **managed shadow copy** of state (`ControllerState[4]`) because ScpVBus has no read-back for what you wrote. Every setter first checks `IsControllerOwned(userIndex)` — which is a full IOCTL round-trip — so **each button press costs two `DeviceIoControl` calls**.

Enums (`VirtualXbox/Enums/`): buttons are literal XInput bitmasks (`Start 0x10 … Y 0x8000`, `Guide 0x0400`); axes `X=1, Y=2, Rx=4, Ry=8`; triggers `Left=0x10000, Right=0x20000`; dpad `Up=1,Down=2,Left=4,Right=8`.

### Native IOCTL layer
`VirtualXboxNative/virtualXbox.h` defines everything:
- Interface GUID `GUID_DEVINTERFACE_SCPVBUS = {F679F562-3164-42CE-A4DB-E7DDBE723909}`
- `FILE_DEVICE_BUSENUM = FILE_DEVICE_BUS_EXTENDER`, base index `0x801`, `METHOD_BUFFERED`
- IOCTLs: `PLUGIN_HARDWARE` (0x801), `UNPLUG_HARDWARE` (0x802), `EJECT_HARDWARE` (0x803), `REPORT_HARDWARE` (0x804, RW), plus SCP extensions `ISDEVPLUGGED` (0x901), `EMPTY_SLOTS` (0x902), `PROC_ID` (0x903)
- Globals **defined in the header** (`g_Gamepad[4]`, `g_vDevice[4]`, `g_hBus`) — works only because it's included once.

`VirtualXboxNative/virtualXbox.cpp`:
- `GetVXbusPath` (line 285) → `SetupDiGetClassDevs(GUID_DEVINTERFACE_SCPVBUS, DIGCF_PRESENT|DIGCF_DEVICEINTERFACE)` → `SetupDiGetDeviceInterfaceDetail`; `GetVXbusHandle` (line 328) `CreateFile`s it with RW share. `VBusExists()` is just "did the path lookup succeed".
- `PlugIn` (line 118): hand-rolled 16-byte buffer, `buffer[0]=0x10` (size), user index little-endian at offsets 4..8. **Bug: byte 3 of the index is written to `buffer[8]` instead of `buffer[7]`** — harmless only because indices are 1–4.
- `XOutputSetState` (line 377): 28-byte report, `buffer[0]=0x1C`, index at 4..7, `buffer[9]=0x14`, then `memcpy` of the 12-byte `XINPUT_GAMEPAD` at offset 10; output is a 9-byte feedback buffer.
- **Rumble and LED feedback do exist natively**: `XOutputSetGetState` (line 420) decodes the feedback buffer — `output[1]==0x08` → vibrate flag, `output[3]` = large motor, `output[4]` = small motor, `output[8]` = LED. `GetVibration` (line 178) scales motors by 256 into `XINPUT_VIBRATION`.
  **But `GetVibration` and `GetState` are never exported to C#** — `VirtualXbox/NativeMethods.cs` imports neither. So **the app has no rumble path at all**; a game's force feedback is silently dropped.
- `GetLedNumber` (line 170) returns `output[8] + 1`.
- Ownership: `IsControllerOwned` compares `GetCurrentProcessId()` against the PID returned by `IOCTL_BUSENUM_PROC_ID` — this is what stops two instances fighting over a slot.
- Slots are hard-limited to **user index 1–4** with explicit range guards in every function.

**Two real interop defects:**
1. `GetEmptyBusSlotsCount` is `BOOL(UCHAR* nSlots)` natively (writes **1 byte**) but declared in C# as `out int count` (`VirtualXbox/NativeMethods.cs:12`). Only the low byte is written; the upper 3 bytes are whatever the marshaller's stack slot held. `VirtualXboxBus.EmptySlotsCount` can therefore return garbage, and `XboxGamepad.PlugIn()` (`KeyboardSplitter/Models/XboxGamepad.cs:149`) gates on `== 0`.
2. `ControllerExists` / `GetCreateProcID` pass `_countof(buffer)` (= **1**) as `nInBufferSize` for a `ULONG[1]` (= 4 bytes) input buffer (`virtualXbox.cpp:77`, `:367`).

### LED / slot correlation
There is no direct mapping from ScpVBus user index → XInput slot. `XboxGamepad.PlugIn()` (`KeyboardSplitter/Models/XboxGamepad.cs:138`) does it heuristically: enumerate the 4 `XinputController` objects, pick the first that is `!IsConnected && Tag == null`, tag it, then `PlugIn(userIndex)` and wait for that XInput slot's `PluggedChanged` to fire — the LED number is then taken from the *XInput* index (`XinputController.LedNumber = PlayerIndex + 1`), not from `GetLedNumber`. This race is the origin of the `XinputSlotsFullException` / `Controller_Plug_In_Failed` paths in `SplitterCore/Emulation/SlotInvalidationReason.cs`.

---

## 3. XInputWrapper — what it wraps

`XInputWrapper/NativeMethods.cs:9`:
```csharp
private const string XinputPath = @"System32\xinput1_3.dll";
// "Use System32 path to prevent x360ce's xinput hook, because it does not
//  return proper XInputGetState for virtual controllers."
```
It targets **xinput1_3 specifically**, not 1_4 or 9_1_0, and uses a *relative* path so the loader resolves it out of `%windir%` rather than the app directory (deliberate anti-hook trick). It imports `XInputGetState/SetState/GetCapabilities/GetBatteryInformation/GetKeystroke` plus **ordinal `#103` = `XInputPowerOffController`** — an undocumented 1_3-only export.

Used for two things:
1. **The controller tester UI** — `KeyboardSplitter/Controls/XboxTestSlot.xaml.cs`, driven by `XinputController.PollerLoop()` (`XInputWrapper/XinputController.cs:316`) at 30 Hz, comparing `PacketNumber` to raise `StateChanged`.
2. **Slot/LED arbitration and disconnect detection** for `XboxGamepad` (above). `XinputController.EmptyBusSlotsCount` is what `EmulationManager` checks before mounting.

Verified on this machine: `System32\xinput1_3.dll` **is present** on Win11 26200 (v10.0.26200.1), as are 1_4 and 9_1_0 — so the README's DirectX 9.0c redist prerequisite is obsolete.

### Separate concern: the `xinput/` project
`xinput/xinput.cpp` + `xinput/xinput.def` build a **drop-in `xinput.dll` proxy you copy next to a game's exe**. `xinput/XInputModuleManager.h` `LoadLibrary`s the real DLL from `GetSystemDirectory()` and forwards every export (including undocumented ordinals 100/101/102/103/104/108). Its only added behavior is in `XInputGetCapabilities`: it reads `xinput.ini` and overrides `pCapabilities->SubType` per user index — so a game sees an arcade stick / wheel / drum kit instead of a gamepad. Deployed by `KeyboardSplitter/UI/XinputSubTypesWindow.xaml.cs:228`. This is an independent feature, unrelated to controller emulation.

---

## 4. Bundled binaries — inventory, versions, signatures

Everything is embedded into the single exe (`KeyboardSplitter/KeyboardSplitter.csproj:332-488`) and extracted to `%TEMP%` at runtime by `KeyboardSplitter/Helpers/ResourceExtractor.cs` (MD5-compare-then-overwrite), with `PATH` repointed to the temp dir in `App.xaml.cs:136`.

| Path | Ver / Company | Authenticode |
|---|---|---|
| `KeyboardSplitter/Lib/interception.dll` | 1.00, Francisco Lopes, "Interception API" | **NotSigned** (user-mode, OK) |
| `KeyboardSplitter/Lib/keyboard_driver.exe` | 1.00, Francisco Lopes, CLI installer | **NotSigned** |
| `InterceptionUninstall/InterceptionUninstall/interception.exe` | same | **byte-identical** to `keyboard_driver.exe` (sha256 `41362F9A…264FF`, 470 528 B) |
| `KeyboardSplitter/Lib/VirtualXboxNative.dll` | 2.2.0.0, djlastnight | NotSigned |
| `KeyboardSplitter/Lib/xinput.dll` | 2.2.0.0, djlastnight | NotSigned |
| `KeyboardSplitter/Lib/{VirtualXbox,Interceptor,SplitterCore,XinputWrapper}.dll` | 2.2.0.0 managed | NotSigned |
| `VirtualXbox/Driver/x64/ScpVBus.sys` | **1.7.1.2**, Nefarius Software Solutions | **Valid**, `CN=Shaul Eizikovich` (DigiCert HA Code Signing CA-1 → DigiCert HA EV Root), leaf **expired 2017-04-13**, Symantec-timestamped |
| `VirtualXbox/Driver/x64/scpvbus.cat` | — | same chain, same expiry |
| `VirtualXbox/Driver/x86/ScpVBus.sys` + `.cat` | 1.7.1.2 | **`CN=WDKTestCert Shaul,130867305994084724` — TEST SIGNED, status `UnknownError`, expired 2025-09-13.** Unusable without testsigning mode. |
| `VirtualXbox/Driver/{x64,x86}/ScpVBus.inf` | `DriverVer=04/19/2016,22.52.24.182`, KMDF 1.9, `Root\ScpVBus`, ClassGuid System | — |
| `VirtualXbox/Driver/{x64,x86}/WdfCoinstaller01009.dll` | 1.9.7600.16385 (Win7 RTM), Microsoft | Valid, MS HW Compatibility Publisher, expired 2023-06-01 |
| `VirtualXbox/Driver/{x64,x86}/devcon.exe` | 10.0.10240.16399, "Microsoft Corporation" | **NotSigned** — Microsoft's WDK devcon.exe *is* signed, so this copy has been stripped/repacked. Supply-chain smell, and WDK EULA does not permit redistributing devcon. |
| `Xbox360Accessories_x64_1.2.exe` (repo root, 7.9 MB) | 6.2.0029.0, Microsoft | Valid, cert expired 2010-01-22. Only needed on Vista/7/XP (`XboxGamepad.cs:44`) |
| `InterceptionUninstall/interception-gui-uninstaller.zip` | prebuilt GUI uninstaller | — |

**Note the version confusion:** `22.52.24.182` is the `DriverVer` string inside `ScpVBus.inf`, not a file version — the `.sys` reports `1.7.1.2`. The README's "must be 22.52.24.182" troubleshooting refers to what Device Manager shows.

**Licenses: there is no LICENSE, COPYING, or NOTICE file anywhere in the repo** (verified). The only attribution is the README credits block pointing at `oblitum/Interception`, `jasonpang/Interceptor`, `nefarius/ScpVBus`, `shauleiz/vXboxInterface`. Upstream license terms must be re-verified from those repos before any redistribution — in particular Interception is copyleft-licensed, and `devcon.exe` is not redistributable at all.

---

## 5. Install / uninstall flow

Everything is gated by `KeyboardSplitter/app.manifest:19` → `<requestedExecutionLevel level="requireAdministrator" />`. **The whole app always runs elevated**, so the driver installers inherit admin (the `Verb = "runas"` on the `ProcessStartInfo` objects is dead code — it's ignored when `UseShellExecute = false`).

Orchestrator: `KeyboardSplitter/Managers/DriversManager.cs`, called from `App.xaml.cs:177 CheckDrivers()` on every startup.

**Install** (`DriversManager.InstallBuiltInDrivers`, line 37):
1. Extract `keyboard_driver.exe` to `%TEMP%`, run `keyboard_driver.exe /install` (`InterceptionDriver.Install`, `Interceptor/InterceptionDriver.cs:63`), capture stdout. Strings inside the exe confirm it writes `system32\drivers\keyboard.sys` + `mouse.sys` and adds `UpperFilters` under class GUIDs `{4D36E96B-…}` (Keyboard) and `{4D36E96F-…}` (Mouse). Output: *"Interception successfully installed. You must reboot for it to take effect."*
2. Extract the 5 ScpVBus files to `%TEMP%\VirtualXbox <ver> resources\` and shell `devcon.exe install ScpVBus.inf Root\ScpVBus` (`VirtualXbox/VirtualXboxBus.cs:31`).
3. Re-read `InterceptionDriver.DriverState`; on `RebootRequired` prompt and run `shutdown.exe -r -t 0`; then `Environment.Exit(0)` in every branch.

**Uninstall** (`DriversManager.UninstallBuiltInDrivers`, line 132): `keyboard_driver.exe /uninstall`, then `VirtualXboxBus.Uninstall()` which force-unplugs pads 1–4 and runs `devcon.exe remove Root\ScpVBus`. Always tells the user to reboot.

**`InterceptionUninstall/` project** is a standalone 2-file WPF rescue tool (`MainWindow.xaml.cs`) for when the main app can't start. It re-implements the WMI+FileVersionInfo state check, validates that the local `interception.exe` really is Francisco Lopes' 1.00 build (company/description/version/product all checked, `:152-162`), runs `/uninstall`, and pattern-matches the stdout string to decide the message. Same reboot requirement.

**Reboot is required in both directions** because Interception is a class **upper filter driver** — the filter stack for the Keyboard/Mouse device classes is only rebuilt on device-stack restart. ScpVBus itself does not need a reboot (root-enumerated, devcon installs it live).

---

## 6. Windows 11 (2026) assessment — replace vs keep

Ground truth from this machine (Win11 Pro **build 26200**):

```
keyboard.sys  Oblita  1.00  Valid  CN=Francisco Lopes da Silva, C=BR   [service: keyboard, Running]
mouse.sys     Oblita  1.00  Valid  CN=Francisco Lopes da Silva, C=BR   [service: mouse, Running]
ScpVBus.sys   1.7.1.2  Valid  CN=Shaul Eizikovich (leaf expired 2017)  [Running]
ViGEmBus.sys  1.21.442.0  Valid  CN=Microsoft Windows Hardware Compatibility Publisher  [Running]
Class UpperFilters — Keyboard: keyboard,kbdclass   Mouse: mouse,mouclass
```

### 🔴 Must replace

**ScpVBus.** The x64 `.sys`/`.cat` are signed by a leaf that expired 2017-04-13 under a cross-signing program Microsoft retired; the x86 pair is **WDK-test-signed and worthless**. It currently loads here only because it was installed before the machine reached its current state and the timestamped signature is grandfathered — a fresh `devcon install` on a clean Secure Boot Win11 24H2/25H2 will be rejected. `WdfCoinstaller01009.dll` (KMDF 1.9 coinstaller) is deprecated and is rejected by universal/DCH INF validation. `devcon.exe` is unsigned-and-non-redistributable.
→ **Use `ViGEmBus`**, which is already installed and running on this machine, WHQL-signed by Microsoft's HW Compatibility Publisher. Rust binding: the `vigem-client` crate (pure-Rust, talks the ViGEm ioctls directly, no C DLL). It gives you Xbox360 pad emulation **plus a working rumble/LED notification channel** — recovering the `GetVibration`/`GetLedNumber` capability that `VirtualXboxNative.cpp` implements but never exposes.

**`VirtualXboxNative.dll` and `VirtualXbox.dll`** — delete entirely. The IOCTL constants in `VirtualXboxNative/virtualXbox.h` are only useful as documentation if you ever need to keep ScpVBus as a fallback.

**`XInputWrapper`'s slot-guessing.** The `!IsConnected && Tag == null` heuristic in `XboxGamepad.PlugIn()` is the root cause of the "Slot is invalidated" bug the README devotes a whole section to. ViGEm returns the user index/LED directly via a target notification callback — the heuristic goes away.

**`Thread.Abort()`-based shutdown** (`Interception.cs:247`) — no equivalent exists in any modern runtime; needs a real cancellation design (see below).

**The synchronous `Dispatcher.Invoke` per keystroke** (`InputManager.cs:278`). In Rust this must become: dedicated receive thread → lock-free channel → everything else. Never let UI work block the `interception_receive` loop.

### 🟡 Keep, but re-wrap

**Interception itself (`keyboard.sys` / `mouse.sys` / `interception.dll`).** Still valid, still signed, still running on Win11 26200. It remains the only practical way to get per-physical-keyboard input *with the ability to suppress it from the OS* — Raw Input can distinguish devices but cannot block; a low-level `WH_KEYBOARD_LL` hook can block but cannot distinguish devices.
→ Rust: the `interception` crate (safe wrapper over the same `interception.dll`), or hand-rolled `libloading`/`#[link]` bindings against the 13 exports listed in §1. Keep shipping `interception.dll` + the installer, but **stop bundling it as an embedded resource extracted to `%TEMP%`** — that pattern (`ResourceExtractor` + `PATH` rewrite + `LoadLibrary`) is exactly what modern AV/ASR rules flag.
→ Known alternatives if you want to drop the kernel filter: **HidHide** (Nefarius, WHQL-signed) to hide the I-PAC from other apps + Raw Input for reading. Worth evaluating for an arcade cabinet, since the I-PACs are dedicated devices you *always* want captured.

**`xinput/` proxy DLL.** Independent, still works, low risk — but it's a per-game file drop, not part of the core pipeline. Port last or not at all.

**`XInputWrapper` for the tester UI.** `xinput1_3.dll` is present on Win11 26200 so it still functions. For a rewrite, read back through ViGEm's notification callback (authoritative for *your* pads) and/or bind `xinput1_4.dll` for a generic tester. Ordinal `#103` and `XInputGetStateEx` (Guide button) exist only in 1_3/1_4 — 1_4 has both.

### 🟢 Already fine / no longer needed
- `Xbox360Accessories_x64_1.2.exe` — inbox since Windows 8; `XboxGamepad.AreXboxAccessoriesInstalled` already returns `true` unconditionally on anything newer (`XboxGamepad.cs:64`). Drop it.
- DirectX 9.0c redist / vcredist 2013 prerequisites from the README — obsolete.
- `Interceptor/Enums/InterceptionKey.cs` scancode tables and `SplitterCore/Preset/*` XML preset model — pure data, port straight across.

### Bugs to fix while porting (all confirmed by reading the source)
| Location | Defect |
|---|---|
| `Interceptor/Interception.cs:721` | `id < MaxDeviceCount` — mouse #10 (id 20) never enumerated |
| `VirtualXbox/NativeMethods.cs:12` | `out int` vs native `UCHAR*` — 3 bytes uninitialized; corrupts `EmptySlotsCount` |
| `VirtualXboxNative/virtualXbox.cpp:77`, `:367` | `nInBufferSize = _countof(ULONG[1]) = 1`, should be 4 |
| `VirtualXboxNative/virtualXbox.cpp:139` | index byte 3 written to `buffer[8]`, should be `buffer[7]` |
| `VirtualXboxNative/virtualXbox.cpp` (exports) | `GetVibration`/`GetState` implemented natively but never P/Invoked — **rumble is silently dead** |
| `Interceptor/Interception.cs:306` | `this.deviceId` shared between receive thread and `SendKey` — data race |
| `Interceptor/InterceptionDevice.cs:58` | `FriendlyName` derives a registry path by string-mangling the HWID; fails to `"n/a"` silently |
| `Interceptor/InterceptionDriver.cs:21` | only validates `keyboard.sys`, never `mouse.sys`, yet the installer installs both |
| `KeyboardSplitter/Managers/InputManager.cs:278` | synchronous `Dispatcher.Invoke` inside the interception receive path — system-wide input freeze risk |
| `VirtualXbox/VirtualXboxController.cs` (all setters) | `IsControllerOwned()` IOCTL before every single `SetButton`/`SetAxis` — doubles syscalls on the hot path |