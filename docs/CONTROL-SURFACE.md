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
| Reload config | tray "Reload config" / stdin `reload` / pipe `reload` / `ksx session reload` → `DaemonCommand::Reload` — a clean stop and a clean start from disk. A mapper SAVE takes the narrower `DaemonCommand::ApplyBindings` instead: binding-only edits hot-swap into the live engine with the pads left plugged, structural changes fall back to the same bounce (see "the binding hot-swap") | M9: `DaemonCommand::Reload`. **M10: Studio's Reload button POSTs `/config/reload` → pipe `reload` (live)** | exists — pipe + CLI + Studio live |
| List / identify devices | `ksx devices [--json]` (both backends, read-only); `ksx winusb status [--json]` for the USB/claim view | M9: same enumeration in-process — strictly read-only, safe mid-session. M10: api devices | exists |
| Pad test | `ksx pads --count N --persona xbox360\|playstation [--json]` (plug, test pattern, unplug) | M9: same routine in-process, only while emulation is stopped (test pads compete for the four XInput slots). M10: api | exists |
| Per-slot persona | TOML edit: `persona = "playstation"` on the `[[slot]]` (aliases `ds4`/`ps4` accepted) | M7 wizard / mapping verbs first; then GUI forms write the same TOML and issue `Reload` | gap — TOML-only **by design** until M7 |
| Preset editing | `ksx map --preset "IPAC P1" --function A --key G [--clear] [--force] [--move-from FUNCTION] [--json]`; chords: `--when B[,C] [--unless K]`; whole-preset: `--restore defaults\|session-backup\|latest-backup`, `--clear-all`, `--list-backups`; pipe `map` / `map-restore` / `map-clear-all` / `map-backups` (same writers: `ksx-app/src/mapping.rs`); TOML edit still first-class | **Studio's `/map` mapper (live)**: click a control → pipe `learn-key` → pipe `map` — every write goes through the one shared writer, never a parallel editor. Conflict detection is server-side in that writer (see below). Studio does not yet DISPLAY chords (later pass) — the CLI/pipe author them and the engine runs them | exists — CLI + pipe + Studio live |
| Learn a key ("press the panel key for P1·A") | pipe `learn-key` / `learn-poll` / `learn-cancel` (asynchronous; see "learn-key semantics" below) | **Studio's mapper drives it (live)**: `/api/learn*` → the pipe verbs. No CLI face yet (`ksx map` takes the key by name; `ksx monitor` shows names) | exists — pipe + Studio |
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

The M7 mapper slice adds four verbs on the same channel:

```
→ {"verb":"map","preset":"IPAC P1","function":"A","key":"G",
   "force":false,"reload":true}          ("clear":true instead of "key" unbinds)
← {"ok":true,"message":"\"IPAC P1\": A = G — the next session start reads it",
   "path":"C:\\…\\presets\\IPAC P1.toml","preset":"IPAC P1","function":"A",
   "key":"G","when":[],"unless":[],"also_drives":[],"moved_from":null,
   "conflicts":[],"flash":[],"reloaded":false}

→ {"verb":"map","preset":"IPAC P1","function":"B","key":"G"}   (G is already A's)
← {"ok":true,"message":"\"IPAC P1\": B = G; G also drives A", …,
   "also_drives":["A"],"moved_from":null}    (a MULTI-BIND — see below)

→ {"verb":"map","preset":"IPAC P1","function":"B","key":"G","move_from":"A"}
← {"ok":true,"message":"\"IPAC P1\": B = G (taken from A — A is now unbound)", …,
   "also_drives":[],"moved_from":{"function":"A","remaining":[],"unbound":true}}

→ {"verb":"map","preset":"IPAC P1","function":"rt","key":"D","when":["F"]}
← {"ok":true,"message":"\"IPAC P1\": rt = D+F", …,
   "when":["F"],"unless":[],"flash":[]}   (a CHORD — see "chords" below)
← {"ok":false,"code":"conflict",
   "error":"refusing to bind G: G is \"IPAC P2\"'s A (slot 2 of \"Steam\") — use --force …",
   "conflicts":[{"scope":"profile","preset":"IPAC P2","function":"A",
                 "profile":"Steam","slot":2}]}

→ {"verb":"learn-key"}      (refused while a session runs — see semantics below)
← {"ok":true,"state":"listening","generation":3,"remaining_ms":9998,
   "device":null,"key":null,"error":null}
→ {"verb":"learn-poll"}
← {"ok":true,"state":"hit","generation":3,"remaining_ms":null,
   "device":"HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000","key":"G","error":null}
→ {"verb":"learn-cancel"}
← {"ok":true,"state":"cancelled", …}
```

### Whole-preset writes: three restore destinations, plus clear-all

```
→ {"verb":"map-restore","preset":"IPAC P1","mode":"latest-backup","reload":true}
← {"ok":true,"mode":"latest-backup",
   "message":"\"IPAC P1\": bindings restored from the newest timestamped backup
              — the previous file is backed up as 20260805-221500 — bindings
              applied live — pads untouched",
   "wrote":"this preset as it was before the most recent restore (…)",
   "backup":{"stamp":"20260805-221500","label":"2026-08-05 22:15:00 UTC",
             "path":"C:\\…\\presets\\IPAC P1.toml.bak-20260805-221500"},
   "path":"C:\\…\\presets\\IPAC P1.toml","preset":"IPAC P1",
   "reloaded":true,"hot_swap":true}

→ {"verb":"map-restore","preset":"IPAC P1","mode":"session-backup"}
← {"ok":false,"error":"no session backup for \"IPAC P1\" — nothing has been
   mapped through the daemon this session, so there is nothing to undo"}

→ {"verb":"map-clear-all","preset":"IPAC P1","reload":true}
← {"ok":true,"mode":"clear-all","message":"\"IPAC P1\": every binding cleared …"}

→ {"verb":"map-backups","preset":"IPAC P1"}          (read-only)
← {"ok":true,"preset":"IPAC P1","backups":[
     {"stamp":"20260805-221500","label":"2026-08-05 22:15:00 UTC","path":"…"},
     {"stamp":"20260804-090000","label":"2026-08-04 09:00:00 UTC","path":"…"}]}
```

`map-restore` (writer: `mapping.rs::restore`; CLI face: `ksx map --preset …
--restore defaults|session-backup|latest-backup`) has **three destinations**,
and every surface must name the destination rather than the word "restore":

| mode | writes | undoes |
|---|---|---|
| `defaults` | the **generic keyboard layout** — `ksx_core::Preset::builtin_default()`: S=A, D=B, A=X, W=Y, Q/E triggers, arrow keys = left stick, Esc=Start, Backspace=Back. Keeps the preset's NAME | nothing — it is the always-there floor |
| `session-backup` | the preset as it was before the daemon's FIRST `map` write of this daemon lifetime (`<preset>.toml.session-bak`; `pipe.rs::map_fn` owns the once-per-lifetime set) | everything mapped since the daemon started |
| `latest-backup` | the preset as it was before the most recent whole-preset write (the newest `<preset>.toml.bak-YYYYMMDD-HHMMSS`) | the previous restore, or a clear-all |

**`defaults` is the one that surprises people, and the labels exist to stop
it.** It does NOT mean "this preset as it shipped" — on an arcade cabinet it
replaces an I-PAC panel map with a desktop-keyboard map. Studio's button
therefore reads "Reset to generic keyboard layout (S/D/A/W…)", `--help` spells
the layout out, and the confirm dialog names every key it writes. The abstract
phrase "restore defaults" appears nowhere in the UI any more.

**Every whole-preset write takes a timestamped backup first** — restore ×3 and
`map-clear-all` alike. The current file is copied to
`<preset>.toml.bak-YYYYMMDD-HHMMSS` (UTC, sortable; a second write inside the
same second gets `-2`, `-3`…) BEFORE the new content is written, and only once
the replacement has been read and validated — so a refusal leaves no stray
backup. Backups are never pruned: they are small, restores are rare and
deliberate, and deleting a cabinet's only copy of a panel map to save kilobytes
is not a trade ksx makes. The response's `backup` field, `ksx map
--list-backups --preset X [--json]` and the pipe's `map-backups` all read the
same list, newest first; Studio labels its third button with the newest
timestamp ("Restore backup from 2026-08-05 14:32:07 UTC") and HIDES the button
when there is none, because offering a road home that does not exist is worse
than not offering one.

`map-clear-all` (writer: `mapping.rs::clear_all`; CLI face: `ksx map --preset …
--clear-all`) unbinds every function while keeping the file structurally valid:
it writes the `empty` built-in's SHAPE — all 25 functions present, each keyed
`"None"` — the same convention single-function `--clear` uses, so a cleared
control stays visible in the legend instead of vanishing.

Refusal codes (`--json` `code`, stable): `unknown-preset`, `unknown-function`,
`unknown-key`, `invalid-guard`, `bad-move-from`, `conflict` (cross-slot only),
`no-session-backup`, `no-backup`, `bad-backup`, `config-error`. A corrupt backup is refused, never written.

### Chords: `--when` / `--unless` (2026-08-06)

```
ksx map --preset "IPAC P1" --function rt --key D --when F
ksx map --preset "IPAC P1" --function lb --key D --when F,C --unless LeftShift
ksx map --preset "IPAC P1" --function rt --clear      # removes the chord too
```

`--when KEYS` / `--unless KEYS` (comma-separated, or repeated) turn the write
into a GUARDED binding — a chord: "this function, but only while these other
keys are (not) also held". Pipe equivalent: `"when":["F"]`, `"unless":[…]`;
both absent from a plain write, so every pre-chord caller is unchanged. They
belong to the BIND action only — clap refuses them alongside `--clear`,
`--restore` or `--clear-all`. `--clear` and any re-map of the same function
remove that function's chord as well as its plain keys (replace-per-function
covers guarded rows), so a cleared control is really cleared. The full
semantics — consumption, specificity, releases — are
docs/INPUT-TRANSFORMS.md §1b.

Two deliberate differences from a plain bind, both about not lying:

- **A chord never conflicts.** Layering `rt = D+F` over keys that already do
  something is the whole point, so the conflict gate is skipped and nothing
  is ever stolen from the bindings the chord sits on top of.
- **It reports the flash instead.** `flash` is `[{"key":"G","bound_to":"A"}]`
  for every constituent that is ALSO bound on its own, and the human message
  spells out what the player will see: ksx does not defer input, so pressing
  that key first shows its own output for a moment before the chord takes
  over. Empty `flash` = the recommended shape (dedicated chord keys, no cost
  at all). The same finding appears as a `[WARN]` in the run plan; it is
  advice, not a refusal.

`invalid-guard` (exit 2, nothing written) covers the guards that cannot mean
anything: the trigger key listed in its own `--when`/`--unless`, a key in both
lists, or a guard with no `--key`. Unknown guard key names are `unknown-key`,
like any other key name. `ksx doctor`-style config validation additionally
refuses **ambiguous equal-specificity chords** — two guards of the same size
on the same trigger that could be satisfied together — at session start, so
which one wins is never a build-order accident.

`map` writes through the SAME `ksx-app/src/mapping.rs::apply` the CLI verb
uses — replace-per-function, `"None"` placeholder on clear, canonical TOML
rewrite (comments do not survive; the store's atomic-write trade), CONFLICT
DETECTION server-side in the writer.

### Multi-bind: one key, many controls (2026-08-06)

**A key already used by another control of the SAME preset is not a conflict —
it is a multi-bind, and it is written.** The engine has no uniqueness
constraint in either direction ("many keys → one function and one key → many
functions are both native", `ksx-core/src/preset.rs`,
docs/INPUT-TRANSFORMS.md §1a): one key compiles to a `SmallVec` of targets and
they all fire together. So the write leaves every other control holding that
key exactly as it was and REPORTS them:

| field | meaning |
|---|---|
| `also_drives` | the other functions of THIS preset the key drives now that the write is done, sorted. Information, never a refusal (`["A","B"]`). Empty for a clear, for a chord, and for an exclusive key. Studio shows the same fact as the legend's "also A · B" badges, which `render_map.rs::shared_labels` re-derives from disk |
| `moved_from` | `null` unless `"move_from"` was asked for; otherwise `{"function":"A","remaining":[],"unbound":true}` — the ONE control the key was taken from, what it kept, and whether it is now unbound |

That is what makes the mapper's **"Map all to one key"** work: it is N ordinary
`map` calls with one key, and all N stick (MAPPER-UX commandment 7 —
duplicates are information, fan-out is the product). Re-binding one control
still replaces only that control's keys; its co-binders keep theirs.

**`"move_from":"A"` (CLI `--move-from A`) is the explicit hand-over**, and the
only way this verb unbinds something it was not asked to bind: it takes THIS
key off THAT one function (which keeps the inert `"None"` if that emptied it,
and keeps its other keys if it had more) and touches nothing else. Never
implicit, never a side effect of `force`. It is refused — before any write —
if it names the function being bound, a function that does not hold that key
(the refusal says what that control actually has), a clear, or a chord:
`bad-move-from`, exit 2.

**The one conflict left is CROSS-SLOT, and it still blocks**: the key bound in
another slot's preset within any games.toml profile that uses the target
preset. That preset is **never auto-edited** — `force` writes the target
anyway and keeps reporting the double binding (`conflicts`, `scope` always
`"profile"` now); silently rewriting a preset the caller did not name would be
worse. So `force` means exactly one thing — "yes, both slots should see that
key" — and it **removes no binding, anywhere, ever**. The genuinely
destructive writes are their own verbs (`map-restore`, `map-clear-all`), each
taking a timestamped backup first.

### `"reload":true` — the binding hot-swap (2026-08-05)

Every write verb (`map`, `map-restore`, `map-clear-all`) takes the same
optional `"reload":true`. It used to mean `DaemonCommand::Reload`: a clean
stop, re-read, start — which unplugged four pads, made Windows play its
disconnect/reconnect chime, made Steam re-enumerate, and made a game in
progress see its controllers vanish. Victor's question, verbatim: "why does it
need to disconnect to reconnect?"

It now enqueues `DaemonCommand::ApplyBindings`, and the control loop picks the
cheapest correct answer:

| change | what happens |
|---|---|
| preset CONTENTS, or a slot pointing at a different preset | **hot swap**: `ksx-core`'s `EngineTables` are rebuilt on the daemon's control thread and moved into the live engine (`Engine::swap_tables`). Pads stay plugged, keyboards stay captured, nothing re-enumerates. Response: `"hot_swap":true`, message "bindings applied live — pads untouched" |
| slot count, slot numbering, persona, keyboard/mouse assignment, blocking policy, capture backend | **bounce**: exactly the old `Reload`, and the message names what changed ("session restarted — slot 3 changed persona … needs the pads replugged"). Response: `"hot_swap":false` |
| the config no longer resolves | nothing is torn down. Tearing a working session down to fail the restart is the worst of both; the response says the session is still running on its old bindings |
| nothing running | nothing to do — the next start reads the file |

The split is drawn where the DRIVERS are: anything that would make the output
thread plug a different pad, or the capture thread block a different device,
takes a real teardown (`run/supervisor.rs::SessionShape::bounce_reason` is the
one place that rule lives). Everything else is a key→function table.

**Hot-path purity is preserved**: the new tables are built off-thread (the
control loop may block and allocate freely) and the engine thread only moves
pointers. **Stuck keys are impossible**: dense key ids belong to the old
tables, so `swap_tables` re-baselines the key state and RETURNS the neutral
states of any control that was held across the edit — the supervisor forwards
them, so a rebind can never strand a pressed virtual button.

The config invariant below is unchanged in substance: config still lives in
hand-editable TOML, changes still land by re-reading that TOML, and there is
still no parallel binary store or GUI-only state. What changed is that a
BINDING-ONLY re-read no longer requires destroying and rebuilding the driver
objects around it.

`ksx session reload` and the tray's "Reload config" keep the blunt
stop-and-start semantics — they exist for "restart whatever changed".

**learn-key semantics** (the honest v1): the daemon observes the next key
press via a Raw Input sink (`ksx-capture::observe_next_key` — instance path +
the same corrected `Key` vocabulary presets store; injected input is ignored
by construction). Because a running session's captured keyboards are
suppressed below win32k — where a Raw Input sink hears nothing — `learn-key`
is **refused while a session is running** instead of timing out silently.

That refusal was re-examined in full on 2026-08-05 (a live session had Victor
clicking a mapper that answered nothing) and deliberately KEPT, because ksx
could obviously tap its own capture stream instead and must not:

1. the capture thread is the one thread on this machine where a bug freezes
   every keyboard until reboot. It is time-critical, allocation-free and
   lock-free on purpose; a convenience feature does not get a code path in it;
2. a key pressed to be LEARNED would also fire its current binding, on every
   slot it fans out to — mapping would inject real gameplay input;
3. rebinding a key while it is physically held could leave a virtual button
   pressed under the old binding and released under the new one: exactly the
   stuck-key class the all-keys-up rule and `swap_tables`' release-on-swap
   exist to prevent;
4. mapping is a between-games activity in every tool in the field study
   (MAME's TAB menu pauses the machine; RetroArch binds from its menu).

What changed instead is the UX around the refusal: Studio's mapper renders the
running state as a banner with a **"Pause emulation & map"** button (the plain
`stop` verb), then a persistent **"Resume emulation"** (the `start` verb with
the profile it remembered), with a "paused for mapping" pill in the header so
nobody walks away from a cabinet they stopped. One click each way, no tray
hunt, no CLI — and the `ksx map` fallback is still printed. Same honest
limit for WinUSB-claimed interfaces: a claimed panel is not in the keyboard
stack, so Raw Input cannot hear it even between sessions (its typethrough
injection is deliberately filtered as injected input) — learning from a
claimed panel through the daemon's own report stream is the M8-adjacent
follow-up. Constants are PadForge's earned recorder numbers
(docs/research/padforge-code-audit.md §1.2): 10 s timeout, 33 ms observer
slices, wait-for-release re-baselining (keys held at learn start are ignored
until released — autorepeat cannot steal a chained learn). The verb is
asynchronous by design: the pipe serves clients sequentially, so `learn-key`
only STARTS the observation and returns; `learn-poll` carries the outcome
plus `remaining_ms` — the visible countdown PadForge never had. A second
`learn-key` supersedes the first; `learn-cancel` stops within one slice.

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

1. ~~No non-interactive mapping verbs yet.~~ **CLOSED (M7 slice, 2026-08-05)**:
   `ksx map` + pipe `map`/`learn-*` + Studio's `/map` mapper, all over one
   writer. Still open from the E5 family: `ksx slot assign` (device/preset
   wiring per slot) — the mapper edits bindings inside a preset, not which
   preset a slot uses.
2. **Per-slot persona is a TOML edit today.** Deliberate until the M7 wizard:
   the hand-editable config *is* the interface, and it round-trips.
3. **learn-key cannot hear a WinUSB-claimed panel** (see semantics above) —
   Interception-backed cabinets learn fine; a migrated cabinet uses `ksx map`
   until the claimed-panel learner lands.
4. **learn-key still needs emulation stopped** — deliberately, for the four
   reasons in "learn-key semantics". Studio makes obeying it one click
   (Pause → map → Resume) rather than a dead end.
5. **`ksx slot assign` (which preset a slot uses) is still a TOML edit.** The
   mapper edits bindings inside a preset; pointing slot 2 at a different
   preset is a games.toml/config.toml change today. Worth noting it is
   nevertheless a HOT change at the engine level — only the binding table
   moves — so it does not need a pad bounce once the verb exists.

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
  can edit, and changes land by RE-READING those files — never a parallel
  binary store, never GUI-only state. How the re-read reaches a running
  session depends on what changed: a binding-only edit is applied by rebuilding
  the engine's dispatch tables off-thread and swapping them in
  (`ApplyBindings`), anything structural is still a clean stop + re-read +
  start (`Reload`). Both paths read the same TOML; neither patches a live
  pipeline's state in place, and the swap releases anything held so it cannot
  strand a pressed control.
- **A surface that cannot act must SAY so, per click.** No control may be a
  silent no-op. When the daemon is unreachable Studio shows a banner at the top
  of the page ("No daemon — ksx Studio can see your config but cannot change
  anything") with the exact command to start one (profile flag included),
  renders every dead control visibly inert — a CSS look, never the `disabled`
  attribute, which would swallow the click that owes an explanation — and
  answers each click by naming the control, the reason, and the `ksx map`
  one-liner that works anyway.
