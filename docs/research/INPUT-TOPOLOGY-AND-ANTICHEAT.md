# Input topology and anti-cheat — three questions answered (2026-08-07)

Three questions came in together, and they turn out to share one mechanism:
**what a program on Windows can and cannot tell about where an input came
from.** A real gamepad, an injected keystroke and a ViGEm pad all end up in the
same buffers, but they arrive by different routes, and every answer below is
decided by which route the consumer is reading.

The three:

1. Can a real controller be merged with the arcade panel so both drive one
   virtual pad — and can a pad be turned into keystrokes that feed ksx?
2. Does MAME only support DirectInput / does it need keys *sent* to it?
3. Would ksx give a keyboard controller aim assist in Call of Duty, and does
   ViGEmBus get accounts banned?

Section 3 is the one that can hurt someone. It separates three grades of
evidence and **never merges them**; the distinction is the whole value of the
section. See the provenance key at the top of it.

---

## 1. Merging a real controller with the arcade panel

### 1.1 The sketch as drawn is not wireable

The idea was `real controller → I-PAC → ksx → virtual pad`. That first arrow
cannot exist. An I-PAC is a **switch-closure encoder**: its inputs are screw
terminals, one side of each button to a terminal, the other side to ground —
Ultimarc's own wording is "Connect one side of each switch to the screw
terminals as indicated on PCB", with "All button inputs referenced to ground"
([I-PAC Ultimate I/O](https://www.ultimarc.com/control-interfaces/i-pacs/i-pac-ultimate-i-o/)).
There is one USB port and it is a *device* port facing the PC. There is no USB
host port, so nothing can be plugged **into** the board — least of all a
gamepad, which speaks USB HID and not "two wires shorted together".

The only chaining Ultimarc documents is board-to-board: "I-PAC4 and I-PAC2
boards and J-PAC can be paired in any combination to increase the total number
of inputs"
([I-PAC4](https://www.ultimarc.com/control-interfaces/i-pacs/i-pac4-board/)).
That adds screw terminals, not a controller port. The cabinet's board is
`VID_D209`/`PID_0430`, which ksx's own vendor table names *Ultimarc I-PAC 4X*
(`crates/ksx-core/src/vendors.rs:58`) — an I-PAC4-class board, screw terminals
only.

One nearby capability should not be confused with this, because it looks
similar and *is* real: the I-PAC is multi-mode firmware and can present itself
as gamepads instead of a keyboard — I-PAC4 does "quad gamepad/mouse or quad
Xinput controllers", Ultimate I/O does "dual gamepad/mouse/LEDs or Dual Xinput
controller". That gets the *panel* to a game as a pad with ksx entirely out of
the loop. It still does not merge anything with a real controller.

### 1.2 The shape that does work is a fan-in, not a chain

```
   [real pad] ─┐
               ├─► merger ─► one virtual pad
   [I-PAC]   ──┘
```

Both sources feed one merger; the merger drives one pad. Every product that
does this does it this way. ksx cannot be that merger today, and the blocker is
larger than "it only reads keyboards": ksx's entire *input vocabulary* is
digital. The capture event is `KeyEvent { device, key, down, t }`
(`crates/ksx-core/src/device.rs:55-64`) — transitions only — while the output
side already carries analog (`PadState` with `lx/ly/rx/ry/lt/rt`,
`crates/ksx-core/src/pad.rs:246`). A real stick has nowhere to land in that type.

### 1.3 Where you can have this today, with no code at all

**MAME already does it natively, and this is the cabinet's main consumer.**
MAME input assignments are sequences, evaluated as sum-of-products with
`not`/`and`/`or`, and the UI lets you *append* an or-operation instead of
replacing an assignment. The documented example is literally cross-device:
"Kbd Down or Joy 1 Down — Pressing the down arrow on the keyboard or moving the
first joystick down activates the emulated input"
([MAME UI docs](https://docs.mamedev.org/usingmame/ui.html);
[input system techspecs](https://docs.mamedev.org/techspecs/inputsystem.html)).
So in MAME: bind the panel button, then bind the controller button as an
alternative, and one emulated input answers to both. Nothing from ksx required.

**reWASD already ships exactly the described feature** for everything else: "A
group of devices permits you to assemble 2, 3 or 4 devices into one group… The
classic alliance is keyboard, mouse, and controller, but you can combine any
devices you wish", and "All the devices in one group will represent one Virtual
controller"
([reWASD device groups](https://help.rewasd.com/how-to-remap/group-of-devices.html)).
A licence is the cheapest possible test of whether the ergonomics are even
enjoyable, before any Rust gets written.

**RetroArch does not support it**, so neither do RetroBat's RetroArch cores.
"Multiple gamepads mapped to one RetroPad" is an open, bounty-tagged feature
request, not a feature
([libretro/RetroArch#7830](https://github.com/libretro/RetroArch/issues/7830)).

### 1.4 The two halves of doing it inside ksx, and which one is expensive

**Reading a pad is the cheap half.** Raw Input is the natural path because most
of the plumbing exists: `crates/ksx-capture/src/rawinput.rs` already runs a
message-only window with `RIDEV_INPUTSINK`, calls `GetRawInputData`, and takes
per-device identity from `GetRawInputDeviceInfoW`/`RIDI_DEVICENAME` — the same
device-interface-path identity model ksx uses everywhere. Extending it means
registering `RIM_TYPEHID` usages alongside the keyboard usage and parsing
reports with `HidP_*`. Microsoft blesses the route explicitly: "You can also
access HID input devices via RawInput API and process input reports via low
level HID API but vibration feedback will not work as with DirectInput"
([GDK input FAQ](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-faq?view=gdk-2510)).

The fan-in shape is nearly free too: `CompositeBackend` already merges several
capture backends into one session — one event stream, one control channel, one
escape latch, one health snapshot, with every child holding a clone of the same
`Sender` (`crates/ksx-capture/src/composite.rs:1-30`). A gamepad reader is
another child, not a new architecture.

Two read paths to *avoid*:

- **XInput.** Capped at four slots, and ksx has already measured on this exact
  machine that its slot identity is unreliable — `get_user_index()` returned
  `[0, 0, OutOfRange, OutOfRange]` while active correlation showed the pads were
  really in slots 2 and 3, LED notifications "wrong when present, usually
  absent" (`docs/research/m2-xinput-findings.md:9-27`). Worse, ksx's *own*
  ViGEm pads occupy XInput slots (`MAX_XINPUT_SLOTS` in
  `crates/ksx-config/src/validate.rs:741-748`), so an XInput reader would be
  reading a namespace containing ksx's own output with no reliable way to tell
  a real pad from a virtual one. That is a feedback loop waiting to happen.
- **GameInput**, unless the deployment cost is accepted deliberately. It is
  Microsoft's current recommendation and a strict superset — "a functional
  superset of all legacy input APIs—XInput, DirectInput, Raw Input, Human
  Interface Device (HID), and WinRT APIs", back to Windows 10 19H1 — and it
  hands back an interface path in `GameInputDeviceInfo`, which solves the
  identity problem XInput cannot. The price is a redistributable ("include that
  redistributable (or newer) as part of your game's installation process to end
  users") plus a service that "arbitrates access to input devices"
  ([same FAQ](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-faq?view=gdk-2510)).
  For a tool that currently installs cleanly, that is a real regression.

**Hiding the real pad is the expensive half**, and it is not optional. reWASD
states it as mandatory: "If you add a Virtual controller mapping, reWASD needs
to hide the physical gamepad from the system, and create an emulated one
instead. If you don't hide the physical gamepad, the mappings may interfere and
cause unpredictable consequences"
([reWASD virtual controller](https://help.rewasd.com/basic-functions/virtual-controller.html)).
Otherwise the game sees both the real pad and the ksx pad and reads double.

Two routes, both with sharp edges:

1. **HidHide.** A KMDF filter driver with a blacklist of device instance paths
   and a whitelist of applications by image name; "when enabled it blocks access
   to the black-listed devices unless the application is explicitly
   white-listed". It exposes an IOCTL API, a scriptable `HidHideCLI.exe` and a
   managed wrapper, so ksx could drive it programmatically rather than telling
   the user to go clicking ([nefarius/HidHide](https://github.com/nefarius/HidHide)).
   **But do not promise that it hides an Xbox pad.** Its stated scope limit is
   the HID layer, and "legacy applications that bypass this layer and interact
   directly with underlying device drivers" are outside its control — and
   nefarius' own tracker carries an open issue, *"Xbox/XInput blocking through
   UI Client broken"*, asking why Xbox controller blocking "is either not
   working at all or not reliably"
   ([HidHide#39](https://github.com/nefarius/HidHide/issues/39)).
   The mechanism behind that is documented by Microsoft: "The XUSB driver
   implements both an XUSB class interface and a HID class interface for devices
   in order to support both XINPUT and DirectInput usage"
   ([DirectInput and XUSB devices](https://learn.microsoft.com/en-us/windows/win32/xinput/directinput-and-xusb-devices)).
   Hiding the HID interface removes the pad from DirectInput and Raw Input, but
   `XInputGetState` talks to the *other* interface and still sees it. A
   DS4/DualSense/generic HID pad has no second path and is the easy case.
2. **WinUSB-claim the pad**, the way ksx already claims the I-PAC. Once an
   interface is bound to `winusb.sys` it has left the driver stack entirely —
   ksx's own module doc puts it as "Windows does not see the board. Not 'ksx
   suppresses its keystrokes' — there is no keyboard there to suppress"
   (`crates/ksx-capture/src/winusb/mod.rs:19-24`). Applied to a gamepad that is
   *structural* hiding with no third-party kernel driver, reusing machinery ksx
   already ships. It is cheap only for HID pads, though: Xbox controllers are
   not HID on the wire — the Linux xpad docs show the interface as
   `Cls=58(unk.) Sub=42 Prot=00`, a vendor-specific class rather than HID class
   `0x03` ([xpad](https://docs.kernel.org/input/devices/xpad.html)) — so an Xbox
   pad means decoding XUSB/GIP reports yourself. And a **Bluetooth** pad has no
   USB interface to claim at all, so this route simply does not exist for
   wireless.

Practical consequence worth asking *before* any work starts: if this is wanted
for an Xbox controller specifically, is it **wired or wireless**, and does the
target game read XInput? Wireless + XInput is the one combination where neither
hiding route works cleanly. That should change the answer up front, not be
discovered three days in.

### 1.5 The reverse direction, and the loop question

Gamepad-to-keystroke remappers (JoyToKey, antimicroX) are a solved category
implemented one way: read the pad, then synthesize keys with `SendInput`.
antimicroX makes it literal — its Windows backend file is
`winsendinputeventhandler.cpp`
([antimicrox/src/eventhandlers](https://github.com/AntiMicroX/antimicrox/tree/master/src/eventhandlers)).
Microsoft: `keybd_event` "has been superseded. Use SendInput instead."

**Can ksx capture its own injected keys?** No, on any backend.

- WinUSB: no feedback path exists — "the claimed interface cannot see injected
  input" (`crates/ksx-platform/src/inject.rs:52-56`).
- Interception: a keyboard *filter* driver never sees `SendInput` traffic at
  all. Confirmed by kernel-driver developers rather than by our own comments —
  Alex Grig: "The injected events don't go through keyboard driver", with Doron
  Holan (Microsoft) confirming `KEYBOARD_INPUT_DATA` carries no injected flag
  because the question does not arise
  ([OSR NTDEV](https://community.osr.com/t/keyboard-filter-driver-identify-injected-input-sendinput/52509)).
- Raw Input: `rawinput.rs` already discards packets with a null `hDevice`.
- Belt and braces regardless: every injected event is stamped
  `KSX_EXTRA_INFO = 0x0000_6B73_7801` (`inject.rs:66`).

**And here is the sting in that answer**: the same property that prevents a loop
prevents the *chain*. Because no ksx backend can see `SendInput` traffic,
`real pad → JoyToKey → keystrokes → ksx → virtual pad` **cannot work**. Only a
`WH_KEYBOARD_LL` hook sees injected keys (via `LLKHF_INJECTED`), and ksx
deliberately has no low-level-hook capture path — kanata is the tool that took
that route (`docs/research/keyboard-capture-2026.md:56`). There is no
self-capture loop to fear, and equally no chain to build.

---

## 2. What MAME actually reads

### 2.1 The belief, corrected

The belief was that MAME "only supports DirectInput" and therefore "needs keys
sent to it". It is wrong as stated but garbles something true.

Current MAME on Windows offers `auto|rawinput|dinput|win32|sdl|none` for
keyboards and mice, and `auto|winhybrid|dinput|xinput|sdlgame|sdljoy|none` for
controllers. DirectInput is present everywhere, and it is *half of the default
controller path*: Windows `auto` = `winhybrid`, which "uses XInput for
compatible game controllers, falling back to DirectInput for other game
controllers"
([MAME commandline reference](https://docs.mamedev.org/commandline/commandline-all.html)).
`winhybrid` spots XInput-compatible pads by the `IG_` marker in the PnP device
ID and logs "Skipping DirectInput for XInput compatible joystick" so nothing is
enumerated twice
([input_winhybrid.cpp](https://github.com/mamedev/mame/blob/master/src/osd/modules/input/input_winhybrid.cpp)).

### 2.2 The kernel of truth: MAME's default keyboard path rejects injected keys

MAME does **not** read the keyboard through DirectInput or Win32 messages by
default. It uses Raw Input, and Raw Input drops software-injected keystrokes.
MAME's docs say so directly: "user-mode keyboard emulation tools such as joy2key
will almost certainly require the use of `-keyboardprovider win32` on Windows
machines".

The mechanism is the same null-handle test ksx relies on for its own loop
safety. MAME's Raw Input module bails on an event with no or unknown device
handle (`if (!input->header.hDevice) return false;`, plus a
`devicelist().end() == target_device` bail), and injected keys arrive with a
null handle. It registers `RIDEV_DEVNOTIFY | RIDEV_INPUTSINK`, so foreground
focus is *not* the issue — **device identity** is
([input_rawinput.cpp](https://github.com/mamedev/mame/blob/master/src/osd/modules/input/input_rawinput.cpp)).
The same failure shows up in the wild: Steam Controller users found MAME
ignoring its keyboard emulation because "MAME uses RAW input by default, which
only accepts inputs from Windows drivers", fixed with
`keyboardprovider win32` / `mouseprovider win32` / `joystickprovider xinput`
([Steam community thread](https://steamcommunity.com/app/353370/discussions/0/350541595120301147/),
community-reported).

So the belief is backwards: MAME wants **real devices**, and it is the
key-injectors that need MAME reconfigured.

### 2.3 ksx's virtual pads work in MAME with no configuration

Verified against the cabinet's own MAME v0.288: `mame.exe -noreadconfig
-showconfig` prints `joystick 1` (with `mouse 0`, `lightgun 0`), and
`C:\RetroBat\emulators\mame-modern\mame.ini` leaves `keyboardprovider auto` and
`joystickprovider auto`. **MAME's documentation is stale here** — it claims
"The default is OFF (`-nojoystick`)" while both the source and the shipped
binary say on. The reliable check to quote in docs is `mame -showconfig`, not
the manual.

MAME 0.252 overhauled controller handling ("better default input assignments
for more controllers", [0.252 release notes](https://www.mamedev.org/?p=522)),
which is why a modern virtual pad works with no setup where a 2015-era MAME
would not. The XInput module builds defaults in a fixed order — A, B, X, Y, then
LT/RT *only* for arcade-stick-class subtypes (`lt_rt_button`), then LB, RB, then
the stick clicks — so on a standard gamepad subtype you get BUTTON1=A,
BUTTON2=B, BUTTON3=X, BUTTON4=Y, BUTTON5=LB, BUTTON6=RB, with the analog
triggers left as axes and the shoulders doubling as UI prev/next. Start maps to
`IPT_START`, Back to `IPT_SELECT`, and devices are named "XInput Player N"
([input_xinput.cpp](https://github.com/mamedev/mame/blob/master/src/osd/modules/input/input_xinput.cpp)).
Back really does insert a coin, because MAME's core default for COIN1 is
`input_seq(KEYCODE_5, input_seq::or_code, JOYCODE_SELECT_INDEXED(0))`
([inpttype.ipp](https://github.com/mamedev/mame/blob/master/src/emu/inpttype.ipp)).

Two gotchas that do bite:

- **Joystick numbering is not stable.** MAME's own docs: "a game pad may be
  assigned to 'Joy 1' initially, but after restarting, the same game pad may be
  reassigned to 'Joy 3'", caused by enumeration order across replug, hub changes
  and reboots. The documented fix is a ctrlr file with
  `<mapdevice device="XInput Player 1" controller="JOYCODE_1" />`, with device
  IDs discovered via `mame -v`
  ([devicemap](https://docs.mamedev.org/advanced/devicemap.html)). Since Joy N's
  defaults land on player N's inputs, drifting numbers scramble a 4-player
  cabinet.
- **XInput caps at four.** Slots 5–8 must use the DS4/HID persona, which
  `winhybrid` picks up over DirectInput — already the plan in
  `docs/research/m6.5-ds4-findings.md:31`.

### 2.4 The uncomfortable finding: for MAME alone, keyboard mode beats ksx

The I-PAC's factory chart **is** MAME's default chart. Ultimarc: the board
"operates in keyboard mode and contains a pre-loaded code set. This matches the
MAME default key codes"
([I-PAC2](https://www.ultimarc.com/control-interfaces/i-pacs/i-pac2/)); MAME's
[default keys](https://docs.mamedev.org/usingmame/defaultkeys.html) are 5/6 =
coin 1/2, 1/2 = start 1/2, arrows = P1 stick, LCtrl/LAlt/Space/LShift/Z/X = P1
buttons 1–6, with P2 on G/D/R/F/A/S/Q/W — the same keys ksx's `arcade-6button`
template already uses. Panel button N = MAME button N for four players, with
zero configuration.

Routing the same panel through ksx's `arcade-6button` template into MAME's
default XInput assignments **scrambles that order and loses a button**
(`crates/ksx-core/src/templates.rs:248-278` read against `input_xinput.cpp`):

| Panel button | ksx template binds | MAME sees |
|---|---|---|
| 1 (LCtrl) | `X` | BUTTON3 |
| 2 (LAlt) | `Y` | BUTTON4 |
| 3 (Space) | `RightBumper` | BUTTON6 |
| 4 (LShift) | `A` | BUTTON1 |
| 5 (Z) | `B` | BUTTON2 |
| 6 (X) | `Trigger::Right` | **an analog axis — no numbered button** |
| 7 (C) | `LeftBumper` | BUTTON5 |

Six-button fighters therefore lose heavy kick outright and get a jumbled layout
until someone remaps. This is a silent failure today, and it is the strongest
argument in this whole document for shipping the ctrlr export in §4.

Also worth knowing: MAME's own notion of "arcade panel support" is a keyboard
remap file, not a gamepad. The shipped `ctrlr` directory on this cabinet
contains `xarcade.cfg`, `hotrod.cfg`, `hotrodse.cfg`, `slikstik.cfg` and
`scorpionxg.cfg` — all `KEYCODE` remaps plus UI port overrides. And the I-PAC
firmware itself already offers "Keyboard/mouse, dual gamepad/mouse and dual
Xinput controller", switched by holding Start1 plus a button for ten seconds,
with Ultimarc recommending "Dinput… unless the application only works with
Xinput". Note **dual** — the hardware path caps at two pads per board, against
ksx's eight.

### 2.5 WinUSB plus MAME is a concrete break, not a theoretical one

ksx's typethrough is `SendInput`-based and already documents the risk: injected
strokes "will [be rejected by] some games' raw-input paths, because `SendInput`
never produces a `WM_INPUT` from a real HID device"
(`crates/ksx-platform/src/inject.rs:34-39`). Combined with §2.2 this stops being
a caveat and becomes a prediction: **with the panel WinUSB-claimed, injected
keys will not reach MAME** unless the user sets `-keyboardprovider win32` (or
`dinput`). Under Interception, keys ksx does not block remain genuine device
input and MAME sees them normally — which is exactly the difference that matters
for the in-progress per-key passthrough work (task #12): it works for MAME on
Interception and silently does not on WinUSB.

---

## 3. Anti-cheat, aim assist, and which driver is actually the problem

### Provenance key — read this before quoting anything below

Three grades of evidence appear here and they are **not** interchangeable.
Collapsing them is how a document like this becomes dangerous.

| Grade | Means |
|---|---|
| **Vendor-documented** | A publisher or driver author states it in writing, in public, under their own name. Citable. |
| **Community-reported** | Forum/user reports, plausible and repeated, with **no** primary confirmation. Directional evidence, not a fact. |
| **Inference** | Our reasoning over the above, or over this repo. Could be wrong; nobody has confirmed it. |

Everything below carries its grade inline. If a claim has no grade, it is a
statement about this repository, not about anyone's policy.

### 3.1 Call of Duty: this is a policy question, not a detection question

**Would ksx produce controller aim assist in CoD?** Very likely yes. CoD grants
aim assist by input type, and ViGEmBus produces a genuine XInput device through
Microsoft's own `xusb22.sys` rather than an imitation of one.

**Would that get the account banned? Yes, and this is the strongest single
finding in the document — vendor-documented, not folklore.** Activision's
[Call of Duty Security and Enforcement Policy](https://support.activision.com/articles/call-of-duty-security-and-enforcement-policy)
lists **"input mapping software"** verbatim among prohibited unauthorized
third-party software: "This includes, but is not limited to, aimbots, wallhacks,
trainers, stats hacks, texture hacks, leaderboard hacks, injectors, **input
mapping software**, or any other software used to deliberately modify game data
on disk or in memory." ksx is input mapping software by its own description. No
detection question needs answering for the policy to already cover it.

The penalty ladder in that same policy (**vendor-documented**) runs warnings →
temporary bans → permanent bans "lasting across past, present, and future Call
of Duty titles", plus matchmaking limits, Ranked Play restrictions, and
**hardware bans applied at the device level rather than the account**. A
hardware ban on a cabinet's motherboard is not fixed by making a new account.

Detection has been shipping against this exact scenario since January 2024
(**vendor-documented**): "Our security detection systems now target players
using tools to activate aim assist while using a mouse and keyboard. The Call of
Duty application will close if detected. Repeated use of these tools may lead to
further account action"
([reported statement](https://www.techspot.com/news/101549-call-duty-anti-cheat-system-now-close-game.html)).
Note that this names a *behaviour* — keyboard-and-mouse obtaining aim assist —
not a product, so being a hobby tool places ksx inside the scope, not outside
it.

The closest precedent to ksx is reWASD, which does the same thing ksx does and
was the named casualty. reWASD's own blog concedes rather than contests: "we do
not recommend buying reWASD for such games as the Call Of Duty series or Apex
Legends", while arguing "Notorious aim assist isn't a reWASD feature and never
was; it is provided by the games themselves"
([reWASD](https://www.rewasd.com/blog/post/cancel-culture), **vendor-documented**
as a statement by reWASD). That argument is equally available to ksx and it did
not work.

The accessibility framing will not help either (**vendor-documented**): "these
devices masquerade as accessibility devices… But these devices are not here to
help with accessibility. They exist to give cheaters an edge", and "Unapproved
third-party devices like Cronus Zen and XIM Matrix have no place in Call of
Duty"
([RICOCHET Season 02](https://news.blizzard.com/en-us/article/24243445/teamricochet-season-02-update)).
ksx's accessibility and couch-co-op motives are genuine; this vendor has
pre-rejected them as an exemption.

**Community-reported and NOT vendor-confirmed:** that CoD refuses to launch when
reWASD is merely *installed* and not running
([reWASD forum](https://forum.rewasd.com/forum/rewasd/technical-questions-aa/241209-rewasd-is-now-banned)).
Multiple forum reports say so; no primary Activision confirmation of
presence-based detection was found. Treat the mechanism as unknown — the Season
02 post affirmatively describes detection as behavioural.

### 3.2 ViGEmBus: nobody documents a position, in either direction

Searched specifically for this and found nothing: **neither Epic/EasyAntiCheat,
BattlEye, Riot Vanguard nor RICOCHET publishes any policy, blocklist entry or
statement naming ViGEmBus.** EAC enforcement is configured per-title by each
studio
([EAC docs](https://dev.epicgames.com/docs/game-services/anti-cheat/using-anti-cheat),
**vendor-documented**), so there is not even a single answer to "does EAC ban
ViGEmBus". Riot's public
["VAN: Incompatible Software"](https://support.riotgames.com/en-us/valorant/performance/van-incompatible-software)
article describes the mechanism generically and names none of these drivers.
No credible report of a ban attributed to ViGEmBus presence alone turned up
either.

**The widely-repeated claim that ViGEmBus is "explicitly whitelisted by several
major publishers" with "no documented bans" is unsourced (inference from
provenance).** It traces only to SEO content farms (`ds4-win.com`, `ds4win.com`,
`tech-insider.org`, `gamepadtester.pro`) with no primary citation
([example](https://ds4-win.com/is-ds4windows-safe-to-use-complete-security-guide/)).
**It must not be repeated in ksx's docs as reassurance.**

ViGEmBus's large legitimate install base does cut both ways
(**vendor-documented**, from its README): 19+ mainstream consumers including
Parsec, DS4Windows, Sunshine, Oculus, HP and 3dRudder, so a blanket
presence-based ban would break a lot of innocent users, which is a real reason
vendors avoid one. But the project has been **retired and read-only since
2 November 2023** ([ViGEmBus](https://github.com/nefarius/ViGEmBus)) — it will
never be patched to satisfy a future anti-cheat requirement, and a future block
would leave ksx with no upstream fix.

### 3.3 "Anti-cheat sees a normal Xbox pad" is true, and is not a safety argument

`docs/research/virtual-gamepad-2026.md:131` currently asserts that anti-cheat
sees a normal Xbox pad, because ViGEmBus creates a PDO matching Microsoft's
inbox `xusb22.sys` — so it *is* XInput rather than an emulation of it. That is
correct **about device enumeration** and says nothing about safety
(**inference**, and the correction below matters).

RICOCHET's Season 02 post is explicit that detection does not live at
enumeration (**vendor-documented**): "Our Season 02 detections focus on how
inputs behave, not which device is plugged in… We analyze input timing,
consistency, and response patterns to distinguish natural human play from
machine-modified input… these detections are built to recognize classes of
machine-driven behavior, even as configurations change"
([RICOCHET Season 02](https://news.blizzard.com/en-us/article/24243445/teamricochet-season-02-update)).

An attestation-signed driver with honest users buys nothing against timing
analysis, and ksx's synthesis of stick deflection from digital key transitions
is precisely the machine-driven input pattern described. A future reader — quite
possibly a future us — will cite that sentence as a green light unless it is
narrowed to what it actually supports.

### 3.4 The bigger practical exposure is Interception, not ViGEmBus

This is the finding with day-to-day consequences on a shared machine, and it is
vendor-documented rather than inferred. **FACEIT blocks the Interception driver
by name** as a "Forbidden driver", stating it "is used by cheats to generate
fake input, which is also the reason other anti-cheat programs block it"
([FACEIT blocked drivers](https://support.faceit.com/hc/en-us/articles/360014237259--Forbidden-driver-error-message-and-blocked-drivers)).

Because Interception is a system-wide `kbdclass`/`mouclass` upper filter driver,
it loads **at boot** — so those titles refuse to launch even when ksx is not
running. That is the part a user would never guess.

EAC is also reported to block it (**community-reported**): upstream issue
[oblitum/Interception#217](https://github.com/oblitum/Interception/issues/217) is
titled "Interception blocked by EAC", and `docs/research/keyboard-capture-2026.md:34`
already records this — but the finding never propagated into anything a user
reads.

**Inference, and a good one:** the planned M6 WinUSB migration removes this
exposure as a free side effect. `winusb.sys` is in-box and WHQL-signed, is not a
class filter, and claims exactly one arcade encoder interface rather than
sitting on every keyboard in the system (`docs/DRIVERS.md:201`). That is a
second, independent argument for M6 alongside the certificate deadline, and it
is currently unstated in the migration rationale.

### 3.5 Where the risk is genuinely zero — which is the cabinet

Activision confirms the RICOCHET kernel driver "will only operate when you play
a game using RICOCHET Anti-Cheat on PC. The driver shuts down when you exit the
game and turns on when you start a new game"
([RICOCHET overview](https://support.activision.com/articles/ricochet-overview),
**vendor-documented**). MAME, RetroArch, RetroBat and LaunchBox ship no
anti-cheat at all. ksx's primary documented topology — T1, one encoder to four
pads, offline — carries **no ban risk whatsoever**, and none of §3 applies to
it. The warning that follows is about competitive online titles only, and should
be written that way so it does not read as fear about the cabinet.

### 3.6 What ksx must not claim, and must not build

ksx currently ships **no user-facing anti-cheat warning anywhere**. A repo-wide
search of all Markdown for anti-cheat / RICOCHET / BattlEye / Vanguard / EAC /
aim assist / ban returns seven hits, all in internal architecture and research
docs (`docs/ARCHITECTURE.md:325` among them) — nothing in `README.md`,
`QUICKSTART.md` or `USE-CASES.md`, i.e. nothing anyone would actually read.

Two rules follow, and they pull in opposite directions on purpose:

- **Never write that ViGEmBus "is safe" or "is whitelisted."** No vendor
  documents a position either way, and the whitelisting claim has no primary
  source. The only wording that is true today *and* stays true if a vendor
  changes its mind next season is: no vendor documents this, so treat it as
  unknown.
- **Never inflate it either.** Offline emulator use has zero exposure (§3.5),
  and saying otherwise would be equally false.

**Explicit non-goal, worth writing down where it survives a future feature
request:** ksx will not implement detection evasion. No driver hiding marketed
as anti-cheat bypass, no serial or VID/PID randomization framed as defeating
fingerprinting, no humanized jitter or input-timing randomization aimed at
behavioural analysis, no "stealth mode". Beyond the ethics, it would reclassify
ksx from an accessibility and arcade tool into a cheat tool — which is exactly
the reclassification that got Interception blocked by FACEIT and EAC, and would
invite the same treatment of ViGEmBus, harming every honest user of both.

---

## 4. What this means for ksx

Nothing in this document was implemented; it is all queued work. Ordered by
value per unit of effort.

### Do now — documentation, no code

| # | Change | Why |
|---|---|---|
| 1 | `README.md`: add a short **"Online games and anti-cheat"** section near the top — a scope statement, not a footnote. Say that ksx is for local play; that it presents a keyboard as a real Xbox controller, which in a game that grants controllers aim assist is treated as the thing anti-cheat exists to stop; that Activision's policy lists "input mapping software" and penalties reach permanent cross-title bans; and that **other titles publish no comparable policy, which means unclear, not permitted.** | §3.1. Today a user has no way to learn this from ksx. |
| 2 | `docs/USE-CASES.md`: add **T8 — competitive online title with anti-cheat → ❌ out of scope by policy, not by capability; will not be supported or worked around.** | Makes the boundary structural under the existing "generality must not cost the primary use case" rule, instead of prose someone can miss. |
| 3 | `docs/DRIVERS.md`: factual compatibility note that the Interception backend installs a system-wide keyboard/mouse class filter that FACEIT blocks by name and EAC is reported to block, **while ksx is not running**, and that WinUSB or uninstalling Interception is the fix. Fold the same point into the M6 rationale as a second argument beside the cert deadline. | §3.4. More useful day to day than the CoD warning, and currently invisible. |
| 4 | `docs/research/virtual-gamepad-2026.md:131`: narrow "anti-cheat sees a normal Xbox pad" to "…**at enumeration**", with one sentence noting that behavioural detection operates downstream where this gives no protection. | §3.3. As written it will be cited as a safety guarantee it does not support. |
| 5 | `CONTRIBUTING`/`ENHANCEMENTS`: record the detection-evasion non-goal. | §3.6. It needs to outlive the conversation that produced it. |
| 6 | `QUICKSTART`/`USE-CASES`: state the honest MAME verdict — for MAME **alone**, leave the panel in keyboard mode and don't run ksx. Run ksx for MAME when you need more than two players as pads, cross-emulator uniformity, or per-key remap/fan-out the I-PAC firmware cannot do. | §2.4. The factory chart already *is* MAME's chart; pretending otherwise wastes the user's evening. |
| 7 | `docs/DRIVERS.md` / WinUSB docs / task #12: with the panel WinUSB-claimed, `-keyboardprovider win32` (or `dinput`) is **required** before any injected keystroke reaches MAME. Per-key passthrough works for MAME under Interception and does not under WinUSB. | §2.5. This is a prediction of a real break, not a caveat. |

### Do next — small, low-risk code

| # | Change | Why |
|---|---|---|
| 8 | **`ksx export mame-ctrlr`**: emit a ctrlr `.cfg` with `<mapdevice device="XInput Player N" controller="JOYCODE_N"/>` per slot, plus `<port type="P1_BUTTON1..6">` newseq lines restoring panel order. | Fixes MAME's documented unstable joystick numbering — which **ksx is uniquely able to pin, because it knows its own pad order** — and repairs the scrambled/lost-button table in §2.4. Idiomatic: it is exactly what MAME's own `xarcade.cfg`/`hotrod.cfg` do. |
| 9 | Either ship an `arcade-6button-mame` template variant binding panel buttons 1–6 to A, B, X, Y, LB, RB, **or** rely on (8) — but do not leave the current silent failure where panel button 6 lands on RT and disappears as a numbered button. | §2.4. Preference: keep the fightstick layout, emit the ctrlr file. |
| 10 | Persona guidance for MAME in docs: slots 1–4 `xbox360`, slots 5–8 `playstation`/DS4. | XInput hard-caps at four; `winhybrid` picks the DS4s up over DirectInput. |
| 11 | When a doc needs MAME's joystick default, quote `mame -showconfig`, never the manual. | The manual says OFF; the binary says ON (§2.3). |

### Only if the merge is actually wanted — scoped deliberately

| # | Change | Why |
|---|---|---|
| 12 | **Try it outside ksx first.** MAME: bind panel and pad to the same input with an "or" (zero code, works today). Everything else: reWASD device groups. RetroArch: not available at any price. | §1.3. Cheapest possible test of whether the ergonomics are even enjoyable. |
| 13 | If it survives that: a **HID gamepad reader as a new `CaptureBackend` child** on the existing `rawinput.rs` sink — register `RIM_TYPEHID`, keep `RIDI_DEVICENAME` as identity, parse with `HidP_*`. Scope v1 to **digital buttons only**, generic HID pads (DS4/DualSense/8BitDo), **not** an Xbox pad. `CompositeBackend` makes the fan-in nearly free. Config shape — one slot listing several sources — is a natural extension of the `DeviceRef` work in task #5. | §1.4. Days, not weeks, *at that scope*. |
| 14 | Budget the two real costs separately and honestly: **analog** (a new input event type through core, config, mapping, the pipe wire protocol and Studio — bigger than the reader itself), and **hiding the real pad** (HidHide, whose Xbox/XInput blocking its own author flags as unreliable, or a WinUSB claim, which is clean for HID pads and does not exist for Bluetooth). | §1.4. Ask "wired or wireless, and does the game read XInput?" **before** starting, not after. |

### Drop

- **Gamepad-to-keystrokes inside ksx.** JoyToKey and antimicroX already do it, it
  round-trips analog through digital keys and loses it, and — the direct answer
  to the loop question — its output cannot feed back into ksx anyway. No ksx
  backend sees `SendInput` traffic. No loop to fear; no chain to build (§1.5).
- **XInput as a read path**, permanently. Unreliable slot identity measured on
  this machine, and it would be reading a namespace containing ksx's own output
  (§1.4).
