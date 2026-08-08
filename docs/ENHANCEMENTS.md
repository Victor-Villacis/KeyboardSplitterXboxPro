# Enhancement Roadmap (evaluated, post-parity)

Victor's "think outside the box" asks, evaluated against the 2026 research
(`research/*.md`). Rule: **parity first (M1–M6), enhancements after** — each item
below is deliberately designed-for now (trait boundaries, config schema) so none
requires a rewrite later.

## E1 — More controller types than Xbox 360 (Xbox One/Series, DualSense, Switch Pro)

**Verdict: yes, via the backend trait — not via ViGEmBus.**

- ViGEmBus only emulates X360 and DS4 (DS4 exists in our vendored client behind
  `unstable_ds4`). It will never gain Xbox One/Series — the project is frozen.
- **HIDMaestro** (MIT, active, user-mode) ships 228 byte-exact profiles incl.
  Xbox Series, DualSense (adaptive triggers), DS4v2, Switch Pro. Our
  `VirtualPadBackend` trait exists precisely so a `hidmaestro.rs` backend can add
  these personas without touching the engine. Prereq: write a small Rust client for
  its MIT-documented shared-memory protocol; verify its WGI double-input bug doesn't
  hit our games.
- Reality check for the cabinet: X360 remains the *most* compatible persona ever made
  (every XInput game, 2006→today). Fancier personas matter only for games that check
  for newer pads. **Priority: low until a concrete game needs it.**

### E1 status 2026-08-06 — M8 started, blocked on the driver being absent

**Feasibility check first, and it failed the way the plan allowed for.** HIDMaestro is
**not installed on this machine** and no installer is present: no `HIDMaestro` service
key, no driver package in `pnputil /enum-drivers`, no `ROOT\HIDMAESTRO` devnode,
nothing under `Program Files`. The only artifact anywhere is a bundled managed assembly
(`HIDMaestro.Core.dll` 1.3.22) inside the read-only PadForge clone — a .NET library,
not a driver.

So M8 shipped the **client, honestly gated**, and nothing that pretends:

- `crates/ksx-hidmaestro` — the protocol client written against
  `research/padforge-code-audit.md` §3: seqlocked shared-memory latch, lifecycle in the
  documented order (sweep → catalog → install; PID pool before enumeration), 16 ms
  idle-dedup keepalive derived from the three driver watchdogs, axis routing **by role
  through the profile's map** (never positional), trigger writes mirrored to two keys,
  and the feedback decode table including the Bluetooth 547→257 length trap.
- `ksx-output`: `HidMaestroBackend` (adapter) + `RoutedBackend` (persona → stack).
- `ksx-core`: `Persona::{DualSense, SwitchPro, XboxSeries}` + `PadBackend`, with
  `Persona::backend()` as the single statement of the routing rule.
- **The three personas are not offered until the section lands.**
  `PadBackend::is_implemented()` says this build cannot create a HIDMaestro device, and
  `Persona::can_plug()` is what the config validator (`Issue::PersonaNotImplemented`),
  `RoutedBackend`, `ksx pads` and `ksx doctor` all read. The gate is that build fact and
  **never a driver probe** — a probe flips to "installed" the moment a user installs
  HIDMaestro, and the plug fails exactly as before, so a probe-based gate is wrong in
  the one case it exists for. Flipping `is_implemented()` in the same commit that lands
  a real driver re-opens every one of those paths with nothing else to change.
- `ksx doctor` reports HIDMaestro's absence at **Info** severity (`hidmaestro-missing`)
  and says nothing depends on it, because nothing does: a cabinet on
  xbox360/playstation loses nothing by not having it, and neither does anyone else.
  The separate `personas-not-implemented` note is what explains the three personas, and
  it prints whether or not the driver is present.

**Verified vs written-to-spec** is stated in `ksx-hidmaestro`'s crate docs as a table.
Short version: the seqlock discipline, keepalive cadence, axis-by-name routing and
lifecycle order are tested (the seqlock with a real writer/reader thread race); the
decode table is pinned to the audit's fixtures but unverified on bytes; **the shared
section's name and byte layout are unknown** and are the one thing the protocol map
does not contain — PadForge consumes the SDK in-process and never opens the section.
Finishing M8 means transcribing that from HIDMaestro's own MIT sources
(`driver.c` / `companion.c`); everything above it is already written.

**Slots 1–4 never move.** `Persona::backend()` pins xbox360 and playstation to ViGEmBus
permanently — HIDMaestro's XInput is a synthesis layer and the direct cause of its WGI
double-input bug, and ViGEm's X360 target is Microsoft's own `xusb22.sys`.

## E2 — Lean into Steam Input?

**Verdict: integrate WITH it, never build ON it.**

- Steam Input cannot do our core job: Windows merges all keyboards, so Steam can't
  tell P1's I-PAC keys from P2's — per-device capture below the OS is exactly why ksx
  exists. And Steam-only mapping abandons MAME/RetroArch/emulators.
- But our virtual X360 pads are *first-class* Steam Input citizens (indistinguishable
  from real hardware — the reason XOutput picked this stack). So Steam sees 4 real
  pads and its per-game configs stack on top for free.
- Planned integration (M5 ksx-games): launch `steam://rungameid/<id>` targets,
  Big Picture-friendly autostart ordering (pads plugged before Steam starts
  enumerating), per-profile pad count.

## E3 — Emulate keyboards too (key→key remapping / synthetic keys)

**Verdict: cheap and worth doing — new `Binding::Key` output.**

- The engine already owns per-device capture; adding a binding variant that emits a
  *different* keystroke (via `interception_send` on the Interception backend, or
  `SendInput` on WinUSB-captured devices — injected keys come from a clean source)
  turns ksx into a per-device key remapper as a side effect (kanata territory, but
  per-keyboard).
- Use cases: I-PAC admin buttons → frontend hotkeys; one panel button → Alt+F4;
  "create keys first, then controllers" flows.
- Config shape is already compatible: `bindings` values just gain a `key:X` form.
- **Priority: medium, after M4** (needs the capture backends live). A full *virtual
  keyboard device* (VHF-based) is not needed for any current use case — synthetic
  injection covers it.

## E4 — More than 4 controllers — **MEASURED 2026-08-04: cheap after all**

**Superseded by experiment.** This section used to say the only route past four
players was HIDMaestro or vJoy. That was wrong, and `research/m6.5-ds4-findings.md`
has the measurement:

> Six ViGEm **DS4** targets plugged and enumerated while four X360 pads already
> held every XInput slot — and the XInput count did not move. DS4 targets are
> plain HID devices; they neither consume nor compete for XInput's four slots.

So **>4 players is a ViGEmBus feature**: one driver, already installed, no second
stack, no protocol client to write. The cabinet shape is slots **1–4 as X360**
(genuine `xusb22.sys` XInput, maximum compatibility) and **slots 5+ as DS4**
(HID/DirectInput — MAME, RetroArch, SDL and Steam Input all read those). An
XInput-only game still sees four; that is Windows, not ksx.

The 4-slot cap itself is unchanged and unfixable: no virtual bus can create
XInput slot 5. What changed is that we no longer need XInput for players 5+.

**Shipped 2026-08-04.** The submit bug (`ERROR_NO_MORE_ITEMS`) was a driver
startup window, not a marshalling error — `Ds4Pdo.cpp` drops reports until the
HID stack starts polling, 1–3 ms after `WAIT_DEVICE_READY`; `wait_ready` now
primes through it and `update` retries transients (fix in the vendored client,
worth offering upstream). The persona plumbing is live end to end: `Persona` in
ksx-core (`persona = "playstation"` in TOML, `ds4`/`ps4` accepted as aliases),
`MAX_SLOTS` raised past 4 with a validation rule that refuses a fifth `xbox360`
slot by name, the `PadState`→DS4 mapper with a documented D-pad SOCD collapse,
and `ksx pads --persona playstation` verified on the cabinet: six pads plugged,
driven, and unplugged with zero XInput slots consumed.

**Raised again to 16 (2026-08-07).** The 8 was a guess about panel sizes, not a
measurement, and it was the only thing standing in the way: nothing slot-keyed
truncates above it, and the one hard limit is the `u8` the engine indexes slots
with (255). Four I-PAC4 boards is a 16-player cabinet. The work was not the
constant — it was the three clap `1..=8` ranges and the two `ksx-api` refusal
strings that did **not** track it, so `ksx setup --slot 9` failed at the parser
before reading a file. All five now format from `MAX_SLOTS`, with tests that
compare against the constant rather than a literal. `MAX_XINPUT_SLOTS` is
untouched at 4 — that one is Windows.

## E5 — AI-drivable CLI (accepted into CURRENT scope, not deferred)

This one is not an enhancement — it's now a design rule for every milestone:

- Every `ksx` command: stable exit codes + `--json` structured output
  (`devices`, `doctor`, `import-legacy`, `pads`, later `map`).
- Config is plain TOML files — an AI assistant (or any script) can write a preset,
  validate it (`ksx doctor --config --json` reports structured issues), and hot-reload
  without any UI.
- Planned: non-interactive mapping verbs (`ksx map --slot 1 --function A --key G`,
  `ksx slot assign 1 --device "P1 I-PAC"`), `ksx devices --json` with instance paths
  so an assistant can wire a whole cabinet from a chat session.
- Future idea (post-M5): a tiny MCP server wrapping the CLI so Claude can configure
  ksx conversationally on the cab. The CLI-first design makes this a thin shim.

## E7 — Forma: native-first, web as supplement (decided 2026-08-04)

[Forma](https://github.com/orgs/getforma-dev/repositories) is our own stack (Rust SSR
server + FMIR binary IR + FormaJS signals/islands). **We dogfood it deliberately**:
ksx becomes the flagship app that proves Forma in production. Its low star count is not
a reason to avoid it — shipping something impressive on it is how it gets adopted.

**The constraint that governs everything else: ksx is a native Windows app first.**
Tray, drivers, virtual bus, native config UI. Forma *enhances*; it never costs
performance and it is never required for the product to work.

### Verified before deciding (fetched from the repos)
- **`forma-server` 0.1.4 is a library, not a server** — no listener, no `main`, **no
  tokio dependency**. `render_page(&PageConfig) -> PageOutput` is a *synchronous pure
  function*. Our daemon keeps its own runtime and listener. MIT, crates.io, MSRV 1.70.
- **`rust-embed`-only asset serving** — one `.exe` shipping its own UI is Forma's
  default path, not a workaround. No Node at runtime (Node ≥18 at build time only).
- **⚠️ No server push anywhere in the Rust half** — no SSE, no WebSocket. The live
  monitor is ours to build in plain axum 0.8 (kmd proves the pattern but shares no code).
- **⚠️ Hardcoded CSP** (`connect-src 'self'`, no extension API) — collides with LAN
  access and cross-origin WS.
- **⚠️ FMIR version skew**: `@getforma/core` 1.5.0 (Jul) vs Rust crates 0.1.4 (Mar).
  `check_ir_compatibility()` exists because drift was anticipated — verify current
  compiler output still parses **before** building anything.
- **⚠️ Windows untested upstream** (forma CI is `ubuntu-latest` only).

### Three surfaces, one engine
1. **Native primary (non-negotiable)** — CLI + tray daemon (M5) + ~~native config UI
   (M9)~~ **native app identity + launcher (M9, revised 2026-08-06 — see Sequencing
   below and `docs/M9-DECISION.md`)**. Zero HTTP, zero web deps in the default build.
   The cabinet works perfectly with no browser in existence — **as a CLI + tray**;
   the graphical mapper is Studio's, and needs a web engine present.
2. **`ksx-api` (M10a)** — one typed surface for Studio *and* the MCP server (E5). The
   native UI does **not** go through it: in-process calls straight to the supervisor, so
   the primary path pays no serialization tax.
3. **ksx Studio (M10b, Forma)** — optional companion UI: embedded axum + `forma-server`
   SSR + our own SSE/WS. Configure the cabinet *from your phone while standing at it* —
   the case where a browser is genuinely the right client, since a cab has no keyboard.

### "Enhance, never compromise" — enforced, not promised
- **Compile-time optional**: `--features studio` for the web surface,
  `--features cabinet` for the 10-foot one. **The default build links NEITHER**
  — no axum, no forma, no HTTP, and no egui, eframe or winit — and the two are
  independent of each other (a cabinet with no browser builds one and not the
  other). Provable, all three ways:

  ```
  cargo tree -p ksx-app -e normal                    | grep -ciE "axum|tokio|forma|eframe|egui|winit"   # 0
  cargo tree -p ksx-app -e normal --features cabinet | grep -ciE "axum|tokio|forma"                     # 0
  cargo tree -p ksx-app -e normal --features studio  | grep -ciE "eframe|egui|winit"                    # 0
  ```
- **Never touches pipeline threads.** Studio subscribes to a lossy fan-out sink; a slow
  browser can never backpressure the engine (same rule as the M4 delta coalescing).
- **Display-rate coalescing** (~60 Hz) for the monitor. Full fidelity lives in
  `--record`, not the socket.
- **Own runtime, normal priority**, isolated from the TIME_CRITICAL capture thread.
- **Localhost by default**; LAN bind is explicit opt-in with a CSPRNG pairing token
  (`ring`, not UUIDv4). Do **not** copy the scaffold's dashboard template — it binds
  `0.0.0.0` with no auth; the minimal template computes a CSP then discards it.

### What ksx gives back (the dogfood loop)
A systems daemon stresses Forma where no web app would, and each gap becomes a feature
request with a real consumer: **server push (SSE/WS)**, **CSP extensibility**,
**embedding ergonomics** (proof it drops into a non-web Rust binary), **Windows build
validation** (first real Windows consumer), and **FMIR version alignment**.

**The demo that sells both**: open a phone at the cab → four virtual pads rendered live
→ mash the arcade panel → buttons light up instantly with real latency numbers → remap
a button from the phone and it works. One demo, two products.

### Sequencing — REVISED 2026-08-06 (see `docs/M9-DECISION.md`)

As planned: M6 WinUSB → M6.5 DS4 spike → M7 GA → M8 HIDMaestro. **Nothing
web-related preceded M6** — the driver deadline outranked the showcase, and it
held.

What changed after that: **M9 is no longer an egui/eframe config UI.** When this
section was written (2026-08-04) ksx Studio was a status page a few hours old,
so the native UI was the only plan for a mapper and making it non-negotiable cost
nothing. Studio then shipped ~23k lines and 139 tests of finished mapper — art,
identities, multi-bind, first-class diagonals, the 37-column macro piano roll,
toasts+undo, a documented design system — and rebuilding that in egui would buy
one thing E7 asked for (in-process, no serialization tax) that **the pipe already
delivers**: `tray.rs` deliberately rejected in-process for UI, CONTROL-SURFACE
grants the pipe exactly the tray's reach, and the "tax" is one JSON line per
human click, nowhere near the capture thread's p99 budget. The full reasoning,
the per-option engineer-day costs, the honest losses and the reversal triggers
are in **`docs/M9-DECISION.md`**.

Revised: **M9 = "ksx is a real Windows application"** — an owned icon (the tray
still uses `IDI_APPLICATION` today), a Start Menu entry, a tray "Open ksx" item,
and an `ksx open` launcher that starts the daemon, waits for the port, and *then*
opens a chrome-less application window (`--app=` + a ksx-owned `--user-data-dir`,
launched via App Paths). ~700–1,000 lines. → **M10a `ksx-api`** — the typed,
transport-free, in-process API both Studio and any future native shell consume;
it is 80% written already as `ksx-studio`'s `StatusSource`/`ControlSource` and
wants extracting into its own crate with an `InProcess` implementation beside the
`PipeClient` one. It is the one item worth building under every option and it
should land before Mapper Build B. → **M10b Studio** continues as the UI.

Held in reserve, priced, not cancelled: a native WebView2 shell (`webview2-com`
directly — not wry, not Tauri) that hosts the same UI with zero HTTP via
`WebResourceRequested`. 12–16 days on top of `ksx-api`, to be spent only when one
of M9-DECISION §7's triggers fires.

**Scope of "zero web deps", stated honestly.** The rule is unchanged for the
*default build*: no axum, no forma, no tokio, `--features studio`, provable with
`cargo tree`. What is no longer promised is a GUI on a machine with **no web
engine at all** (LTSC/IoT with Edge stripped, a hardened image). There, ksx still
works completely — the CLI is a complete surface by CONTROL-SURFACE's standing
rule, and the tray is native Win32 — but there is no graphical mapper. That is a
real, bounded loss and M9-DECISION §5 does not argue it away.

### Using kmd today
`npx @getforma/kmd` in this repo browses `docs/` (6 design docs + 9 research reports)
as a local dashboard. Ships a prebuilt Windows x64 binary; no product coupling.
kmd is MIT-licensed (Copyright (c) 2026 Victor Villacis) — nothing blocks anyone
else adopting it.

## E6 — Reuse of existing open source (standing policy)

Already practiced; the survey (`research/prior-art-rust-architecture.md`) found no
project that solves the whole problem, but every load-bearing piece is reused:
`vigem-client` (vendored), `kanata-interception` (kanata's shipping fork), `nusb`,
`windows-rs`; HIDMaestro's MIT internals are plan B; PadForge/kanata/AutoHotInterception
are the reference implementations we crib patterns from. New wheels are invented only
where the survey proved none exist (per-device capture + fan-out engine).

## E8 — Cabinet lighting from pad feedback (Victor, 2026-08-04)

**The idea.** Games and emulators (RetroArch, any XInput-era title) send rumble
and LED-slot feedback to Xbox 360 pads. Our virtual X360 pads **already receive
it**: the XUSB notification callback delivers
`Feedback { large_motor, small_motor, led_number }` per pad into a lossy
64-deep queue (`FEEDBACK_QUEUE_CAP`, `try_send`, per-pad `dropped_feedback`
counter for `ksx doctor`), drained non-blocking by
`VirtualPadBackend::poll_feedback` — see `ksx-output/src/{backend,vigem}.rs`.
Today that data is discarded: nothing consumes the queue. The enhancement is to
bridge it to cabinet hardware. Ultimarc **PacLED64** and **I-PAC Ultimate I/O**
are USB LED controllers with documented protocols; a feedback consumer maps pad
feedback onto their outputs.

**Mapping ideas.**
- Rumble burst → that player's button-cluster flash.
- `led_number` → lighted start button per player (the LED slot *is* the player
  assignment the bus announces).
- Idle → attract pattern across the panel.

**Honest constraints.**
1. **The PlayStation persona can NEVER feed this.** ViGEmBus has no DS4
   feedback/notification IOCTL — `Persona::has_feedback` is `false` for
   PlayStation, and `ksx-core/src/persona.rs` states it plainly: no lightbar or
   rumble will ever arrive on one of those pads. This is Xbox slots (1–4) only;
   players 5+ light nothing from game feedback.
2. **MAME drives cabinet LEDs itself** through its outputs system, straight to
   the same Ultimarc boards. The bridge must **yield to MAME** (off for MAME
   profiles) — it exists for XInput-era games, which only know how to rumble a
   pad and have no other way to reach a cabinet.
3. **Hardware required**: LED-wired buttons plus a controller board. **OPEN
   QUESTION whether Victor's cabinet has them** — answer this before writing
   any code; it blocks everything.
4. **Feedback is the ONLY upstream channel ViGEm exposes.** There is no other
   game→ksx data path to mine — this is not the first of a series of
   integrations, it is the whole catalogue.

**Verdict: real, and cheap on the ksx side** — a feedback consumer thread
draining `poll_feedback` and speaking the Ultimarc protocol, touching no
pipeline thread (the same "lossy consumer off to the side" shape as E7's
monitor). **Post-M7 priority, blocked on the hardware question.**

### E8 update 2026-08-05 — hardware CONFIRMED, LEDBlinky in the picture, generalized to a feedback bus

Victor: the cabinet **has LED-wired buttons**, and **LEDBlinky** (Arzoo's
arcade LED software — the de-facto standard: per-game button lighting from its
own controls database, attract modes, native LaunchBox integration) is planned
for it. That changes the design from "ksx drives a PacLED64 directly" to a
**coexistence model**:

- **LEDBlinky owns the static layer** — which buttons are lit/active for the
  current game, attract mode between games. It already knows per-game control
  panels; re-implementing that database would be madness.
- **ksx owns the dynamic layer LEDBlinky cannot see** — real-time rumble and
  player-slot feedback arriving at the virtual pads mid-game. LEDBlinky has no
  concept of rumble; ksx is the only process holding that stream.
- Integration options to spike, in order of politeness: (1) drive spare LED
  channels ksx reserves for itself; (2) momentary overrides through whatever
  interface LEDBlinky/the LED controller exposes (LEDBlinky has a documented
  command-line/API surface; the LedWiz/PacLED64 protocols are open); (3) full
  direct-drive only when LEDBlinky is not running.

**Generalized (Victor: "send it to whatever else"): the feedback consumer is a
BUS, not a lamp driver.** One consumer thread drains `poll_feedback` and fans
out to pluggable sinks behind a small trait:

- cabinet LEDs (PacLED64 / Ultimate I/O / via LEDBlinky, above);
- **OpenRGB** — this PC already has Aura (VID 0B05) and an NZXT Kraken
  (VID 1E71) on WinUSB; rumble → case/room RGB flash is a spike away via
  OpenRGB's open SDK server;
- **ksx Studio** — the live feed already carries it. `LiveFrame::feedback`
  is on the wire and reaches the browser through `/api/live`; what is missing
  is only the RENDER (rumble pulses on the on-screen controller — the PadForge
  live-echo pattern, outbound direction). No new transport is needed;
- whatever else earns a sink (OBS overlay, marquee light, audio cue). Sinks
  are lossy consumers of a lossy stream by contract — a slow sink can never
  backpressure the engine (same rule as everything else near the pipeline).

Status: **unblocked**, still post-M7 in sequence. The Studio live socket it
was to be designed alongside SHIPPED on 2026-08-08 (`ksx-app/src/daemon/
live_pipe.rs` + `ksx-studio/src/live.rs`), so the sink trait now has a
concrete second consumer to be shaped against rather than a hypothetical one —
and `PadFeedback` is already a field of every frame both of them read.
