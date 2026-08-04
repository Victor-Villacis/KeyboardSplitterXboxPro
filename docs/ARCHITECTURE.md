# ksx Architecture

Full design rationale: [`research/design-architecture.md`](research/design-architecture.md)
and [`research/design-risk-review.md`](research/design-risk-review.md).

## Pipeline

```
[capture thread]  TIME_CRITICAL. interception_receive / nusb interrupt read.
                  Only: resolve device → pass/suppress (arc-swap snapshot) →
                  bounded crossbeam channel (1024; drop+count, never block).
       ↓
[engine thread]   ksx-core: per-device key state → precompiled Key→(slot,Binding)
                  index → PadState mutation (all-keys-up, opposite-axis snap,
                  one-kbd→many-slots fan-out, emergency escapes) → diff → deltas.
       ↓
[output thread]   vigem update() per changed pad; LED/rumble via notification.

[main thread]     CLI/supervision/hot-reload; later tray + UI. May die freely:
                  the input path never notices.
```

Rules that keep us honest (each maps to a legacy defect — see risk review §1/§3):

1. **No tokio / no allocation / no locks in the capture thread.** The legacy app
   dispatched every keystroke synchronously onto the WPF UI thread; a stalled UI froze
   all input system-wide until reboot.
2. **Crash-only blocking.** Process death must return keyboards in <1 s without
   cleanup. A watchdog force-releases capture if the consumer stalls.
3. **Identity = device instance path** (`HID\VID_D209...\8&2A0D0500&0&0000`), never
   positional indices (`Keyboard_01` drifted on every replug).
4. **Slot number ≠ XInput user index.** User index comes from ViGEm's notification
   callback, not polling or guessing.
5. **p99 capture→submit latency < 1 ms**, measured by `ksx doctor --latency`
   (hdrhistogram); the histogram is a permanent debug feature, not a one-off.

## Milestones

| M | Scope | Exit criteria (cabinet-tested) |
|---|---|---|
| M0 | Restructure + scaffold | `cargo test` green; `ksx --version`; legacy app untouched |
| M1 | ksx-core + ksx-config + importer | `ksx import-legacy` converts the real cab XML with zero warnings; proptests green |
| M2 | ViGEm output | 4 pads in joy.cpl, LED order right, kill → pads vanish |
| M3 | Interception capture | attribution + blocking verified; taskkill recovery <1 s ×5; LCtrl×5 in fullscreen |
| M4 | End-to-end parity | 4-player real-game session via `ksx run`; p99 < 1 ms |
| M5 | Profiles/autostart/tray/installer | cold boot → daemon → frontend → game → clean exit |
| M6 | WinUSB backend | same session with Interception **uninstalled**; 2-week soak |
| M7 | UI (decision deferred; egui/eframe leading) | preset edit without hand-editing TOML |

## Interim operational caution (M0–M5)

While the Interception backend is the only capture path, **do not chase Windows
feature updates** on the cabinet: the machine already runs the cross-signed-driver
audit CI policy, and the audit→enforcement flip is what kills the legacy stack.
M6 removes the dependency. Recovery runbook: [`RECOVERY.md`](RECOVERY.md).
