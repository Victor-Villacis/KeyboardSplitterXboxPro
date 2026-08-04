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
`MAX_SLOTS` raised to 8 with a validation rule that refuses a fifth `xbox360`
slot by name, the `PadState`→DS4 mapper with a documented D-pad SOCD collapse,
and `ksx pads --persona playstation` verified on the cabinet: six pads plugged,
driven, and unplugged with zero XInput slots consumed.

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
1. **Native primary (non-negotiable)** — CLI + tray daemon (M5) + native config UI (M9).
   Zero HTTP, zero web deps in the default build. The cabinet works perfectly with no
   browser in existence.
2. **`ksx-api` (M10a)** — one typed surface for Studio *and* the MCP server (E5). The
   native UI does **not** go through it: in-process calls straight to the supervisor, so
   the primary path pays no serialization tax.
3. **ksx Studio (M10b, Forma)** — optional companion UI: embedded axum + `forma-server`
   SSR + our own SSE/WS. Configure the cabinet *from your phone while standing at it* —
   the case where a browser is genuinely the right client, since a cab has no keyboard.

### "Enhance, never compromise" — enforced, not promised
- **Compile-time optional**: `--features studio`. The default build links no axum, no
  forma, no HTTP — provable with `cargo tree`.
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

### Sequencing
M6 WinUSB → M6.5 DS4 spike → M7 GA → M8 HIDMaestro → **M9 native UI** → **M10 Studio**.
**Nothing web-related precedes M6** — the driver deadline outranks the showcase.

### Using kmd today
`npx @getforma/kmd` in this repo browses `docs/` (6 design docs + 9 research reports)
as a local dashboard. Ships a prebuilt Windows x64 binary; no product coupling.
⚠️ kmd has no LICENSE file — fine for us, but it blocks anyone else adopting it.

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
