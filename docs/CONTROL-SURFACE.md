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
| Start emulation | `ksx run` (foreground session); daemon: tray "Start emulation" / headless stdin `start` → `DaemonCommand::Start` | M9: enqueue `DaemonCommand::Start` on the control loop the UI hosts in-process. M10: api start verb wrapping the same command | exists |
| Stop emulation | tray "Stop emulation" / stdin `stop` → `DaemonCommand::Stop`; the emergency escapes (LeftCtrl ×5, Ctrl+Alt+Del) live in the capture thread, not in any control surface | M9: `DaemonCommand::Stop`. M10: api stop. Escapes are deliberately NOT a GUI concern — see invariants | exists |
| Session status + live health | tray tooltip (`DaemonState::tooltip`: `RunState` + `LiveHealth` while running, `LastSession` after); stdin `status` → `DaemonCommand::Status`; `ksx run --latency` for the rolling latency summary | M9: poll the same `SharedState` snapshot (`DaemonState`) the tray polls — small, cloneable, no borrows of anything live. M10: api serializes `DaemonState` | exists |
| Reload config | tray "Reload config" / stdin `reload` → `DaemonCommand::Reload` — a clean stop and a clean start from disk, never a hot-patch | M9: `DaemonCommand::Reload`. M10: api reload | exists |
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

## The honest gaps

1. **A RUNNING daemon has no external control channel.** `DaemonCommand`
   travels one crossbeam channel whose only senders are the tray thread and the
   headless stdin reader — no socket, no pipe, no way for a second process to
   enqueue a command. This blocks nothing before M9: the native UI *hosts* the
   supervisor in-process (the same relationship the tray already has), so it
   maps 1:1 onto `DaemonCommand` with no new plumbing. M10a's `ksx-api` is what
   adds the remote surface — for Studio and the E5 MCP shim alike. **Nothing
   needs building before M9.**
2. **No non-interactive mapping verbs yet.** `ksx map` / `ksx slot assign` are
   M7 work and already specified in E5. Until they exist, "edit a binding" has
   no verb — and therefore, by the rule above, no GUI mapping either.
3. **Per-slot persona is a TOML edit today.** Deliberate until the M7 wizard:
   the hand-editable config *is* the interface, and it round-trips.

## Invariants a GUI must not break

Each one maps to a legacy defect or a measured constraint
(`ARCHITECTURE.md` rules 1–5, `ENHANCEMENTS.md` E7 "enhance, never compromise"):

- **Never touch pipeline threads.** The tray can only enqueue a
  `DaemonCommand`; the M9 native UI and M10 Studio get exactly the same reach
  and no more. Live data flows out through snapshots (`DaemonState`,
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
