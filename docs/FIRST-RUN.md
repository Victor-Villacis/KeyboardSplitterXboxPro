# First run: download to gaming, with no CLI and no TOML

The product is for **someone who is not us**. They have a keyboard, a game, and
no interest in device instance paths. This file is the flow they walk, stated as
a spec, and it is the acceptance test for whether ksx is a product or a toolkit
with a web page attached.

Written 2026-08-08 from the owner's description. The original audit found an
installer that offered `ksx doctor`, five jargon-heavy Start-menu entries, and
a split-vs-freeze choice reachable only by editing TOML. Those findings are
historical: the product flow described below is now implemented. The physical
fresh-user acceptance in §7 remains the release gate.

> **The CLI is a development surface and is not the product.** Existing
> `ksx <verb>` contracts stay complete and documented, and new backend
> capabilities should get a CLI driver. The current matrix is honest about two
> debts: `ksx stage` and `ksx games new|update|delete` remain planned while their
> typed backend contracts and Studio faces exist (`docs/SURFACES.md` §3c and
> §10). The installer advertises neither, and no step below may require them. If
> a step in this file can only be done from a shell, that product step is
> unfinished.

## §1 The seven moments

Numbered because the rest of this file and the code refer to them.

1. **Get it.** A `.exe` from the releases page. One file.
2. **Install it.** Double-click, click through, done. It offers to start ksx —
   and a desktop icon exists whether or not they accept.
3. **Start it.** The console-free `ksx-launcher.exe` starts the installed
   `ksx.exe open`, the tray icon appears, and Studio opens directly at
   `/start` in its own app window. An idle control host is running, but no
   emulation session exists: nothing is captured and no pads exist. The tray's
   operate-only cabinet window and saved-setup Start action remain gray until
   first-run Save gives them a runnable setup.
4. **Choose a keyboard.** They see their real devices, named. They pick one.
5. **Choose a controller.** They pick what it should become. It appears
   **ready** — and they can change their mind freely, because nothing has been
   plugged, claimed, or written yet.
6. **Map it.** Press a key, pick the button. Macros if they want. Then the one
   question that matters: **split or freeze?**
7. **Play.** Start it live. The pad connects, the keyboard becomes a controller,
   and Guide can ask Windows to open Game Bar when **Allow your controller to
   open Game Bar** is enabled in Windows Settings > Gaming > Game Bar. From
   there they can use the gaming UI their Windows account already has set up.

## §2 What "ready" means, and why staging is a real type

**A staged setup is its own implemented value and never touches disk.** It
holds the chosen device, persona and complete authoring preset per controller,
plus the blocking choice. It lives in the daemon for the length of the visit.
`StagedSlotView::authoring` is optional for wire compatibility, but every live
slot served by the current daemon supplies it.

`/map?target=stage&slot=N` reuses the ordinary visual mapper for buttons,
multiple keys, turbo and macros. The backend prepares a pure staged bind or
macro edit, validates conflicts across all staged controllers, and sends one
`StageEdit::SetBindings` only when accepted. A refusal leaves the staged value
unchanged. It takes no disk backup and triggers no config reload because no
file is being edited.

Nothing is claimed, nothing is plugged and no config file is written until the
user acts at moment 7. Save and Play remain separate: Save commits the staged
value; Play starts that same value without first saving it.

This is not a UI convenience. It is the difference between an app you can
explore and one that punishes you for clicking.

Consequences that fall out, and are requirements:

- Deleting a staged controller is free and complete. No file, no backup, no
  trace.
- A staged setup can be discarded wholesale. "Start over" must always work.
- The user may leave without saving and lose only what they typed.
- What is staged is what plays. There is no second translation step where a
  saved file means something different from what the screen showed.

## §3 Split or freeze — the question, in the user's words

Asked once, after mapping, before playing. Both answers already exist as
`ksx_core::Blocking`; what is new is *asking*.

- **Freeze this keyboard** (`Blocking::Whole`) — every key on it drives the pad
  and nothing else. No typos into the game, no accidental Windows shortcuts.
  This is what most people want for a dedicated arcade panel.
- **Split this keyboard** (`Blocking::BoundKeys`) — mapped keys drive the pad;
  everything else still types. This is what lets one keyboard serve player 1 and
  player 2, and what lets someone keep using their only keyboard.

Two things must be said on that screen, not buried:

1. **The escape hatch is always live**: LeftCtrl five times stops emulation, in
   both modes, and it is handled in the capture thread where no UI can break it.
2. Freeze is not permanent and not global. It applies to that keyboard, for that
   session, and stopping the session ends it.

## §4 What the installer must do (moment 2)

- **Install the bundled controller driver, having asked.** Without ViGEmBus
  there is no bus for a virtual pad to appear on, so every one of the moments
  below can be performed perfectly and moment 7 still plugs nothing. Setup is
  the **only** point in the product where an administrator token exists and
  the user has already consented to it (`ksx install-drivers` needs one and ksx
  never self-elevates), so it is the only place this can happen without a
  terminal — and §7 makes "without a terminal" the test. It is a `[Tasks]`
  checkbox, ticked by default, whose label names the driver and says what it is
  for; `docs/DRIVERS.md` is right that installing a kernel driver silently
  throws away the consent, and a checkbox nobody can read is silence with extra
  steps. What it runs is `ksx install-drivers --yes`, not the bundled `.exe`,
  because that verb owns the hash pin, the signature pin and the sealed handle.
  A failure here **never** fails the install: a machine with no ViGEmBus still
  wants the ksx that configures and maps, so the wizard says what happened,
  names the way back, and carries on.
- **Desktop icon by default.** Not `Flags: unchecked`. The audit's finding: a
  user who declines the launch prompt has to go hunting through a Start menu.
- **Offer to launch ksx**, not to run a diagnostic. `ksx doctor` is a developer
  verb; it prints driver tables into a console. A first-run user who accepts a
  "run this now" prompt must get the app.
- **One Start menu entry: `ksx`.** The four others (`daemon (tray only)`,
  `Studio (serve only)`, `cabinet`, `setup wizard`) are surfaces and dev tools,
  not products. They stay reachable — the verbs are not deleted — but a menu of
  five names a new user cannot rank is a menu that teaches nothing.
- **Every customer entry targets `ksx-launcher.exe`.** It is a Windows GUI
  subsystem binary that starts the sibling `ksx.exe open` with
  `CREATE_NO_WINDOW`; the Start shortcut, desktop shortcut and post-install
  launch are the same action. The elevated installer uses `runasoriginaluser`
  so the daemon and Chromium profile belong to the person who installed ksx,
  not to the administrator account that approved Setup.
- **No PATH task or registry mutation.** CLI/dev verbs remain installed for
  support and development, but the customer is offered no terminal integration
  or shortcut to them.

## §5 What the first screen must do (moments 3–4)

Clean, because it genuinely is: no config, no emulation session, no capture and
no pads. A plain idle daemon/control host must stay alive even when the default
configuration has no slots; otherwise `/start` could not stage the first one.
An explicitly requested empty game profile or a broken configuration still
refuses. `ksx open` waits for this host and Studio, then opens `/start` (with the
existing bounded browser fallback if the preferred Chromium app window cannot
be opened).

- **Devices are listed without being asked for**, with a visible rescan. A user
  who just plugged something in must not have to know a scan exists.
- **Named the way a human names them** — "Logitech keyboard", not
  `USB\VID_046D&PID_C31C&MI_00\7&...`. The vendor table already does this; the
  path belongs in small print for support, never as the identifier on screen.
- **Say what each device can do**, because it is not guessable: a Bluetooth
  keyboard can be split but never WinUSB-claimed, and a device with no keyboard
  interface cannot be picked at all. `docs/DEVICE-IDENTITY.md` and the transport
  column carry this already.
- **Nothing on this screen may claim, plug, or write.** Looking is never a
  commitment — the same rule `ksx device scan` already follows against `pick`.
- **Say it if Play cannot work.** A machine with no ViGEmBus can walk every
  moment above and plug nothing at the end, so whether a pad can be created is
  read (`MachineSource::pad_bus`) and stated before the Play button. Three
  states, not two: cannot, could-not-tell, and fine — and the remedy is a
  sentence naming the installer, never a button, because §3 of
  `docs/SURFACES.md` marks driver installation `never` for this surface and
  that has not changed.

## §6 What must never happen

Each of these has already happened once in this project's history.

- A screen reports success while nothing works (a session read healthy while
  the panel was dead). If a step cannot be verified, it says so.
- A failed read renders as an empty result — "you have no devices" when the
  truth is "I could not enumerate" (`SURFACES.md` §1b).
- A user is asked to type or paste a device path. Ever.
- A customer shortcut flashes a console window.
- An empty default configuration kills the control host before `/start` can
  stage the first controller.
- A fresh empty configuration offers an active cabinet window or saved-setup
  Start action in the tray; both stay gray until Save, while Open ksx remains
  available.
- A staged mapper GET or accepted edit writes a file before Save, or a refused
  edit changes the staged setup.
- An action that looked like a menu choice turns out to have installed a driver
  or claimed a board. Claiming is always explicit, always separately confirmed,
  and per `SURFACES.md` §3 never on the browser surface at all.
- The only way out of a mistake is a shell command.

## §7 How we will know it works

Not "the pages render". The test is a person who has never seen ksx, on a
machine that has never run it, getting from a downloaded `.exe` to a controller
moving in a game **without opening a terminal, without editing a file, and
without being told what to do next by us**.

The acceptance run uses the exact CI-built installer and a fresh standard
Windows user. It records the installer SHA/version and verifies: the default
ViGEmBus checkbox and outcome; one customer shortcut; the unelevated original
user and correct browser profile; no console flash; empty-config idle bootstrap
to `/start`; staged multi-key/turbo/macro editing; Play before Save; Save and
restart parity; profile create/update/delete/switch without TOML or CLI; and a
real virtual controller moving in a game. Until that is true, every green test
suite in this repo is measuring something narrower than the product.

The Guide clause has an OS prerequisite and a physical acceptance gate. The
first-run screen must name the Windows setting above and offer
`ms-settings:gaming-gamebar` as the direct remedy; ksx must not silently change
that per-user preference. Unit tests can prove that the default layout maps
Player 1's Left Windows key and Player 2's Numpad `*` key to Guide, but they
cannot prove that Windows displayed Game Bar. Moment 7 remains unverified until
a fresh Windows user with Game Bar enabled turns on that controller setting,
presses each default Guide key after Play, and observes Game Bar open from the
virtual pad without using a terminal or another keyboard.
