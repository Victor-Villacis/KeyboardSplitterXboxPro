# Quickstart — from a fresh machine to four working players

You have never run ksx. You have a PC, a panel (an arcade encoder, or just some
keyboards), and twenty minutes. This is the whole path.

Nothing here assumes you have seen the legacy Keyboard Splitter, and nothing
here asks you to write TOML. If you *want* to write TOML, everything below has
a plain-text file behind it and you can edit it by hand at any point — that is
a design rule, not an accident (`docs/USE-CASES.md`, non-negotiable 6).

---

## 0. What ksx is about to do

One keyboard becomes several Xbox (or PlayStation) controllers. A four-player
arcade panel is *one* USB keyboard sending four blocks of scancodes; ksx reads
those blocks, splits them by slot, and drives four virtual pads. Games see
controllers. They never see a keyboard.

While a session is running, ksx **blocks** the keyboards it is using — only
those, and only while emulating — so your panel's `1` does not also type "1"
into whatever has focus. Every other keyboard keeps typing normally.

> **Read the escapes before your first session.** With the panel captured,
> `Ctrl+C` cannot reach ksx. `LeftCtrl` pressed five times toggles capture off;
> `Ctrl`+`Alt`+`Del` stops emulation. Both are evaluated inside the capture
> thread, so they work even when everything else is wedged. `README.md`
> §Emergency escapes has the details, `docs/RECOVERY.md` has the rest.

---

## 1. Drivers (once per machine)

Two drivers, different jobs:

| | What it does | Needed? |
|---|---|---|
| **ViGEmBus** | creates the virtual controllers | **always** |
| **Interception** | reads and blocks the keyboards | for the Interception capture mode (see §2) |

### ViGEmBus

ksx ships the installer and verifies it against two independent pins — its
SHA-256 and its Authenticode signer — before it will run it. It never downloads
anything.

```powershell
ksx install-drivers                 # report + verify, run nothing
ksx install-drivers --yes           # execute (elevated terminal required)
```

ksx never self-elevates. If it needs an admin token it says so and stops.

### Interception

ksx does **not** install it — its licence is LGPL/non-commercial, so bundling
its installer is fine and shipping it inside a commercial product is not
(`docs/DRIVERS.md`). Get it from
[the project](https://github.com/oblitum/Interception), run
`install-interception.exe /install` from an elevated prompt, and reboot.

### Check

```powershell
ksx doctor
```

Verdicts with stable codes; exit 2 means something will genuinely not work.
Run this first whenever anything is strange — it is faster than guessing.

---

## 2. Which capture mode

Two ways for ksx to read your panel. Pick with the table, not with a coin.

| | **Interception** (default) | **WinUSB claim** |
|---|---|---|
| Extra driver | yes (third-party) | no — Windows' in-box `winusb.sys` |
| Two identical encoders | ❌ **cannot tell them apart** — ksx refuses to start rather than guess | ✅ identity is the USB port path |
| Panel when ksx is not running | types normally | **dead** (the daemon re-injects its keys while running; nothing types when it is not) |
| Device limit | 10 keyboards, and ids drift on replug/resume | none |
| Signing future | cross-signed cert expired 2012; dies when Microsoft's 2026 policy flips to enforcement | WHQL, unaffected |
| Undo | uninstall + reboot | `ksx winusb release --yes`, no reboot |

**Start with Interception** unless one of these is true:

- you have **two identical boards** (two I-PAC2s, two of the same cheap
  keyboard) — Interception cannot separate them, and this is the common shape
  for a 4-player build. Go straight to WinUSB.
- you are setting up a machine to still work after the 2026 cross-signing
  cutover, and you can commit to ksx being resident (autostart) so the panel
  keeps typing in menus.

WinUSB has its own runbook — `docs/MIGRATION-WINUSB.md`, and read
`docs/RECOVERY.md` §2 **before** you start it. Everything below works
identically on either mode.

---

## 3. See your panel

```powershell
ksx devices
```

Read-only: it opens nothing, claims nothing, sets no filter. You get one row
per keyboard with its id, and Ultimarc boards are tagged `[I-PAC]`.

If two rows share an id, that is the identical-boards problem above.

---

## 4. Get a preset — the fast way

A preset is a key → controller-function map. ksx ships ready-made ones, so a
standard panel needs **no mapping session at all**:

```powershell
ksx preset list --templates
```

| Template | The panel it is for |
|---|---|
| `arcade-6button` | Two-player, six-button fighting panel on the factory/MAME chart. P1 = arrows + `LeftCtrl` `LeftAlt` `Space` `LeftShift` `Z` `X`, start `1`, coin `5`. P2 = `R` `F` `D` `G` + `A` `S` `Q` `W` `I` `K`, start `2`, coin `6`. Buttons 7–8, if wired, become LB and LT. |
| `arcade-4way` | Four-player, two-button cabinet on MAME's four-player chart (P1–P4 blocks). |
| `keyboard-wasd` | One ordinary keyboard: WASD = left stick, arrows = right stick, numpad = D-pad, `Space`=A, `C`=B, `R`=X, `F`=Y. |
| `default` | The legacy app's layout, for people migrating. |
| `empty` | Every control listed, nothing bound — a blank worksheet. |

Every arcade template binds each stick direction to **both** the D-pad and the
left stick, because some games read only one of them. That is ksx fan-out, not
a duplicate; `ksx map --function dpad.up --clear` removes half of it if you ever
want only the stick.

Make yourself two presets off one encoder — the primary arcade topology:

```powershell
ksx preset new "P1" --from-template arcade-6button --player 1
ksx preset new "P2" --from-template arcade-6button --player 2
```

`--player` picks the key block, not the slot: on an I-PAC, player 2's buttons
are a *different set of scancodes from the same board*. That is exactly what
makes one keyboard drive four pads.

Add `--dry-run` to see the TOML without writing it. `--force` overwrites an
existing preset and copies the old one to `<preset>.toml.bak-<timestamp>` first.

---

## 5. Get a preset — the sure way

Your panel is not on any chart, or it was reprogrammed, or you would rather
press the buttons than trust a table:

```powershell
ksx setup
```

The wizard, in order:

1. **"Hold a key on the panel for player 1."** You identify the panel by
   *pressing* it — never by picking a hardware id out of a list, which on a
   cabinet with two identical boards is not a question anyone can answer.
2. **One control at a time.** The prompt names a POSITION — `SOUTH`, not `A`,
   because the letter is somewhere else on a Nintendo pad and most panels are
   labelled by position anyway. Press the button; it binds and moves on.
3. **Skip by pressing nothing.** Each prompt shows a countdown
   (`--step-secs`, default 6) and skips the control when it runs out. **Two
   silent prompts in a row end the run** and skip everything left — bailing out
   of the optional tail costs about twelve seconds.
4. **ALREADY TAKEN.** A key that already drives another control in this run is
   refused the moment you press it, with the control that holds it named, and
   the prompt stays put.
5. **`Escape` cancels** the whole run. It is the only reserved key.
6. **A review screen, then an audit.** It warns when the panel can reach
   neither START nor BACK — on a cabinet, those are the exit keys, and finding
   out you have none *after* the panel is captured is a bad afternoon.
7. **Nothing is written until you say yes.** "No" discards everything. Only
   after "yes" does it ask whether to wire the slot up, and it asks rather than
   assumes.
8. **"Set up the next player?"** — so P1 through P4 is one continuous run.

Two things to know before you start it:

- **Stop emulation first.** While `ksx run` has the panel captured, its
  keystrokes are suppressed below win32k and the wizard hears nothing at all.
- `--dry-run` walks the whole wizard and writes nothing; `--json` prints the
  outcome as one object on stdout (prompts stay on stderr).
- `--profile "MAME 4P"` wires finished slots into that `games.toml` profile
  instead of `config.toml`.

---

## 6. Wire the slots

`ksx setup` offers to do this for you. By hand, `config.toml` (or `ksx.toml`
next to the exe, if you are running portable) looks like:

```toml
schema_version = 1

[[device]]
id = 'HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000'   # from `ksx devices`
alias = "Cabinet panel"

[[slot]]
number = 1
keyboard = "Cabinet panel"
preset = "P1"

[[slot]]
number = 2
keyboard = "Cabinet panel"       # SAME board — this is the fan-out
preset = "P2"
```

Four players is four `[[slot]]` blocks. One board can feed all of them; several
boards can feed one each (give each its own `[[device]]` and alias); a mix is
fine.

Slots 5–8 exist too, but XInput only has four user indices — pads beyond four
need `persona = "playstation"` on the slot (`ksx pads --help` explains the
difference).

Check it without touching a single driver:

```powershell
ksx run --dry-run
```

It prints the resolved plan — which preset each slot got, how many bindings it
has, which devices may be captured and which slots they feed — and exits. Exit 2
means the configuration does not resolve, and it says why.

---

## 7. Play

```powershell
ksx run
```

Pads are plugged, the assigned keyboards are captured, and the escape banner is
printed **before** any blocking starts. Press a button on the panel; the
controller moves.

Attach a game to it:

```toml
# games.toml
[[game]]
title = "MAME 4P"
path = "C:\\mame\\mame.exe"
process_name = "mame.exe"
```

```powershell
ksx run --game "MAME 4P"
```

The game starts *after* the pads exist (a game launched earlier sees zero
controllers), and emulation stops when it exits.

For a cabinet, make it permanent:

```powershell
ksx daemon                      # tray icon; start/stop on demand
ksx autostart --enable          # ...at every logon
```

---

## 8. Fix one binding

You do not re-run the wizard to change one button — that is the whole point of
having two flows (`docs/MAPPER-UX.md` commandment 3):

```powershell
ksx map --preset "P1" --function A --key G          # bind
ksx map --preset "P1" --function A --key S --key G  # two keys, one control
ksx map --preset "P1" --function A --clear          # unbind
ksx map --preset "P1" --list-backups                # every write leaves one
ksx map --preset "P1" --restore latest-backup       # undo
```

Or open the mapper and click:

```powershell
ksx studio          # then http://127.0.0.1:4460/map
```

(Studio is a compile-time feature: `cargo build -p ksx-app --features studio`.
The default build links no web stack at all.)

---

## When it goes wrong

| Symptom | First thing to try |
|---|---|
| Anything at all | `ksx doctor` |
| "refused to start", exit 2 | it printed why — usually two boards with one id, or a preset name that does not exist |
| The wizard hears nothing | a session is running (stop it), or the panel is WinUSB-claimed (`ksx winusb status`) |
| Keyboard stuck captured | `LeftCtrl` ×5, or `Ctrl`+`Alt`+`Del`, or kill `ksx.exe` — process death always returns the keyboards |
| Everything is on fire | `docs/RECOVERY.md` |

## Where to go next

- `README.md` — the commands in full, exit codes, the driver story
- `docs/USE-CASES.md` — which topologies are proven, which are untested
- `docs/INPUT-TRANSFORMS.md` — chords, macros, turbo, SOCD
- `docs/MAPPER-UX.md` — why the mapper works the way it does
- `docs/CONTROL-SURFACE.md` — every verb, CLI and pipe and web
