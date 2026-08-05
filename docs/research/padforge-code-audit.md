# PadForge code audit (local clone, studied 2026-08-05)

Deep read-only audit of the PadForge clone at
`C:\Users\Victor\controller-project` (commit "Merge v4-dev for 4.1.0", main).
Companion to [padforge-ui-lessons.md](padforge-ui-lessons.md), which covered
the README-level patterns; this doc is the code-level evidence. All paths
below are relative to the clone root unless absolute.

**License wall (established, re-confirmed here)**: PadForge's own code is
CC BY-NC-SA per README §License (`README.md:632-636`); the in-tree
`LICENSE.txt` is inherited x360ce MIT boilerplate — treat ALL in-tree code as
NOT copyable. Concepts, architecture, constants-as-facts, and protocol
behavior are free. 3D `.obj` meshes (`PadForge.App/3DModels/`) are from
Handheld Companion, CC BY-NC-SA — unusable. 2D art is from
AL2009man/Gamepad-Asset-Pack — **MIT, verified 2026-08-05** (see §0).

## Verdict table

| Thing | Verdict | Why |
|---|---|---|
| Gamepad-Asset-Pack 2D controller art | **TAKE** | MIT verified upstream; SVG source (better than PadForge's PNG exports); credit AL2009man |
| HIDMaestro protocol facts + constants (§3) | **TAKE** | HIDMaestro itself is MIT; PadForge's usage just confirms/extends upstream docs — cite, don't copy comments |
| Handheld Companion 3D meshes | **SKIP** | CC BY-NC-SA; source CC0/CC-BY glTF elsewhere for Studio's 3D view |
| Any PadForge code, XAML, layout tables, comments | **SKIP** (as text) | CC BY-NC-SA; everything below is concept-only |
| Mapping row model: N sources → combine mode → 1 target (§2.1) | **PORT** | Clean superset of ksx presets; string-keyed forward-compat is smart |
| Custom formula engine (sandboxed expr over sources) (§2.1) | **PORT (later)** | Power feature; small Pratt parser + fixed fn registry is a weekend in Rust |
| Shift layer semantics Hold/Toggle/Latch/Cycle/Sticky (§2.2) | **PORT** | Exact roadmap vocabulary for ksx layers; string layer keys avoid schema migration |
| SOCD cleaner: Off/Neutral/LastWins/FirstWins pure step fn (§2.3) | **PORT** | Directly upgrades our fixed neutral-on-conflict DS4 hat collapse; trivially testable |
| Deadzone shapes (6) + monotone-spline curve LUT (§2.4) | **PORT** | ScaledRadial default + "x,y;x,y" curve strings + 256-entry LUT = right shape for Rust |
| Macro system (§2.5) | **PORT (subset)** | 12 trigger modes / 52 action types is a decade of accretion; take the trigger/step/repeat skeleton only |
| Profile-format practices: additive migration, append-only enums, content-addressed settings (§2.6) | **PORT** | Process lessons for FMIR/preset schema evolution |
| Record-a-binding state machine (§1.2) | **PORT** | Constants and edge cases (wait-for-release, axis hold cycles, bipolar chaining) are earned knowledge |
| 2D hit zones: per-pad coordinate tables + normalized polygon hit paths (§1.4) | **PORT (adapted)** | ksx should use SVG element ids instead (we get SVG upstream); keep the generator-script idea |
| 30 Hz UI mirror + dirty-flag render coalescing (§1.3) | **PORT** | Engine at 1 kHz, UI echo at 30 Hz, repaint only on change — exactly Studio's WS budget answer |
| Web controller transport (§4) | **PORT (lessons)** | HttpListener+WS+JSON works; their zero-auth model is the anti-pattern — ksx pairs like their Remote Link, not their web controller |
| Dashboard: Hz counter, per-slot stage ledger, flame states (§5) | **PORT** | One-screen instinct confirmed at code level; steal the *shape*, not the XAML |
| `docs/ui-exposure-ledger.md` process | **PORT** | "Every persisted setting has a UI card or it's a bug" — adopt as a ksx Studio audit ritual |
| Their conflict handling (there is none) | **N/A — beat it** | No duplicate-binding detector anywhere; ksx can differentiate here cheaply |

---

## 0. Gamepad-Asset-Pack verification (the TAKE)

Fetched https://github.com/AL2009man/Gamepad-Asset-Pack 2026-08-05:

- **License: MIT**, confirmed. README asks users to "credit this project (as
  per MIT License)". PadForge's own credits agree (`README.md:576,633`:
  "2D controller PNG schematics … MIT, by AL2009man").
- **Contents**: controller overlays (full schematics), controller icons,
  connection/wireless icons for 10 controller types: Arcade, DualShock 3/4,
  DualSense, Nintendo Switch, Steam Controller, Steam Deck, Simple Gamepad,
  Xbox 360, Xbox One, Xbox Series X|S. Button-prompt icons live in a sibling
  repo.
- **Formats**: SVG primary (Inkscape), plus PNG and XCF. Some themes pack
  alternates in one SVG (e.g. Xbox One launch vs S models as toggleable
  layers).
- **Caveat**: author discourages commercial *game* use (assets are ripped or
  recreated from official sources; platform-cert risk). ksx is a tool, not a
  shipped game — same use class as PadForge. Keep the MIT attribution and
  note the trademark-adjacent nature (Microsoft/Sony trade dress) in
  THIRD-PARTY-NOTICES.
- **Consequence**: ksx gets the SVGs *upstream*, not PadForge's PNG
  derivatives — crisper, theme-able, and SVG ids give us hit zones for free
  (see §1.4).

## 1. UI structure and the mapping UX

### 1.1 Shell and navigation

`PadForge.App/MainWindow.xaml` (+ `.cs`, 7.7k lines) is a WPF-UI
`FluentWindow` (Mica backdrop, content into titlebar), 1100×720. Three rows:
branding bar → `ui:NavigationView` left rail (48 px compact / 244 px open) →
status bar. Rail items are built in code (`BuildNavigationItems`):
**Dashboard, Profiles, Devices, then one dynamic entry per virtual
controller slot (Pad1..Pad16, grouped Xbox → PlayStation → Extended → KbM →
MIDI), "+ Add Controller", footer Settings/About.** Not tabs — a
sidebar-of-pages, and all pages are **eagerly instantiated and
visibility-toggled** (no Frame navigation), which is why their render loops
carry `IsVisible`/minimized guards.

The per-pad page (`Views/PadPage.xaml`, 12,433 lines — the monolith warning)
has a two-tier tab strip: slot-scoped tabs (Preview, Mappings, Macros,
Menus, Bass Shakers) then device-scoped tabs (Sticks, Triggers, FFB, Wheel,
Impulse Triggers, Adaptive Triggers, Lighting, Gyro, Touchpad, Audio,
Pointer, Mouse) whose visibility is capability-driven (`HasGyro`,
`HasTouchpad`, Sony VID 0x054C, wheel VID/PID). Progressive disclosure by
capability, not by burying settings.

Status bar = message with 5 s decay + profile pill + device count + live
polling Hz + engine flame glyph (state-colored: ember running, gold
idle/stopping).

### 1.2 Record-a-binding flow (the state machine)

`PadForge.App/Services/RecorderService.cs` (1,399 lines), a single-session
recorder on the UI thread. Entry points: per-row Record toggle
(`MappingItem.ToggleRecordCommand`), **clicking a control on the 2D/3D
preview** (`ControllerElementRecordRequested`; clicking the same element
again cancels), Map All, extra-source and freeform (dialog) variants.

Constants worth keeping (RecorderService.cs:31-60):

- `PollIntervalMs = 33` (~30 Hz DispatcherTimer), `TimeoutSeconds = 10`
- `AxisThreshold = 16384` (~25% deflection), `AxisHoldCycles = 3`
  (~100 ms sustained before an axis wins — kills stick-noise false grabs;
  mouse devices bypass the hold because deltas are instantaneous)
- MIDI: CC threshold 10/127, pitch 6000/65535, relative-encoder band ±16

Flow: on start it snapshots a **baseline input state for every device
assigned to the slot** (not just the dropdown selection — the dropdown does
NOT gate recording; the winning device's GUID is stamped onto the mapping).
Detection order per tick, first-to-fire wins: timeout → wait-for-release
gate → touchpad click → button rising edge → POV → touchpad gesture → MIDI
note/CC → axis candidate with hold-cycle confirmation.

States: Idle → Recording → (WaitForRelease on chained recordings, which
re-baselines only after *all* devices are neutral) → Complete/Cancel/
Timeout. **Bipolar auto-chain**: recording a button-class input onto a
bidirectional axis target auto-starts a second recording for the opposite
direction ("Now map: Left…"), with the page force-switched to the Preview
tab so the directional arrow is visible.

Cancel: click again / row button again / timeout. **No ESC-to-cancel and no
numeric countdown** — feedback is a persistent status-bar prompt, a pulsing
row highlight (0.4 s opacity pulse, with a static-tint fallback when the OS
reports reduced motion), and a 400 ms flash on the preview element.

**Conflict handling: none.** No duplicate-binding detector exists anywhere
(grep across ViewModels/Views/Services). Recording overwrites the row. The
one protective rule: clicking a stick quadrant when the row already holds a
full analog axis appends a detached extra source (with invert for the
negative quadrant) instead of clobbering, then defaults combine to MaxAbs.
ksx should do better: we already know every binding in a preset; a
conflict chip ("also bound to P2·B — steal?") is cheap differentiation.

### 1.3 Live input echo: engine → visuals

Two-stage pump, and the rates matter:

1. Engine polls at ~1 kHz on a background thread. A WPF
   `DispatcherTimer(Render)` at **33 ms (~30 Hz)** mirrors engine state into
   the pad ViewModels (`InputService.UiTimer_Tick`), gated off entirely when
   the window is minimized, and per-device previews gated on page
   visibility. Backgrounded, the refresh lane drops to 1 Hz.
2. Views (2D, 3D, schematic) subscribe to `CompositionTarget.Rendering`
   with a **dirty flag**: VM property changes set `_dirty`; the render
   callback repaints once per frame only if dirty and visible. Effective
   repaint = min(display refresh, 30 Hz), coalesced.

Highlight mechanics: 2D uses **visibility-swap of pre-positioned "pressed"
PNGs** (the active art *is* the highlight; hover = 0.4 opacity; recording
flash = 400 ms opacity toggle), triggers fill via an animated clip rect,
sticks translate by `value/32767 × StickMaxTravel` (30 px Xbox / 25 px
Sony). 3D uses **material swap** (default vs highlight material per mesh
group; ember fallback #FF6B2C) and lerps material colors for analog
deflection, with brushes cached per-mesh so the render loop never
allocates. Their annotation overlay's stated rule is a good motto: "no
storyboards, no Effects, all live state is plain brush/property swaps."

ksx Studio translation: daemon → WS at a throttled echo rate (30 Hz is
proven sufficient for "it feels live"), browser applies state to SVG classes
in rAF with a dirty check. Don't stream 1 kHz to the DOM.

### 1.4 2D hit zones

`PadForge.App/2DModels/` is **PNG only** (one folder per pad type: XBOX360,
XBOXONE, XBOXSERIES, DS4, DualSense, SWITCHPRO, STEAMDECK, STEAMCONTROLLER,
MOUSE; one `*_base.png` body + one PNG per control's pressed state). Zones
are NOT in the art: `PadForge.App/Models2D/ControllerOverlayLayout.cs` is an
**auto-generated C# table** ("AUTO-GENERATED by tools/overlay_positions.py")
— per pad type: base size, stick max travel, and an array of overlay
elements `(imageFile, targetName, elementType, x, y, w, h, hitPath)` where
`hitPath` is a normalized 0..1 **polygon traced around the art's opaque
pixels** (e.g. Xbox 360 ButtonA: 16-vertex polygon inside a 127×106 rect at
(1178,528) on a 1545×955 base). The polygon is applied as a WPF `Clip` on a
transparent hit rectangle, which bounds hit-testing — so hover/click only
fire on actual button pixels. Stick rings resolve clicks by quadrant math
(center 30% radius = stick click, else dominant axis ± direction). Element
types: Button, Trigger, TriggerBase, StickRing, StickClick, FaceButtonGroup,
Touchpad.

ksx: since we take the *SVG* pack upstream, per-control `<g id>` elements
replace the whole coordinate-table apparatus — CSS `:hover`/classes give
highlight and hit-testing natively. Keep two PadForge ideas: a build-time
script that derives layout data from art (never hand-edit), and quadrant
resolution for stick zones.

### 1.5 Polish inventory (what makes it feel finished)

- **Owned accent, both themes**: app-fixed ember #FF6B2C (never the Windows
  accent), with a dark palette (Cold #58B6E4 / EmberHot #FFA24D) and a
  re-derived light palette (#1E6E9F / #C24A12) — semantic tokens
  (Ember/Cold/Wait brushes), a telemetry monospace font for numbers.
- **Reduced-motion respect**: animations gated on
  `SystemParameters.ClientAreaAnimation` with static fallbacks.
- **Frozen, shared glow effects** set from triggers, never animated
  (animating Effects from style triggers crashed them at startup).
- **HUD windows that never steal focus**: shift-layer flyout and
  profile-switch toast are Win11-volume-OSD replicas (`WS_EX_NOACTIVATE |
  WS_EX_TOOLWINDOW`, slide 300 ms in / 250 ms out); radial menu overlay adds
  `WS_EX_TRANSPARENT` so it can't eat a click.
- **First-run**: dim overlay + 5-stop spotlight tour (engine card → slots →
  nav rail → services → status bar), gated on a settings flag, re-runnable
  from Settings.
- **Empty states everywhere**, all localized (9 languages shipped).
- **Purpose-built micro-controls**: draggable curve editor with a live
  input dot; a quarter-arc trigger gauge showing raw pull (cold needle) vs
  post-curve output (ember sweep), explicitly value-driven "never on a
  clock"; profile pill with a 900 ms flare on auto-switch.
- **Process**: `docs/ui-exposure-ledger.md` — an audit table of every
  persisted engine setting vs its UI control; "A field with no card is a
  bug, not a gap." Adopt this ritual for ksx presets vs Studio.

## 2. Engine concepts → ksx roadmap vocabulary

Pipeline context first: `InputManager` runs ~1000 Hz on a dedicated thread
(multimedia timer `timeSetEvent(1 ms)` + sub-ms busy spin; idle mode drops
to ~20 Hz when no slot has an online device; focus-loss neutralizes all
outputs and releases latched macro keys). Steps: 1 enumerate (every 2 s,
not every tick) → 2 read input states → 3 per-device mapping eval → 4
combine per-slot across devices → 4b macros → 5 submit to virtual
controllers (SOCD applied here, last) → 6 copy state out for UI.

### 2.1 Mapping model

One `MappingRow` = **one output target (string name), N sources, a combine
mode**. Source `Kind` is a *string* discriminator ("unknown treated as
Direct" — forward compatibility by construction): Direct, Incremental
(ramped accumulator with up/down buttons, cruise or snap-back), InvertOnHold,
Ramped (attack/release envelope), WindingStick (JSM-style 900° winding),
AngleToAxisX/Y, MotionLeanX (gravity tilt). Descriptors are strings too
("Button 3", "Axis 1", "POV 0 Up", "Gyro X", "Flick Stick Right"…).

**Combine modes — 8, not 6** (UI list in `MappingItem.cs:1833`):
`MaxAbs` (default for axes; max by |value|, sign preserved), `Sum`,
`Average`, `OR` (default for buttons), `AND`, `XOR` (exactly-one, not
parity), `Custom` (formula), `StickTrim` (trigger targets only: last source
trims a held level, gates scale it — a sim-racing feature). Critical
correctness note they learned: multi-source rows must be evaluated **once
per frame across all devices**, not once per device, or
Sum/Average/AND/XOR/Custom silently degrade to OR/MaxAbs.

**Custom formulas** (`MappingExpression.cs`): hand-rolled tokenizer +
Pratt parser → cached AST. Variables `a..z` = sources in row order,
`s[i]` indexed, `aD..zD` = "source active" flags; literals + `pi` (no `e` —
it shadowed the 5th source, a shipped bug); ternary, boolean, comparison,
arithmetic ops; fixed function registry (abs, min, max, clamp, sign, floor,
ceil, round, sqrt, sin, cos, tan, atan2, lerp, pow, hypot, deadzone);
unknown functions rejected at parse; eval returns 0 on NaN/Inf/throw —
"the engine never throws into the polling loop."

*ksx fit*: our preset rows are 1 key → 1 control. Sources-list + combine
mode is a strict superset that costs one enum + one Vec in FMIR; `MaxAbs`/
`OR` defaults reproduce today's behavior exactly, so it's backward-neutral.
The expression engine maps to a tiny Rust crate (or `evalexpr`-style) —
defer until a user asks, but reserve `combine: "custom"` + `expr` in the
schema now.

### 2.2 Shift layers

`ShiftActivator` per layer; **layers are string keys** (`LayerMask`,
"Base"/"Shift1"/anything) so adding layers is UI-only, never a schema
migration. Activation modes (mode is a string; "Latch" in the UI is the
`"Custom"` value on the wire — naming scar to avoid):

- **Hold**: engaged while held; optional release-linger (`ReleaseDelayMs`)
  where a re-press inside the window cancels disengage.
- **Toggle**: edge flips; optional `AutoCancelMs` — auto-disengage if no
  row on that layer produced output for N ms (activity-stamped).
- **Latch** (`"Custom"`): single-valued override slot; press sets, press
  again clears, a different latch replaces. Overrides the hold-stack
  entirely.
- **Cycle**: ordered layer list + next/previous buttons (prev may live on a
  different device); pure cursor-stepper with wrap and include-base options.
- **Sticky**: typewriter shift — one-shot that snapshots device state on
  engage and disengages on the falling edge of the *consuming* input.
- **Passive**: no activator; reachable only via Cycle/Latch.

Activator inputs: button, chord (cross-device second leg), or axis with
threshold/half/invert. Edge modifiers: long-press arm delay, double-press
window (Steam's 442 ms default cited), fire-on-release, and
`PostponeMapping` (whether the activator button also acts as a mapping
source — otherwise it's suppressed as a source while exerting). Multiple
engaged holds resolve **last-engaged-wins** via a stack.

Layer change semantics: default is REPLACE (Base rows drop; unmapped
targets go neutral); per-activator `InheritUnmapped` flips to
overlay-with-fallthrough, where a layer row with zero sources is
"transparent" unless it sets `NoInherit` (explicit blocker). There is no
release-all event — releases fall out of the per-frame recompute, plus
explicit runtime-clear on profile switch.

*ksx fit*: this is the vocabulary our layer roadmap item should adopt
verbatim: `hold | toggle | latch | cycle | sticky` + `inherit` flag per
layer. Keyboard-only makes it simpler for us (no axis activators needed at
first). FMIR change: `layer` field per row + activators list per preset.

### 2.3 SOCD

`SocdCleaner` / `SlotButtonSocd`: modes **Off, Neutral, LastWins,
FirstWins** (no up-priority mode). Core is a pure, testable step function
`StepPair(mode, a, b, prevA, prevB, ref winner) → (suppressA, suppressB)`
with deterministic same-frame tie-breaks and a documented subtlety: when
the winner releases, the still-held partner re-presses the same frame via
ordinary change detection. Applied at **two points**: (a) keyboard VC
"Snap Tap" — on the logical key bitset *before* change detection; (b)
gamepad slots — in Step 5 **immediately before submit**, so physical +
mapped + macro contributions are cleaned uniformly. Pairs are configured as
strings ("vkA:vkB" for keys; target names like "DPadLeft:DPadRight" for
pads).

*ksx fit*: our DS4 hat collapse is hardcoded neutral. Port the enum + pure
step function into the output stage (same "clean last, clean everything"
placement), surface per-preset `socd: neutral|last_wins|first_wins|off`.
Direct upgrade, small diff, very testable.

### 2.4 Thresholds, deadzones, curves

- **Deadzone shapes** (slot-level enum): Axial, Radial, ScaledRadial
  (default — circular + rescale), SlopedAxial, SlopedScaledAxial, Hybrid.
  (They also have a *per-source* shape field with a different value coding
  — documented in-tree as "never copy one into the other". Lesson: one
  coding, one enum.)
- Per-axis: deadzone %, anti-deadzone (output floor), max-range, and a
  global axis-to-button threshold (50% default, per-source overridable).
- **Curves, two systems**: named exponent presets (Relaxed √x, Linear,
  Wide x^1.5, Aggressive x², ExtraWide x^2.5 — sign-preserving) and user
  control-point curves serialized as `"x,y;x,y;…"`, evaluated as a
  **monotonic cubic spline baked into a 256-entry LUT** cached by curve
  string — O(1) lookup on the hot path. NOT bezier.
- Per-source shaping order (documented): shape-deadzone → sensitivity →
  outer range → curve exponent → anti-deadzone → clamp; all default off.
- No trigger hysteresis field exists (closest: flick-stick 0.9× release).

*ksx fit*: keys are digital so most of this is for our analog-emulation
side (WASD→stick ramps). The Ramped source kind (attack/release envelope
with autocenter and a faster reverse multiplier) is precisely the "keyboard
stick feel" feature; the curve-string + LUT idea ports directly for stick
response tuning.

### 2.5 Macros

`MacroItem` (7k lines — a warning about scope). Skeleton worth porting:

- **Trigger modes** (12): OnPress, OnRelease, WhileHeld, Always,
  CustomExpression, HoldForMs, DoublePress, TriplePress, SinglePress
  (deferred press that coexists with DoublePress on the same button),
  Toggle, Turbo, ShortPress.
- **Trigger source**: physical device buttons OR the slot's combined
  virtual output (both are useful).
- **Repeat**: Once, FixedCount, UntilRelease, with re-arm delay.
- **Actions**: 52 types — press/release/toggle/turbo per channel (VC
  button, key, mouse), Delay, axis set/hold/latch/scale, rumble ± trigger
  motors, lightbar, sounds, cursor control, run program, text block…
  Enums are **append-only with pinned ordinals** because a clipboard leg
  serializes them numerically.
- Macros are **shift-layer-scoped** (closing a layer clears its toggles),
  and focus-loss releases all latched macro keys.

*ksx fit*: presets could grow a `macros` array with the trigger/steps/
repeat triad (OnPress/WhileHeld/Turbo + press/release/delay steps covers
90% of arcade use — combo buttons, turbo). Cap the action set hard;
PadForge's 52 is the cautionary tale.

### 2.6 Profile format & runtime switching

- Serialization: one `XmlSerializer` file (`PadForge.xml` next to the exe),
  save = write-temp-then-replace (they shipped a truncate-first data-loss
  bug; atomic replace is the fix), 250 ms autosave debounce. JSON only for
  sub-payloads (clipboard envelopes, HIDMaestro profile blobs).
- **No schema version integer.** Migration is field-additive ("old XML
  lacks the attribute → default → old behavior") + explicit per-field
  migrator seams (`MappingSetMigrator` collapses the legacy per-device
  descriptor fields into MappingSet rows; two devices on one output become
  one row with two sources). Append-only numeric enums where serialized.
- **Content-addressed pad settings**: shared across device links by a
  checksum of a field dump — dedupe by value, not by reference.
- Runtime switching, three independent paths that all converge on one
  apply function: (a) **per-game auto-switch** — a 30 Hz foreground-window
  poll with an (hwnd,pid)→exe-path cache, profiles carry pipe-separated
  exe paths, manual-override latch so the user can pin a profile until the
  foreground actually changes; (b) **global shortcuts** — cross-device
  button/axis combos with modes Specific/Next/Previous/ToggleWindow/
  ToggleVCsDisabled, evaluated regardless of active profile; (c) **NFC** —
  cleverly NOT a special path: each registered tag UID becomes a stable
  synthetic raw-button index (1..255), so tags flow through the ordinary
  trigger machinery. Switches show a non-activating toast overlay.

*ksx fit*: ksx already has profiles-as-files; the portable lessons are
atomic replace on save, additive migration discipline, and per-game
auto-switch (foreground poll + exe match + manual-override latch) as a
future `ksx` daemon feature. The NFC trick — exotic inputs normalized to
synthetic buttons early — matches ksx's "everything becomes a key event"
instinct.

## 3. HIDMaestro client integration (M8 protocol intelligence)

Where: `PadForge.App/Common/Input/HMaestro*.cs` (client),
`InputManager.Step5.VirtualDevices.cs` (lifecycle),
`PadForge.App/Resources/HIDMaestro/HIDMaestro.Core.dll` (bundled SDK).
HIDMaestro upstream is MIT (github.com/hifihedgehog/HIDMaestro), so
everything here is open protocol; PadForge's comments *confirm* driver
internals (they cite `driver.c` / `companion.c` line behavior directly).

### 3.1 Transport & SDK surface

- **In-process SDK over a shared-memory section + event signaling**, not a
  pipe/COM/IOCTL client. C# surface: `HMContext` (driver install, profile
  load, controller factory) → `HMProfile` (one of 225+ embedded JSONs:
  VID/PID, strings, `DescriptorHex`, `AxisMap`, Sticks/Triggers layout,
  `ExtendedOutputReport`) → `HMController` (one virtual device;
  `SubmitState(HMGamepadState)` / `SubmitRawReport(bytes)`; events
  `OutputReceived`, `OutputDecoded`).
- Driver is UMDF2. **The consumer drives the cadence** — "no internal
  pumping thread"; the driver reads a seqlocked latch from the shared
  section. `HMGamepadState`: `Axes` = `Dictionary<HMAxis, float>` keyed by
  HID usage (`HMAxis` = ushort `page<<8|usage`, e.g. Z=0x0132), all values
  normalized **[0,1]** (sticks 0.5 center, HID convention Y+ = down —
  XInput Y must be flipped; triggers 0 = released); `Buttons` = `[Flags]`
  uint (bits 0..12 named A..Share; bits beyond map to descriptor button
  positions, so 128-button profiles work through the same mask); `Hat` =
  8-way enum. Extended fields: touchpad fingers (0..1919/0..1079 + 7-bit
  tracking ids + packet counter), gyro/accel (deg/s ×16, g ×8192, DS5
  sensor timestamp in 0.33 µs ticks = µs×3), battery (tenths + charging),
  mic-mute, headphones.

### 3.2 Lifecycle

Order matters (learned via their preflight comments):

1. `HMContext.RemoveAllVirtualControllers()` — sweep stale device nodes
   from crashed sessions FIRST, or `InstallDriver`'s
   RemoveOldDriverPackages fails with "device using INF".
2. `new HMContext()` → `LoadDefaultProfiles()` (returns count) →
   `InstallDriver()` (requires elevation — PadForge runs elevated always).
3. Per slot: `ctx.CreateController(profile)`; **publish PID FFB pool/state
   BEFORE the device enumerates** if the descriptor carries a PID block
   (DirectInput issues GetFeature(PidPool) during CreateEffect — lazy init
   on first output packet is too late). PID-block detection: descriptor hex
   contains `09 21 A1 02` (Usage Set Effect Report + Logical Collection).
4. Teardown: dispose dispatchers before `controller.Dispose()` (final
   OutputReceived events fire during dispose); park feedback index at -1
   first so late async callbacks no-op. `ProcessExit` hook calls
   `RemoveAllVirtualControllers()` as crash insurance.

Two contexts is their pattern: a metadata-only `HMContext` for the UI's
profile catalog (never installs, never creates devices) and the engine's
live context in Step 5.

### 3.3 Report submission path & the watchdogs

Hot path: per-tick (1 kHz) map XInput-convention state → the profile's
wire axes. Two traps they hit that any Rust client must dodge:

- **AxisMap is authoritative, not positional convention**: Sony profiles
  declare Z→rightStickX, Rx→leftTrigger etc. (inverse of XInput's
  rightStick=Rx/Ry, triggers=Z/Rz). Routing by "standard axes" produced a
  phantom 50% L2/R2 on every PlayStation output (center 0.5 landed on the
  trigger byte → OS reads 0x80 → auto-asserts the coupled digital button).
  Resolve roles through the profile's AxisMap (keys are hex HID usages,
  2-digit keys promoted to page 1).
- **Trigger writes must be mirrored** to both the canonical position and
  the trigger row's wire-field key: HM's HID vs GIP/XUSB lanes disagreed
  across SDK generations about which position they read, and one-sided
  writes pinned XInput/WGI triggers at 50% (their discussion #130).

**Idle dedup with a 16 ms keepalive** — the number is derived, not chosen:
identical frames may be skipped (the driver latches state), but three
driver watchdogs bound how long SeqNo may sit still — 500 ms without an
event signal recycles handles; >250 event-signals-without-SeqNo-advance
recycles; and the GIP companion tears down the mapping and **zeroes XInput
state** after >500 consecutive unchanged-SeqNo READS, a count driven by
reader rate (8 ms pump + every XUSB GET_STATE), not time. A 250 ms
keepalive caused one-frame releases of held buttons under heavy consumer
mixes; 16 ms tolerates ~31k reads/s and still cuts idle submits ~94% at
1 kHz. Changes always submit same-tick.

Sony extras ride either `SubmitRawReport` (USB Report 0x01 full layout,
submitted after the gamepad state so GIP stays consistent) or the extended
SubmitState (BT Report 0x31 vendor blob).

### 3.4 Feedback path (rumble/LED/FFB back)

Two events:

- `OutputDecoded` (driver-parsed): Sony `leftMotor`/`rightMotor` bytes +
  a pre-stripped `sdlPassthrough` byte[] (47 B DS5 / 31 B DS4, already
  USB-form regardless of BT framing+CRC) forwarded verbatim to the physical
  pad via `SDL_SendGamepadEffect`. **Trust gate** (their 2026-07-25 audit,
  matches linux-hid hid-playstation.c): motors valid only if declared
  report size present (>= not ==; Windows sizes BT host writes to the
  LARGEST declared output report — 547 B — clamped to the driver's 256 B
  slot, so equality never held on BT), CRC valid, AND validFlag0 asserts
  the motor bit (DS4 mask 0x01, DS5 0x03). Failing the gate = preserve
  previous motors (a lightbar-only report means "ignore", not "stop").
- `OutputReceived` (raw packets, `HMOutputSource` ∈ XInput / HidOutput /
  HidFeature):
  - XInput IOCTL: `[00, 08, leftHi, rightHi, reserved]`; 7+ bytes appends
    impulse-trigger magnitudes at offsets 4/5 (XINPUT_VIBRATION_EX);
    a 5-byte packet zeroes trigger motors. Chromium sends dual-rumble as a
    hi=127/hi=0 square wave — don't filter zeros.
  - Xbox Series BT short HID (len 4..7): `[trigL, trigR, motorL, motorR,
    dur, delay, loop]`, motors 0..100 (scale ×655) — SDL's XboxOne rumble
    payload minus its 2-byte header.
  - Xbox legacy HID (len ≥8): motors at bytes 5/6.
  - PID FFB: full HID-PID decode (Set Effect/Constant/Periodic/Condition,
    Effect Operation, Block Free, Device Control/Gain) via their
    `HMaestroFfbDecoder`, plus a per-tick `ApplyIfDue` because **effect
    durations expire on the device clock** — without a per-tick pass the
    last vibration latches forever when the game goes quiet (their Jedi
    Outcast stuck-rumble bug).
  - Sony vendor feature 0x80 (dualsense-tester's sine/calibration
    commands) is forwarded to the physical pad; GetFeature responses are
    served synchronously by the driver, no deferred path.

### 3.5 Profile selection & the catalog

225+ embedded profile JSONs, enumerated via `LoadDefaultProfiles`.
PadForge buckets them by vendor-prefix + name-token filters (Microsoft +
"Xbox"; Sony + "DualShock/DualSense"; exact "switch-pro"; everything else →
Extended), filters out profiles without a captured HID descriptor
(`IsDeployable` — CreateController throws on those), and supports
user-imported profile JSONs staged through a temp dir into the same loader.
Default slugs: **`xbox-series-xs-bt`** for Xbox slots — deliberately not
xbox-360-wired, because **browsers on WGI/GameInput paths don't route FFB
to the X360 XUSB companion** (silent vibration in web gamepad testers);
the Series BT profile takes the HID output path browsers drive reliably.
`dualshock-4-v2` for PlayStation; a synthetic "Custom" profile
(descriptor built at runtime: 2 sticks/2 triggers/hat/11 buttons + PID FFB
block) under the faux VID 0xBEEF (their in-app squat convention: PIDs
0xCA7x = synthetic input sources, 0xF0xx = synthetic outputs).

### 3.6 WGI double-input

Their answer is **HidHide, expanded correctly**: when hiding a mapped
physical pad, they expand the HID instance ID to the **base container plus
all sibling HID children** (`ExpandToBaseContainerAndChildren`), because
blacklisting only the SDL-visible interface leaves XInput/WGI seeing the
controller through the XUSB parent or other HID descendants (Xbox 360
wired = XUSB parent with multiple HID children). BT transport forms differ
(`VID_` underscore vs `VID&` ampersand for BT Classic) and the naive
substring check missed BT DualSense entirely. No WGI-side workaround exists
for the *virtual* pad — the ui-lessons doc's "re-test WGI double-input"
note stands, but the physical-side hiding recipe above is the load-bearing
half for splitter scenarios.

*ksx M8 consequences*: the Rust client is an FFI (or reimplementation)
against an MIT SDK whose contract is now well mapped: context →
install-with-preflight-sweep → per-controller shared-section submit at our
tick rate with 16 ms keepalive dedup, AxisMap-driven axis routing, mirrored
trigger writes, PID pool published before enumeration, and the feedback
decode table above. Capture these as M8 acceptance tests.

## 4. The web controller (phones as gamepads)

`PadForge.App/Services/WebControllerServer.cs` +
`PadForge.Engine/Common/WebControllerDevice.cs` + `PadForge.App/WebAssets/`.

- **Server**: `System.Net.HttpListener` on `http://+:8080/` (wildcard bind
  → needs elevation/urlacl; they detect error 5 and message it), dedicated
  accept thread, `MaxClients = 16`. Static assets are **embedded
  resources** (no files to deploy); a `netsh advfirewall` rule is added on
  start. Discovery is manual: they derive the LAN IP via the
  UDP-connect-to-8.8.8.8 trick and display `http://ip:8080` as text. No
  mDNS, no QR.
- **Transport**: WebSocket upgrade (any path!), **JSON text frames**.
  Phone→PC: `{type:"input", kind:"button"|"axis"|"pov", code, value}`
  (axes 0..65535, codes 0=LX..5=RT; POV centidegrees or -1) and
  `{type:"touchpad", finger, x, y, down}`. PC→phone:
  `{type:"connected", padId, name}` and `{type:"rumble", left, right}` →
  browser Vibration API pulse (intensity × 200 ms). Per-session send
  semaphore (managed WebSocket forbids overlapped sends).
- **Latency strategy: none needed** — purely event-driven sends on touch
  events, with dedup only for stick (last-value) and POV (changed). No
  rAF batching, no fixed tick. For 16 phones of casual input this is fine;
  lesson: don't over-engineer the phone→PC lane, DO throttle the PC→phone
  echo lane (see §1.3).
- **Engine integration**: the web client materializes as an
  `ISdlInputDevice` peer of physical devices (VID 0xBEEF / PID 0xCA7E,
  `web://{clientId}` device path, GUIDs derived by hashing —
  **per-layout** product GUIDs so a reconnect can't migrate an xbox360 row
  onto a ds4 client). Copy-on-write state: mutators clone-and-swap under a
  lock, the 1 kHz reader takes the reference lock-free. Register/unregister
  via `InputManager.RegisterExternalDevice` — the pipeline neither knows
  nor cares it's a phone.
- **Security: none.** No PIN/token/TLS; client id is self-asserted
  sessionStorage UUID; any LAN device that reaches the port is a
  controller. Notably their own **Remote Link** feature (PC↔PC input
  streaming, `PadForge.Engine/RemoteLink/`) has the real model: identity
  keys, pairing approval, trust store, anti-replay window, identity
  protection modes. The asymmetry is the lesson: **ksx Studio's LAN mode
  must start at Remote-Link-grade pairing (pair-once + token), not
  web-controller-grade open ports** — ksx Studio is a *config* surface,
  strictly more sensitive than a gamepad.
- **Phone UI**: chooser page → controller page driven by a served
  `/api/layout` JSON (base image + %-positioned overlays with input codes
  — same layout-table idea as §1.4, serialized); bundled nipplejs for
  sticks (static mode, multitouch, synthesized L3/R3 on quick tap);
  single-zone 8-way d-pad via atan2 sectors (reads `changedTouches`, not
  `touches` — multi-finger bug they fixed); hit zones inflated ~40% for
  thumbs; landscape lock via CSS; reconnect-on-tap; an iOS
  reload-once workaround for WebSocket-after-navigation failures.

*ksx fit*: "phone as config remote / mapper display" (our E-idea) can lift
the architecture wholesale — embedded assets, WS JSON, device-as-peer —
with pairing from their Remote Link column, not their web one.

## 5. Dashboard / diagnostics ideas

`DashboardViewModel` + `DashboardPage.xaml`: one vertically scrolled page.

- **Input Engine card**: engine power toggle (flame glyph, four states:
  running=ember+drop-shadow glow, idle=gold, stopping=gold flashing,
  stopped=outline), status text, **polling rate**, online/total device
  count. Polling rate is honest and cheap: a tick counter over a 1 s
  stopwatch window on the poll thread → `{0:F1} Hz`, forced to 0 (shown as
  "—") on stop/idle/suspend, published on the 30 Hz UI lane.
- **Per-slot cards**: power + delete + type segment (6 VC types) +
  device roster with battery glyph (Segoe glyph buckets by 10%) and BT
  marker + a **6-entry "stage ledger"** (Sticks / Triggers / Gyro /
  Lighting / Touchpad / Audio — lit ember when that pipeline stage is
  active for the slot, tooltip shows detail lines). Drag-to-reorder.
- **Services cards** (uniform shape: description, enable, port +
  reset-to-default button, flame status): DSU motion server (26760), web
  controller (8080), Remote Link (27500), overlays.
- **Driver status**: just two rows (HidHide, MIDI Services), probed in
  MainWindow on a 5 s timer; versions shown on Settings, not Dashboard.
  No HIDMaestro version row (it's an in-process library), no latency/
  jitter display anywhere.

*ksx status page adoption list*: 1 s-window Hz counter per input source and
per virtual pad (we can also show per-device event rate — PadForge can't);
the four-state engine glyph with "stopping" as a distinct visible state;
per-pad stage/activity ledger (for ksx: capture → map → inject lanes);
driver-health rows with install/version + a reset-port affordance next to
every port field. Skip: their services sprawl; keep one screen.

## 6. Gaps ksx can beat (observed, not copied)

- No binding-conflict detection (§1.2) — cheap win for Studio.
- No countdown timer during record — a 10 s silent timeout surprises;
  show remaining seconds.
- No latency/jitter telemetry despite a 1 kHz engine — ksx already thinks
  in polling terms; a p99 tick-to-inject figure would out-diagnose them.
- Web controller has zero auth (§4) — table stakes for us.
- 12k-line page XAML and a 7k-line macro VM — the monolith trajectory to
  avoid; Forma islands keep us honest.

## Sources

- Clone: `C:\Users\Victor\controller-project` (read-only; not modified).
- Key files cited: `PadForge.App/Common/Input/HMaestroVirtualController.cs`
  (1,348 lines), `HMaestroProfileCatalog.cs`,
  `InputManager.Step5.VirtualDevices.cs`, `Services/RecorderService.cs`,
  `Services/WebControllerServer.cs`, `Models2D/ControllerOverlayLayout.cs`,
  `PadForge.Engine/Data/*.cs`, `PadForge.Engine/Common/Mapping/*.cs`,
  `ViewModels/{MappingItem,MacroItem,DashboardViewModel}.cs`,
  `docs/ui-exposure-ledger.md`.
- Asset pack: https://github.com/AL2009man/Gamepad-Asset-Pack (MIT,
  fetched 2026-08-05).
- HIDMaestro upstream: https://github.com/hifihedgehog/HIDMaestro (MIT).
