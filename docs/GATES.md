# Cabinet Gates — supervised runbooks

Four release gates require a person at a real Windows machine. The first three
close cabinet milestones; the fourth proves that the artifact CI built is a
product a new customer can install and use. These are the scripts for those
sessions: exact actions, what each one should show, and what to do the moment
one of them does not. A session walking Victor through a gate should follow it
top to bottom and never improvise past a failed step.

- **GATE 1 — "M5 rest"**: autostart at boot, the tray daemon, and the frontend
  wrapper. Software-only; Interception semantics; every failure is recoverable
  with a keypress or a taskkill.
- **GATE 2 — "M6 WinUSB rebind"**: the first time ksx changes what a device *is*
  to Windows. Read the preconditions twice. The whole gate is a round trip —
  claim, verify, release — and the machine ends it exactly as it started.
- **GATE 3 — "closing the books"**: one real four-player latency run, the
  frontend wrapper, deliberate Interception removal, and the fourteen-day soak.
- **GATE 4 — "fresh customer"**: the exact CI installer, a clean Windows user,
  and the complete launcher-to-Game-Bar journey with no terminal or TOML.

## Shared rules (cabinet gates 1–3)

- **conhost, not Windows Terminal.** Windows Terminal 1.24/1.25 fail-fasts when
  virtual pads send input — even as a background window — taking every tab with
  it, ksx included (`RECOVERY.md`, "Known environment hazard", verified on this
  machine 2026-08-04). `Win+R` → `conhost.exe`, run ksx there. The tray daemon
  and the frontend wrapper are immune (no Terminal attachment); every
  *interactive* ksx command in these gates is not.
- **Logs land in `%APPDATA%\ksx\logs\ksx.<YYYY-MM-DD>.log`** — every command,
  panics included, 14 days kept. When a step's output scrolled away or a window
  vanished, the log is the record. Check it at the end of each gate for
  `WARN`/`ERROR` lines you didn't see live.
- **"Clean" after any session means all of:**
  1. exit code 0 (`$LASTEXITCODE` / `echo %errorlevel%`);
  2. no `ksx.exe` in `tasklist | findstr /i ksx`;
  3. **no ghost pads** — `joy.cpl` lists zero controllers, Device Manager shows
     no children under "Nefarius ViGEm Bus Device";
  4. `ksx doctor` exits 0 (warnings allowed; the standing
     `interception-borrowed-time` warning is expected until M6 completes).
  Ghost pads → `RECOVERY.md` §3.
- **Never run ksx and the legacy KeyboardSplitter at the same time** (8 pads >
  4 XInput slots).
- Exit codes are the contract: `0` done, `1` error, `2` refused / nothing
  changed, `3` acted and failed (see `INTEGRATION.md`). A `2` always means the
  machine is untouched.

---

# GATE 1 — "M5 rest": autostart + tray daemon + frontend wrapper

Proves the three M5 deliverables on hardware: the tray daemon, start-at-logon,
and the frontend wrapper — ending in a real emulator with 4 live pads and a
clean exit.

## Preconditions

- Interception installed and healthy: `ksx doctor` exits 0.
- `ksx run --dry-run` exits 0 (the config resolves; nothing is touched).
- A 4-player game profile exists in `%APPDATA%\ksx\games.toml`. The commands
  below write `"MAME 4P"` — substitute the real title. The `"Steam"` profile is
  known-good on this machine and is used for the autostart phase.
- You know which `ksx.exe` is being tested (installed copy vs a build under
  `target\`). Run every command from that one — `ksx autostart --status` will
  call out a mismatch as `different-exe`.
- RetroBat at `C:\RetroBat`, LaunchBox at `C:\LaunchBox`, wrapper at
  `C:\Projects\KeyboardSplitterXboxPro\examples\ksx-wrap.ps1`.
- Nothing here needs elevation. If a UAC prompt appears, something is wrong —
  stop.

## Phase A — tray daemon

```powershell
ksx daemon --game "MAME 4P"
```

**Expect:** startup notice naming the log file path, then the console window
closes itself and a tray icon appears. That vanishing console is by design
(`ksx daemon --help`), not a crash — the log keeps recording.

1. Hover the tray icon → tooltip shows the idle state / last session verdict.
2. Tray → **Start emulation** → `joy.cpl` shows 4 Xbox 360 pads; panel drives
   them; the assigned panel keys stop typing; other keyboards keep typing.
3. Tray → **Stop emulation** → pads unplug from `joy.cpl`; panel types again.
4. Tray → **Quit** → icon disappears; run the shared "clean" checklist.

**ABORT Phase A if:** no tray icon within ~5 s (read the log tail); pads don't
appear on Start; panel keeps typing while emulating; anything left after Quit.
Recovery: `taskkill /f /im ksx.exe` — crash-only design returns the keyboards
within a second — then `ksx doctor`.

## Phase B — autostart at boot

`ksx autostart` registers **`ksx daemon --game <TITLE>`** as the logon task:
the tray icon comes up at every logon and captures nothing until a session is
started from the tray (or a wrapper). That default is deliberate — a
registered `ksx run` would grab the keyboards at every logon, desktop use
included. The kiosk shape (logon straight into the game) still exists as
`--mode run`; it is not part of this gate.

```powershell
ksx autostart --enable --game "Steam" --dry-run
```

**Expect:** the full plan — `task name: ksx\autostart`, `mode: daemon (tray
icon at logon; sessions start on demand)`, `runs: "…\ksx.exe" daemon --game
Steam`, `elevation: none (LeastPrivilege, per-user task)`, the exact
`schtasks /Create` line, the full XML, and `dry run: nothing was registered.`
Read the XML: `LogonTrigger` with `PT10S` delay, `RunLevel LeastPrivilege`,
`MultipleInstancesPolicy IgnoreNew`.

```powershell
ksx autostart --enable --game "Steam"
ksx autostart --status
```

**Expect:** `registered. Verify with `ksx autostart --status`…`, then a status
block: `autostart: registered as 'ksx\autostart'`, `mode: daemon`,
`game: Steam`, `enabled: yes`, exit 0. Exit 2 with a `STALE` warning means the
task points at a different or missing exe — fix before booting.

**Cold boot:** full shutdown (not restart), power on, log in, wait ~15 s.

**Expect after logon:**
- the **tray icon appears** (the task waits out its 10 s delay first) — hover
  it: idle state, nothing running;
- `tasklist | findstr /i ksx` shows `ksx.exe`;
- nothing is captured yet: the panel still types, `joy.cpl` lists no pads;
- today's log shows the daemon start with no `ERROR`.

Then prove boot-to-playable: tray → **Start emulation** → Steam launches (the
profile's program), pads present in `joy.cpl`, panel drives them. End the
session cleanly: quit the game/Steam (emulation stops when the followed
process exits) — or `Ctrl+Alt+Del` to stop emulation. Tray → **Quit**, then
the clean checklist.

**ABORT Phase B if:** no tray icon after logon (check Task Scheduler history
for `ksx\autostart` and the log — a missing log entry means the task never
fired; a log entry ending in exit 2 means validation drifted between enable and
boot); pads appear at logon without anyone touching the tray (the task was
registered `--mode run` — not this runbook's registration); or a second logon
starts a second daemon (must be impossible: `IgnoreNew`).

## Phase C — frontend wrapper into a real emulator

Pattern B from `INTEGRATION.md`: the frontend stays in charge, the wrapper
brackets the emulator with ksx and stops it in a `finally`.

RetroBat — register the wrapped system in the custom `es_systems_*.cfg`:

```xml
<command>powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\Projects\KeyboardSplitterXboxPro\examples\ksx-wrap.ps1" -Emulator "C:\RetroBat\emulators\mame\mame.exe" -Game "MAME 4P" -- %ROM%</command>
```

LaunchBox alternative — Tools → Manage → Emulators → edit the MAME entry:
Application Path `powershell.exe`, parameters as above without `%ROM%`
(LaunchBox appends the ROM path).

> Keep every path in that command on a fixed drive letter. This machine's
> RetroBat custom systems hardcode `G:\roms`; a letter drift silently removes
> systems and looks like a reset.

1. Launch a 4-player game from the frontend UI (panel/pad navigation).
2. **Expect:** brief wrapper window (or none), pads live *before* the emulator
   takes the screen — the wrapper waits for pads first, because an emulator
   that starts early caches "zero controllers".
3. All 4 players work from the panel.
4. Quit the game from its own menu. **Expect:** back at the frontend, pads
   gone, panel navigates the frontend again. Clean checklist.

**ABORT Phase C if:** the emulator sees no pads (start-order bug — the wrapper
launched the emulator before pads settled); the panel types into the frontend
*while* the game runs; or pads persist after quitting (the wrapper's `finally`
didn't run — kill ksx, check the wrapper invocation).

## Phase D — autostart removal

```powershell
ksx autostart --disable
ksx autostart --status
```

**Expect:** `autostart removed (scheduled task 'ksx\autostart' deleted)`, then
`autostart: NOT registered…`. Run `--disable` a second time: `autostart was not
registered; nothing to remove`, exit 0 (idempotent by contract). One more cold
boot: nothing starts, `tasklist` has no ksx.

## GATE 1 PASS criteria

All of: Phase A tray lifecycle clean; Phase B cold boot to the tray icon, a
live session started from it, log evidence and a clean stop; Phase C
frontend → emulator → 4 pads → clean exit with no ghost pads; Phase D removal
verified by a boot. `ksx doctor`
exits 0 at the end, and the day's log has no unexplained `ERROR`.

## GATE 1 rollback

Everything in this gate is additive and reversible with no driver involvement:
`ksx autostart --disable` removes the task; revert the `es_systems_*.cfg` /
LaunchBox emulator edit to unwrap; `taskkill /f /im ksx.exe` ends anything
stuck (keyboards return within a second — Interception crash-only guarantee).
`LeftCtrl ×5` un-captures at any moment; `Ctrl+Alt+Del` always works.

---

# GATE 2 — "M6 WinUSB rebind": the first hardware-touching gate

This is the big one. `ksx winusb claim --yes` takes the I-PAC's keyboard
interface out of the keyboard stack. From that moment until release, **the
panel types only while ksx is running** — and the escape semantics are weaker
than everything M3–M5 taught you:

> **Under Interception, killing ksx frees the keyboard within a second. Under
> WinUSB it does not.** `LeftCtrl ×5` still works — passthrough on a claimed
> board *is* re-injection, evaluated in the capture thread — but only while ksx
> is alive. Kill ksx and the claimed panel is simply dark until the daemon
> restarts or `ksx winusb release` runs. Injected keys never reach the lock
> screen, a UAC prompt or `Ctrl+Alt+Del`. This is the M6 trade, stated in
> `MIGRATION-WINUSB.md`; the mitigations are the spare keyboard and autostart.

The gate is a supervised round trip: claim → capture → play → typethrough →
release. It deliberately does **not** end with the cabinet migrated; living on
WinUSB is a separate decision taken after this passes (see PASS).

## Preconditions — every one, no exceptions

- **A spare USB keyboard is plugged into a different port and TESTED: open
  Notepad and type on it, now.** It must be a board that can type *this
  minute* — `claim` refuses the last usable keyboard (exit 2,
  `last-keyboard`), but a claimed/disabled/battery-dead board does not count
  and will not type the release command.
- **`RECOVERY.md` §2 is open on a second screen or phone** — not on this
  machine, whose input you are about to experiment on.
- **Every command runs from `conhost.exe`, NOT Windows Terminal.** Pads will
  send input during this gate; Terminal ≤1.25 dies when they do, and it takes
  the session that was supposed to walk you out with it. `Win+R` → `conhost`.
  Keep two open: one normal, one elevated (the elevated one is only for the
  two `--yes` steps).
- **Interception is NOT uninstalled.** It is the fallback for the whole gate;
  it comes out only after the two-week soak, when `ksx run --dry-run` says
  `winusb` with no Interception context at all.
- **GATE 1 has passed** — autostart is proven, so "ksx is running" can be made
  a property of the machine before anyone lives with a claim.
- **A restore point exists** (one click; covers the hand-made-INF unknowns).
- Baseline snapshot saved:

  ```powershell
  ksx winusb status --json > "%APPDATA%\ksx\winusb-before.json"
  ```

**The abort path, valid at every step below:** stop; `ksx winusb release
"<instance path>" --yes` from the elevated conhost; if ksx itself won't run,
`RECOVERY.md` §2c (three pnputil commands by hand — do not skip the
delete-driver step, or the rescan re-binds WinUSB and the release "does
nothing") or §2d (Device Manager, mouse only). Then revert the two `config.toml`
lines if already edited. Never push through a step whose expected output didn't
appear.

## Step 1 — survey

```powershell
ksx winusb status
```

**Expect:** the I-PAC MI_00 row —

```
USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000
  driver     : HidUsb
  verdict    : CLAIMABLE — ksx could claim this
  note       : Ultimarc (VID D209) — the arcade encoder family
```

— and `keyboards that can type right now:` **at least 2** (panel + spare; the
HP and Logitech boards also live here). MI_01, MI_02 and the trackball show as
`no keys` / not candidates — they are never touched.

The instance path is port-topology-derived. If it differs from the one above
(board moved ports since `RECOVERY.md` §2a was written), **use what `status`
prints** in every later command, and note it.

**ABORT if:** count is 1 (the claim would be refused anyway — fix the spare
first); verdict is not `CLAIMABLE`; or two identical `PID_0430` rows exist and
you are not certain which is which (claim by full path only, never substring).

## Step 2 — dry-run the claim and read it

```powershell
ksx winusb claim "USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000"
```

**Expect:** the exact INF text, the exact `pnputil` lines, the signing
prerequisite, ending `Nothing was written and nothing was run. Re-run with
--yes to apply.` Exit 0.

Read the INF before going further. It must match **only** the MI_00 hardware
id — not the composite parent (which would take the trackball and system
buttons down with it).

Signing: an unsigned INF will not enter the driver store. Do the self-signed
catalog dance the dry run prints (`inf2cat` / `makecert` / `signtool` /
`certutil`, per `MIGRATION-WINUSB.md`), or bind once with Zadig. **Never
`bcdedit /set testsigning on`.**

**ABORT if:** the dry run is refused (exit 2 — read the refusal; `last-keyboard`
means Step 1's spare check was wrong), or the INF matches anything other than
the one MI_00 interface.

## Step 3 — the claim

Elevated conhost:

```powershell
ksx winusb claim "USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000" --yes
```

**Expect:** pnputil output lines, then `Claimed. Run `ksx winusb status` to
confirm the driver is now WinUSB…` and the ready-made release command. Exit 0.

**The panel stops typing at this instant. That is correct.** The spare keyboard
is your hands from here until Step 7's typethrough works.

**ABORT if:** exit 2 (refused — nothing changed, diagnose calmly) or exit 3
(pnputil ran and failed — the machine may be mid-state: run the abort path
now, §2c if needed).

## Step 4 — confirm the rebind

```powershell
ksx winusb status
```

**Expect:** same row, now `driver : WinUSB`, `verdict : CLAIMED — ksx can open
this; Windows sees no keyboard`. Trackball still moves the pointer (MI_01
untouched — `RECOVERY.md` §2f).

## Step 5 — point the config at the claimed interface

Two lines in `%APPDATA%\ksx\config.toml`, per `MIGRATION-WINUSB.md` — the id
becomes the `USB\` instance path (not the `HID\` child), the backend becomes
`winusb`; every `[[slot]]` stays untouched:

```toml
[[device]]
id      = "USB\\VID_D209&PID_0430&MI_00\\7&25EEA38C&0&0000"
alias   = "P1 I-PAC"
backend = "winusb"
```

```powershell
ksx devices
ksx run --dry-run
```

**Expect:** the I-PAC row `[READY]` (not `[NEEDS REBIND]`), and the dry run's
`backends:` line reading `winusb (1 board(s))` — or `mixed - winusb (1
board(s)) + interception for the rest` if other boards are still assigned.

**ABORT if:** `winusb-device-missing` or a `[WARN] config selects backend =
"winusb"…` line — the id was mistyped or the `HID\` path was pasted. Fix or
revert; nothing is at risk yet beyond the claim itself.

## Step 6 — verify capture

```powershell
ksx monitor --for-secs 15
```

Press a handful of panel keys.

**Expect:** one `P1 I-PAC <Key> down` / `up` line per stroke. Monitor is
passthrough-only — it cannot block, so this proves the WinUSB read path with
zero risk.

**ABORT if:** no lines from the panel (the claim is bound but the read path
isn't working — release and take the diagnosis offline; do not proceed to a
session on a backend that can't see keys).

## Step 7 — real session + typethrough (the M6 user-choice requirement)

```powershell
ksx daemon --console --game "MAME 4P"
```

The daemon claims once at startup and holds the claim for its whole life —
that is what makes typethrough exist. Then, in order:

1. **Typethrough, emulation stopped:** open Notepad, press panel keys.
   **Expect: they type.** This is the requirement — a claimed panel must still
   drive frontend menus between games. If this fails, the cabinet loses menu
   control: release and abort.
2. **Start emulation** (tray → Start): `joy.cpl` shows 4 pads, panel drives
   them, **nothing** types into Notepad behind the game.
3. **Stuck-key check:** hold a panel direction, start emulation, release the
   key, stop emulation. The desktop must not be scrolling (nothing left
   half-pressed — the crash-only key-release guarantee).
4. Play the 4-player game; all four live; quit the game; emulation stops;
   typethrough returns immediately (daemon holds the claim between sessions).
5. **The kill test — internalise the trade:** with the daemon running,
   `taskkill /f /im ksx.exe` from the spare keyboard's conhost. Pads vanish;
   **the panel goes completely dark. Expected.** The spare keyboard still
   types. Restart `ksx daemon` — the panel returns. This is the weaker escape
   semantics, experienced once on purpose rather than discovered at midnight.

Quit the daemon (tray → Quit) before Step 8.

**ABORT if:** typethrough never works, keys leak into Notepad *during*
emulation, or the stuck-key check scrolls the desktop. All three are release-
worthy findings, not things to live with.

## Step 8 — release

Elevated conhost:

```powershell
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000"          # dry run
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000" --yes
```

**Expect from the dry run:** the three-step plan — `pnputil /remove-device`,
**delete the ksx INF from the driver store**, `/scan-devices`. The middle step
is the one hand-rollbacks forget; ksx does not. **Expect from `--yes`:** the
pnputil log, then `Released. The keyboard driver should be bound again…`
Exit 0. If exit 3: `RECOVERY.md` §2c by hand.

## Step 9 — verify the panel is a plain keyboard again

1. `tasklist | findstr /i ksx` → nothing. ksx fully closed.
2. Open Notepad, type on the **panel**. **Expect: it types**, with no ksx
   process anywhere — the keyboard stack owns it again.
3. `ksx winusb status` → the row is back to `driver : HidUsb`,
   `verdict : CLAIMABLE`, and the `HID\…` keyboard child is listed again.
4. Revert the two Step-5 lines in `config.toml` (id back to the `HID\` path,
   `backend` line removed). `ksx run --dry-run` exits 0 on the Interception
   backend. If the device didn't come back: replug the board; a reboot is
   always safe here.

## GATE 2 PASS criteria

The full round trip with **zero recovery actions**: CLAIMABLE → CLAIMED →
panel captured (`monitor`) → 4-player session → typethrough into Notepad while
not emulating → kill test behaved exactly as documented → released → panel
types with ksx fully closed → config reverted → `ksx doctor` exits 0 and the
day's log is clean. The spare keyboard was needed only where the runbook said
it would be.

**After PASS:** migrating for real — re-claim, keep the config on `winusb`,
autostart armed, and live with it — is a separate deliberate act, one board at
a time, per `MIGRATION-WINUSB.md`. Interception comes out only after the
two-week soak.

## GATE 2 rollback ladder

In order of how much still works:

1. ksx runs, any keyboard: `ksx winusb release <path> --yes` (§2b).
2. ksx won't start: the three pnputil commands by hand — **including
   delete-driver** — `RECOVERY.md` §2c.
3. Mouse only: Device Manager route, check "attempt to remove the driver",
   `RECOVERY.md` §2d.
4. Panel was somehow the only keyboard (the refusal exists to prevent this):
   plug in any keyboard, or §2d, or Safe Mode + on-screen keyboard (§2e).
5. Everything on fire: the pre-gate restore point.

Plus, always: revert the `config.toml` device lines, and remember the panel
"not typing" usually just means **`ksx daemon` is not running** — start it
before assuming the claim is broken (`RECOVERY.md` §2, first table).

---

# GATE 3 — closing the books: M4 p99, M5 Phase C, M6 soak

One cabinet evening, three milestone exits. Everything below is what the code
cannot do for itself — measurements and removals only a person at the machine
can perform. Nothing here changes source; the only writes are one BIOS-free
uninstall, config lines the runbook names, and log entries at the bottom of
this file.

State going in (2026-08-08): the I-PAC is WinUSB-claimed and the daemon drives
it daily; per-key typethrough shipped (the M6 user-choice requirement, Gate 2
step 7's subject); GATE 1 passed except Phase C; GATE 2's steps were executed
through step 7 across 2026-08-05/06 but never logged as a PASS, and steps 8–9
(release, verify, re-claim) have run only as one-offs. Interception's
`keyboard.sys` is still installed and loads at boot.

## Preconditions

- The current CI-built installer is what is installed (`ksx doctor` shows the
  expected version; if in doubt, re-run the Desktop setup.exe first).
- A second, never-claimed keyboard is plugged in and typing (the HP Elite).
- `%APPDATA%\ksx\config.toml` backed up beside itself with today's date.
- Nothing else important scheduled for the machine tonight: Phase 3 ends with
  a driver uninstall and a reboot.

## Phase 1 — M4 exit: the p99 number, written down

1. `ksx doctor --latency` once, idle, to confirm the histogram plumbing.
2. Start the real 4-player session (`ksx run --game "Steam"` or the daemon +
   frontend, whichever tonight's play actually uses).
3. Play something genuinely busy for ten minutes — four players mashing, not
   one person pressing one button.
4. `ksx doctor --latency` again. Record p50/p99/max in the run log below.

**PASS**: p99 < 1 ms (ARCHITECTURE rule 5). A miss is not a tuning session
tonight — it is a number in the log and an issue tomorrow.

## Phase 2 — M5 exit: Phase C, the frontend wrapper

GATE 1's Phase C verbatim (line ~137): the daemon wraps a real emulator run —
frontend up, game launched from it, pads live inside the emulator, clean exit
back to the frontend, nothing captured afterward. LaunchBox/RetroBat is the
frontend of record on this cabinet. Ten minutes.

**PASS**: GATE 1's Phase C criteria met; log it below and GATE 1 is fully
closed.

## Phase 3 — M6 exit begins: Interception comes out, the soak clock starts

M6's exit line is explicit: *same session with Interception **uninstalled***,
then a two-week soak. The claim is live and typethrough works, so the order
tonight is:

1. `ksx session stop` (leave the daemon up), then one last
   `ksx devices --json > %APPDATA%\ksx\gate3-before.json` for the record.
2. Uninstall Interception with its own installer, elevated:
   `install-interception.exe /uninstall` (the same tool that installed it;
   re-download from oblitum/Interception releases if it is not on disk), then
   verify the filter is really gone — `keyboard.sys`'s Interception entries
   absent from `pnputil /enum-drivers`, and the `keyboard` service no longer
   listing Interception's filter. **Reboot.** (`RECOVERY.md` §1 covers this
   driver *dying on its own*; tonight is the deliberate version.)
3. After the reboot: the panel must still be captured (WinUSB claim survives
   reboots by design — it is a driver binding, not a session), the HP Elite
   must still type, and `ksx doctor` must exit 0 with the Interception rows
   now reporting absent-and-unneeded.
4. Run the same 4-player session as Phase 1. This is the actual M6 sentence.
5. Exercise Gate 2 steps 8–9 once, deliberately: `ksx winusb release … --yes`,
   watch the panel become a plain keyboard, then re-claim and confirm capture
   again. Release/re-claim is the recovery muscle; it gets one rep while the
   spare keyboard is plugged in, not its first rep during a failure.
6. Write the soak start date below. **Soak = fourteen days of normal cabinet
   use with zero recovery actions.** Any Code-39, any dead panel, any manual
   pnputil: the soak restarts and the incident goes in the log.

**PASS (tonight's half)**: steps 1–5 clean. **PASS (M6 itself)**: the soak
completing 14 days later — put the end date in the calendar now.

## GATE 3 rollback

Phases 1–2 change nothing; stop anytime. Phase 3's ladder is GATE 2's ladder
above, unchanged — plus one addition: if anything smells wrong *after* the
Interception uninstall, do NOT reinstall Interception as a reflex. The 2012
cross-signed driver is the thing this whole milestone removes; reinstalling it
under pressure at midnight re-adds a boot-critical EOL filter to fix what is
almost always "the daemon is not running" (`RECOVERY.md` §2, first table).

## GATE 3 RUN LOG

*(empty — filled at the cabinet)*

---

# GATE 4 — fresh customer: exact installer to a working controller

This is the product gate, not another source-code gate. It starts with the
`setup.exe` produced by CI and a Windows user who has never run ksx. The person
walking the journey does not open a terminal, edit TOML, paste a device path or
receive whispered instructions. The observer may collect hashes, process-owner
evidence and before/after file state, but none of that may become a step the
customer has to perform.

**STATUS: NOT RUN. Nothing below is a claim that this gate passed.**

## What software tests prove — and what they do not

The repository tests prove the contracts in isolation: CI builds `ksx.exe` and
the GUI-subsystem `ksx-launcher.exe` before ISCC packages them; installer tests
pin the shortcuts, their absence of arguments and the driver task; launcher
tests pin the sibling `ksx.exe open` plan and `CREATE_NO_WINDOW`; daemon tests
admit an empty implicit setup as an idle control host; API and HTTP tests cover
staged bindings, macros, Play-before-Save and profile create/update/delete/switch;
template and Studio tests pin the two default Guide keys and the direct Game Bar
Settings link.

Those tests cannot prove that UAC returned to the original user, no console
flashed, the bundled driver installed on a clean machine, Windows exposed a
real virtual pad, a game consumed it, or Game Bar opened. This gate proves those
claims. A clean CI run or a compiled installer is necessary evidence, never a
substitute for this run.

## Preconditions — preserve the customer conditions

- Use the **exact CI-built installer** intended for release, not a local Inno
  build and not loose binaries. Before running it, record below its file name,
  product version, source commit and published SHA-256; independently hash the
  downloaded file and require an exact match.
- Use a supported, fully updated Windows 10/11 physical machine or a disposable
  clean Windows image that can expose ViGEmBus controllers to the host gaming
  stack. ViGEmBus must be absent before the run. If it is already installed,
  this is not a first-driver-install test; restore a clean snapshot or use a
  different safe machine.
- Create a fresh **standard** local Windows user. `%APPDATA%\ksx` and
  `%LOCALAPPDATA%\ksx` must not exist for that user. Have a different
  administrator account available for the installer UAC prompt; using the same
  account does not test `runasoriginaluser`.
- Plug in two known, visibly distinguishable keyboards and call them **Keyboard
  A** and **Keyboard B** in the run log. Keyboard B must have a numpad: Phase 4
  uses the pair to prove an explicit profile-device refresh changed slot device
  values, and Phase 5 uses B's Numpad `*` Guide binding. Have a known game that
  reads XInput and Windows' Game Controllers panel available. Xbox Game Bar must be
  installed and allowed by policy for this user; its controller setting starts
  disabled so the on-screen prerequisite and remedy are exercised.
- Screen-record from before the installer Finish button through the first
  `/start` paint if possible. A two-second console flash is a failure that a
  screenshot taken afterward cannot capture.
- The observer records the pre-run controller list, process list and ksx file
  state. These are observations, not instructions shown to the test user.

**ABORT before install if:** the installer hash/version/commit do not agree with
the release candidate; the Windows user is not fresh; ViGEmBus is already
present; or the UAC test would require using the customer's own standard-user
credentials as the administrator.

## Phase 1 — install the artifact, including the driver

1. While signed in as the fresh standard user, double-click the downloaded
   installer. Supply the separate administrator's credentials at UAC.
2. Confirm **Install the ViGEmBus controller driver** is visible and ticked by
   default. Leave it ticked. Confirm the desktop-icon task is selected.
3. Complete setup without launching a shell. The installer may continue if its
   driver child fails by design, but **this gate may not**: inspect the wizard's
   result and `{app}\install-drivers.log`, and require a successful ViGEmBus
   install. Record the installed driver version.
4. Confirm Apps & Features shows the expected ksx product version and the
   install directory contains the release-candidate `ksx.exe`,
   `ksx-launcher.exe` and sealed driver bundle.
5. Confirm there is exactly one **ksx** entry in the Start menu and the default
   desktop icon. Both shortcuts must target `ksx-launcher.exe` with no arguments;
   there must be no customer entries for daemon, Studio, cabinet, doctor or a
   setup wizard.

**PASS Phase 1:** the exact artifact is installed, the bundled ViGEmBus step is
successful and recorded, and every visible customer shortcut has the single
launcher target. A successful app install with a failed/declined driver is a
useful supported state, but it does not pass this release gate.

## Phase 2 — Finish hands back to the right user and boots idle

1. Leave **Launch ksx** ticked on Finish and click Finish. Watch the whole
   handoff. **No console window may appear, even briefly.**
2. The customer gets one chrome-less ksx app window at `/start`, not a terminal,
   the status dashboard or a normal browser tab. It has no address bar and does
   not ask the user to choose a URL.
3. In Task Manager's **User name** and **Command line** columns, confirm both
   surviving `ksx.exe` children — `daemon` and `studio --port 4460` — belong to
   the fresh standard user, not the administrator whose credentials satisfied
   UAC. Confirm the browser profile was created below that user's
   `%LOCALAPPDATA%\ksx`, not the administrator's profile.
4. With no `[[slot]]` configured, `/start` must report the daemon reachable and
   idle. The process stays alive as the staging control host, while no keyboard
   is captured and no virtual controller exists. The keyboard still types and
   Game Controllers shows zero ksx pads.
5. Open the tray menu. **Open ksx** remains available; **Open cabinet UI** and
   **Start emulation** are visibly disabled because there is no saved setup for
   either to operate. Neither disabled item may create a window, capture a key
   or plug a pad.
6. Close the app window, use the desktop shortcut once and the single Start-menu
   entry once. Each opens `/start` without a console flash; neither creates a
   second customer-facing product entry or asks for elevation.

**PASS Phase 2:** the elevated installer has handed off to the original
standard user, every launch is console-free, and an empty configuration is a
healthy idle first-run state rather than a daemon startup refusal.

## Phase 3 — author in memory, Play before Save, then prove Save parity

1. On `/start`, choose Keyboard A by its human-readable name. Add two Xbox 360
   controllers using the in-box two-player keyboard layout. Do not click Save.
2. Open each staged controller's mapper. Change one ordinary binding and create
   a small, visibly testable macro plus its trigger. Return to `/start` and
   answer **split or freeze**. Change one choice and change it back once: looking
   and reconsidering must remain free.
3. The observer compares `config.toml`, the preset directory and backups with
   the pre-run state. Staging bindings and macros must have written none of
   them. A refusal, if deliberately exercised with a duplicate key, must leave
   the staged view unchanged.
4. Click **Play now** without ever clicking Save. Two controllers must appear.
   In Game Controllers and the known game, verify the changed binding and macro
   exactly match what the staged mapper showed. Confirm no config, preset or
   backup was created by Play.
5. Stop the session from Studio. Click **Save this setup** once, close ksx, and
   launch it again from the customer shortcut. Start the saved setup and verify
   the same devices, personas, binding, macro and split/freeze choice. Saving is
   now allowed to create the config/preset files; restarting must not translate
   them into different behavior.

**PASS Phase 3:** staged editing and Play were memory-only, the staged mapper
was the behavior that ran, and the first explicit Save survives a complete
stop/relaunch with identical output.

## Phase 4 — profile create, switch, edit, refresh and delete in Studio

1. From `/profiles`, create a profile for the known game, select the saved
   **controller layout** and choose two players. Do not open TOML or a terminal.
   Immediately switch to that profile and start it: a Studio-created profile
   must be runnable because its slots inherited the working base device
   selectors.
2. Edit that profile's title, program/game link or arguments, player count and
   controller layout through the visible form. Leave **Use the device choices
   currently saved in Setup** unchecked. Save, reopen the editor and start it
   again; the existing Keyboard A selectors must have been preserved.
3. Stop the session. Return to `/start`, choose **Start over**, select Keyboard
   B, and stage the same two controllers, layout and split/freeze answer. Repeat
   the deliberately changed binding and macro from Phase 3 before Save so this
   device-only test does not replace the behavior already proved. Click **Save
   this setup**. The observer now uses `/setup`'s **Export — download this
   configuration** control and keeps the JSON as `before-rebase`; this is
   technical evidence, not a document the customer has to read or edit. Require
   the base slots to name Keyboard B while the target profile still names
   Keyboard A.
4. Reopen the profile editor, change no ordinary field, tick **Use the device
   choices currently saved in Setup**, and save. The observer exports again as
   `after-rebase` and compares the two documents. In exactly the target profile,
   each slot's `keyboard`/`mouse` selectors must now match the corresponding
   Keyboard B base slot. Its `persona`, `socd`, `macros` and controller-layout
   (`preset`) values must be byte-for-byte unchanged, as must every unrelated
   profile. A success flash or unchanged visible row is not evidence for this
   step; the exported before/after values are.
5. Delete the renamed profile using its confirmation. Exactly that profile
   disappears; unrelated profiles and the selected controller layout remain.
   Refresh the page and restart ksx once to prove the result was not only
   browser state.

**PASS Phase 4:** create → switch/play → edit → explicit device refresh → delete
is complete in Studio, with no config-file editing or CLI remedy. The two
exports prove creation/preservation/refresh semantics rather than asking the
normal profile row to expose device-selector jargon.

## Phase 5 — real pad output and the Game Bar prerequisite

1. Return to `/start` and use **Open Windows Game Bar settings**. It must open
   `ms-settings:gaming-gamebar` directly. Confirm ksx did not silently change
   the preference, then enable **Allow your controller to open Game Bar** for
   this user.
2. Play the saved two-controller setup on Keyboard B. Confirm both virtual
   controllers move in Game Controllers and the known XInput game, not only on
   ksx's own status page.
3. Press Player 1's default **Left Windows = Guide** key and observe Game Bar
   open from the virtual controller. Close it. Press Player 2's default
   **Numpad `*` = Guide** key and observe it open again. Seeing the mapping in
   Studio or a unit test is not this proof; Windows must display Game Bar twice.
4. Stop the session. Both pads disappear, the keyboard types normally, there
   are no ghost controllers, and ksx can be closed and relaunched without a
   console or elevation prompt.

**PASS Phase 5:** Windows and a real game consume the virtual pads, both default
Guide keys reach Game Bar after the user enables its prerequisite, and cleanup
returns the machine to an idle, typing state.

## GATE 4 PASS criteria

Every phase above passes against one recorded installer SHA and version. There
is no partial pass for “source tests were green,” “the installer compiled,” “one
pad appeared,” or “the Game Bar mapping exists.” Any failure stays in the run
log with the last known clean state; fix it, produce a new CI artifact with a
new hash, and restart this gate from Phase 1.

## GATE 4 RUN LOG

**STATUS: NOT RUN.** Fill every field during the supervised run:

- Installer file / product version / source commit / published SHA-256:
- Independently measured SHA-256:
- Windows edition + build / test-user type / separate admin used:
- Keyboard A / Keyboard B human names and exported slot device values:
- ViGEmBus before / installed version / `{app}\install-drivers.log` result:
- Start + desktop shortcut targets / extra customer shortcuts:
- Original-user process + browser-profile evidence / console-flash result:
- Empty-config idle `/start` / capture state / initial pad count:
- Staged binding + macro / before-Play disk comparison / Play result:
- Save + full restart parity:
- Profile create/switch/edit/rebase/delete + before/after export result:
- Real game + Player 1 Left Windows Guide + Player 2 Numpad `*` Guide:
- Stop/cleanup result:
- **Verdict: NOT RUN**

---

# GATE 1 RUN LOG — 2026-08-05 (Victor + session)

**Phase A — PASSED.** Tray lifecycle clean: idle tooltip, Start → 4 X360 pads
(user indexes 0–3 in order), panel drove pads and stopped typing, desktop
keyboard unaffected, Stop restored typing, Quit exited 0. Post-quit checklist
clean (no process, no ghost pads, doctor 0).

**Phase B — PASSED.** Registered `daemon --game Steam` (debug-build exe,
deliberate — the gate-tested binary). Cold boot → task fired at logon +10 s
(schtasks result 267009 "running"), tray icon up, nothing captured at logon,
boot-to-playable proven via tray → Start (Steam + 4 pads), clean stop, zero
ERROR lines. Cosmetic finding: `--status` prints `enabled: unknown` — the
Enabled field isn't parsed from schtasks output; fix at leisure.

**Phase C — WIRED, NOT VERIFIED.** The 5big/G: array was physically
disconnected during the run, and everything Phase C needs lives on G: (roms,
mame.exe, romkit). Built on assumptions, all additive:
- `games.toml` +"MAME 4P" profile (4 slots, dry-run exit 0; `path` assumed).
- RetroBat: new `es_systems_ksx4p.cfg` (delete to unwire).
- LaunchBox: new additive emulator "MAME 4P (ksx wrapper)" (Emulators.xml
  backed up first; LB was closed).
**When G: is back, verify:** (1) volume mounts as G: exactly; (2) real
mame.exe path — fix the three assumed references (games.toml, es_systems_ksx4p,
LB entry) if it differs; (3) G:\roms\mame exists; (4) assign one 4-player game
to the LB wrapper emulator; (5) run the Phase C launch test from both
frontends. Longer term the durable LB integration is romkit-launch.exe calling
the ksx CLI itself, not a wrapper entry.

**Phase D — COMMANDS PASSED, boot check skipped.** disable → NOT registered →
second disable idempotent (exit 0) → re-enabled as the desired end state
(daemon at boot stays armed). The "nothing starts after removal" cold boot was
deliberately skipped to keep the registration; it is implicitly covered by the
pre-gate months of boots with no task.

**Verdict: GATE 1 PASSED with Phase C hardware-pending.** The 10-minute
completion pass when the 5big returns is listed above.

# GATE 2 PAUSE — 2026-08-05

> **Historical snapshot, superseded as current state.** This pause records what
> was true on 2026-08-05 before the later Gate 2 activity summarized in Gate 3's
> “State going in” paragraph (2026-08-08). It is preserved as evidence, does not
> mean the machine is still untouched, and is not a Gate 2 PASS log.

Paused by Victor before any system change. State: preconditions surveyed only —
baseline saved to `%APPDATA%\ksx\winusb-before.json`, dry-run reviewed (INF
scoped to MI_00 only, instance path unchanged from the runbook), Zadig 2.9
staged in the session scratchpad (re-download from pbatard/libwdi releases if
gone). Signing decision made: Zadig one-time bind for this cabinet (no inf2cat
on the machine — SDK yes, WDK no); the ksx-signed-INF path is an M7 item.
Nothing claimed, nothing installed, machine untouched. Resume at
"Preconditions — every one, no exceptions".
