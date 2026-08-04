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
ksx run --game "Street Fighter"   # layout + block flags from a games.toml profile
ksx run --dry-run            # resolve and print the plan; touches no driver
ksx run --latency            # rolling capture→submit p50/p99/max every 5 s
ksx run --json               # one summary object on stdout (human text on stderr)
```

Startup order is deliberate: pads first (a missing ViGEmBus is found while every
keyboard is still normal), then capture in passthrough, then blocking — for the
bound keyboards only.

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
| 0 | clean stop — the `Ctrl+Alt+Del` escape, `--dry-run`, or `Ctrl+C` where it can be delivered |
| 1 | unexpected error |
| 2 | refused to start: invalid config, unknown `--game`, a missing driver, two keyboards sharing one hardware id, or any pad-plug failure. Nothing was plugged and no filter was set |
| 3 | started, then a runtime failure tore it down (thread death, capture panic, stall watchdog). Keyboards were released first |

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
| M5 | Profiles, autostart, tray daemon, installer | – |
| M6 | WinUSB capture backend (post-2026 survival path) | – |
| M7 | UI | – |

## Workspace

```
crates/ksx-core           pure mapping engine (CI-tested, proptest)
crates/ksx-config         TOML config + presets
crates/ksx-legacy-import  legacy UTF-16 XML → TOML importer
crates/ksx-capture        CaptureBackend: interception / winusb / rawinput-identify
crates/ksx-output         VirtualPadBackend: ViGEmBus
crates/ksx-platform       hotplug, driver health, install, autostart
crates/ksx-games          game/profile registry + launch
crates/ksx-app            the `ksx` binary
crates/vigem-client       vendored CasualX/vigem-client (MIT)
legacy/                   original C# solution (frozen, reference only)
docs/                     architecture, driver story, recovery runbook, research
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
