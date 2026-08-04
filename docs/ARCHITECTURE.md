# ksx Architecture

Full design rationale: [`research/design-architecture.md`](research/design-architecture.md)
and [`research/design-risk-review.md`](research/design-risk-review.md).

## Pipeline

```
[capture thread]  TIME_CRITICAL. interception_receive / nusb interrupt read.
                  Only: resolve device → emergency escapes (evaluated HERE, before
                  the pass/suppress decision, acting on this thread's own
                  passthrough latch — nothing downstream can starve them) →
                  pass/suppress (arc-swap snapshot) → bounded crossbeam channel
                  (1024; drop+count, never block).
       ↓
[engine thread]   ksx-core: per-device key state → precompiled Key→(slot,Binding)
                  index → PadState mutation (all-keys-up, opposite-axis snap,
                  one-kbd→many-slots fan-out) → diff → deltas, try_send with
                  per-slot coalescing (a newer PadState supersedes an older one;
                  the engine never blocks on the output thread).
       ↓
[output thread]   vigem update() per changed pad; LED/rumble via notification.
                  Reports failures upward; the supervisor drives teardown order.

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

| M | Status | Scope | Exit criteria (cabinet-tested) |
|---|---|---|---|
| M0 | ✅ done | Restructure + scaffold | `cargo test` green; `ksx --version`; legacy app untouched |
| M1 | ✅ done | ksx-core + ksx-config + importer | `ksx import-legacy` converts the real cab XML with zero warnings; proptests green |
| M2 | ✅ done | ViGEm output | 4 pads in joy.cpl, LED order right, kill → pads vanish |
| M3 | ✅ done | Interception capture | attribution + blocking verified; taskkill recovery <1 s ×5; LCtrl×5 in fullscreen |
| M4 | 🔨 code done, cabinet gate pending | End-to-end parity (`ksx run`) | 4-player real-game session via `ksx run`; p99 < 1 ms |
| M5 | 🔨 code done, cabinet gate pending | Game launching, autostart, tray daemon, install-drivers, frontend integration | cold boot → daemon → frontend → game → clean exit |
| M6 | – | WinUSB backend | same session with Interception **uninstalled**; 2-week soak |
| M7 | – | UI (decision deferred; egui/eframe leading) | preset edit without hand-editing TOML |

### M4 supervisor (`crates/ksx-app/src/run/`)

`ksx run` is the pipeline above, wired up. Two orderings are contractual and
asserted in CI (`src/run/pipeline_tests.rs`, mock backends, no drivers):

- **startup** — plug pads and resolve their XInput user indices → start capture
  in passthrough → enable blocking for exactly the keyboards bound to slots.
  A bus failure therefore always happens while every keyboard is still normal.
- **teardown** — uncapture, *then* unplug. A drop guard sends `SetPassthrough`
  even on unwind, so a supervisor panic still frees the keyboards; a dead engine
  or output thread is treated as fatal for the same reason.

Slot layout comes from `[[slot]]`, or from a `games.toml` profile with
`--game <Title>` (M5 adds the launching). `--dry-run` resolves and prints the
plan without touching a driver. Hotplug: the capture thread republishes the
device set from its idle path; a bound device that disappears releases its keys
(`Engine::release_device`), invalidates its slots with the ported
`InvalidationReason`, and the session keeps running. Config hot-reload is **not**
in M4.

### M5 (`crates/ksx-games`, `crates/ksx-app/src/{daemon,autostart,install}.rs`)

Everything M5 adds hangs off the M4 pipeline without changing it. The pipeline's
two contractual orderings are unchanged and still asserted the same way.

**Game launching** is a `SessionHook` on the supervisor with a no-op default
(`NoHook`), called at one specific point in startup: after `PadsPlugged`,
`CaptureStarted` and `BlockingEnabled`. That ordering is contractual and asserted
in CI against the trace *as it stood when the game was spawned*, not against a
note written afterwards — a game started before the pads exist enumerates zero
controllers and caches that answer for the session.

Exit detection is a pure state machine (`ksx-games::tracker`) with the clock and
the process list injected, so the 3-second launcher rule and the 60-second
hand-off grace are exercised in microseconds by fakes:

```text
                 launched (handle held)
                          |
     alive ---------------+--------------- exited
       |                                     |
   [Running] <-------------------------  lifetime > 3s ? --yes--> [Exited] -> stop, exit 0
                                             |no  (it was a launcher)
                                             v
                                  process_name configured ? --no--> [Unresolvable]
                                             |yes                     warn once,
                                             v                        keep running
                                   [LauncherHandoff]  --60s--------------^
                                             | found it
                                             v
                                       [Tracking pid]  --3 misses--> [Exited]
```

A `steam://` target starts at `LauncherHandoff` directly (the shell returns
immediately; there was never a handle). Three consecutive misses, not one, end a
tracked session — `snapshot()` returns an empty vector on failure, and one failed
enumeration must never read as "the player quit". **ksx never kills the game**:
there is no kill primitive in `ksx-platform::process` and a source-level test
keeps it that way.

**Exit-code mapping.** A `--game` profile's missing exe is caught with the config,
before a pad is plugged → 2. A launch that fails at launch time is necessarily
after the pads are up → 3. The game exiting → 0. The 2/3 line is still exactly
"was a keyboard filter ever armed".

**The daemon** splits into a control loop (pure, CI-tested with a fake session
factory) and a Win32 tray thread that can only enqueue a `DaemonCommand`. The
tray owns no channel to the capture, engine or output threads and calls into none
of those crates — structurally, not by convention. `--headless` exposes the
identical command set on stdin, and the tray falls back to it if the icon cannot
be created. "Reload config" is a clean stop and a clean start from disk, never a
hot-patch of a live pipeline.

**Autostart** validates before it registers — same resolution `ksx run` performs,
plus the profile's exe — because the failure it prevents (a cold boot to an
instant exit 2, on a console nobody sees) has no other detection path.
`--status` reports a stale registration and exits 2 on one.

**install-drivers** is the only elevated command, so it carries the only two
privilege-boundary defences in the codebase: an elevated search restricted to
non-user-writable directories, and a check-and-use with no gap (`SealedFile`:
one `FILE_SHARE_READ` handle, hashed and Authenticode-checked through it, held
open across `CreateProcess`). Both are documented in `docs/DRIVERS.md`.

**Frontend integration**: `docs/INTEGRATION.md` + `examples/ksx-wrap.ps1`, whose
contract is that ksx is stopped on every exit path — and whose safety net is that
killing ksx is always safe, because blocking is released on process death with no
cleanup.

## Interim operational caution (M0–M5)

While the Interception backend is the only capture path, **do not chase Windows
feature updates** on the cabinet: the machine already runs the cross-signed-driver
audit CI policy, and the audit→enforcement flip is what kills the legacy stack.
M6 removes the dependency. Recovery runbook: [`RECOVERY.md`](RECOVERY.md).
