# First run: download to gaming, with no CLI and no TOML

The product is for **someone who is not us**. They have a keyboard, a game, and
no interest in device instance paths. This file is the flow they walk, stated as
a spec, and it is the acceptance test for whether ksx is a product or a toolkit
with a web page attached.

Written 2026-08-08 from the owner's description, after an audit found the
install path offers to run `ksx doctor`, the Start menu shows five entries of
jargon, and the split-vs-freeze choice — the one question that decides whether
a player can still type — exists in `ksx-core` and is reachable only by editing
TOML by hand.

> **The CLI is a development surface and is not the product.** `ksx <verb>` stays
> complete and stays documented, because `docs/SURFACES.md` §3 makes the CLI the
> place every capability lives. It is not what a customer is handed, not what the
> installer advertises, and no step below may require it. If a step in this file
> can only be done from a shell, that step is unfinished.

## §1 The seven moments

Numbered because the rest of this file and the code refer to them.

1. **Get it.** A `.exe` from the releases page. One file.
2. **Install it.** Double-click, click through, done. It offers to start ksx —
   and a desktop icon exists whether or not they accept.
3. **Start it.** Tray icon appears, ksx opens in its own window. Nothing is
   captured, no pads exist, no daemon work has happened beyond being ready.
4. **Choose a keyboard.** They see their real devices, named. They pick one.
5. **Choose a controller.** They pick what it should become. It appears
   **ready** — and they can change their mind freely, because nothing has been
   plugged, claimed, or written yet.
6. **Map it.** Press a key, pick the button. Macros if they want. Then the one
   question that matters: **split or freeze?**
7. **Play.** Start it live. The pad connects, the keyboard becomes a controller,
   and Guide opens Game Bar so they can launch a game without leaving it.

## §2 What "ready" means, and why staging is a real type

Moment 5 is where the current design breaks. Today a persona choice is a
`[[slot]]` written to `config.toml`, and pads appear when a *session* starts.
So "pick PS4, look at it, change to Xbox 360" would be three file writes and
two backups, for a decision the user has not made yet.

**A staged setup is its own value and never touches disk.** It holds: the chosen
device, the chosen persona per slot, the bindings so far, and the blocking
choice. It lives in the daemon for the length of the visit. Nothing is claimed,
nothing is plugged, no config file is written, until the user says so at moment
7 — and even then, saving and playing are separate acts.

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
- **PATH stays opt-in and unchecked.** It is a developer convenience, and this
  file's premise is that the customer never opens a shell.

## §5 What the first screen must do (moments 3–4)

Clean, because it genuinely is: no config, no daemon session, no pads.

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

## §6 What must never happen

Each of these has already happened once in this project's history.

- A screen reports success while nothing works (a session read healthy while
  the panel was dead). If a step cannot be verified, it says so.
- A failed read renders as an empty result — "you have no devices" when the
  truth is "I could not enumerate" (`SURFACES.md` §1b).
- A user is asked to type or paste a device path. Ever.
- An action that looked like a menu choice turns out to have installed a driver
  or claimed a board. Claiming is always explicit, always separately confirmed,
  and per `SURFACES.md` §3 never on the browser surface at all.
- The only way out of a mistake is a shell command.

## §7 How we will know it works

Not "the pages render". The test is a person who has never seen ksx, on a
machine that has never run it, getting from a downloaded `.exe` to a controller
moving in a game **without opening a terminal, without editing a file, and
without being told what to do next by us**.

Until that is true, every green test suite in this repo is measuring something
narrower than the product.
