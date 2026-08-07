# Migrating a device from Interception to WinUSB

Read [`RECOVERY.md`](RECOVERY.md) §2 **before** you start, and keep it open. The
whole of it fits on a phone screen.

## What actually changes

You are not swapping a driver ksx talks to. You are changing **what the device
is** to Windows.

| | Interception (M3–M5) | WinUSB claim (M6) |
|---|---|---|
| What the device is to Windows | an ordinary HID keyboard | a generic USB device with no HID stack |
| Who can read it | everyone (ksx filters the class stack) | only the process that opened the WinUSB interface |
| Blocking | a kernel filter drops strokes; scoped, revocable, racy in principle | structural — there is nothing to block, Windows never sees the strokes |
| Identity | Interception hardware id (`HID\VID_D209&PID_0430&REV_0056&MI_00`) — **shared by identical boards** | USB device instance path — unique per physical port |
| Device limit | 10 keyboards, and the id **increments on every replug/resume** until the device stops working | none |
| When ksx is not running | keyboard works normally | **the device does nothing** |
| Driver signing | cross-signed cert expired 2012; dies when the 2026 CI policy flips to enforcement | in-box `winusb.sys`, WHQL-signed, unaffected |
| Getting out | uninstall Interception, reboot | `ksx winusb release --yes`, no reboot |

The row that matters most is the second-to-last one in the table above and the
one everybody skips: **a claimed panel types only while ksx is running.** More
precisely, while `ksx daemon` is running: the daemon claims once at startup and
keeps the claim for its whole life, typing the panel's keys whenever it is not
emulating (`docs/ARCHITECTURE.md` §M6). `ksx run` claims for one session and
releases on the way out, so with `ksx run` the panel is dark before and after
the run.

## Before you claim

1. **Plug in a second keyboard on a different USB port and leave it unassigned.**
   `ksx winusb claim` refuses to take the machine's last keyboard (exit 2,
   `last-keyboard`), but a spare is what makes every recovery path in
   `RECOVERY.md` §2 a two-minute job instead of a Safe Mode trip. It has to be a
   keyboard that can type *right now*: the refusal does not count one that is
   already claimed, disabled, driverless, or paired-but-disconnected (a
   Bluetooth keyboard with no batteries in it), and it counts one physical board
   once however many keyboard-class nodes it presents. `ksx winusb status` shows
   both lists.
2. **Take a restore point.** One click, and it covers the case where a hand-made
   INF does something unexpected.
3. **Set up autostart**, so "ksx is running" is a property of the machine:
   ```powershell
   ksx autostart --enable --game "MAME 4P"
   ksx autostart --status
   ```
4. **Write down the instance path** of the interface you are about to claim:
   ```powershell
   ksx winusb status --json > winusb-before.json
   ```
   Keep that file. If the board ever moves ports, its path changes and every
   command in the runbook has to be re-derived from a fresh `status`.

## The claim

```powershell
ksx winusb status
# find the row with  verdict: CLAIMABLE  and  note: Ultimarc

ksx winusb claim "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"
# DRY RUN by default: prints the exact INF, the exact pnputil lines, and the
# signing prerequisite. Nothing is written and nothing is run.
```

Read the INF. It matches on **one interface's hardware id** — not the composite
parent, which would claim MI_01 and MI_02 with it and take the panel's trackball
and system buttons down as well.

### Signing: the step that stops everyone

`winusb.sys` is signed. Your INF is not, and 64-bit Windows will not add an
unsigned third-party INF to the driver store. `claim --dry-run` prints the full
recipe; the short version:

```powershell
# (a) self-signed catalog — the cabinet answer, what Zadig/libwdi automate
inf2cat /driver:"C:\Users\<you>\AppData\Roaming\ksx\winusb" /os:10_X64
makecert -r -pe -ss PrivateCertStore -n CN=ksx-cabinet ksx-cabinet.cer
signtool sign /fd sha256 /s PrivateCertStore /n ksx-cabinet "...\ksx-winusb-vid-d209-pid-0430-mi-00.cat"
certutil -addstore -f Root ksx-cabinet.cer
certutil -addstore -f TrustedPublisher ksx-cabinet.cer
```

**Do not enable test-signing mode.** `bcdedit /set testsigning on` disables a
Secure Boot guarantee machine-wide to install one INF. The watermark is the least
of it.

The alternative is to skip ksx's INF entirely and bind the interface once with
[Zadig](https://zadig.akeo.ie/) by hand — it does the self-signed-certificate
dance for you. ksx does not care how the interface got bound to `winusb.sys`,
only that it is; `ksx winusb status` will report it as `CLAIMED` either way.
(`release` still knows how to remove *its own* INF; a Zadig-installed one you
remove the same way, with `pnputil /delete-driver` on whatever `pnputil
/enum-drivers` says it published.)

### Then

```powershell
# elevated
ksx winusb claim "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000" --yes
ksx winusb status        # the row should now read  driver: WinUSB   verdict: CLAIMED
```

## After you claim

### Point ksx at the claimed interface: two lines in `config.toml`

A claimed interface is a **different device identity** and a **different
backend**, and both live in the same `[[device]]` block.

**Let the picker write it.** `ksx device pick` reads the live enumeration,
chooses the weakest id that still names your board alone, and rewrites the entry
in place — keeping the alias, so every `[[slot]]` keeps working:

```powershell
ksx device scan                 # confirm the board reads as CLAIMED
ksx device pick "P1 I-PAC"      # an existing alias is a valid target
```

By hand, the same edit:

```toml
[[device]]
# was: id = "HID\\VID_D209&PID_0430&REV_0056&MI_00"   (Interception hardware id)
id      = 'usb:d209:0430:00'
alias   = "P1 I-PAC"
backend = "winusb"          # was "interception" (the default)
```

A full instance path — `id = "USB\\VID_D209&PID_0430&MI_00\\7&25EEA38C&0&0000"`
— is still accepted and is what configs written before selectors existed hold.
Prefer the `usb:` form: it is the same board, spelled in a way that does not
depend on which socket it is in or on which machine wrote the file
(`docs/DEVICE-IDENTITY.md` §2).

**Every `[[slot]]` stays exactly as it is.** Slots reference the *alias*, not the
id, so the whole migration is those two lines per board — presets, bindings,
games and autostart are untouched. That indirection is why there is no
`--migrate` command and no old-id alias table: an alias table would keep a dead
Interception id alive in the config forever, and it could not express the case
this milestone exists to fix (two identical boards share one hardware id, so one
old id would have to map to two new ones).

Both edits are needed and each fails loudly on its own:

- **New id, old backend** → the plan asks Interception for a `USB\` path it
  will never enumerate; the run refuses.
- **Old id, new backend** → `winusb-device-missing`, and the error names the
  likely cause. `ksx devices` reports the same thing as
  `[WARN] config selects backend = "winusb" for … but no USB interface has that
  instance path`.

`ksx devices` prints the exact string to paste, and marks each row `[READY]`
(configured **and** rebound) or `[NEEDS REBIND]` (configured, still on the
keyboard stack). Migrate one board at a time and it will tell you where you are:

```powershell
ksx devices            # the "usb interfaces (winusb backend candidates)" section
ksx run --dry-run      # the "backends:" line says interception / mixed / winusb
```

`ksx winusb status` reports the same `ksx_device_id` for each interface — the
HID keyboard path while it exists, the USB interface path once claimed:

```powershell
ksx winusb status --json | ConvertFrom-Json |
  Select-Object -ExpandProperty candidates |
  Where-Object state -eq 'claimed' |
  Select-Object instance_id, ksx_device_id
```

Note that the HID path and the USB interface path are **different instances** of
the same board: on this cabinet the HID child is
`HID\VID_D209&PID_0430&MI_00\8&2A0D0500&0&0000` while the USB interface node —
the one ksx uses once claimed — is
`USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000`. Copy the `USB\` one.

### Test in this order

1. **Emulation stopped, daemon running.** Tap panel buttons. The frontend menu
   should move. This is the re-injection path; if it does not work, nothing else
   matters yet.
2. **Start emulation.** `joy.cpl` should show 4 pads and the panel should drive
   them. Nothing should type onto the desktop behind the game.
3. **Hold a direction, start emulation, release it.** Then stop emulation and
   check the desktop is not scrolling. This is the stuck-key case, and it is the
   one thing a quick smoke test misses.
4. **Kill ksx while emulating** (`taskkill /f /im ksx.exe`). The pads vanish. The
   claimed panel goes silent — **expected**, and the thing to internalise before
   you trust the setup. Your spare keyboard still works.
5. **Reboot.** The daemon should come back via autostart and the panel should
   type again with no intervention.

### What you lose

- **Anti-cheat compatibility for re-injected keys.** They carry
  `LLKHF_INJECTED`. While *emulating* this is irrelevant (the game sees XInput
  pads, which are real virtual devices), but a game driven by the panel's
  keystrokes rather than the pads may reject them.
- **The secure desktop.** No injected key reaches the lock screen, a UAC prompt
  or `Ctrl+Alt+Del`. Use the spare keyboard.
- **Crash-only recovery of the *blocking*.** Killing ksx frees an
  Interception-captured keyboard within a second; it does not un-bind a WinUSB
  interface. The claim outlives the process, so a claimed panel that is not being
  driven by ksx is a silent panel until `ksx winusb release`. What crash-only
  still guarantees is that nothing is left half-pressed: the backend releases
  every key it injected on every exit path, panic included.

  The emergency escapes themselves keep working and still hand the keyboard
  back — passthrough on a claimed board *is* re-injection, so `LeftCtrl ×5`
  makes the panel type again immediately, from the capture thread, with no
  supervisor involved. One gesture on any board frees every board in the session.
  What it cannot do is reach the secure desktop; that is what the spare keyboard
  is for.

### What you gain

- The 2026 driver deadline stops being your problem. Run `ksx doctor` — the
  `interception-borrowed-time` verdict goes away once the filter is out of the
  keyboard class stack.
- Two identical I-PACs become distinguishable (`USE-CASES.md` T4).
- No more replug/resume id drift, no 10-device ceiling, no "reboot required".
- One less kernel driver on the machine.

## Rolling back

```powershell
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"        # dry run
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000" --yes  # elevated
```

Release does three things, and the middle one is the one people forget by hand:
`pnputil /remove-device`, **delete the ksx INF from the driver store**, then
`/scan-devices`. Without the delete, the rescan re-binds WinUSB — the ksx INF
matches on hardware id and outranks the in-box `input.inf`, which only matches on
compatible id — and it looks like the removal did nothing.

If ksx will not start at all, `RECOVERY.md` §2c has the same three commands to
run by hand and §2d has the Device Manager route that needs only a mouse.

## Running both backends at once

Supported and often correct: claim the I-PAC, leave everything else on
Interception (or on nothing at all). Blocking is per-device either way. The
migration is per-interface, so you can move one board, live with it for a week,
and move the next — which is the recommended pace, not a limitation.

Mechanically, a mixed session runs one capture thread per claimed board plus one
Interception thread, behind a single `CompositeBackend`. They share one health
state and one escape latch, so `LeftCtrl ×5` anywhere frees everything, and the
supervisor above sees exactly one backend. `ksx run --dry-run` prints which
shape a session would take:

```text
  backends: mixed - winusb (1 board(s)) + interception for the rest
```

When the last `[[device]]` flips to `winusb`, that line reads `winusb (N claimed
board(s))` and the run never creates an Interception context at all — which is
the point at which the driver can be uninstalled.
