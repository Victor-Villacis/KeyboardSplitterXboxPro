# KeyboardSplitterXbox — Architecture & Data Flow Map

Repo root: `C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro`
(paths below are relative to that root)

---

## 1. Projects in `KeyboardSplitterXbox.sln`

The solution has **8** projects (the task list omitted `xinput`).

| Project | File | Type / TFM | Platform | Purpose |
|---|---|---|---|---|
| **KeyboardSplitter** | `KeyboardSplitter/KeyboardSplitter.csproj` | WPF `Exe`, .NET Framework **v4.0** | **x86 only** (Debug\|x86, Release\|x86) | The whole app: WPF UI, concrete `InputManager`/`EmulationManager`/`Splitter`/`XboxGamepad`, preset & game persistence, driver install orchestration, self-extracting single-EXE loader. |
| **SplitterCore** | `SplitterCore/SplitterCore.csproj` | Library, v4.0, AnyCPU | — | Abstraction layer: `ISplitter`, `IInputManager`, `IEmulationManager`, `IEmulationSlot`, `IVirtualGamepad`, `IPreset`, `InputDevice`/`Keyboard`/`Mouse`, `InputKey` enum, `FunctionType`. **Not UI-free** — it references `PresentationFramework`/`WindowsBase` and `EmulationSlotBase : UserControl`. |
| **Interceptor** | `Interceptor/Interceptor.csproj` | Library, v4.0, PlatformTarget **x86** | — | P/Invoke wrapper around oblitum `interception.dll` + device enumeration + driver install/state detection. |
| **VirtualXbox** | `VirtualXbox/VirtualXbox.csproj` | Library, v4.0, PlatformTarget **x86** | — | P/Invoke wrapper around `VirtualXboxNative.dll`; ScpVBus driver install/uninstall via embedded `devcon.exe`; caches controller state in managed memory. |
| **VirtualXboxNative** | `VirtualXboxNative/VirtualXboxNative.vcxproj` | Native DLL, PlatformToolset **v120** (VS2013), Win32 + x64 configs | Output → `KeyboardSplitter\Lib\` | C++ port of shauleiz **vXboxInterface**: opens the ScpVBus device interface and issues `DeviceIoControl` IOCTLs. |
| **XinputWrapper** | `XInputWrapper/XinputWrapper.csproj` | Library, v4.0, AnyCPU/x86 | — | Managed XInput client (`XInputGetState` etc.) used to *observe* the virtual pads: detect plug/unplug, read LED/player index, controller test window. |
| **xinput** | `xinput/xinput.vcxproj` | Native DLL, v120 | Output → `KeyboardSplitter\Lib\` | **XInput proxy/shim DLL**, dropped next to a game's EXE to override `XINPUT_CAPABILITIES.SubType` from an `xinput.ini` (makes the virtual pad report as wheel/arcade stick/etc.). Not part of the input pipeline. |
| **InterceptionUninstaller** | `InterceptionUninstall/InterceptionUninstall/InterceptionUninstaller.csproj` | WPF `Exe` (`Uninstall.exe`), v4.0, AnyCPU/x86 | standalone | Tiny recovery tool: ships `interception.exe` as Content, runs `/uninstall`, checks driver state via WMI. Completely independent of the rest. |

### Reference / dependency graph

```
KeyboardSplitter.exe (WPF, x86)
 ├─ProjectRef→ SplitterCore      (interfaces + WPF base classes)
 ├─ProjectRef→ Interceptor  ──P/Invoke→ interception.dll ──→ oblitum keyboard.sys/mouse.sys filter drivers
 ├─ProjectRef→ VirtualXbox  ──P/Invoke→ VirtualXboxNative.dll ──DeviceIoControl→ ScpVBus.sys ──→ 4x virtual XInput pads
 └─ProjectRef→ XinputWrapper──P/Invoke→ xinput9_1_0/xinput1_3 (system) ──reads back→ the virtual pads
    (xinput.dll proxy is only *extracted next to a game*, never loaded by the app)
```

All `<Private>False</Private>` — the referenced DLLs are **not copied** next to the EXE. Instead every library builds into `KeyboardSplitter\Lib\` and is then re-embedded into the EXE as `EmbeddedResource` (`KeyboardSplitter.csproj` lines ~455-470): `Lib\Interceptor.dll`, `Lib\SplitterCore.dll`, `Lib\VirtualXbox.dll`, `Lib\XinputWrapper.dll`, `Lib\VirtualXboxNative.dll`, `Lib\interception.dll`, `Lib\xinput.dll`, `Lib\keyboard_driver.exe`. Result: one portable EXE.

---

## 2. The input pipeline: keystroke → virtual pad

### 2.1 Kernel → Interceptor

- **`Interceptor/NativeMethods.cs:9-49`** — raw P/Invoke surface to `interception.dll`: `interception_create_context`, `_set_filter`, `_wait`, `_receive`, `_send`, `_get_hardware_id`, `_is_keyboard`, `_is_mouse`, `_is_invalid`.
- **`Interceptor/Interception.cs:211` `Load(bool startThreads)`** — creates the context, spawns two threads (see §3).
- **`Interceptor/Interception.cs:587` `DriverCallback()`** — the heart. Sets filters (`KeyboardFilterMode.All`, mouse filter), then a blocking loop:
  ```
  while (Receive(ctx, deviceId = Wait(ctx), ref stroke, 1) > 0)
  ```
  Per stroke it: (a) resolves `deviceId` (1..10 = keyboards, 11..20 = mice) to an `InterceptionDevice`; (b) converts the scancode via `GetKeyboardKeys` → `KeysHelper.GetCorrectedKey` (`Interceptor/KeysHelper.cs:61`, handles E0/E1 prefixed extended keys, numpad vs arrows, Pause); (c) writes into the per-device key-state table `keyStates[device.StrongName][key]`; (d) raises `InputActivity`; (e) **if the handler did not set `args.Handled`, re-sends the stroke to the OS via `interception_send`** — this is the block/pass-through decision (`Interception.cs:649-659`).
- Devices are identified by a **StrongName** (`Interceptor/InterceptionDevice.cs:34`) of the form `Keyboard_01`, `Mouse_03` — derived from the Interception device slot index, *not* from the hardware ID. `FriendlyName` is looked up in `HKLM\System\CurrentControlSet\Enum\<hwid>`.

### 2.2 Interceptor → SplitterCore layer

- **`KeyboardSplitter/Managers/InputManager.cs:58`** constructs `new Interception(KeyboardFilterMode.All, mouseFilter)` and `Load()`s it.
- **`InputManager.cs:242` `OnInterceptionInputActivity`** — the bridge. Converts `InterceptionDevice`→`InputDevice` and `InterceptionKey`→`InputKey` (`KeyboardSplitter/Helpers/InputHelper.cs`), builds `InputEventArgs`, re-raises as `IInputManager.InputActivity`, and copies `args.Handled` back into the Interception event args. It also appends to `InputMonitorHistory` (the UI log) and runs `CheckForEmergencyHit`.
- The two enums are asserted to be **binary-identical** at startup (`InputManager.cs:170 CheckKeysEnumerations`) — `InterceptionKey` and `SplitterCore.Input.InputKey` must have identical names, values and `ushort` underlying type. In Rust this whole conversion layer collapses to one enum.

### 2.3 SplitterCore → mapping → VirtualXbox

- **`KeyboardSplitter/Models/Splitter.cs:396` `InputManager_InputActivity`** — the dispatcher:
  1. bail if emulation not started;
  2. if the device is in `AssignedInputDevices`, set `e.Handled = ShouldBlockKeyboards / ShouldBlockMice` → **this is what swallows the keystroke from Windows**;
  3. for each slot whose `Keyboard`/`Mouse` matches the device, call `TranslateInput` four times — once per `FunctionType`: Button, Trigger, Axis, Dpad.
- **`Splitter.cs:68` `TranslateInput(InputEventArgs, IEmulationSlot, FunctionType)`** — resolves mappings then applies:
  - `Splitter.cs:217 GetMappings` → `slot.Preset.FilterByKey(inputKey)` (`KeyboardSplitter/Presets/Preset.cs:56`) produces the subset of preset entries bound to that key, including "custom functions" expanded via `Helpers/CustomFunctionHelper.cs`.
  - `Splitter.cs:325 AreAllKeysUp` — on key-up, the function is only released if *every* key mapped to it is up (multi-key-to-one-function support).
  - `Splitter.cs:124 SetButton` / `:139 SetTrigger` (0 ↔ 255) / `:154 SetAxis` / `:182 SetDpad` (bitwise OR/AND-NOT of direction flags). Each reads the cached current state first and **returns early if unchanged** — a de-dupe that avoids redundant IOCTLs.
  - `Splitter.cs:196 HasOppositeAxisKeysDown` — the opposite-direction handling: releasing "left" while "right" is still held snaps the axis to the opposite extreme instead of center.
- **`KeyboardSplitter/Models/XboxGamepad.cs`** implements `IVirtualGamepad` and forwards to the static **`VirtualXbox/VirtualXboxController.cs`** (`SetButton:112`, `SetTrigger:143`, `SetDPad:171`, `SetAxis:66`), which (a) checks `IsOwned` (process affinity), (b) calls the native export, (c) mirrors the value into `VirtualXbox/ControllerState.cs` so `GetXxxValue` is a cheap managed read rather than a driver round-trip.
- **`VirtualXbox/NativeMethods.cs`** → `VirtualXboxNative.dll` cdecl exports → **`VirtualXboxNative/virtualXbox.cpp`** → `DeviceIoControl(g_hBus, IOCTL_BUSENUM_*)` on the ScpVBus device interface GUID `{F679F562-3164-42CE-A4DB-E7DDBE723909}` (`VirtualXboxNative/virtualXbox.h:25`, IOCTL codes at `:54-70`). `IsControllerOwned` compares the creating PID with `GetCurrentProcessId()` — ownership is per-process.

### 2.4 Full one-line trace

```
keyboard.sys (oblitum filter)
  → interception_wait/_receive
  → Interception.DriverCallback              Interceptor/Interception.cs:587
  → InterceptionEventArgs (InputActivity)
  → InputManager.OnInterceptionInputActivity KeyboardSplitter/Managers/InputManager.cs:242   [Dispatcher.Invoke → UI thread]
  → IInputManager.InputActivity
  → Splitter.InputManager_InputActivity      KeyboardSplitter/Models/Splitter.cs:396
  → Splitter.TranslateInput (x4 FunctionTypes) Splitter.cs:68
  → Preset.FilterByKey / GetKeys              KeyboardSplitter/Presets/Preset.cs:56
  → XboxGamepad.SetButtonState/...            KeyboardSplitter/Models/XboxGamepad.cs:212
  → VirtualXboxController.SetButton           VirtualXbox/VirtualXboxController.cs:112
  → NativeMethods.SetButton (cdecl)
  → VirtualXboxNative.dll → DeviceIoControl(IOCTL_BUSENUM_REPORT_HARDWARE)
  → ScpVBus.sys → virtual XUSB device → XInput
(and back in DriverCallback: if args.Handled → stroke is NOT re-sent → key never reaches Windows)
```

---

## 3. Threading / event model

**Three threads plus the WPF Dispatcher:**

1. **Interception callback thread** — `Interceptor/Interception.cs:223-226`. `ThreadPriority.Highest`, `IsBackground = true`. Runs the blocking `Wait`/`Receive` loop. **Pure blocking-callback, no polling.** This is the latency-critical thread.
2. **Device-connection thread** — `Interception.cs:228-231`, `ConnectionCallback()` at `:686`. Polls `RescanInputDevices()` every **1000 ms** to detect USB plug/unplug (it brute-force calls `interception_get_hardware_id` for device ids 1..20).
3. **XInput poller** — `XInputWrapper/XinputController.cs:196 StartPolling` → `PollerLoop:~313`. Started in `MainWindow.Window_Loaded` (`MainWindow.xaml.cs:235`), stopped in `Window_Closed`. `XInputGetState` on all 4 slots at **~30 Hz** (`DefaultUpdateFrequency = 1000/30`). Purely observational: fires `PluggedChanged` (used by `XboxGamepad.OnXinputControllerPluggedChanged` to learn the LED/player number after plug-in) and `StateChanged` (controller test window).
4. **WPF Dispatcher (UI thread)** — everything from step 2 onward in the pipeline runs here.

### The latency hot spot (critical for the rewrite)

`InputManager.OnInterceptionInputActivity` (`InputManager.cs:270-284`) marshals **every keystroke** onto the UI thread:

```csharp
if (isMouseClick && GlobalSettings.IsMainWindowActivated)
    this.Dispatcher.BeginInvoke(action);   // async, only for mouse clicks on own window
else
    this.Dispatcher.Invoke(action);        // SYNCHRONOUS — blocks the interception thread
```

So the mapping work, the `InputMonitorHistory` string-builder append, the preset LINQ queries and the driver IOCTL all happen on the UI thread while the kernel filter thread waits. Consequences visible in the code:
- `App.xaml.cs:33-57` warns that an attached debugger / blocked UI thread will **freeze every keyboard and mouse on the machine until reboot**.
- `MainWindow` auto-collapses the input monitor after 60 s "to save CPU time" (`MainWindow.xaml.cs:73, 232-233`).
- `GlobalSettings.IsMainWindowActivated` forces `e.Handled = false` so you can still use your own UI (`InputManager.cs:281-284`).

Other timing details: `Interceptor/DelayedTask.cs` (a one-shot `System.Timers.Timer`) auto-releases mouse-wheel and mouse-move "keys" after 50 ms (`Interception.cs:60-61, 668`). `EmulationManager.Start` enforces a **5-second cooldown** between start/stop (`Managers/EmulationManager.cs:85`). Shutdown uses `Thread.Abort()` on both interception threads (`Interception.cs:254-262`) — not portable to Rust; needs a proper cancellation/`WaitWithTimeout` design.

**Emergency escapes** (all in `InputManager.cs:304 CheckForEmergencyHit`): Ctrl+Alt+Del → `EmergencyStop` → stop emulation; LeftCtrl x5 → toggle `ShouldBlockKeyboards`; RightCtrl x5 → toggle `ShouldBlockMice`. Handled in `Splitter.cs:330-394` with sound feedback.

---

## 4. Driver embedding, extraction and installation

### 4.1 Startup extraction (single-EXE trick)

`App` static ctor (`KeyboardSplitter/App.xaml.cs:31-61`) → `LoadAssemblies()` (`:130`):

1. Sets the **process `PATH` env var** to `ApplicationInfo.AppTempDirectory` = `%TEMP%\djlastnight's Gaming Keyboard Splitter vX.Y.Z` (`ApplicationInfo.cs:25-31`).
2. Extracts the two **native** DLLs there: `KeyboardSplitter.Lib.interception.dll` and `KeyboardSplitter.Lib.VirtualXboxNative.dll` via `Helpers/ResourceExtractor.cs:15` (writes only if missing or **MD5 differs**).
3. Loads the four **managed** DLLs straight from memory: `ManagedAssemblyLoader.Load()` (`AssemblyLoaders/ManagedAssemblyLoader.cs:13`) does `Assembly.Load(byte[])` and caches by `FullName`; `App.CurrentDomain_AssemblyResolve` (`App.xaml.cs:301`) serves them on demand.

Note: despite the README claiming `LoadLibrary` via P/Invoke, the actual mechanism is **PATH manipulation + lazy P/Invoke resolution** — there is no `LoadLibrary` import anywhere (`Helpers/NativeMethods.cs` only has advapi32/dwmapi/gdi32).

### 4.2 Driver installation (first run)

`Application_Startup` (`App.xaml.cs:219`) → `ReportDriversState()` → `CheckDrivers()` (`:177`) → prompt → `DriversManager.InstallBuiltInDrivers()` (`Managers/DriversManager.cs:37`):

- **Interception**: extracts embedded `KeyboardSplitter.Lib.keyboard_driver.exe` (oblitum's CLI installer, 470 KB) to temp and runs it with `/install`, `Verb = "runas"`, stdout captured (`Interceptor/InterceptionDriver.cs:63`). State is detected by a **WMI query on `Win32_SystemDriver` where Name='keyboard'** *plus* a version-info check on `%system32%\drivers\keyboard.sys` (CompanyName `Oblita`, ProductVersion `1.00`, FileDescription `Keyboard Upper Filter Driver`) → `Installed` / `RebootRequired` / `NotInstalled` (`InterceptionDriver.cs:21-60`).
- **ScpVBus**: `VirtualXbox/VirtualXboxBus.cs:31 Install()` extracts the arch-appropriate payload and shells `devcon.exe install ScpVBus.inf Root\ScpVBus`; uninstall is `devcon remove Root\ScpVBus` after force-unplugging pads 1-4 (`:62`).
- Always ends in `Environment.Exit(0)` — install requires an app restart, usually a reboot.
- **Xbox 360 Accessories driver** is *not* installed by the app; `Models/XboxGamepad.cs:44` only checks for `%SystemDrive%\Program Files\Microsoft Xbox 360 Accessories\XBoxStat.exe` on XP/Vista/7 and returns `true` unconditionally on Win8+. The repo ships `Xbox360Accessories_x64_1.2.exe` at root for manual install.

### 4.3 x86 / x64 handling

- The managed app is **hard-locked to x86** (every `KeyboardSplitter` solution configuration maps to `x86`), so `interception.dll` and `VirtualXboxNative.dll` are always the 32-bit builds — a 32-bit process talking to a 64-bit kernel driver via IOCTL, which is fine.
- Only the **ScpVBus driver payload** is arch-split: `VirtualXboxBus.LoadDriverResourcesAndGetDevconPath()` (`VirtualXboxBus.cs:97`) picks `VirtualXbox.Driver.x64.*` or `.x86.*` from `Environment.Is64BitOperatingSystem` and extracts `scpvbus.cat`, `ScpVBus.inf`, `ScpVBus.sys`, `WdfCoinstaller01009.dll`, `devcon.exe`.
- `InterceptionDriver.GetSystem32DirectoryPath()` (`:157`) handles WOW64 by redirecting to `%windir%\sysnative` for the driver-file check.
- Manifest `KeyboardSplitter/app.manifest` requests **`requireAdministrator`**; supportedOS list stops at Windows 10.

---

## 5. UI technology, coupling, startup sequence

- **WPF**, not WinForms. `ProjectTypeGuids` includes the WPF guid; `App.xaml` is the `ApplicationDefinition`; `StartupObject = KeyboardSplitter.App`. `System.Windows.Forms` is referenced but only for `Application.StartupPath`, `Application.ExecutablePath` and `Screen`.
- Custom chrome: `UI/CustomWindow.cs` + `Controls/CustomTitlebar.xaml` + `Helpers/AeroHelper.cs` (DWM blur, `RegistryMonitor` watching `HKCU\Software\Microsoft\Windows\DWM`).
- Windows/controls: `MainWindow.xaml`, `Controls/EmulationSlot.xaml`, `Controls/PresetControl.xaml`, `Controls/OnScreenController.xaml`, `Controls/GameList.xaml`, `Controls/LedIndicator.xaml`, `Controls/XboxTestSlot.xaml`, `UI/{Settings,GameEditor,GameItemEditor,InputDetector,XinputControllerTest,XinputSubTypes,Faq,HowItWorks,About,MessageBox,HelpDialog}Window.xaml`.

### The coupling problem (biggest thing to fix in the rewrite)

The "core" is **not** separable from WPF:

- `SplitterCore/SplitterBase.cs:9` — `SplitterBase : DependencyObject`, all state as `DependencyProperty`.
- `SplitterCore/Emulation/EmulationSlotBase.cs:10` — **`EmulationSlotBase : UserControl`**. `EmulationManager` even *enforces* this: `Managers/EmulationManager.cs:47` throws `EmulationSlotTypeException` if a slot is not a `UserControl`. So an emulation slot **is** a UI control; the domain model and the view are the same object.
- `Managers/InputManager.cs:13` and `Managers/EmulationManager.cs:18` are both `DependencyObject`s and call `this.Dispatcher.Invoke` directly.
- `Models/XboxGamepad.cs:16` is a `DependencyObject` + `INotifyPropertyChanged`.
- Global lookup by walking the visual tree: `Helpers/SplitterHelper.cs:7 TryFindSplitter()` returns `((MainWindow)App.Current.MainWindow).Splitter` — used from `PresetDataManager`, `EmulationManager`, `App`.

### Startup sequence

1. `App` **static ctor** (`App.xaml.cs:31`): debugger warning → `StartLogging()` → `CheckForObsoleteOS()` → `LoadAssemblies()` (extract natives, load managed from memory).
2. `Application_Startup` (`:219`): parse args (`allow-multi-instance`, `game=<Title>`), single-instance `Mutex "KB_XBOX_SPLITTER_SINGLE_INSTANCE_MUTEX"`, `FreeConsole()` unless autostarting a game, hook `AssemblyResolve`, `ReportDriversState()`, `CheckDrivers()`.
3. `MainWindow` ctor → `MainWindow.Window_Loaded` (`:228`): `GlobalSettings.TryApplySettings()` → `XinputController.StartPolling()` → enumerate devices via `InputManager.ConnectedInputDevices` (exits if zero) → compute default `SlotsCount = min(max(#keyboards, #mice), 4)`.
4. Setting `SlotsCount` fires `OnSlotsCountChanged` (`:344`) which lazily constructs **`new Splitter(SlotsCount)`** — and *that* ctor (`Models/Splitter.cs:22`) is what creates the `InputManager` (loading the Interception driver), the per-slot `XboxGamepad`s and `EmulationManager`.
5. Optional CLI autostart: `game.TryStart()` (`MainWindow.xaml.cs:263`).
6. Emulation only begins when the user hits Start (`StartEmulationCommand` → `EmulationManager.Start()`), which plugs the virtual pads in.
7. Shutdown: `Window_Closing` (unsaved-preset prompt) → `Window_Closed` → `Dispose()` (`Splitter.Destroy()`, save games) → `StopPolling()` → `GlobalSettings.TrySaveToFile()`. Also `Application_SessionEnding` (`App.xaml.cs:269`) saves on Windows logoff/shutdown.

---

## 6. Config persistence

All four files are written **relative to the current working directory** (`splitter_settings.xml`, `splitter_presets.xml`, `splitter_games.xml` use bare filenames), except the log which uses `Application.StartupPath`. All are `XmlSerializer` + **UTF-16 (Unicode)** encoded. None are in `%APPDATA%` — this is a portable-app layout that breaks if CWD ≠ EXE dir.

| File | Const | Root type | Written by | Read by |
|---|---|---|---|---|
| `splitter_settings.xml` | `GlobalSettings.SettingsFileName` — `KeyboardSplitter/GlobalSettings.cs:43` | `[XmlType("SplitterSettings")]` | `GlobalSettings.TrySaveToFile():103` — on `Window_Closed` and `Application_SessionEnding` | `TryApplySettings():125` in `Window_Loaded` |
| `splitter_presets.xml` | `PresetDataManager.PresetsFilename` — `KeyboardSplitter/Presets/PresetDataManager.cs:12` | `[XmlType("preset_data")]` → `<preset name="...">` with `<button>/<trigger>/<axis>/<dpad>/<custom>` children | `WritePresetDataToFile():96` — from `Window_Closing` prompt, or session-ending | static ctor `:18` → `ReadPresetDataFromFile():31` |
| `splitter_presets_backup.xml` | `PresetsBackupFilename` — `PresetDataManager.cs:14` | same | auto-created on a parse failure (`:56-69`) | — |
| `splitter_games.xml` | `GameDataManager.GameDataFilename` — `KeyboardSplitter/Managers/GameDataManager.cs:10` | `GameData` → `Game` (`Models/Game.cs`, `[XmlAttribute("Title")]`, path, args, notes, `BlockKeyboards`, `BlockMice`, `ObservableCollection<SlotData>`) | `WriteGameDataToFile():48` — from `MainWindow.Dispose()` | static ctor `:14` |
| `splitter_log.txt` | `LogWriter.cs:11-13` — `Application.StartupPath` | plain text, UTF-16 | truncated on `Init()`, appended per `Write()` (opens/closes the file on **every** line) | — |
| `xinput.ini` | `KeyboardSplitter/UI/XinputSubTypesWindow.xaml.cs:64,173` | INI, `[SubTypes] 1=..4=` | written **next to the target game EXE**, together with an extracted copy of the `xinput.dll` proxy (`:228`) | read by `xinput/xinput.cpp` inside the game process |

Settings content is minimal (`GlobalSettings.cs:15-98`): `MouseMoveDeadZone` (0-12, pushed into `Interception.MouseMoveDeadZone`), `DisplayEmulationInformation`, `SuggestInputDevicesForNewSlots`, `StartingVirtualControllerUserIndex` (1-4). Presets: two hardcoded immutable presets (`Preset.ImuttablePresets`, `Presets/Preset.cs:19-22`) are stripped before serialize and re-inserted after deserialize (`Presets/PresetData.cs:37-49, 56-68`). `Presets/PresetUpgrader.cs` migrates older schemas.

---

## Notes that matter most for a Rust rewrite

1. **The domain model is trapped in WPF.** `EmulationSlotBase : UserControl`, `SplitterBase : DependencyObject`, `Dispatcher.Invoke` in the hot path, `SplitterHelper.TryFindSplitter()` reaching through `App.Current.MainWindow`. A Rust core with a message channel to the UI removes all of this and, more importantly, removes the synchronous UI-thread hop from the per-keystroke path (`InputManager.cs:278`).
2. **Device identity is positional, not stable.** `Keyboard_01`..`Keyboard_10` come from the Interception device slot index (`InterceptionDevice.cs:51`); hardware IDs are only used for the friendly name. For an arcade cabinet with fixed I-PACs you want stable identity keyed on hardware ID / container ID instead.
3. **Two parallel key enums kept in lockstep by a runtime assertion** (`InputManager.cs:170`) — collapse to one.
4. **State mirroring is already the right idea**: `VirtualXbox/ControllerState.cs` + the early-return checks in `Splitter.SetButton/SetAxis/...` mean only genuine transitions hit the driver. Keep that; in Rust it becomes a per-pad `XUSB_REPORT` diffed before submit.
5. **Driver-layer replacements to research**: ScpVBus + vXboxInterface → **ViGEmBus** + `vigem-client` (Rust crate) — a signed, maintained driver with x360/DS4 emulation, and the LED/player-index feedback that `XinputWrapper`'s 30 Hz poller exists to synthesize is delivered by callback instead, which deletes the whole `XInputWrapper` project. For per-keyboard capture, oblitum Interception is still the only mature per-device keyboard filter; the alternative is a Raw Input (`WM_INPUT`, `RIDEV_INPUTSINK` + `RIDEV_NOLEGACY`) backend that gives per-device identity without a kernel driver but blocks input globally rather than per-device — worth prototyping since `ShouldBlockKeyboards` is already an all-or-nothing toggle for assigned devices.
6. **Toolchain rot**: VS2013 (`PlatformToolset v120`), .NET Framework 4.0, WMI for driver state, `devcon.exe` shelling, `Thread.Abort()` on shutdown, x86-only. Nothing here should survive the port.
7. **Config layout**: CWD-relative XML files with no `%APPDATA%` fallback; the log is reopened per line. Move to a versioned TOML/JSON under a known dir.