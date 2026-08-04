I have enough to produce the inventory.

# KeyboardSplitterXbox — Feature & Domain Inventory (rewrite requirements)

Repo root: `C:/Users/Victor/AppData/Local/Temp/claude/C--Users-Victor/d8a02a70-5f2a-41d9-8b64-ee16d664ad9d/scratchpad/KeyboardSplitterXboxPro`
All paths below are relative to that root.

---

## 1. Mapping / Preset model

### 1.1 File & lifecycle
- File name: `splitter_presets.xml`, in the app's working directory. Const at `KeyboardSplitter/Presets/PresetDataManager.cs:12`.
- Read once at static-ctor time (startup), written on exit / session-end. Encoding is **UTF-16 (`Encoding.Unicode`)** — `KeyboardSplitter/Presets/PresetData.cs:70`.
- Parse failure behavior (`PresetDataManager.cs:31-94`): copies the file to `splitter_presets_backup.xml` (only if a backup doesn't already exist), logs to `splitter_log.txt`, shows an error, and **sets every emulation slot's `InvalidateReason = Presets_Parse_Failed`** so emulation can't start.
- Strict parsing: `serializer.UnknownNode += throw` (`PresetData.cs:31`) — any unknown element/attribute aborts the whole load. A rewrite should decide whether to keep that strictness (recommend: lenient + warn).
- Import/Export presets from arbitrary XML files: `KeyboardSplitter/MainWindow.xaml.cs:617-768`. Import falls back to a **v1 legacy schema upgrader** (`KeyboardSplitter/Presets/PresetUpgrader.cs`) which also rewrites `LeftTrigger`→`Left`, `RightTrigger`→`Right`, and maps old `<pov>`→dpad.
- Dirty-tracking: `PresetDataManager.IsPresetChanged` compares serialized XML of in-memory vs on-disk preset; on close the app prompts "save unsaved presets?" and also detects **deleted** presets (`MainWindow.xaml.cs:275-337`).

### 1.2 Exact schema
Root `<preset_data>` → repeated `<preset name="...">` (`Preset.cs:15-49`, element classes in `SplitterCore/Preset/*.cs`):

| Element | Attributes | Text content | Source |
|---|---|---|---|
| `<button id="uint">Key</button>` | `id` = XboxButton bitflag | `InputKey` enum name | `SplitterCore/Preset/PresetButton.cs` |
| `<trigger id="uint">Key</trigger>` | `id` = XboxTrigger | `InputKey` | `SplitterCore/Preset/PresetTrigger.cs` |
| `<axis id="uint" value="short">Key</axis>` | `id` = XboxAxis, `value` = **signed 16-bit position** | `InputKey` | `SplitterCore/Preset/PresetAxis.cs` |
| `<dpad direction="int">Key</dpad>` | `direction` = XboxDpadDirection | `InputKey` | `SplitterCore/Preset/PresetDpad.cs` |
| `<custom function="uint">Key</custom>` | `function` = XboxCustomFunction | `InputKey` | `SplitterCore/Preset/PresetCustom.cs` |

Element order in the serialized preset is Buttons, Triggers, Axes, Dpads, CustomFunctions.

### 1.3 ID value tables (must be preserved bit-for-bit for file compatibility)
`VirtualXbox/Enums/*.cs`:
- **XboxButton**: Start `0x0010`, Back `0x0020`, LeftThumb `0x0040`, RightThumb `0x0080`, LeftBumper `0x0100`, RightBumper `0x0200`, Guide `0x0400`, A `0x1000`, B `0x2000`, X `0x4000`, Y `0x8000`.
- **XboxTrigger**: Left `0x10000`, Right `0x20000`.
- **XboxAxis**: X `1`, Y `2`, Rx `4`, Ry `8`.
- **XboxAxisPosition**: Min `-32768`, Center `0`, Max `32767`.
- **XboxDpadDirection** (flags): Off `0`, Up `1`, Down `2`, Left `4`, Right `8`.
- **XboxCustomFunction**: a single flat enum spanning all of the above — Dpad_Up `0x1` … Button_Y `0x8000`, Left_Trigger `0x10000`, Right_Trigger `0x20000`, Axis_X_Min `0x100000`, Axis_X_Max `0x200000`, Axis_Y_Min `0x400000`, Axis_Y_Max `0x800000`, Axis_Rx_Min `0x1000000`, Axis_Rx_Max `0x2000000`, Axis_Ry_Min `0x4000000`, Axis_Ry_Max `0x8000000`.

### 1.4 Custom axis ranges (`<axis id="1" value="-16384">None</axis>`)
- There is **no dedicated code path** — it falls straight out of `PresetAxis.Value` being a raw `short`. Any `(id, value)` pair not in the standard Min/Max set is simply an extra row in the preset (`README.md:104-123`).
- Only creatable by hand-editing the XML; the UI has no "add axis at N%" affordance, but once present it renders as a normal row. `KeyboardSplitter/Converters/XboxAxisConverter.cs` casts `short → XboxAxisPosition` for display, so a non-Min/Center/Max value displays as the bare number.
- Runtime semantics (`KeyboardSplitter/Models/Splitter.cs:154-215`):
  - Key down → `SetAxisState(axis, presetAxis.Value)`.
  - Key up → Center (0), **unless** the opposite-extreme key is currently held, in which case it snaps to the opposite of `Min`/`Max` (hardcoded to Min/Max, *not* to the custom value — see §8.5).
  - `Preset.GetKeys(PresetAxis)` matches on `Axis == axis && Value == value`, so a custom-valued axis is a distinct binding from the Min/Max ones on the same axis.

### 1.5 Hardcoded presets
- `Preset.ImuttablePresets` (`Presets/Preset.cs:21-22`) — protected presets are stripped on load and skipped on save (`PresetData.cs:37-47`, `54-68`), then re-inserted at the head of the list. They cannot be saved, deleted, or have custom functions added (UI gates in `Controls/PresetControl.xaml.cs:328-338`, FAQ Q at `UI/FaqWindow.xaml.cs:110-117`).
- **`default`** (`Preset.cs:274-317`): Start=Escape, Back=Backspace, LeftThumb=LeftShift, RightThumb=RightShift, LB=Z, RB=C, Guide=LeftWindows, A=S, B=D, X=A, Y=W; LTrigger=Q, RTrigger=E; Axis X Min=Left / Max=Right, Y Min=Down / Max=Up, Rx Min=Numpad4 / Max=Numpad6, Ry Min=Numpad2 / Max=Numpad8; Dpad Up=I, Down=K, Left=J, Right=L; **plus one custom function `Button_A → Enter`** (this is the "Bind button A to Enter" feature from the UI help, pt. 9).
- **`empty`**: documented in README (`README.md:91`), FAQ and help text ("all slots that used a deleted preset load the empty preset"), and `CreateEmptyPreset()` exists at `Preset.cs:265-272` — **but it is dead code in this fork.** `ImuttablePresets` contains only `Default`. A rewrite should decide: restore `empty` as a real protected preset, or drop the references. `Preset.Reset()` (`Preset.cs:230-263`) is what "empty" means: every button/trigger/dpad and both Min+Max of every axis present with `InputKey.None`, no custom functions. `Reset()` is also what a newly-created preset starts from (`PresetControl.xaml.cs:177-179`).

---

## 2. Slot model

- Max **4** slots (`Managers/EmulationManager.cs:34`), min 1 (`ChangeSlotsCountBy` refuses to remove the last). Slot numbers 1..4, allocated to the lowest free number.
- A slot = `{ SlotNumber, IVirtualGamepad, Keyboard, Mouse, IPreset, InvalidateReason }` (`SplitterCore/Emulation/IEmulationSlot.cs`, `EmulationSlotBase.cs`). Keyboard *and* Mouse are both always present; `Keyboard.None` / `Mouse.None` are sentinel devices (`SplitterCore/Input/Keyboard.cs:5`).
- **Gamepad user index ≠ slot number.** `userIndex = ((i + startingIndex - 2) % 4) + 1` from `GlobalSettings.StartingVirtualControllerUserIndex` (`Models/Splitter.cs:51-52`; a differing formula in `EmulationManager.cs:310`). Also decoupled: the **LED number**, discovered after plug-in by observing which XInput slot lit up (`Models/XboxGamepad.cs:100-129`, `413-437`).
- **"Mount" = plug the virtual pad into the ScpVBus.** `EmulationManager.Start()` (`EmulationManager.cs:75-204`) does, per slot: reject if no input device → `PlugIn()` → on success `slot.Lock()` (freeze the UI row) and subscribe to `Disconnected`. UI text calls it "N Virtual Xbox 360 Controllers **mounted** into the system" (`MainWindow.xaml.cs:378-386`).
- Start-time guards: ≥5 s between start/stop (bypassable with `Start(forced: true)`, which the game autostart uses); no slot may already be invalidated; `Slots.Count <= XinputController.EmptyBusSlotsCount` (Windows caps at 4 total XInput devices).
- Stop (`EmulationManager.cs:206-240`): clears XInput tags, resets LED to 0, unlocks slots, force-unplugs owned pads.
- **`SlotInvalidationReason`** (`SplitterCore/Emulation/SlotInvalidationReason.cs`) — the complete set: `None`, `VirtualBus_Not_Installed`, `Additional_Drivers_Not_Installed`, `VirtualBus_Full`, `Controller_Already_Plugged_In`, `Controller_In_Use`, `Keyboard_Unplugged`, `Mouse_Unplugged`, `Presets_Parse_Failed`, `Controller_Plug_In_Failed`, `XinputBus_Full`, `Controller_Unplugged`, `No_Input_Device_Selected`. Each maps 1:1 to a typed exception in `KeyboardSplitter/Exceptions/Gamepad/`.
- **Slot reset** (`EmulationManager.OnSlotResetRequested`, `:406-463`): only when emulation is stopped; force-unplugs the pad for controller-class reasons, clears keyboard/mouse to `None` for device-class reasons, and rebuilds the slot object in place.
- A slot is invalidated at construction if its target user index is already plugged in (`Controls/EmulationSlot.xaml.cs:49-52`).
- **Device suggestion**: when `GlobalSettings.SuggestInputDevicesForNewSlots` is on (default), new slots auto-pick the first not-yet-assigned keyboard and mouse; initial slot count = `min(max(#keyboards, #mice), 4)` (`MainWindow.xaml.cs:251-261`).
- A slot with **both** keyboard and mouse = `None` becomes an **on-screen (mouse-click) controller**: `IsOnScreenControllerActive`, `Controls/OnScreenController.xaml.cs` drives the virtual pad by clicking a gamepad picture.

---

## 3. Games list + CLI autostart

### 3.1 `splitter_games.xml`
`Managers/GameDataManager.cs:11`, `Models/GameData.cs`, `Models/Game.cs`, `Models/SlotData.cs`. UTF-16 encoded. Schema:

```
<Games>
  <Game Title="" Notes="" Path="" Arguments="" BlockKeyboards="true" BlockMice="false">
    <Slot Number="1" GamepadUserIndex="1" Keyboard="<HWID>" Mouse="<HWID>" Preset="<name>" />
  </Game>
</Games>
```

- Devices are persisted by **hardware ID string**, not by Interception device index — so games survive re-enumeration (`Models/SlotData.cs:44-48`, resolved back in `Game.TryStart`).
- Icon and default Title/Notes are auto-extracted from the exe (`Icon.ExtractAssociatedIcon`, `FileVersionInfo.ProductName` / `.FileDescription`) — `Game.cs:RetrieveGameIcon/RetrieveGameDetails`.
- **`GameStatus`** validation (`Game.GetStatus`, enum at `KeyboardSplitter/Enums/GameStatus.cs`): `NotSet`, `InvalidSlotsCount` (must be 1–4), `InvalidSlotNumber` (1–4), `InvalidGamepadUserIndex` (1–4), `KeyboardMissing` (HWID not currently connected), `MouseMissing`, `PresetMissing`, `ExeNotFound` (missing file or non-`.exe`), `OK`. Status is recomputed on load and on edit; only `OK` games appear in the Play menu (`MainWindow.xaml.cs:581-615`).
- Load failure is non-fatal: the list is replaced with a single pseudo-game whose title is the error message (`GameDataManager.cs:39-42`).

### 3.2 `Game.TryStart()` (`Models/Game.cs:216-330`) — the important sequence
1. Refuse if emulation is already running, or exe missing, or status ≠ OK.
2. Resolve each `<Slot>` into a fresh `EmulationSlot` (keyboard/mouse by HWID, preset by name), **replace the whole slots collection**, set `MainWindow.SlotsCount`.
3. Apply `ShouldBlockKeyboards` / `ShouldBlockMice` from the game's attributes.
4. `EmulationManager.Start(forced: true)` — bypasses the 5-second throttle.
5. If launched via CLI (`App.autostartGameName` set) → **auto-hide the main window** and skip the "run the game now?" prompt; otherwise prompt Yes/No.
6. Launch on a background task: `ProcessStartInfo { FileName = GamePath, WorkingDirectory = dirname(GamePath), Arguments, UseShellExecute = true }`, then `process.WaitForExit()`.
7. **Exit detection**: continuation fires `OnProcessExited()` only if the process lived **> 3 seconds** (guard against launchers that hand off to another process and return immediately).
8. On exit: CLI mode → `EmulationManager.Stop()` + `Environment.Exit(0)`; GUI mode → prompt "'X' has been closed. Do you want to stop the emulation?".

### 3.3 CLI
`App.Application_Startup` (`KeyboardSplitter/App.xaml.cs:227-270`):
- `game=<Title>` → sets `App.autostartGameName`; matched **case-sensitively and exactly** against `Game.GameTitle` in `MainWindow.Window_Loaded` (`MainWindow.xaml.cs:263-272`); throws `InvalidOperationException` if not found.
- `allow-multi-instance` → skips the single-instance mutex `KB_XBOX_SPLITTER_SINGLE_INSTANCE_MUTEX`.
- If no `game=` arg, calls `FreeConsole()` (pure-GUI mode); with `game=` the console stays attached so CLI output is visible.

---

## 4. Detectors & monitors

- **Input detector window** (`KeyboardSplitter/UI/InputDetectorWindow.xaml.cs`) is one component in two modes (`Enums/InputDetectorTarget.cs`):
  - `Device` mode = **keyboard detector / mouse detector** — "press any key on the keyboard you want in this slot"; filtered by `InputDetectorDeviceFilter.{KeyboardOnly, MouseOnly, KeyboardAndMouse}`. Wired from `Controls/EmulationSlot.xaml.cs:128-188`.
  - `Key` mode = **key detector** — bound to a specific `IPresetElement` + slot; shows the target function name and gamepad; **rejects input from a device other than the slot's assigned one** with a "Different keyboard detected: X" warning rather than silently accepting it.
  - Mouse events are only accepted while the cursor hovers a dedicated mouse icon zone (`isMouseHover`); mouse-move events are ignored unless `EnableMouseMoveDetection`. Detected input is swallowed (`e.Handled = true`).
- **Input monitor** (`Managers/InputManager.InputMonitorHistory`, UI in `MainWindow.xaml`): rolling text log of every `key on device pressed/released` across all devices; mouse-move keys excluded. Collapsed by default and **auto-collapses after 60 s** — explicitly "to save CPU time and to avoid using this app as keylogger" (`UI/HelpDialog.xaml.cs:51-52`, timer in `MainWindow.xaml.cs:71-73`). Cleared on emulation start and on collapse.
- **Realtime USB detection**: a dedicated polling thread inside `Interceptor` (`Interceptor/Interception.cs:686-715`) rescans slots 1..20 every **1000 ms**, diffs against the last snapshot, and raises `InputDeviceConnectionChanged(device, isRemoved)`. `Managers/InputManager.cs:287-302` rebuilds the keyboard/mouse lists and re-raises. `Models/Splitter.InputManager_InputDeviceChanged` (`:422-446`) then invalidates any slot whose device was removed (`Keyboard_Unplugged` / `Mouse_Unplugged`) and unplugs its virtual pad.
  - Note the existing bug worth not carrying over: the removal handler uses `slot.Keyboard == null || slot.Keyboard == e.ChangedDevice`, so any slot with a null device gets invalidated too.
- **XInput controller tester** (`UI/XinputControllerTestWindow.xaml.cs` + `Controls/XboxTestSlot.xaml`): enumerates the 4 XInput slots, shows a live panel per connected controller, auto-refreshes on plug/unplug when the checkbox is ticked. Backed by `XInputWrapper/XinputController.cs` — a static polling thread at **30 Hz** (`DefaultUpdateFrequency = 1000/30`) raising `StateChanged` / `PluggedChanged`, plus battery/wired/subtype info. README explicitly recommends validating a preset here before launching a game (`README.md:73`).
- **Controller subtype patcher** (`UI/XinputSubTypesWindow.xaml.cs`): writes an `xinput.ini` `[SubTypes]` block next to a target exe and drops a shim `xinput1_3.dll` / `xinput1_4.dll` / `XINPUT9_1_0.DLL`, letting a virtual pad present as Wheel / ArcadeStick / FlightStick / DancePad / Guitar / GuitarAlternate / Drumkit / GuitarBass / ArcadePad (`XInputWrapper/Enums/ControllerSubtype.cs`).

---

## 5. Keyboard input blocking & REMOTE blocking/unblocking

- **Blocking** = suppressing the stroke at the driver level so Windows never sees it. `Interception.DriverCallback` (`Interceptor/Interception.cs:648-659`) raises `InputActivity`; if the handler sets `Handled`, the stroke is **not** re-`Send`-ed and dies there. Otherwise it's forwarded normally.
- Scope: only devices in `Splitter.AssignedInputDevices` are ever blocked (populated on emulation start from every slot's keyboard+mouse, cleared on stop) — `Models/Splitter.cs:396-406`, `448-467`. Unassigned keyboards keep working (FAQ Q at `UI/FaqWindow.xaml.cs:79-81`).
- Two independent global flags on `ISplitter`: `ShouldBlockKeyboards` (default **true**) and `ShouldBlockMice` (default **false**) — `SplitterCore/SplitterBase.cs:26-44`. UI checkboxes "Block keyboards" / "Block mice" at `KeyboardSplitter/MainWindow.xaml:106-137`.
- Blocking is only active while emulation is started (`InputManager_InputActivity` returns early otherwise).
- Safety valve: when the splitter's own main window is focused, `e.Handled` is forced back to `false` so you can't lock yourself out of the app (`Managers/InputManager.cs:281-284`, via `GlobalSettings.IsMainWindowActivated`).
- **REMOTE blocking/unblocking = "emergency mode"** (`Managers/InputManager.cs:304-403`): tap a key 5 times in a row (5 downs + 5 ups, with any intervening different key resetting the counters) on *any* device:
  - **LeftControl ×5** → toggles `ShouldBlockKeyboards`
  - **RightControl ×5** → toggles `ShouldBlockMice`
  - Distinct audio feedback per direction: `Resources/connected.wav` (blocking on) / `disconnected.wav` (blocking off) — `Models/Splitter.cs:338-394`.
  - **Ctrl+Alt+Delete (any combination of L/R Ctrl, L/R Alt, Delete or NumpadDelete) → `EmergencyStop` → full `EmulationManager.Stop()`.**
- The point of "remote" is that it works while a game holds fullscreen focus (`MainWindow.xaml:114-118` tooltip, FAQ at `UI/FaqWindow.xaml.cs:119-124`).
- Consequence encoded in the UI: binding `LeftControl` in a preset renders the label **red** (`Controls/PresetControl.xaml:130`), because (a) some keys emit the LeftControl scancode alongside their own and (b) it collides with emergency mode.

---

## 6. Xbox "custom functions"

- Purpose: **map one key to a function of any category, and allow many-to-one** — i.e. one key firing several Xbox functions at once ("Xbox Button A and Xbox Button B pressed together when the user presses Space") — `UI/HelpDialog.xaml.cs:47-50`.
- Implemented as a flat `XboxCustomFunction` enum covering all 26 button/trigger/axis/dpad endpoints in one numbering space, so a `<custom>` row needs no category attribute (`VirtualXbox/Enums/XboxCustomFunction.cs`).
- `KeyboardSplitter/Helpers/CustomFunctionHelper.cs` is the whole translation layer: `GetFunctionType()`, `GetXboxButton/Trigger/DpadDirection()`, `GetXboxAxis(fn, out position)`, the reverse `GetFunction(XboxButton)`, and `SetFunctionState()` used by the on-screen controller.
- Management UI: "Add custom function" appends a `PresetCustom(Button_A, InputKey.None)` and scrolls to end; "Delete custom function" removes it. Both are **disabled for protected presets** (`Controls/PresetControl.xaml.cs:206-231`).
- Crucially, custom functions participate in the *reverse* lookup too: `Preset.GetKeys(...)` for a normal button/trigger/axis/dpad also collects keys bound via matching custom functions, and vice versa (`Presets/Preset.cs:71-228`). This is what makes "all keys mapped to this function are released" correct when a function is driven from two directions. **This bidirectional aggregation is the subtlest part of the domain model and must be preserved.**
- Unlike the other four categories, custom functions have **no per-element value** — an axis custom function always drives full Min/Max (see §8.5).

---

## 7. Per-keyboard vs global behavior; sharing rules

**Per-slot (per-device):** assigned keyboard, assigned mouse, preset, gamepad user index, LED, invalidation reason, lock state.

**Global (app-wide):** `ShouldBlockKeyboards`, `ShouldBlockMice`, emergency-mode hotkeys, mouse-move dead zone, input monitor, the whole preset library, `splitter_settings.xml` (`GlobalSettings.cs`: `MouseMoveDeadZone` 0–12, `DisplayEmulationInformation`, `SuggestInputDevicesForNewSlots`, `StartingVirtualControllerUserIndex` 1–4).

**Sharing:**
- **One keyboard → many slots: supported by the engine.** `InputManager_InputActivity` iterates **all** slots and translates for every slot whose keyboard *or* mouse matches the event device (`Models/Splitter.cs:408-419`) — no `break`. So the same physical keyboard can feed 2+ virtual pads (with different presets) simultaneously.
- **Many keyboards → one slot: not supported.** A slot holds exactly one `Keyboard` and one `Mouse` reference.
- **However, one slot can be fed by a keyboard *and* a mouse at once** — that's the de-facto 2-device-per-slot case, and the key detector switches its filter accordingly (`Controls/PresetControl.xaml.cs:259-281`).
- The auto-suggest logic avoids assigning an already-used device to a *new* slot, but nothing prevents the user from manually assigning one device to several slots.
- Device identity: `InputDevice.Match()` compares `DeviceID + HardwareID + StrongName + FriendlyName` (`SplitterCore/Input/InputDevice.cs:24-38`). `StrongName` is positional (`Keyboard_01`…`Keyboard_10`, `Mouse_01`…`Mouse_10` derived from the Interception slot index — `Interceptor/InterceptionDevice.cs:35-56`); `FriendlyName` is read from `HKLM\System\CurrentControlSet\Enum\<hwid>` `DeviceDesc` with the `REV_xx&` segment stripped (`:58-100`). **Interception index is positional and unstable across replugs — persistence uses HardwareID instead (see §3.1).**

---

## 8. Edge cases, known issues, guidance embedded in code/README

1. **Keyboard ghosting** (`README.md:63-74`). Not fixable in software. Guidance: buy an anti-ghosting keyboard or rebind; test at `drakeirving.github.io/MultiKeyDisplay`. The shipped `default` preset **has known ghosting**: LS and RS both at lower-left (LeftArrow + DownArrow + Numpad4 + Numpad2) won't register on cheap boards. README preemptively refuses ghosting bug reports and directs users to the built-in XInput tester first.
2. **ScpVBus version pinning to 22.52.24.182** (`README.md:49-56`). The version lives only in `VirtualXbox/Driver/x64/ScpVBus.inf` line `DriverVer=04/19/2016,22.52.24.182`. **There is no runtime version check anywhere in the code** — `VirtualXboxBus.IsInstalled` is just `VBusExists()`. A mismatched (newer *or* older) ScpVBus manifests only as a downstream "Slot is invalidated" error; the documented fix is manual uninstall in Device Manager → reboot → let KS reinstall → reboot. **A rewrite should detect and report the bus version explicitly** rather than leaving it as folklore.
3. **"Slot is invalidated" error** — thrown by `EmulationManager.CheckForInvalidatedSlots` (`:339-354`), listing each offending slot and reason. It is the single visible symptom of ~12 distinct root causes (§2), which is exactly why it's confusing in the wild.
4. **Windows Update breaking Interception** — `README.md:142` ships a separate GUI uninstaller (`InterceptionUninstall/`) "in case you recently updated your W10". Driver state detection (`Interceptor/InterceptionDriver.cs:19-63`) is a three-way check — WMI `Win32_SystemDriver` where `Name='keyboard'` **plus** a file identity check on `system32\drivers\keyboard.sys` (CompanyName `Oblita`, ProductVersion `1.00`, FileDescription `Keyboard Upper Filter Driver`) — yielding `Installed` / `NotInstalled` / **`RebootRequired`** (file present but service not registered). Install/uninstall shell out to `keyboard_driver.exe /install|/uninstall` and require elevation; failure is surfaced as "you must run the application as administrator".
5. **Custom axis + opposite-key handling mismatch**: `SetAxis` snaps to hardcoded `XboxAxisPosition.Min/Max` when the opposite key is held (`Models/Splitter.cs:161-171`), ignoring any custom `value`. Likewise `CustomFunctionHelper.SetFunctionState` always drives full-scale. Custom axis ranges therefore behave inconsistently when two opposing keys overlap.
6. **Debugger/UI-thread deadlock hazard**: if the app blocks its UI thread, the Interception callback never returns and **all keyboards and mice stay dead until reboot** — the release build warns about this at startup (`App.xaml.cs:29-56`). The C# design does all translation on the WPF dispatcher (`Managers/InputManager.cs:242-285`). **This is the single strongest argument for the Rust rewrite to keep the interception loop off any UI thread.**
7. **Ghost keyboards**: Windows reports more keyboards than are physically attached (101/102-key driver artifacts; some mice enumerate as keyboards) — documented, not fixed (`UI/FaqWindow.xaml.cs:83-88`).
8. **Interception device limits**: `MaxKeyboardsCount = 10`, `MaxMiceCount = 10`, IDs 1–10 keyboards / 11–20 mice (`Interceptor/Interception.cs:12-20`). Device rescan loops `id = 1; id < MaxDeviceCount` — off-by-one, device 20 is never enumerated (`:721`).
9. **Enum coupling guard**: `InputManager.CheckKeysEnumerations()` (`:170-213`) asserts at startup that `SplitterCore.Input.InputKey` and `Interceptor.Enums.InterceptionKey` have identical names, values and underlying type (`ushort`), throwing `InvalidProgramException` otherwise. `InputKey` also carries pseudo-keys for mouse input: `MouseLeftButton` 20001 … `MouseMoveDown` 20013 (`SplitterCore/Input/InputKey.cs`). In Rust this should be one type, not two mirrored enums.
10. **Windows caps XInput at 4 controllers total** — pre-flight check produces an explicit message counting already-connected physical pads (`EmulationManager.CheckXinputBus`, `:356-386`).
11. **Xbox 360 Accessories driver** required on XP/Vista/Win7 only (detected via `Program Files\Microsoft Xbox 360 Accessories\XBoxStat.exe`); Windows 8+ returns `true` unconditionally (`Models/XboxGamepad.cs:47-72`). Irrelevant on Win11 — drop it.
12. **Modal dialogs vs. mouse interception**: every `OpenFileDialog`/`SaveFileDialog` is bracketed with `Interception.DisableMouseEvents = true/false` or the dialog is unusable (`MainWindow.xaml.cs:629-631`, `735-737`, etc.).
13. **Portability hack**: all managed and native DLLs (including `interception.dll`, `VirtualXboxNative.dll`, ScpVBus `.inf/.sys/.cat`, `devcon.exe`) are embedded resources, extracted at runtime to `%TEMP%\<app name+version>\` and loaded via `PATH` manipulation + `LoadLibrary` (`App.xaml.cs:128-171`, `VirtualXbox/VirtualXboxBus.cs:95-107`, `README.md:129-137`). Yields a single portable exe.
14. **Session-end handling**: on Windows shutdown/logoff the app force-saves settings + presets and destroys the splitter (`App.Application_SessionEnding`) — important, since leaving the Interception filter loaded with a dead process is what bricks input.
15. Deleting a preset in use is documented to fall back to the `empty` preset (`UI/HelpDialog.xaml.cs:41`) — but `empty` doesn't exist in this fork (§1.5), so that path is broken.
16. Preset deletion and overwrite are explicitly **not undoable** (FAQ `:66-72`).

---

## Recommended requirement groupings for the Rust backend

| Crate/module | Covers |
|---|---|
| `preset` | §1 schema (serde, XML-compatible read + write), ID tables, `default`/`empty`, `Reset()`, bidirectional `GetKeys` aggregation (§6), v1 upgrade path, dirty tracking, import/export |
| `input` | Interception (or successor) device enumeration, HWID + friendly-name resolution, per-device key-state table, 1 Hz hotplug poll → arrival/removal events, blocking decision hook, emergency-mode detector (§5) |
| `emulation` | Slot model, mount/unmount, user-index & LED decoupling, the 13 invalidation reasons, slot reset, XInput bus capacity checks (§2) |
| `translate` | The `InputActivity` → per-slot mapping → `Set{Button,Trigger,Axis,Dpad}` pipeline including opposite-axis logic and the all-keys-released rule (§1.4, §6) |
| `games` | `splitter_games.xml`, `GameStatus` validation, launch + >3 s exit detection, CLI `game=` autostart with autohide + forced start (§3) |
| `drivers` | Interception 3-state detection & install, ScpVBus install/uninstall **plus an explicit version check** (§8.2, §8.4) |

Modern replacements to evaluate for the driver layer: **ViGEmBus + `vigem-client`** (nefarius' successor to ScpVBus; the ScpVBus pinning problem disappears) and, for per-device capture, **Interception via `interception-rs`** or a **Raw Input (`WM_INPUT`) + `RIDEV_NOLEGACY`** approach — Raw Input gives per-device identity natively and needs no kernel driver, at the cost of only being able to block for the whole class rather than per device.