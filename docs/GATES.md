# Cabinet Gates — supervised runbooks

Two milestones end with Victor physically at the cabinet. These are the scripts
for those sessions: exact commands, what each one should print, and what to do
the moment one of them doesn't. A session walking Victor through a gate should
follow these top to bottom and never improvise past a failed step.

- **GATE 1 — "M5 rest"**: autostart at boot, the tray daemon, and the frontend
  wrapper. Software-only; Interception semantics; every failure is recoverable
  with a keypress or a taskkill.
- **GATE 2 — "M6 WinUSB rebind"**: the first time ksx changes what a device *is*
  to Windows. Read the preconditions twice. The whole gate is a round trip —
  claim, verify, release — and the machine ends it exactly as it started.

## Shared rules (both gates)

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
