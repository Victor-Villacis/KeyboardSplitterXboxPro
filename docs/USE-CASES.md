# Use Cases & Generality

ksx is built for one cabinet but should serve everyone the legacy app served — and
the people it turned away. This file records which topologies are **proven**, which
are **untested**, which are **blocked**, and what it would take to unblock them.

Rule for every item here: **generality must not cost the primary use case.** The
non-negotiables are listed at the bottom.

## Topology matrix

| # | Topology | Who | Status |
|---|---|---|---|
| T1 | One multi-player encoder (I-PAC2/4) → 2–4 pads by key subsets | arcade cabinets (**the primary case**) | ✅ supported and exercised on hardware; the release-grade four-player latency/game run remains Gate 3 phase 1 |
| T2 | N distinct keyboards → one pad each | couch co-op, two people one PC — *the legacy app's headline case* | ⚠️ supported by design, **never tested**; needs a second physical keyboard bound to a slot |
| T3 | Mixed: encoder + regular keyboard(s) | cabinet with a control station | ⚠️ same as T2 |
| T4 | **Two identical devices** (2× I-PAC2, or two of the same cheap USB keyboard) | very common for 4-player builds and co-op | ⚠️ **never silently confused, still not usable together.** `DeviceSelector`'s port rung tells twins apart in config and `Match::Ambiguous` refuses rather than guessing (`docs/DEVICE-IDENTITY.md` §2). But an INF binds by hardware id, which both boards share, so `winusb.rs` refuses `SharedHardwareId` for BOTH while both are plugged — claiming one would claim every one. Telling them apart *during a claim* needs per-device installation, which ksx does not do. Untested: nobody here owns a second board |
| T5 | One keyboard → one pad | single-player remapper, accessibility | ✅ works (degenerate case of T1) |
| T6 | More than 4 pads | 6-player cabinets | ✅ **solved, and measured** — not by HID personas as this row used to predict, but by ViGEm's OTHER target: `research/m6.5-ds4-findings.md` plugged six DS4 targets alongside four X360 pads and the XInput count did not move. `Persona::playstation` takes no XInput slot, which is how players 5+ exist. `MAX_SLOTS` is 16; `ksx pads` warns before plugging XInput pads a game cannot see |
| T7 | Laptop internal keyboard as a player | portable setups | ⚠️ untested; Interception filters the class stack so it should work |

## The blocker worth fixing: T4, identical devices

Two devices of the same model report the **same Interception hardware id**, so ksx
cannot tell them apart and (since M4) refuses to start rather than silently
capturing an unassigned board and driving pads with it. That refusal is correct —
but it turns away a large share of realistic setups: buying two identical encoders
or two identical cheap keyboards is the *obvious* thing a person building a
2-player rig does.

Three ways out, cheapest first:

1. **Identify by Interception slot number, disambiguated at setup.** The driver
   already numbers devices 1..10 distinctly, even when hardware ids collide. A
   setup step ("press a key on player 1's panel") learns which slot is which, and
   the config stores hwid + learned slot. Cost: slot numbers drift across
   replug/resume (documented as R2), so ksx must detect drift and ask again
   rather than silently mis-routing. **Highest value per unit of work.**
2. **Per-device WinUSB installation.** The runtime identity is already the
   unique USB instance path, but the current INF binds by shared hardware id.
   ksx therefore refuses `SharedHardwareId` while twins are connected: claiming
   one with today's installer would claim both. T4 needs an installation method
   scoped to one device instance, not merely the existing runtime selector.
3. **RawInput correlation for identity only.** `crates/ksx-capture/src/rawinput.rs`
   already reports the per-device instance path; use it during setup to map
   physical panel → device, never for blocking (the blocking variant of this hack
   is rejected by design).

Recommendation: keep the refusal, test (1) only with explicit drift detection,
and research a per-device installation path for (2). Until then, two different
encoder models are the honest supported workaround.

## A new user's first ten minutes

The old path assumed an imported configuration and exposed the developer CLI.
The current product path is shipped in Studio:

- The installer creates one console-free customer launcher and optionally runs
  it as the original, unelevated Windows user. It opens `/start`, not Status.
- An empty configuration starts an idle background service with no capture,
  virtual controllers, or session. Setup can therefore hold a fresh in-memory
  draft instead of deadlocking on the absence of a saved controller.
- Setup lists ordinary keyboards first and unusual HID-capable devices in a
  separate optional disclosure. Device choice, controller type, layout, and
  split/freeze answer remain drafts until Save or Play.
- `/map?target=stage` reuses the complete visual mapper for bindings, multiple
  keys, auto-fire, and macros. Staged writes are exact-slot, atomic operations;
  a refusal leaves the draft unchanged.
- Save and Play are separate. Play can use the draft without writing it; Save
  does not start a session.
- Saved Games supports create, switch, edit, optional device rebase, and delete
  in Studio. Created games inherit the matching saved Setup device choices and
  are immediately plannable.
- [`QUICKSTART.md`](QUICKSTART.md) now documents this customer journey with no
  terminal or file editing. Developer commands remain documented in README.

What remains is physical proof, not a missing customer workflow: Gate 4 must run
the exact installer on a clean standard-user machine, and T2/T4 still need the
second physical keyboards/encoders the current lab does not own. Interception is
not bundled because of its licence; ViGEmBus remains the default installer
checkbox.

## Notes on breadth we already have for free

- **Scancode-based**, so keyboard layout and language don't matter — bindings are
  physical key positions. (Preset key *names* are US-layout names; that's cosmetic,
  but worth saying in docs so a German user isn't confused by `Y`/`Z`.)
- **Portable mode** (`ksx.toml` next to the exe) already suits USB-stick cabinets.
- **No admin at runtime** — only driver installation needs elevation.
- **Any encoder that presents as a keyboard** works: I-PAC, Xin-Mo, Zero Delay,
  GP-Wiz, or a plain keyboard. Nothing in ksx is I-PAC-specific except a friendly
  `[I-PAC]` tag on `VID_D209`.

## Non-negotiables (generality must not break these)

1. **One keyboard → many slots fan-out.** The primary case. Any "simplification"
   to one-device-one-pad is a regression, and `crates/ksx-app/tests/replay.rs`
   pins it against a real recorded session.
2. **Hot-path purity**: no allocation, no locks, no I/O on the capture thread.
   No feature earns an exception.
3. **Escapes stay in the capture thread** and unstarvable.
4. **Blocking scope**: only devices bound to slots, only while emulating.
5. **Crash-only**: process death always returns the keyboards.
6. **Config stays plain TOML** and hand-editable; wizards write it, never replace it.

## Current sequencing

1. Run Gate 4 with the exact CI-built installer and record its SHA/version.
2. Complete Gate 3's real four-player latency/game run, frontend wrap, driver
   removal, and 14-day soak.
3. Test T2 with two distinct physical keyboards and T4 with two identical
   devices. Do not turn an unowned-hardware claim into a checkmark.
4. Add code signing before calling the installer frictionless; unsigned
   SmartScreen remains a release risk even when the binary is correct.
