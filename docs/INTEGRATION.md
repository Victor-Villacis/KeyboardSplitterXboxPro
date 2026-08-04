# Frontend integration (LaunchBox, RetroBat, anything else)

How ksx fits into a cabinet that is already driven by a frontend. Three
patterns, in order of how much you want ksx to be in charge.

The rule underneath all of them: **something must always stop ksx.** While ksx
runs, the keyboards bound to slots are captured — that is the whole product. A
frontend that launches ksx and forgets it leaves a cabinet whose panel does not
type. Each pattern below says exactly what does the stopping.

---

## Pattern A — ksx launches the game (`ksx run --game`)

The simplest one, and the one the legacy `game=` autostart did.

```toml
# %APPDATA%\ksx\games.toml
[[game]]
title = "Street Fighter II"
path  = 'C:\RetroBat\emulators\mame\mame.exe'
arguments = "sf2ce"

[[game.slot]]
number = 1
keyboard = 'HID\VID_D209&PID_0430&REV_0056&MI_00'
preset = "IPAC P1"
# ...slots 2-4
```

```
ksx run --game "Street Fighter II"
```

ksx plugs the pads, arms capture, **then** starts MAME, and stops emulation the
moment MAME exits. Exit code 0. Nothing else is involved, so nothing else can
forget to clean up.

**Point your frontend's "emulator" at ksx** with the profile name as the
argument, and let ksx run the emulator.

### When the thing you start is not the thing you play

Launchers hand off. `steam.exe` passes the request to the running client and
returns in under a second; a `.bat`, a 32→64-bit trampoline and most storefronts
do the same. ksx keeps the legacy rule in shape but not in number — **a process
that exits within 10 seconds was a launcher, not the game** (legacy's 3 s
mis-classified a 5-second `steam.exe` hand-off as the player quitting; override
per profile with `launcher_grace_ms`) — and then goes looking for the real one:

```toml
[[game]]
title = "Portal 2"
path  = "steam://rungameid/620"
process_name = "portal2.exe"   # <- what to follow after the hand-off
```

For 60 seconds after the hand-off ksx watches for `process_name`; when it
appears, that process becomes the session. Quit it and emulation stops.

If a `steam://` profile has **no** `process_name`, ksx says so loudly, names the
file and the line to add, and **runs anyway** — the pads work, the game works,
and the emergency escapes still end the session. It does not refuse.

---

## Pattern B — the frontend launches the emulator, wrapped

Use this when the frontend must stay in charge of the emulator (per-game
arguments, media hooks, its own exit handling — LaunchBox's normal mode).

`examples/ksx-wrap.ps1` starts ksx, runs the emulator, and stops ksx in a
`finally` block. Every exit path is traced in the script's own header comment.

### LaunchBox

LaunchBox has no per-emulator wrapper field, so wrap at the emulator level:

1. **Tools → Manage → Emulators → (your emulator) → Edit**
2. Set **Application Path** to `powershell.exe`
3. Set **Default Command-Line Parameters** to:

   ```
   -NoProfile -ExecutionPolicy Bypass -File "C:\ksx\examples\ksx-wrap.ps1" -Emulator "C:\RetroBat\emulators\mame\mame.exe" -Game "MAME 4P" --
   ```

   LaunchBox appends the ROM path after this, and everything after `--` goes to
   the emulator verbatim.

4. Untick **Use Quotes** only if your ROM paths have no spaces; leave it on
   otherwise.

Alternatively, LaunchBox's **Running Script** / **Exit Script** hooks
(Tools → Manage → Emulators → Edit → *Running Script*) can start and stop ksx
around a game. That is tidier in the UI but **less safe**: if LaunchBox crashes
between the two, the exit script never runs. The wrapper's `finally` covers
more paths, so prefer the wrapper.

### RetroBat / EmulationStation

RetroBat launches emulators through its own `emulatorLauncher`. The reliable
seam is the same wrapper, registered as a custom emulator in
`es_systems_*.cfg`:

```xml
<system>
  <name>mame</name>
  <command>powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\ksx\examples\ksx-wrap.ps1" -Emulator "C:\RetroBat\emulators\mame\mame.exe" -Game "MAME 4P" -- %ROM%</command>
</system>
```

`%ROM%` lands after `--` and is passed straight through.

> **Drive letters.** RetroBat's custom systems hardcode their ROM paths. If the
> array holding your ROMs changes drive letter, systems silently vanish and it
> looks like a reset. The same applies to the paths in this `<command>` — keep
> them on a fixed letter.

---

## Pattern C — the daemon (`ksx daemon`)

ksx stays resident with a tray icon and emulation is toggled on demand: Start,
Stop, Reload config, Open config folder, Quit. Use it when the cabinet runs a
desktop session and you want emulation available without a console window.

```
ksx daemon --game "MAME 4P"
ksx daemon --headless        # same commands on stdin: start|stop|reload|config|status|quit
```

The tray thread has no path to the capture, engine or output threads — it can
only enqueue a command — so a wedged tray costs you a menu, never your
keyboards. That separation is deliberate: the legacy app dispatched every
keystroke onto its WPF UI thread, and a stalled UI froze every keyboard on the
machine until reboot.

The tooltip shows the current state and surfaces capture-health problems
(reboot required, watchdog tripped, dropped events) from the last session.

---

## Starting at boot

```
ksx autostart --enable --game "MAME 4P"     # register
ksx autostart --status                      # what is registered, and is it stale
ksx autostart --disable                     # remove
ksx autostart --enable --dry-run            # print the exact XML + schtasks line
```

A **per-user** scheduled task (`InteractiveToken`, `LeastPrivilege` — never
elevated), triggered at logon with a 10-second delay so the shell, the frontend
and USB enumeration are settled first.

`--enable` **validates before it registers**: the configuration must pass the
same checks `ksx run` applies, the `--game` profile must exist, and its
executable must be on disk. Otherwise it refuses with exit 2. A typo caught here
is one line of output; the same typo registered is a cabinet that cold-boots to
nothing, on a console nobody is looking at.

`--status` also reports a **stale** registration — ksx moved or was reinstalled
and the task still points at the old path — and exits 2 when it finds one, so a
health check can notice.

---

## Exit codes a wrapper should branch on

| code | meaning | what a script should do |
|---|---|---|
| 0 | the session ended as asked (game exited, escape pressed, `--dry-run`) | continue |
| 1 | unexpected error | log it |
| 2 | **refused to start** — bad config, unknown profile, missing exe, missing driver, ambiguous keyboards. Nothing was plugged, no keyboard was ever captured | do **not** launch the game; show the error |
| 3 | started, then torn down by a runtime failure (including a failed game launch). Keyboards were released first | log it; the machine is in a safe state |

The 2/3 line is exactly "was a keyboard filter ever armed". A 2 means the
machine is untouched.

---

## Things that will bite you

**Never run ksx and the legacy KeyboardSplitter at the same time.** Eight
virtual pads into four XInput slots ends badly. The autostart task uses
`MultipleInstancesPolicy=IgnoreNew` for the same reason — a second logon must
not start a second session.

**Ctrl+C does not stop a captured session.** Interception suppresses captured
keystrokes below win32k, so Windows never generates a console break event. From
a script, kill the process instead — `taskkill /f /im ksx.exe`, or
`$proc.Kill()` as the wrapper does. That is safe by design: ksx is crash-only,
and the driver releases its filters when the process handle closes. Keyboards
come back in under a second with no cleanup.

**The emergency escapes always work**, including under a fullscreen game:
`LeftCtrl ×5` toggles capture off, `Ctrl+Alt+Del` stops emulation. They are
evaluated inside the capture thread, so they survive a wedged engine, a wedged
output thread, or a wedged frontend.

**ksx never closes the game it started.** Stopping emulation leaves the game
running; your keyboard simply starts typing into it again. A wrapper that wants
the game gone must do that itself.

**Start ksx before the emulator, always.** A game that starts first enumerates
zero controllers and caches the answer. Pattern A gets this right by
construction; Pattern B's wrapper waits for the pads before launching.
