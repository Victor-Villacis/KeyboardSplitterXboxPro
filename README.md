# KeyboardSplitterXboxPro (`ksx`)

Split one or more keyboards — including arcade encoders like the Ultimarc I-PAC that
present as keyboards — into up to **4 virtual Xbox 360 controllers** on Windows 11.

This is a ground-up **Rust** rebuild of djlastnight's
[KeyboardSplitterXbox](https://github.com/djlastnight/KeyboardSplitterXbox)
(2016, C#/.NET 4.0, unmaintained). The original lives on in [`legacy/`](legacy/) as the
behavior reference; the Rust workspace at the repo root is the project now.

## Why rebuild it

The legacy app still works, but both of its drivers are dead ends in 2026:

| Legacy component | Problem | Replacement |
|---|---|---|
| [Interception](https://github.com/oblitum/Interception) capture driver | Cross-signed with a cert that expired in 2012; Microsoft's 2026 servicing update removes trust for the entire cross-signing program (this repo's `docs/research/keyboard-capture-2026.md` has the full story). Author unreachable since ~2017; 10-device ID-drift limit. | Kept short-term via [`kanata-interception`](https://crates.io/crates/kanata-interception); replaced by a **WinUSB/`nusb` direct-claim backend** (in-box driver, structural blocking + identity, no signing cliff). |
| ScpVBus + vXboxInterface virtual bus | Pre-ViGEm abandonware, expired certs, version-pinned installs. | **ViGEmBus 1.22.0** (attestation-signed, real XInput slots via Microsoft's own `xusb22.sys`) through a vendored pure-Rust [`vigem-client`](https://github.com/CasualX/vigem-client). |

No maintained alternative to this tool exists — the 2026 survey of reWASD, UCR, kanata,
XOutput and friends is in [`docs/research/`](docs/research/).

## Core behavior (preserved from the legacy app, property-tested)

- **One keyboard → many pads**: a single I-PAC4 fans out to 4 controllers via disjoint
  key-subset presets. Per-device routing alone is not enough; this is first-class.
- Per-device capture with **OS-input blocking** scoped to assigned devices, active only
  while emulation runs — unassigned keyboards keep typing.
- Emergency escapes evaluated in the capture thread, before the blocking decision and
  upstream of every queue, so nothing can starve them: `LCtrl ×5` toggles keyboard
  capture, `RCtrl ×5` mouse capture, `Ctrl+Alt+Del` stops emulation.
- Crash-safe by construction: if `ksx` dies or hangs, a drop guard + watchdog return
  your keyboards. No reboot, ever.
- All-keys-up release rule, opposite-axis snap, state-diffed pad updates.

## `ksx run`

Plugs the virtual pads, captures the keyboards your slots are bound to, and
translates. Everything else on the machine keeps typing.

```sh
ksx run                      # slot layout from config.toml
ksx run --game "Street Fighter"   # layout + block flags from a games.toml profile,
                                  # and start the profile's program
ksx run --game "X" --no-launch    # apply the profile, launch nothing
ksx run --dry-run            # resolve and print the plan; touches no driver
ksx run --latency            # rolling capture→submit p50/p99/max every 5 s
ksx run --json               # one summary object on stdout (human text on stderr)
```

Startup order is deliberate: pads first (a missing ViGEmBus is found while every
keyboard is still normal), then capture in passthrough, then blocking — for the
bound keyboards only — and only **then** the game. A game started before the
pads exist enumerates zero controllers and never asks again.

### Launching a game (`--game`)

When the profile has a `path`, `ksx run --game` starts it after the pads are up
and stops emulation when it exits (exit 0). Two behaviours ported from the
legacy app and extended:

- **A process that exits within 10 seconds was a launcher, not the game.**
  `steam.exe`, a `.bat`, a 32→64-bit trampoline: they hand off and return.
  Legacy used 3 s and that was measurably too tight — Steam on this cabinet
  takes 5 s to hand off, and stopping emulation then leaves a player mid-launch
  with no pads. Tune it per profile with `launcher_grace_ms` (milliseconds):
  lower it if you want a short session noticed sooner, raise it for a launcher
  that is slower still.
- **After a hand-off, ksx hunts for the profile's `process_name` for 60 s** and
  then follows *that* process to its exit. This is a separate timer from the one
  above and is unchanged. Legacy stopped at the launcher rule and simply ran
  forever.

```toml
[[game]]
title = "Portal 2"
path  = "steam://rungameid/620"
process_name = "portal2.exe"   # required for URLs: the shell returns instantly
```

A `steam://` profile with no `process_name` gets a loud warning naming the exact
file and the line to add — and runs anyway. The pads work, the game works, and
the emergency escapes still end the session.

**ksx never kills a game it started.** Stopping emulation leaves the game
running; your keyboard simply starts typing into it again.

### Emergency escapes

Printed as a banner before anything can block a keystroke, and evaluated on
**every** keyboard, captured or not — so they work from a keyboard the config
never mentions, with a fullscreen game holding focus:

| Hotkey | Effect |
|---|---|
| `LeftCtrl` ×5 | toggle keyboard capture off/on (beep confirms; high = on) |
| `RightCtrl` ×5 | reserved for mouse capture — logged only, M4 never touches `mouse.sys` |
| `Ctrl`+`Alt`+`Del` | stop emulation: unplug the pads, release the keyboards, exit 0 |

Press the same key 5 times with no other key in between. The gestures are
evaluated **inside the capture thread**, before the pass/suppress decision, so
they keep working when everything downstream (engine, output thread, a ViGEm
driver call) is wedged.

#### `Ctrl+C` does not work while your keyboards are captured

Interception suppresses captured strokes *below win32k*, so Windows never
generates a `CTRL_C_EVENT` and `ksx`'s console handler never runs. Use
`LeftCtrl ×5` or `Ctrl`+`Alt`+`Del` instead. `Ctrl+C` only works from a keyboard
`ksx` is **not** capturing, or before blocking is enabled.

`taskkill /f /im ksx.exe` also returns every keyboard — process death frees the
filters with no cleanup at all — but you need an input device you can still act
from. The mouse is never captured in M4, so a mouse-driven Task Manager is
always a way out.

### Exit codes

| code | meaning |
|---|---|
| 0 | clean stop — the `Ctrl+Alt+Del` escape, **the `--game` game exiting**, `--dry-run`, or `Ctrl+C` where it can be delivered |
| 1 | unexpected error |
| 2 | refused to start: invalid config, unknown `--game`, **a `--game` profile whose exe does not exist**, a missing driver, two keyboards sharing one hardware id, or any pad-plug failure. Nothing was plugged and no filter was set |
| 3 | started, then a runtime failure tore it down (thread death, capture panic, stall watchdog, **a game that failed to launch**). Keyboards were released first |

The 2/3 line is exactly "was a keyboard filter ever armed". A 2 means the
machine is untouched.

## The other commands

```sh
ksx devices                       # keyboards as the driver sees them (read-only)
ksx monitor --for-secs 10         # live per-device key stream, never blocks
ksx pads --count 4                # plug 4 test pads, LED order, kill-recovery
ksx doctor                        # driver health, CI-policy state, verdicts
ksx import-legacy --dry-run       # legacy XML -> TOML
ksx winusb status                 # WinUSB claim state per USB interface (read-only)
```

### `ksx daemon` — stay resident with a tray icon

```sh
ksx daemon --game "MAME 4P"       # tray icon; emulation on demand
ksx daemon --headless             # same commands on stdin
ksx daemon --start                # begin a session immediately
```

Menu (and headless commands): **start**, **stop**, **reload** config, open
**config** folder, **quit**. The tooltip shows the current state and any capture
health problem from the last session (reboot required, watchdog tripped, dropped
events).

The tray runs on its own thread with its own message pump and has **no** path to
the capture, engine or output threads — it can only enqueue a command. A wedged
tray costs you a menu, never your keyboards. (The legacy app dispatched every
keystroke onto its WPF UI thread; a stalled UI froze every keyboard on the
machine until reboot.)

Exit codes: 0 clean, 1 error, 2 the configuration does not resolve.

### `ksx autostart` — cold boot to a live tray

```sh
ksx autostart --enable --game "MAME 4P"
ksx autostart --status
ksx autostart --disable
ksx autostart --enable --dry-run     # exact XML + schtasks line, registers nothing
```

A **per-user** logon task — `InteractiveToken`, `LeastPrivilege`, never elevated
— running `ksx daemon [--game <TITLE>]` 10 seconds after logon: the tray icon
comes up and **nothing is captured** until a session is started from the tray or
a wrapper. That default is deliberate — a registered `ksx run` would grab the
assigned keyboards at every logon, desktop use included. `--mode run` registers
exactly that instead, for kiosk cabinets where logon-straight-into-the-game is
the point. Idempotent.

`--enable` validates before it registers: the config must pass the same checks
`ksx run` applies, the profile must exist, and its executable must be on disk.
Otherwise it refuses with exit 2 — a typo caught here is one line of output, the
same typo registered is a cabinet that cold-boots to nothing on a console nobody
sees.

`--status` also reports a **stale** registration (ksx moved, the task did not)
and exits 2 when it finds one.

Exit codes: 0 done, 1 error, 2 refused / stale.

### `ksx install-drivers` — the bundled ViGEmBus, verified

```sh
ksx install-drivers                 # report + verify; runs nothing
ksx install-drivers --dry-run
ksx install-drivers --yes           # execute (needs an elevated terminal)
ksx install-drivers --repair --yes  # run setup again over an existing install
```

Two independent pins must both hold before anything runs: the installer's
**SHA-256** and its **Authenticode signer**, both recorded in
[`docs/DRIVERS.md`](docs/DRIVERS.md). The file is opened **once** with writers
and deleters locked out, hashed and signature-checked through that handle, and
the handle stays open across execution — so the bytes that were checked are the
bytes that run. When elevated, ksx also refuses to search any directory a
standard user could write to.

A file that fails verification is refused, and ksx will not print a command line
for it either. ksx never downloads anything and never self-elevates.
Interception is reported but never installed (non-commercial licence).

The signer pin is not "the certificate is valid today" — code-signing
certificates outlive by design the binaries they sign. ksx does what Windows
does: an **expired** certificate is accepted only when a timestamp
countersignature, *verified* rather than assumed, places the signing time inside
the certificate's validity window. The bundled ViGEmBus 1.22.0 asset is exactly
that case (certificate expired 2025-02-16, signature timestamped 2023-11-02), and
`install-drivers` reports it as `expired-timestamp-verified` with the date shown.
An expired certificate with no timestamp that survives checking is still refused.
See [`docs/DRIVERS.md`](docs/DRIVERS.md) for the four checks and the state codes.

Exit codes: 0 nothing to do / installed, 1 error, 2 refused (verification
failed, installer missing, elevation needed), 3 the installer ran and failed.

### `ksx winusb` — escape the 2026 driver cliff

```sh
ksx winusb status                       # read-only: which interfaces, which driver, claimable?
ksx winusb status --json
ksx winusb claim "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000"        # DRY RUN
ksx winusb claim "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000" --yes  # elevated
ksx winusb release "USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000" --yes
```

Rebinds **one** USB interface from the keyboard stack to Microsoft's in-box
`winusb.sys`, after which ksx reads its HID interrupt endpoint directly. Blocking
and identity stop being enforced and start being structural: the interface is not
in the keyboard stack, so nothing else on the machine can see a keystroke from it,
and two identical I-PACs stop being indistinguishable ([`USE-CASES.md`](docs/USE-CASES.md) T4).
No third-party kernel driver, so nothing to expire in 2026.

**The trade, and you should decide about it before you run anything:** a claimed
panel is no longer a keyboard. It types only while `ksx daemon` is running — the
daemon holds the claim for its whole lifetime and re-injects the panel's
keystrokes with `SendInput` whenever emulation is stopped, including between two
games, so frontend menus keep working — and **if ksx is not running it does
nothing at all.** Injected keys also never reach the lock screen, a UAC prompt or
`Ctrl+Alt+Del`. Mitigations: `ksx autostart --enable`, one ordinary keyboard on
another port, and the one-command rollback. `claim` **refuses** to take the
machine's last keyboard (exit 2) — counting only keyboards that can type right
now, one per physical board, so a claimed, disabled or paired-but-disconnected
keyboard cannot be mistaken for your spare.

Both mutating verbs are **dry runs by default**: they print the exact INF and the
exact `pnputil` command line and change nothing until `--yes`. The INF must be
signed before `pnputil` will take it (`winusb.sys` is signed; a third-party INF
pointing at it is not) — the dry run prints the `signtool` recipe and the Zadig
alternative, and explicitly tells you not to enable test-signing.

Migration walkthrough: [`docs/MIGRATION-WINUSB.md`](docs/MIGRATION-WINUSB.md).
Rollback: [`docs/RECOVERY.md`](docs/RECOVERY.md) §2 — including the Device
Manager route that needs only a mouse.

Exit codes: 0 reported/done, 1 error, 2 refused (unknown or ambiguous device,
not a keyboard interface, already claimed, elevation needed, **or it is the only
keyboard**), 3 pnputil ran and failed.

### Frontend integration

LaunchBox and RetroBat wiring, plus a wrapper that always stops ksx:
[`docs/INTEGRATION.md`](docs/INTEGRATION.md) and
[`examples/ksx-wrap.ps1`](examples/ksx-wrap.ps1).

## Status

Backend-first rewrite in progress — milestone plan in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), post-parity ideas (more pad types,
8-player, key remapping, AI-drivable CLI) in
[`docs/ENHANCEMENTS.md`](docs/ENHANCEMENTS.md). The legacy app remains the working
fallback until `ksx` passes the full cabinet test matrix.

| Milestone | Scope | Status |
|---|---|---|
| M0 | Repo restructure + workspace scaffold | ✅ |
| M1 | Pure engine + TOML config + legacy XML importer | ✅ |
| M2 | ViGEm output layer | ✅ |
| M3 | Interception capture layer | ✅ |
| M4 | End-to-end parity (`ksx run`) | ⏳ code complete, cabinet gate pending |
| M5 | Profiles, autostart, tray daemon, installer | ⏳ code complete, cabinet gate pending |
| M6 | WinUSB claim: rebind tooling, recovery, keystroke re-injection | ⏳ tooling + injection done, backend + cabinet gate pending |
| M7 | UI | – |

## Workspace

```
crates/ksx-core           pure mapping engine (CI-tested, proptest)
crates/ksx-config         TOML config + presets
crates/ksx-legacy-import  legacy UTF-16 XML → TOML importer
crates/ksx-capture        CaptureBackend: interception / winusb / rawinput-identify
crates/ksx-output         VirtualPadBackend: ViGEmBus
crates/ksx-platform       driver health, install, autostart, WinUSB rebind, SendInput
crates/ksx-games          game launch + exit detection (launcher hand-off)
crates/ksx-app            the `ksx` binary
crates/vigem-client       vendored CasualX/vigem-client (MIT)
legacy/                   original C# solution (frozen, reference only)
examples/                 frontend wrapper scripts
docs/                     architecture, integration, driver story, recovery, migration, research
```

## License

New Rust code: MIT OR Apache-2.0. Third-party driver/binary terms are catalogued in
[`docs/DRIVERS.md`](docs/DRIVERS.md) — notably Interception is LGPL/**non-commercial**
and the vendored `vigem-client` is MIT.

## Credits

- **djlastnight** — the original Gaming Keyboard Splitter
- **Francisco Lopes (oblitum)** — Interception
- **Nefarius Software Solutions / Benjamin Höglinger-Stelzer** — ViGEmBus
- **shauleiz** — vXboxInterface
- **CasualX** — vigem-client
- **jtroo** — kanata-interception
