# Use Cases & Generality

ksx is built for one cabinet but should serve everyone the legacy app served — and
the people it turned away. This file records which topologies are **proven**, which
are **untested**, which are **blocked**, and what it would take to unblock them.

Rule for every item here: **generality must not cost the primary use case.** The
non-negotiables are listed at the bottom.

## Topology matrix

| # | Topology | Who | Status |
|---|---|---|---|
| T1 | One multi-player encoder (I-PAC2/4) → 2–4 pads by key subsets | arcade cabinets (**the primary case**) | ✅ proven on hardware (M4) |
| T2 | N distinct keyboards → one pad each | couch co-op, two people one PC — *the legacy app's headline case* | ⚠️ supported by design, **never tested**; needs a second physical keyboard bound to a slot |
| T3 | Mixed: encoder + regular keyboard(s) | cabinet with a control station | ⚠️ same as T2 |
| T4 | **Two identical devices** (2× I-PAC2, or two of the same cheap USB keyboard) | very common for 4-player builds and co-op | ❌ **blocked** — ksx refuses to start (see below) |
| T5 | One keyboard → one pad | single-player remapper, accessibility | ✅ works (degenerate case of T1) |
| T6 | More than 4 pads | 6-player cabinets | ❌ XInput caps at 4; needs HID personas (ENHANCEMENTS E4) |
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
2. **WinUSB claim (M6).** Identity becomes the USB device path — structurally
   unique per port, no collisions, no drift. Solves T4 as a side effect of the
   work already planned for the 2026 driver deadline.
3. **RawInput correlation for identity only.** `crates/ksx-capture/src/rawinput.rs`
   already reports the per-device instance path; use it during setup to map
   physical panel → device, never for blocking (the blocking variant of this hack
   is rejected by design).

Recommendation: (1) now as a setup-time feature, (2) as the durable answer.

## Adoption gaps (a new user's first ten minutes)

The cabinet's config came from importing 10-year-old XML. Someone starting fresh
has none of that:

- **`ksx setup` wizard** (biggest gap): enumerate devices → "press a key on player
  N's panel" using the existing RawInput identify primitive → pick a preset
  template → write `config.toml`. Without this, first use means hand-writing TOML.
- **Preset templates** shipped in-box: `arcade-6button` (fighting-game layout),
  `arcade-4button`, `wasd-keyboard`, `arrows-keyboard`, plus the ported legacy
  `default`. Today only `default`/`empty` exist.
- **`ksx install-drivers` should also offer Interception**, not just ViGEmBus — a
  fresh machine has neither. (License note in `docs/DRIVERS.md`: Interception is
  LGPL/non-commercial; bundling its installer is fine, commercial use is not.)
- **Quickstart in the README** written for someone who has never seen the legacy
  app, ending at "your panel now moves a controller".
- **`ksx map` verbs** (ENHANCEMENTS E5) so a preset can be edited without TOML —
  and so an AI assistant can configure a cabinet conversationally.

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

## Suggested sequencing

Fold into the roadmap after M5, without delaying the M6 deadline work:

- **M6** (unchanged, deadline-driven): WinUSB backend — also solves T4 durably.
  Design question raised by generality: with the encoder claimed by WinUSB it is
  no longer a keyboard, so **frontend navigation needs ksx to inject keystrokes**
  when not emulating. Design that in, or the cabinet loses menu control.
- **M8 "general availability"** (new): `ksx setup` wizard, preset templates,
  Interception in `install-drivers`, quickstart docs, `ksx map` verbs, and a
  tested T2/T4 path. This is what turns "Victor's cabinet software" into
  "the thing people install instead of the abandoned app".
