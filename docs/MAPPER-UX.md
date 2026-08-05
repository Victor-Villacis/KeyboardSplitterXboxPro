# Mapper UX specification — synthesized from the field study (2026-08-05)

Sources: `docs/research/` field studies of the commercial tier (reWASD, Steam
Input, Xbox Accessories, 8BitDo, DS4Windows/JoyToKey/AntiMicroX/x360ce,
Synapse/G HUB) and the emulation/arcade lineage (EmulationStation/RetroBat —
read from this cabinet's own install — RetroArch, MAME, BYOAC folklore,
operator TEST menus, fighting-game button check). Victor's brief: "the most
complex tool with the simplest interface."

## The commandments (each one evidence-backed, none negotiable)

1. **The physical press is the pointer.** Selecting a control by pressing it
   (reWASD hook, JoyToKey row-highlight, x360ce Record, ES "HOLD A BUTTON ON
   YOUR DEVICE") is the universal grammar of every loved mapper; naming
   controls from lists is the universal marker of hated ones. On a panel of
   30 identical buttons this is existential, not cosmetic.
2. **Every mapping screen is also a button-check screen.** Live echo on the
   mapping surface itself (x360ce's lit-up pad, MAME's Input Devices, Naomi
   INPUT TEST, Tekken-7-at-character-select). RetroArch had to retrofit one
   by community bounty. Requires the live socket; until it lands, the learn
   modal's capture feedback is the interim echo.
3. **Two flows, one gesture each.** ES proves the sequential wizard (press,
   press, press — auto-advance, hold-to-skip, completeness audit, nothing
   saved until OK) is perfect for FIRST CONTACT and miserable for
   corrections (~40 s of hold-to-skip to fix one bind). MAME proves
   press-in-place single rebind is the correction flow. Ship both. Fixing
   one binding must never cost more than three actions.
4. **Render as summary, legend as table.** The controller drawing decorated
   with binding state (Razer's modified-vs-stock highlight) for glancing;
   the legend grid (JoyToKey's one enduring virtue) for scanning. Both are
   the same data, both are click targets.
5. **Minimal nouns, guaranteed road home.** reWASD's profile→config→slot→
   apply pyramid is its own forum's top complaint; the Xbox app's immutable
   default + one-click restore is the quiet masterstroke. ksx exposes
   exactly two nouns (profile, preset), single rebinds commit immediately
   (MAME-style), the wizard commits transactionally (ES-style), and every
   preset keeps a session-start backup + the built-in defaults as the
   always-there floor.
6. **Speak positions and presses, never labels.** Prompt "SOUTH", not "A"
   (ES's Nintendo-proof vocabulary); persona-aware display (✕ on a
   PlayStation slot); conflicts flashed inline the moment they happen
   ("ALREADY TAKEN — G is P2's A"), not after.
7. **Duplicates are information, not errors.** Steam proves overlap is a
   feature; Synapse-4's silent wipe of binding A when saving B is the
   cardinal sin. In ksx this is doubly true: keyboard fan-out (one key
   driving several slots) is THE product. The v5 conflict dialog softens
   accordingly: same-preset duplicate = flag + one-tap steal; CROSS-SLOT
   duplicate = informational badge ("also P2's A — this is fan-out"), never
   a blocking dialog. No save ever touches a binding other than the one
   being edited without showing it first.
8. **Player identity is static; the chain is visible.** I-PAC bakes P1–P4
   into scancodes and MAME never asks again; RetroArch's dynamic ports are
   a decade of cabinet forum grief. ksx slots are static by config, and one
   screen must show the whole chain: physical key → slot/persona → control —
   RetroArch's binds-vs-remaps saga proves invisible layers cost a decade
   of confusion even when the model is right.
9. **The best mapping session is none.** I-PAC ships MAME-ready; RetroBat
   compiles one mapping into 130 emulators per launch, with the competing
   mapper disabled. ksx already imports the legacy presets; M7 preset
   templates for standard panels keep the out-of-box experience at zero
   mapping. The mapper exists for the exceptions.

## The three builds (in order)

**Build A — finish v5 to spec (now).** Layout fix (in flight: clean hover
zones + legend) plus: press-to-select (panel press focuses the control on
the open mapper — reuses the learn observer in a passive "select" mode,
idle-only like learn), softened conflict semantics per commandment 7,
restore-defaults affordance (per-preset: session-start backup + built-in
floor), and persona-aware prompt vocabulary everywhere.

**Build B — the wizard.** "Set up this slot": ES's flow, ksx's engine —
sequential position-named prompts (SOUTH/EAST/WEST/NORTH, dpad, shoulders,
sticks-as-wedges), auto-advance on press, hold-any-key-to-skip, inline
ALREADY TAKEN, audit before commit (warn if start/back unmapped — the
panel's exit keys), transactional (nothing written until OK; DISCARD always
visible). Per-slot, then "next slot →" chaining for P1→P4 first contact.
This is also the seed of the M7 setup wizard — same component, pointed at a
fresh machine.

**Build C — button check (needs the live socket).** The test view one action
away from the mapper, and eventually on it: press panel keys → the virtual
controls light across ALL slot renders simultaneously (the fan-out made
visible — four pads glowing from one keystroke is also the product demo).
Doubles as wiring diagnostics (the operator TEST heritage) and as the
mapper's live echo (commandment 2). Same socket feeds the E8 light bus and
the 3D viewer later — one stream, three consumers.

## Explicitly deferred (recorded so they're chosen, not forgotten)

- MAME-style OR-chaining (multiple physical keys per control) — the preset
  model can express it; UI later.
- Steam-style activators (hold/double-press) — engine feature first, UI
  after; belongs with shift-layers vocabulary from the PadForge audit.
- Community preset sharing (Steam's playtime-ranked configs) — M7+.
- WinUSB-claimed panels can't be learned via RawInput (injected typethrough
  is invisible) — the learn path needs a capture-side tap when M6 migration
  becomes real; recorded in CONTROL-SURFACE.
