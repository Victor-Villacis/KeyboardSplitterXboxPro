# Mapping UX field study — both tracks (2026-08-05)

Two parallel studies feeding docs/MAPPER-UX.md. Full agent reports preserved
in the session transcripts; this file carries the load-bearing evidence.

## Track 1 — commercial tier (web-researched, cited)

Studied: reWASD, Steam Input/Big Picture, Xbox Accessories, 8BitDo Ultimate
V2, DS4Windows, JoyToKey, AntiMicroX, x360ce, Razer Synapse, Logitech G HUB.

Five patterns every good mapper shares: (1) device render as home screen AND
summary, decorated with binding state (Razer highlights modified keys);
(2) physical press = selection (reWASD hook / JoyToKey yellow row / x360ce
Record); (3) echo on the mapping surface, not a separate test page;
(4) two-step base gesture with depth behind a per-binding gear (Steam's
activators); (5) a guaranteed road home (Xbox immutable default + restore).

Three classic mistakes: (1) object-model bureaucracy — reWASD's
profile→config→slot→save-vs-apply is its own forum's top UX complaint, with
trial users returning to Steam Input over it; (2) making users NAME controls
instead of PRESS them (dropdown pairs, "Button 13" rows); (3) silent side
effects — Razer Synapse 4 has a documented bug where saving one remap wipes
another; Steam meanwhile proves duplicates are a feature to display, never
auto-resolve.

Also noted: Steam's community configs ranked by total playtime; 8BitDo's
"Sync to Device" as an explicit commit verb; G HUB's drag-a-command-onto-a-
button as a second modality.

## Track 2 — emulation/arcade lineage (local evidence + web)

**EmulationStation/RetroBat wizard** (strings read from this cabinet's
`emulationstation2.po`; output in `es_input.cfg`): press-to-identify device
selection ("HOLD A BUTTON ON YOUR DEVICE"), sequential position-named
prompts ("SOUTH", not "A"), auto-advance, "HOLD FOR %iS TO SKIP",
inline "ALREADY TAKEN", hotkey completeness audit before commit,
transactional OK/DISCARD. Failure mode: monolithic — fixing ONE bind costs
~40 s of hold-to-skip; no single-bind entry point.

**RetroBat's architecture find** (this machine): `retroarch.cfg` ships
`input_autodetect_enable = "false"` — RetroBat deliberately disables
RetroArch's mapping brain and its emulatorLauncher recompiles input configs
for ~130 emulators FROM THE ONE es_input.cfg ON EVERY LAUNCH (see
emulatorLauncher.log). One source of truth, mechanically compiled, drift
impossible. The strongest architectural lesson in the study.

**RetroArch**: offers BOTH set-all and per-row rebind (correct); its
binds-vs-remaps layering is conceptually right and chronically confusing
because no screen shows the chain end-to-end — a decade of forum threads.
Autoconfig profiles carry `_label` fields (human names per device). Cabinet
grief: dynamic device→port assignment (#11088) vs I-PAC's static P1–P4
scancode blocks that never renegotiate.

**MAME**: TAB menu = in-context single-control rebind as the PRIMARY flow,
per-game deltas over general defaults, OR-chaining (one action ← several
physical inputs) with explicit clear-vs-cancel, and a live Input Devices
test menu. 40 years of muscle memory in this cabinet's user base.

**Arcade/operator heritage**: JAMMA/JVS INPUT TEST menus (press → value
flips in place) are the ancestor of every echo pattern; fighting games put
button check AT character select (Tekken 7, GG Strive praised for
rebind-in-place there); BYOAC folklore identifies buttons by pressing them
into Notepad/joy.cpl — nobody reads pinouts by choice. I-PAC ships
MAME-ready = the best mapping UI is none.

## Verdict

Both worlds converge on the same grammar: press to identify, echo in place,
wizard for first contact + single rebind forever after, one source of truth
compiled outward, positions not labels, visible layers, static player
identity, zero-mapping defaults. The spec in docs/MAPPER-UX.md turns these
into ksx's three builds (A: finish v5 to spec; B: the wizard; C: button
check on the live socket).
