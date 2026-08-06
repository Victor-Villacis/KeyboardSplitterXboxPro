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
   accordingly: same-preset duplicate = **not a conflict at all** — it is a
   multi-bind, written with no dialog and then SHOWN ("also A · B");
   CROSS-SLOT duplicate = informational badge ("also P2's A — this is
   fan-out"), never a blocking dialog. No save ever touches a binding other
   than the one being edited without showing it first — and since 2026-08-06
   the WRITER cannot either: the only path that unbinds a control the caller
   did not name is `--move-from`/`"move_from"`, which names it in the request
   and again in the response (`moved_from`).
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

**Build A status (v7, 2026-08-05).** Landed on the mapper page: every zone
now carries its own IDENTITY on the art (persona-aware, canonical colours —
the vendored drawing has no letters, so "I can see G is mapped to A but I
can't see the A xbox button" was a real gap in commandment 4's "render as
summary"); MULTI-SELECT (Ctrl/Shift-click on desktop, a "Select multiple"
toggle for touch, a floating bar with "Map all to one key") which exposes the
engine's native multi-bind (docs/INPUT-TRANSFORMS.md §1a) with one captured
key written to N controls; and commandment 7 finished for the same-preset
case — a key already used by another control in the SAME preset is no longer
offered a "Replace" dialog, it is written and then SHOWN as a group ("also
A · B" badges on every co-bound legend row, cool-toned key tags on the art).
Cross-slot duplicates keep their existing informational dialog.

**Engine side: CLOSED (2026-08-06).** `ksx map` no longer moves a key between
controls. A same-preset duplicate is written as a multi-bind, every other
control keeps the key, and the response reports the co-bindings
(`also_drives`) — so the multi-select arm's N sequential writes all stick and
the page's honest report ("P now drives A · B · RT") is the one it prints.
The old move survives as an explicit, singular `--move-from FUNCTION` /
`"move_from"`, which names exactly what it unbound; `--force` now only means
"bind here anyway despite ANOTHER SLOT's preset" and removes nothing. The
legend still derives sharing from disk, never from what the UI assumed.

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

## The 2026-native layer (what no tool in the field study could do)

The study's tools span 1997–2026, but even the newest think like desktop
apps. ksx Studio is a web surface backed by an AI-drivable CLI — these are
the capabilities that stack only for us, ranked by leverage:

1. **Close the loop in the browser: the Gamepad API sees our virtual pads.**
   Our X360/DS4 targets are real controllers, so `navigator.getGamepads()`
   in the very page doing the mapping can read them — no socket, no new
   backend. Press a panel button → ksx translates → the virtual pad changes
   → the mapper's render lights up. That is END-TO-END verification of the
   entire product pipeline, drawn on the mapping surface (commandment 2),
   and it makes Build C's core value buildable TODAY: the physical-side
   echo still wants the live socket, but the virtual-side echo — the half
   users actually need to trust the chain — is free. (Caveats: page must be
   visible, first read needs a user gesture, mapping-order quirks per
   browser — feature-detect and degrade to socket echo later.)
2. **AI-assisted mapping (E5 grown up).** Every mapper in the study makes
   humans do the layout. ksx's CLI verbs + MCP mean an assistant can be a
   first-class mapping surface: "set P3 and P4 up like P1 but mirrored",
   "this preset for street fighter — what's wrong?", "map this new panel"
   → the assistant drives `ksx map`/the wizard and the page shows the
   result live. The mapper UI and the AI share one control surface by
   construction — no other tool in the field can say that.
3. **QR-code handoff.** The status page (cab screen) shows a QR; the phone
   scans it and lands in the mapper. When LAN mode ships (pairing token,
   E7), the QR carries the pairing — the 2026 answer to "type this IP on
   your phone". Zero cost to print the QR now for localhost-forwarded
   setups; full value at LAN time.
4. **PWA install.** Manifest + the service worker we already ship → "Add to
   Home Screen" and the phone-at-cab surface becomes an app with an icon,
   full-screen, no browser chrome. Cheap; do it in Build A polish.
5. **Command palette (Ctrl+K).** Every CONTROL-SURFACE verb searchable in
   one keystroke — start Steam profile, open P2 mapper, restore defaults.
   The 2026 power-user pattern, and for us it's a thin view over verbs that
   already exist.
6. **Multi-surface sync.** Cab TV shows the big render, phone drives the
   mapping, both fed by the same poller/socket state — presenter mode falls
   out of the architecture (islands + shared API) rather than being built.
7. **Platform polish as table stakes**: View Transitions for screen moves,
   `prefers-reduced-motion` honored, container queries for the phone/TV
   split, full keyboard-and-pad navigability of the mapper itself (Steam
   proved config UIs should be drivable from the thing being configured —
   on a cabinet, that's the panel).

Build placement: #1 lands IN Build C (and pulls Build C earlier — no longer
socket-blocked for its core); #4/#7 fold into Build A; #3 ships its QR half
with Build B's wizard (the "new machine" moment); #2 begins as soon as the
MCP shim (E5) exists — the verbs are already there; #5/#6 ride later Studio
passes.
