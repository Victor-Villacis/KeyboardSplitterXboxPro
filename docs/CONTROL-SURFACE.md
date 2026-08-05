# ksx Control Surface

The contract the future GUI builds against — M9 (native config UI) and M10
(Studio, over `ksx-api`). See `ENHANCEMENTS.md` E5/E7 for the milestones and
`ARCHITECTURE.md` for the thread model everything below defers to.

**The standing rule: every front-door action must map to an existing backend
verb — no GUI-only code paths.** A button in the native UI is a `DaemonCommand`
enqueued on the in-process control loop, or a CLI verb's implementation called
in-process; a button in Studio is a `ksx-api` call wrapping the same verb. If
an operation has no verb, the GUI does not get the operation until the verb
exists — which makes the gap list below the GUI's dependency list, not a wish
list.

## Operation → surface map

| Operation | Today's surface | GUI mapping (M9 in-process / M10 ksx-api) | Status |
|---|---|---|---|
| Start emulation | `ksx run` (foreground session); daemon: tray "Start emulation" / headless stdin `start` / pipe `start` / `ksx session start [--game TITLE]` — all → `DaemonCommand::Start` | M9: enqueue `DaemonCommand::Start` on the control loop the UI hosts in-process. **M10: Studio's Start button POSTs `/session/start` → pipe `start` → the same command (live)** | exists — pipe + CLI + Studio live |
| Stop emulation | tray "Stop emulation" / stdin `stop` / pipe `stop` / `ksx session stop` → `DaemonCommand::Stop`; the emergency escapes (LeftCtrl ×5, Ctrl+Alt+Del) live in the capture thread, not in any control surface | M9: `DaemonCommand::Stop`. **M10: Studio's Stop button POSTs `/session/stop` → pipe `stop` (live).** Escapes are deliberately NOT a GUI concern — see invariants | exists — pipe + CLI + Studio live |
| Session status + live health | tray tooltip (`DaemonState::tooltip`: `RunState` + `LiveHealth` while running, `LastSession` after); stdin `status` → `DaemonCommand::Status`; pipe `status` / `ksx session status [--json]` (state + game + profiles + last/live health); `ksx run --latency` for the rolling latency summary | M9: poll the same `SharedState` snapshot (`DaemonState`) the tray polls — small, cloneable, no borrows of anything live. **M10: Studio's session panel renders the pipe `status` response (live)** | exists — pipe + CLI + Studio live |
| Reload config | tray "Reload config" / stdin `reload` / pipe `reload` / `ksx session reload` → `DaemonCommand::Reload` — a clean stop and a clean start from disk, never a hot-patch | M9: `DaemonCommand::Reload`. **M10: Studio's Reload button POSTs `/config/reload` → pipe `reload` (live)** | exists — pipe + CLI + Studio live |
| List / identify devices | `ksx devices [--json]` (both backends, read-only); `ksx winusb status [--json]` for the USB/claim view | M9: same enumeration in-process — strictly read-only, safe mid-session. M10: api devices | exists |
| Pad test | `ksx pads --count N --persona xbox360\|playstation [--json]` (plug, test pattern, unplug) | M9: same routine in-process, only while emulation is stopped (test pads compete for the four XInput slots). M10: api | exists |
| Per-slot persona | TOML edit: `persona = "playstation"` on the `[[slot]]` (aliases `ds4`/`ps4` accepted) | M7 wizard / mapping verbs first; then GUI forms write the same TOML and issue `Reload` | gap — TOML-only **by design** until M7 |
| Preset editing | TOML edit; validated by the same resolution `ksx run`/`ksx daemon` perform at start | M7 `ksx map --slot 1 --function A --key G` (E5); GUI = a form over that verb, never a parallel writer | gap — verbs are M7 |
| Game profiles | TOML edit (`games.toml`); consumed by `ksx run --game`, `ksx daemon --game`, `ksx autostart --game` | Editing: M7 verbs (E5 `ksx slot assign` family), then GUI forms over them. Consuming: `DaemonCommand`/api as above | gap for editing; consuming exists |
| WinUSB claim / release / status | `ksx winusb status` (read-only); `claim`/`release` are dry runs by default, act only with `--yes` + an admin token | M9: same verbs in-process, preserving dry-run-first and the explicit consent step. M10: `status` is safe over the api; `claim`/`release` stay local + elevated | exists |
| Autostart | `ksx autostart --enable/--disable/--status` (validates the config before registering) | M9: same verb in-process. M10: api | exists |
| Install drivers | `ksx install-drivers [--dry-run] [--yes]` — the only elevated command (SealedFile pins, no self-elevation) | M9: same verb; the GUI never self-elevates either, it reports and stops exactly as the CLI does. M10: report-only over the api | exists |
| Import legacy | `ksx import-legacy [--from DIR] [--dry-run] [--json]` | M9: same verb; the `--json` shape is already a GUI-renderable report | exists |
| Doctor | `ksx doctor [--latency] [--json]` — stable codes, `{report, advice}` | M9: same verb, render the JSON. M10: api | exists |

## The daemon control channel (M10a first slice — CLOSED the old gap 1)

A running daemon now serves `\\.\pipe\ksx-daemon` (`ksx-app/src/daemon/pipe.rs`).
Formerly: "a RUNNING daemon has no external control channel" — `DaemonCommand`'s
only senders were the tray thread and the headless stdin reader. The pipe is the
third front end with **exactly the tray's reach**: it enqueues the same
`DaemonCommand` values the tray menu produces onto the same crossbeam channel,
reads the same `DaemonState` snapshot the tray polls, and reads games.toml from
disk. It has no path to the factory, the panel, or any pipeline thread, and it
runs on one plain thread — no async runtime, so E7 rule A (default build links
no tokio/axum/forma) still holds.

**Protocol** — one JSON request line in, one JSON response line out, per
connection; then the server disconnects. Kept deliberately dumb.

```
→ {"verb":"status"}
← {"ok":true,"run":"running","slots":4,"message":null,"game":"Street Fighter",
   "tooltip":"ksx — running, 4 pad(s)\ngame: Street Fighter",
   "profiles":[{"title":"Street Fighter","detail":"C:\\games\\sf.exe — 2 slots"}],
   "last":null,"live":{"reboot_required":false,"watchdog_tripped":false,"dropped_events":0}}

→ {"verb":"start","profile":"Street Fighter"}     ("profile" optional)
← {"ok":true,"message":"running (4 slot(s))"}
← {"ok":false,"error":"already running"}           (refusal example)

→ {"verb":"stop"}
← {"ok":true,"message":"stopped"}                  (or {"ok":false,"error":"not running"})

→ {"verb":"reload"}
← {"ok":true,"message":"running (4 slot(s))"}
```

Action verbs poll the snapshot up to 5 s for the outcome; an unsettled command
answers `ok:true` with a "requested — check `ksx session status`" message
rather than guessing. `start`'s profile title is validated by the daemon's
normal plan resolution (the same path a tray Start takes); an unknown title
comes back as the resolver's error and the previously configured profile is
restored. Verbs the pipe does NOT offer, deliberately: `quit` (walk to the
machine or use the tray — a remote surface should not be able to make the
panel permanently dead), `config` (meaningless off-machine).

**Trust model**: the pipe is created with the DEFAULT security descriptor —
the creating user, SYSTEM, and administrators; nobody else connects. Same-user
processes can already `taskkill` the daemon, so the pipe grants no new
authority. No token, no auth layer, and localhost-only Studio keeps it off the
network.

**Concurrency**: one server thread, sequential connections. The next pipe
instance is created before the current connection is served, so a second
client (two Studios, a racing `ksx session`) queues instead of failing;
clients also retry briefly on `ERROR_PIPE_BUSY` / `FILE_NOT_FOUND`. A daemon
that is not running fails the connect cleanly → `ksx session` exit 2; Studio
renders the controls disabled with the reason. A daemon **older than the
pipe** looks identical to "not running" on this surface — the process-list
row on the Studio page is what catches that case.

**Clients**: `ksx session status|start [--game TITLE]|stop|reload` (`--json`
prints the raw response; exit 0 = done, 1 = daemon refused / pipe error,
2 = no control channel) and Studio's session panel (below). The E5 MCP shim
gets the same channel for free.

## The honest gaps

1. **No non-interactive mapping verbs yet.** `ksx map` / `ksx slot assign` are
   M7 work and already specified in E5. Until they exist, "edit a binding" has
   no verb — and therefore, by the rule above, no GUI mapping either.
2. **Per-slot persona is a TOML edit today.** Deliberate until the M7 wizard:
   the hand-editable config *is* the interface, and it round-trips.

## Invariants a GUI must not break

Each one maps to a legacy defect or a measured constraint
(`ARCHITECTURE.md` rules 1–5, `ENHANCEMENTS.md` E7 "enhance, never compromise"):

- **Never touch pipeline threads.** The tray can only enqueue a
  `DaemonCommand`; the pipe thread, the M9 native UI and M10 Studio get
  exactly the same reach and no more. Live data flows out through snapshots (`DaemonState`,
  `HealthSlot`) or a lossy fan-out sink — a slow or wedged UI can cost a
  window, never a keyboard. The legacy WPF app died of the opposite
  arrangement.
- **Capture-thread purity.** No tokio, no allocation, no locks in the capture
  thread — and therefore no GUI-serving code anywhere near it. Any live
  monitor coalesces to display rate (~60 Hz); full fidelity lives in
  `--record`, not a socket a browser can backpressure.
- **Escapes stay in the capture path.** LeftCtrl ×5 and Ctrl+Alt+Del are
  evaluated inside the capture thread, upstream of every channel. A GUI stop
  button is a convenience on top; it must never become a link in the escape
  chain, because the escapes' one property is that they work when everything
  downstream — the GUI included — is wedged.
- **Config stays hand-editable TOML.** The GUI edits the same files the user
  can edit, and changes land via `Reload` (clean stop, re-read from disk,
  clean start) — never a hot-patch of a live pipeline, never a parallel
  binary store, never GUI-only state.
