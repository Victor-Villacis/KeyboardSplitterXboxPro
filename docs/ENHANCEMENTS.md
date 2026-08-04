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

## E4 — More than 4 controllers (8-player)?

**Verdict: possible, with the one hard truth: XInput stops at 4.**

- The 4-slot cap is Windows' `xusb22.sys`/XInput architecture, not ksx. No virtual bus
  can create XInput slot 5.
- Beyond 4 → **HID/DirectInput pads**: HIDMaestro non-Xbox profiles are unlimited;
  MAME, RetroArch/SDL, and Steam Input games happily use 8+ DirectInput pads (X-Men
  6-player cab territory). Same E1 backend unlocks it: slots 1–4 = X360/XInput,
  slots 5+ = HID personas.
- ksx-core already treats slot count as data, not a constant; config schema won't
  change. **Priority: low until the cab grows past 4 players.**

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

## E6 — Reuse of existing open source (standing policy)

Already practiced; the survey (`research/prior-art-rust-architecture.md`) found no
project that solves the whole problem, but every load-bearing piece is reused:
`vigem-client` (vendored), `kanata-interception` (kanata's shipping fork), `nusb`,
`windows-rs`; HIDMaestro's MIT internals are plan B; PadForge/kanata/AutoHotInterception
are the reference implementations we crib patterns from. New wheels are invented only
where the survey proved none exist (per-device capture + fan-out engine).
